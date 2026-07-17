//! norn: a terminal IRCv3 client built on the norn engine.
//!
//! Connects (TCP, optionally TLS), runs the bring-up handshake, then stays
//! connected — feeding messages into the engine and rendering the resulting
//! events until the server closes the connection. Sending user input is a
//! follow-up step.

mod config;
mod render;
mod transport;

use clap::Parser;
use irc_engine::{BringupMachine, Connection, Engine};

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

    conn.run(&mut machine, &mut engine, render::render).await?;

    eprintln!("connection closed.");
    Ok(())
}
