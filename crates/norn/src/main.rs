//! norn: a terminal IRCv3 client built on the norn engine.
//!
//! Connects (TCP, optionally TLS), runs the bring-up handshake, then owns the
//! runtime loop: it reads server messages and stdin concurrently, feeds messages
//! to the engine, renders events, answers `PING`, auto-joins configured
//! channels, and requests recent history whenever we join one. On disconnect it
//! reconnects with exponential backoff until the user quits.

mod config;
mod input;
mod render;
mod transport;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use irc_engine::{BringupMachine, ChatHistoryRequest, Connection, Engine, Event};
use irc_proto::{Command, Message};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

use config::Cli;

/// How many messages of scrollback to request when joining a channel.
const HISTORY_LIMIT: usize = 50;

/// Reconnect backoff bounds.
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let cli = Cli::parse();

    // Read stdin once, for the whole program. `quit` is set on /quit so we stop
    // reconnecting.
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let quit = Arc::new(AtomicBool::new(false));
    tokio::spawn(input::run(tx, quit.clone()));

    let mut backoff = BACKOFF_START;
    loop {
        match run_session(&cli, &mut rx).await {
            Ok(true) => backoff = BACKOFF_START, // registered; reset backoff
            Ok(false) => {}
            Err(err) => eprintln!("connection error: {err}"),
        }

        if quit.load(Ordering::SeqCst) {
            break;
        }
        eprintln!("reconnecting in {}s...", backoff.as_secs());
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }

    Ok(())
}

/// Run one connection from connect through disconnect. Returns whether it
/// registered (used to reset the reconnect backoff).
async fn run_session(cli: &Cli, rx: &mut mpsc::UnboundedReceiver<String>) -> std::io::Result<bool> {
    // Rebuild settings each attempt: a fresh engine, machine, and (for SCRAM) a
    // fresh nonce.
    let settings = cli.clone().into_settings();
    let auto_join = settings.auto_join.clone();
    let mut my_nick = settings.bringup.nick.clone();

    let scheme = if settings.conn.tls { "ircs" } else { "irc" };
    eprintln!(
        "connecting to {scheme}://{}:{} as {my_nick}...",
        settings.conn.host, settings.conn.port
    );

    let stream = transport::connect(&settings.conn).await?;
    let mut conn = Connection::new(stream);
    let mut machine = BringupMachine::new(settings.bringup);
    let mut engine = Engine::new();

    // Bring-up. Capture the nick the server accepted (may differ after a 433).
    for event in conn.run_bringup(&mut machine).await? {
        render::render(&event);
        if let Event::Registered { nick } = &event {
            my_nick = nick.clone();
        }
    }
    if !machine.is_registered() {
        eprintln!("registration did not complete.");
        return Ok(false);
    }

    // Auto-join; each self-join triggers a history request.
    for channel in &auto_join {
        conn.send(&format!("JOIN {channel}")).await?;
    }

    // Runtime loop: server messages and user input, concurrently.
    let mut input_open = true;
    loop {
        tokio::select! {
            incoming = conn.recv() => {
                match incoming? {
                    Some(msg) => on_message(msg, &mut engine, &mut conn, &my_nick).await?,
                    None => break, // server closed the connection
                }
            }
            line = rx.recv(), if input_open => {
                match line {
                    Some(line) => conn.send(&line).await?,
                    None => input_open = false, // stdin closed; keep running
                }
            }
        }
    }

    eprintln!("connection closed.");
    Ok(true)
}

/// Handle one server message: answer `PING`, render events, and request history
/// when we ourselves join a channel.
async fn on_message<S>(
    msg: Message,
    engine: &mut Engine,
    conn: &mut Connection<S>,
    my_nick: &str,
) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if matches!(&msg.command, Command::Named(name) if name == "PING") {
        let token = msg.params.first().cloned().unwrap_or_default();
        conn.send(&format!("PONG :{token}")).await?;
    }

    for event in engine.handle(msg) {
        render::render(&event);
        if let Event::MemberJoined { target, who, .. } = &event {
            if who.nick == my_nick {
                // Load recent scrollback for the channel we just joined.
                let line = engine
                    .request_history(ChatHistoryRequest::latest(target.clone(), HISTORY_LIMIT));
                conn.send(&line).await?;
            }
        }
    }
    Ok(())
}
