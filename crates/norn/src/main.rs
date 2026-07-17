//! norn: a terminal IRCv3 client built on the norn engine.
//!
//! Step 2 opens a real connection: TCP (optionally TLS), runs the bring-up
//! handshake, and prints the resulting events. The live event loop arrives in
//! step 3.

mod config;
mod transport;

use clap::Parser;
use irc_engine::{BringupMachine, Connection};

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

    let events = conn.run_bringup(&mut machine).await?;
    for event in &events {
        println!("{event:?}");
    }

    if machine.is_registered() {
        eprintln!("registered.");
    } else {
        eprintln!("connection ended before registration completed.");
    }

    Ok(())
}
