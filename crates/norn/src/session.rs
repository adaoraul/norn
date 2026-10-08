//! Per-network session tasks and the UI event/command channels.
//!
//! Each network runs as an independent task owning its `Connection`,
//! `BringupMachine`, and `Engine`, with its own reconnect loop. Tasks share no
//! mutable state: they emit tagged [`UiEvent`]s to one shared channel and
//! receive [`NetCommand`]s on a per-network channel. This keeps concurrency
//! trivial (only `Send` channel handles cross tasks).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use irc_engine::{
    BringupMachine, ChatHistoryRequest, ChatMessage, Connection, Engine, Event, MessageKind,
    Selector,
};
use irc_proto::{Command, Message, Source};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

use crate::config::NetworkSettings;
use crate::transport;

/// Index identifying a network among the connected set.
pub type NetworkId = usize;

/// How much scrollback to request when we join a channel.
const HISTORY_LIMIT: usize = 50;
/// How often we time a PING round trip to the server.
const LAG_INTERVAL: Duration = Duration::from_secs(30);
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
    /// The latest PING round-trip time to the server.
    Lag(Duration),
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
    // Whether SASL logged us in; if so, NickServ auto-identify stays dormant.
    let mut authenticated = false;

    for event in conn.run_bringup(&mut machine).await? {
        match &event {
            Event::Registered { nick } => {
                my_nick = nick.clone();
                let _ = ui_tx.send(UiEvent {
                    net: id,
                    kind: UiEventKind::ConnState(ConnState::Registered { nick: nick.clone() }),
                });
            }
            Event::AuthResult(Ok(_)) => authenticated = true,
            _ => {}
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

    // Auto-identify is a one-shot fallback per connection.
    let mut identified = false;

    // Round-trip timing: the first tick fires at once, so lag shows up soon
    // after registering instead of half a minute later.
    let mut probe = LagProbe::default();
    let mut lag_tick = tokio::time::interval(LAG_INTERVAL);

    loop {
        tokio::select! {
            incoming = conn.recv() => {
                match incoming? {
                    Some(msg) => {
                        on_message(
                            id, msg, &mut engine, &mut conn, &mut my_nick, settings,
                            authenticated, &mut identified, &mut probe, ui_tx,
                        )
                        .await?
                    }
                    None => return Ok(RunOutcome::Dropped), // server closed
                }
            }
            _ = lag_tick.tick() => {
                let line = probe.next_ping(Instant::now());
                conn.send(&line).await?;
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
/// request history when we ourselves join a channel, and auto-identify to
/// NickServ once if configured and not already SASL-authenticated.
#[allow(clippy::too_many_arguments)]
async fn on_message<S>(
    id: NetworkId,
    msg: Message,
    engine: &mut Engine,
    conn: &mut Connection<S>,
    my_nick: &mut String,
    settings: &NetworkSettings,
    authenticated: bool,
    identified: &mut bool,
    probe: &mut LagProbe,
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
            if who.nick.eq_ignore_ascii_case(my_nick) {
                let line = engine
                    .request_history(ChatHistoryRequest::latest(target.clone(), HISTORY_LIMIT));
                conn.send(&line).await?;
                // Seed away state for members already gone (away-notify only
                // reports live changes, not the state at join time).
                conn.send(&format!("WHO {target}")).await?;
                // Ask for the channel's current modes for the header.
                conn.send(&format!("MODE {target}")).await?;
            }
        }
        // Our own PING came back: report the round trip, not the PONG itself.
        if let Event::Pong { token } = &event {
            if let Some(lag) = probe.reply(token, Instant::now()) {
                let _ = ui_tx.send(UiEvent {
                    net: id,
                    kind: UiEventKind::Lag(lag),
                });
            }
            continue;
        }
        // Follow our own nick across /nick and forced changes, so later joins
        // are still recognised as ours.
        if let Event::NickChanged { old, new } = &event {
            if old.eq_ignore_ascii_case(my_nick) {
                *my_nick = new.clone();
            }
        }
        // NickServ auto-identify fallback: fire once on the identify prompt when
        // SASL did not already log us in. `identify_line` returns `None` unless
        // auto-identify is on and a password is available; the password is built
        // inside it and never reaches here as a value.
        if !*identified && !authenticated {
            if let Event::MessageReceived(chat) = &event {
                if is_identify_prompt(chat) {
                    if let Some(line) = settings.identify_line() {
                        conn.send(&line).await?;
                        *identified = true;
                        let _ = ui_tx.send(UiEvent {
                            net: id,
                            kind: UiEventKind::Info("identified with services".to_string()),
                        });
                    }
                }
            }
        }
        let _ = ui_tx.send(UiEvent {
            net: id,
            kind: UiEventKind::Engine(event),
        });
    }
    Ok(())
}

/// Times PING/PONG round trips. Pure bookkeeping (the caller supplies the
/// clock), so the engine stays sans-I/O and this stays testable.
#[derive(Debug, Default)]
struct LagProbe {
    seq: u64,
    outstanding: Option<(String, Instant)>,
}

impl LagProbe {
    /// Start a probe at `now`; returns the line to send. An unanswered earlier
    /// probe is dropped (a late reply to it is simply ignored).
    fn next_ping(&mut self, now: Instant) -> String {
        self.seq += 1;
        let token = format!("norn-{}", self.seq);
        self.outstanding = Some((token.clone(), now));
        format!("PING :{token}")
    }

    /// Match a PONG token against the outstanding probe. `Some(round trip)`
    /// when it is ours, `None` for anything else (including a stale token).
    fn reply(&mut self, token: &str, now: Instant) -> Option<Duration> {
        let (expected, sent) = self.outstanding.as_ref()?;
        if expected != token {
            return None;
        }
        let lag = now.saturating_duration_since(*sent);
        self.outstanding = None;
        Some(lag)
    }
}

/// Whether a message is a NickServ NOTICE asking us to identify.
fn is_identify_prompt(chat: &ChatMessage) -> bool {
    if chat.kind != MessageKind::Notice {
        return false;
    }
    let from_nickserv = matches!(
        chat.sender.as_ref(),
        Some(Source::User { nick, .. }) if nick.eq_ignore_ascii_case("NickServ")
    );
    from_nickserv && chat.text.to_lowercase().contains("identify")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(nick: &str, text: &str) -> ChatMessage {
        ChatMessage {
            time: None,
            msgid: None,
            account: None,
            sender: Some(Source::User {
                nick: nick.into(),
                user: None,
                host: None,
            }),
            target: "me".into(),
            text: text.into(),
            kind: MessageKind::Notice,
        }
    }

    #[test]
    fn lag_probe_times_the_matching_reply() {
        let mut probe = LagProbe::default();
        let t0 = Instant::now();
        assert_eq!(probe.next_ping(t0), "PING :norn-1");
        let lag = probe.reply("norn-1", t0 + Duration::from_millis(250));
        assert_eq!(lag, Some(Duration::from_millis(250)));
        // Answered once; a duplicate reply is not a second measurement.
        assert_eq!(probe.reply("norn-1", t0 + Duration::from_secs(1)), None);
    }

    #[test]
    fn lag_probe_ignores_foreign_and_stale_tokens() {
        let mut probe = LagProbe::default();
        let t0 = Instant::now();
        probe.next_ping(t0);
        assert_eq!(probe.reply("something-else", t0), None);
        // A new probe supersedes the unanswered one.
        assert_eq!(probe.next_ping(t0), "PING :norn-2");
        assert_eq!(probe.reply("norn-1", t0), None);
        assert!(probe.reply("norn-2", t0).is_some());
    }

    #[test]
    fn identify_prompt_detection() {
        // A NickServ NOTICE mentioning identify (any case) is a prompt.
        assert!(is_identify_prompt(&notice(
            "NickServ",
            "This nickname is registered. Please IDENTIFY."
        )));
        assert!(is_identify_prompt(&notice(
            "nickserv",
            "type /msg NickServ identify <password>"
        )));
        // A non-NickServ sender is not.
        assert!(!is_identify_prompt(&notice("someone", "please identify")));
        // A PRIVMSG (not NOTICE) is not.
        let mut pm = notice("NickServ", "identify");
        pm.kind = MessageKind::Privmsg;
        assert!(!is_identify_prompt(&pm));
        // An unrelated NickServ notice is not.
        assert!(!is_identify_prompt(&notice(
            "NickServ",
            "you are now logged in"
        )));
    }
}
