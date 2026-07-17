//! norn: a terminal IRCv3 client built on the norn engine.
//!
//! Connects (TCP, optionally TLS), runs the bring-up handshake, then stays
//! connected: it renders incoming events and, concurrently, reads stdin so you
//! can type messages and slash-commands (see [`input`]).

mod config;
mod input;
mod render;
mod transport;

use clap::Parser;
use irc_engine::{BringupMachine, Connection, Engine};
use tokio::sync::mpsc;

use config::Cli;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let settings = Cli::parse().into_settings();

    let scheme = if settings.conn.tls { "ircs" } else { "irc" };
    eprintln!(
        "connecting to {scheme}://{}:{} as {}...",
        settings.conn.host, settings.conn.port, settings.bringup.nick
    );

    let stream = transport::connect(&settings.conn).await?;
    let mut conn = Connection::new(stream);
    let mut machine = BringupMachine::new(settings.bringup);
    let mut engine = Engine::new();

    // Read stdin in the background; lines flow to the connection via the channel.
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    tokio::spawn(input::run(tx));

    conn.run(&mut machine, &mut engine, &mut rx, render::render)
        .await?;

    eprintln!("connection closed.");
    Ok(())
}
