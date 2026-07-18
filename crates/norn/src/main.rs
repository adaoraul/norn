//! norn: a terminal IRCv3 client built on the norn engine.
//!
//! Resolves one or more networks (from a TOML config file and/or CLI flags),
//! spawns an independent connection task per network, and renders their merged
//! event stream. The default UI is the ratatui TUI; `--plain` uses the line
//! renderer instead.

mod config;
mod input;
mod plain;
mod render;
mod session;
mod transport;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use clap::Parser;
use tokio::sync::mpsc;

use config::Cli;
use session::{NetCommand, UiEvent};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let cli = Cli::parse();
    let networks = match config::load_networks(&cli) {
        Ok(networks) => networks,
        Err(err) => {
            eprintln!("norn: {err}");
            std::process::exit(2);
        }
    };

    let quit = Arc::new(AtomicBool::new(false));
    let (ui_tx, ui_rx) = mpsc::unbounded_channel::<UiEvent>();
    let mut cmd_txs: Vec<mpsc::UnboundedSender<NetCommand>> = Vec::new();
    let mut net_names: Vec<String> = Vec::new();

    for (id, settings) in networks.into_iter().enumerate() {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<NetCommand>();
        cmd_txs.push(cmd_tx);
        net_names.push(settings.name.clone());
        let ui_tx = ui_tx.clone();
        let quit = quit.clone();
        tokio::spawn(session::run_network(id, settings, ui_tx, cmd_rx, quit));
    }
    drop(ui_tx); // so ui_rx closes once every network task ends

    // TODO(phase 3): default to the TUI; `--plain` selects the line renderer.
    let _ = cli.plain;
    plain::run(ui_rx, cmd_txs, net_names, quit).await;

    Ok(())
}
