//! norn: a terminal IRCv3 client built on the norn engine.
//!
//! Resolves one or more networks (from a TOML config file and/or CLI flags),
//! spawns an independent connection task per network, and renders their merged
//! event stream. The default UI is the ratatui TUI; `--plain` uses the line
//! renderer instead.

mod addons;
mod commands;
mod config;
mod input;
mod plain;
mod render;
mod session;
mod settings;
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
    let Startup {
        connect,
        definitions,
        client,
        aliases,
        triggers,
        path,
    } = match config::resolve_startup(&cli) {
        Ok(startup) => startup,
        Err(err) => {
            eprintln!("norn: {err}");
            std::process::exit(2);
        }
    };

    let quit = Arc::new(AtomicBool::new(false));
    let (ui_tx, ui_rx) = mpsc::unbounded_channel::<UiEvent>();
    let mut cmd_txs: Vec<mpsc::UnboundedSender<NetCommand>> = Vec::new();
    let mut metas: Vec<NetworkMeta> = Vec::new();

    for (id, settings) in connect.into_iter().enumerate() {
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

    if cli.plain {
        drop(ui_tx); // so ui_rx closes once every network task ends
        let net_names = metas.iter().map(|m| m.name.clone()).collect();
        plain::run(ui_rx, cmd_txs, net_names, quit).await;
        Ok(())
    } else {
        // The TUI keeps a `ui_tx` clone so it can spawn networks at runtime and
        // so `ui_rx` stays open even with zero networks (the console stays up).
        tui::run(
            ui_rx,
            ui_tx,
            cmd_txs,
            metas,
            definitions,
            client,
            aliases,
            triggers,
            path,
            quit,
        )
        .await
    }
}
