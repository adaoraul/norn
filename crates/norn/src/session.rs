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

use irc_engine::{BringupMachine, ChatHistoryRequest, Connection, Engine, Event};
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
    /// Disconnected; will retry after the given delay.
    Reconnecting {
        /// Backoff before the next attempt.
        delay: Duration,
    },
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
pub enum NetCommand {
    /// Send a raw line to the server.
    Raw(String),
    /// Quit this network (and stop reconnecting).
    Quit(Option<String>),
}

/// Run one network for the life of the program: connect, register, run, and
/// reconnect with backoff until told to quit.
pub async fn run_network(
    id: NetworkId,
    settings: NetworkSettings,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
    mut cmd_rx: mpsc::UnboundedReceiver<NetCommand>,
    quit: Arc<AtomicBool>,
) {
    let mut backoff = BACKOFF_START;
    loop {
        let _ = ui_tx.send(UiEvent {
            net: id,
            kind: UiEventKind::ConnState(ConnState::Connecting),
        });

        match run_once(id, &settings, &ui_tx, &mut cmd_rx).await {
            Ok(true) => break, // user quit
            Ok(false) => {}
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

/// One connection attempt. Returns `Ok(true)` if the user asked to quit.
async fn run_once(
    id: NetworkId,
    settings: &NetworkSettings,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
    cmd_rx: &mut mpsc::UnboundedReceiver<NetCommand>,
) -> std::io::Result<bool> {
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
        return Ok(false);
    }

    for channel in &settings.auto_join {
        conn.send(&format!("JOIN {channel}")).await?;
    }

    loop {
        tokio::select! {
            incoming = conn.recv() => {
                match incoming? {
                    Some(msg) => on_message(id, msg, &mut engine, &mut conn, &my_nick, ui_tx).await?,
                    None => return Ok(false), // server closed
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(NetCommand::Raw(line)) => conn.send(&line).await?,
                    Some(NetCommand::Quit(reason)) => {
                        let reason = reason.unwrap_or_else(|| "norn".to_string());
                        conn.send(&format!("QUIT :{reason}")).await?;
                        return Ok(true);
                    }
                    None => return Ok(true), // UI gone
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
            }
        }
        let _ = ui_tx.send(UiEvent {
            net: id,
            kind: UiEventKind::Engine(event),
        });
    }
    Ok(())
}
