//! Per-network session tasks and the UI event/command channels.
//!
//! Each network runs as an independent task owning its `Connection`,
//! `BringupMachine`, and `Engine`, with its own reconnect loop. Tasks share no
//! mutable state: they emit tagged [`UiEvent`]s to one shared channel and
//! receive [`NetCommand`]s on a per-network channel. This keeps concurrency
//! trivial (only `Send` channel handles cross tasks).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use irc_engine::{BringupMachine, ChatHistoryRequest, Connection, Engine, Event, Selector};
use irc_proto::{Command, Message};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

use crate::config::NetworkSettings;
use crate::transport;

/// Index identifying a network among the connected set.
pub type NetworkId = usize;

/// How much scrollback to request when we join a channel.
const HISTORY_LIMIT: usize = 50;
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Connection lifecycle status for a network.
#[derive(Debug, Clone)]
pub enum ConnState {
    /// Attempting to connect / register.
    Connecting,
    /// Registered with the confirmed nick.
    Registered {
        /// The nick the server accepted.
        nick: String,
    },
    /// Dropped unexpectedly; will retry after the given delay.
    Reconnecting {
        /// Backoff before the next attempt.
        delay: Duration,
    },
    /// Idle after a user `/disconnect`; stays until `/connect` revives it.
    Disconnected,
    /// Stopped for good (user quit).
    Closed,
}

/// An event surfaced to the UI, tagged with its network.
pub struct UiEvent {
    /// Which network produced it.
    pub net: NetworkId,
    /// The payload.
    pub kind: UiEventKind,
}

/// The payload of a [`UiEvent`].
pub enum UiEventKind {
    /// A semantic engine event.
    Engine(Event),
    /// A connection-state transition.
    ConnState(ConnState),
    /// A human-readable notice.
    Info(String),
}

/// A command from the UI to a network task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetCommand {
    /// Send a raw line to the server.
    Raw(String),
    /// Page older history for a channel, before a known message id. Routed
    /// through the engine so the request is labeled and its `complete` flag is
    /// computed (the UI cannot reach the engine's label allocator directly).
    RequestHistory {
        /// The channel to page.
        target: String,
        /// Fetch messages before this message id (the oldest currently held).
        before: String,
        /// Maximum messages to fetch.
        limit: usize,
    },
    /// (Re)connect an idle network task.
    Connect,
    /// Disconnect but keep the task alive (idle), with an optional quit reason.
    /// A later `Connect` revives it.
    Disconnect(Option<String>),
    /// Quit this network for good (and stop the task).
    Quit(Option<String>),
}

/// Why a connection attempt ended.
enum RunOutcome {
    /// The user quit this network for good; stop the task.
    Quit,
    /// The user asked to disconnect; go idle until a `Connect`.
    Disconnected,
    /// The connection dropped (or bring-up failed); reconnect with backoff.
    Dropped,
}

/// Run one network task for the life of the program. The task owns a
/// desired-connection state: when connected it dials and, on an unexpected
/// drop, reconnects with backoff; when disconnected (via `/disconnect`) it idles
/// on the command channel until a `Connect` revives it. Only `Quit` (or the UI
/// closing) ends the task.
pub async fn run_network(
    id: NetworkId,
    settings: NetworkSettings,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
    mut cmd_rx: mpsc::UnboundedReceiver<NetCommand>,
    quit: Arc<AtomicBool>,
) {
    let mut backoff = BACKOFF_START;
    // Spawned tasks always want to connect (they were just `/connect`ed, or are
    // a startup network).
    let mut want_connected = true;

    loop {
        if quit.load(Ordering::SeqCst) {
            break;
        }
        if !want_connected {
            // Idle: wait for a control command.
            match cmd_rx.recv().await {
                Some(NetCommand::Connect) => {
                    want_connected = true;
                    backoff = BACKOFF_START;
                }
                Some(NetCommand::Quit(_)) | None => break,
                _ => {} // ignore raw/history/disconnect while already idle
            }
            continue;
        }

        let _ = ui_tx.send(UiEvent {
            net: id,
            kind: UiEventKind::ConnState(ConnState::Connecting),
        });

        match run_once(id, &settings, &ui_tx, &mut cmd_rx).await {
            Ok(RunOutcome::Quit) => break,
            Ok(RunOutcome::Disconnected) => {
                let _ = ui_tx.send(UiEvent {
                    net: id,
                    kind: UiEventKind::ConnState(ConnState::Disconnected),
                });
                want_connected = false;
                continue;
            }
            Ok(RunOutcome::Dropped) => {}
            Err(err) => {
                let _ = ui_tx.send(UiEvent {
                    net: id,
                    kind: UiEventKind::Info(format!("connection error: {err}")),
                });
            }
        }

        if quit.load(Ordering::SeqCst) {
            break;
        }
        let _ = ui_tx.send(UiEvent {
            net: id,
            kind: UiEventKind::ConnState(ConnState::Reconnecting { delay: backoff }),
        });
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }

    let _ = ui_tx.send(UiEvent {
        net: id,
        kind: UiEventKind::ConnState(ConnState::Closed),
    });
}

/// One connection attempt: connect, register, then run the post-registration
/// loop until it ends. The return value tells the caller whether to stop, idle,
/// or reconnect.
async fn run_once(
    id: NetworkId,
    settings: &NetworkSettings,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
    cmd_rx: &mut mpsc::UnboundedReceiver<NetCommand>,
) -> std::io::Result<RunOutcome> {
    let stream = transport::connect(&settings.conn).await?;
    let mut conn = Connection::new(stream);
    let mut machine = BringupMachine::new(settings.bringup());
    let mut engine = Engine::new();
    let mut my_nick = settings.nick().to_string();

    for event in conn.run_bringup(&mut machine).await? {
        if let Event::Registered { nick } = &event {
            my_nick = nick.clone();
            let _ = ui_tx.send(UiEvent {
                net: id,
                kind: UiEventKind::ConnState(ConnState::Registered { nick: nick.clone() }),
            });
        }
        let _ = ui_tx.send(UiEvent {
            net: id,
            kind: UiEventKind::Engine(event),
        });
    }
    if !machine.is_registered() {
        return Ok(RunOutcome::Dropped);
    }

    for channel in &settings.auto_join {
        conn.send(&format!("JOIN {channel}")).await?;
    }

    loop {
        tokio::select! {
            incoming = conn.recv() => {
                match incoming? {
                    Some(msg) => on_message(id, msg, &mut engine, &mut conn, &my_nick, ui_tx).await?,
                    None => return Ok(RunOutcome::Dropped), // server closed
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(NetCommand::Raw(line)) => conn.send(&line).await?,
                    Some(NetCommand::RequestHistory { target, before, limit }) => {
                        let request = ChatHistoryRequest::before(target, Selector::msgid(before), limit);
                        let line = engine.request_history(request);
                        conn.send(&line).await?;
                    }
                    Some(NetCommand::Connect) => {} // already connected
                    Some(NetCommand::Disconnect(reason)) => {
                        let reason = reason.unwrap_or_else(|| "norn".to_string());
                        conn.send(&format!("QUIT :{reason}")).await?;
                        return Ok(RunOutcome::Disconnected);
                    }
                    Some(NetCommand::Quit(reason)) => {
                        let reason = reason.unwrap_or_else(|| "norn".to_string());
                        conn.send(&format!("QUIT :{reason}")).await?;
                        return Ok(RunOutcome::Quit);
                    }
                    None => return Ok(RunOutcome::Quit), // UI gone
                }
            }
        }
    }
}

/// Handle one server message: PING keepalive, feed the engine, forward events,
/// and request history when we ourselves join a channel.
async fn on_message<S>(
    id: NetworkId,
    msg: Message,
    engine: &mut Engine,
    conn: &mut Connection<S>,
    my_nick: &str,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if matches!(&msg.command, Command::Named(name) if name == "PING") {
        let token = msg.params.first().cloned().unwrap_or_default();
        conn.send(&format!("PONG :{token}")).await?;
    }

    for event in engine.handle(msg) {
        if let Event::MemberJoined { target, who, .. } = &event {
            if who.nick == my_nick {
                let line = engine
                    .request_history(ChatHistoryRequest::latest(target.clone(), HISTORY_LIMIT));
                conn.send(&line).await?;
                // Seed away state for members already gone (away-notify only
                // reports live changes, not the state at join time).
                conn.send(&format!("WHO {target}")).await?;
            }
        }
        let _ = ui_tx.send(UiEvent {
            net: id,
            kind: UiEventKind::Engine(event),
        });
    }
    Ok(())
}
