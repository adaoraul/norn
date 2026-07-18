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
mod tui;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use clap::Parser;
use tokio::sync::mpsc;

use config::{Cli, Startup};
use session::{ConnState, NetCommand, UiEvent};
use tui::state::NetworkMeta;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let cli = Cli::parse();
    let networks = match config::resolve_startup(&cli) {
        Ok(Startup::Connect(networks)) => networks,
        Ok(Startup::WroteTemplate(path)) => {
            println!(
                "Created a starter config at {}.\nEdit it to add a network, then run norn again.",
                path.display()
            );
            return Ok(());
        }
        Ok(Startup::NoNetworks(path)) => {
            println!(
                "No networks configured in {}.\nAdd a [[network]] block (or pass --server/--nick).",
                path.display()
            );
            return Ok(());
        }
        Err(err) => {
            eprintln!("norn: {err}");
            std::process::exit(2);
        }
    };

    let quit = Arc::new(AtomicBool::new(false));
    let (ui_tx, ui_rx) = mpsc::unbounded_channel::<UiEvent>();
    let mut cmd_txs: Vec<mpsc::UnboundedSender<NetCommand>> = Vec::new();
    let mut metas: Vec<NetworkMeta> = Vec::new();

    for (id, settings) in networks.into_iter().enumerate() {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<NetCommand>();
        cmd_txs.push(cmd_tx);
        metas.push(NetworkMeta {
            name: settings.name.clone(),
            my_nick: settings.nick().to_string(),
            state: ConnState::Connecting,
        });
        let ui_tx = ui_tx.clone();
        let quit = quit.clone();
        tokio::spawn(session::run_network(id, settings, ui_tx, cmd_rx, quit));
    }
    drop(ui_tx); // so ui_rx closes once every network task ends

    if cli.plain {
        let net_names = metas.iter().map(|m| m.name.clone()).collect();
        plain::run(ui_rx, cmd_txs, net_names, quit).await;
        Ok(())
    } else {
        let accent = ratatui::style::Color::Rgb(0x6b, 0xac, 0xae);
        tui::run(ui_rx, cmd_txs, metas, true, accent, quit).await
    }
}
