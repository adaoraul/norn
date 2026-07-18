//! The `--plain` renderer: prints the merged multi-network event stream and
//! sends typed input to the first network.
//!
//! A debugging/scripting fallback for the TUI. Buffer switching and per-network
//! input routing are the TUI's job; here input always targets the first network.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use crate::render;
use crate::session::{ConnState, NetCommand, NetworkId, UiEvent, UiEventKind};

/// Run the plain renderer until all networks close or the user quits.
pub async fn run(
    mut ui_rx: mpsc::UnboundedReceiver<UiEvent>,
    cmd_txs: Vec<mpsc::UnboundedSender<NetCommand>>,
    net_names: Vec<String>,
    quit: Arc<AtomicBool>,
) {
    let mut stdin = BufReader::new(tokio::io::stdin()).lines();
    let mut current_target: Option<String> = None;
    let mut stdin_open = true;

    loop {
        tokio::select! {
            event = ui_rx.recv() => {
                match event {
                    Some(event) => print_event(&event, &net_names),
                    None => break, // all networks ended
                }
            }
            line = stdin.next_line(), if stdin_open => {
                match line {
                    Ok(Some(line)) => {
                        let result = crate::input::translate(&line, &mut current_target);
                        if let Some(feedback) = result.feedback {
                            println!("-- {feedback}");
                        }
                        if result.quit {
                            quit.store(true, Ordering::SeqCst);
                            for tx in &cmd_txs {
                                let _ = tx.send(NetCommand::Quit(Some("norn".to_string())));
                            }
                            break;
                        }
                        if let Some(tx) = cmd_txs.first() {
                            for line in result.lines {
                                let _ = tx.send(NetCommand::Raw(line));
                            }
                        }
                    }
                    _ => stdin_open = false, // stdin closed; keep rendering
                }
            }
        }
    }
}

fn net_name(names: &[String], id: NetworkId) -> &str {
    names.get(id).map(String::as_str).unwrap_or("?")
}

fn print_event(event: &UiEvent, names: &[String]) {
    let net = net_name(names, event.net);
    match &event.kind {
        UiEventKind::Engine(engine_event) => {
            for line in render::render(engine_event) {
                println!("[{net}] {line}");
            }
        }
        UiEventKind::Info(text) => println!("[{net}] {text}"),
        UiEventKind::ConnState(state) => {
            let text = match state {
                ConnState::Connecting => "connecting...".to_string(),
                ConnState::Registered { nick } => format!("registered as {nick}"),
                ConnState::Reconnecting { delay } => {
                    format!("reconnecting in {}s...", delay.as_secs())
                }
                ConnState::Closed => "connection closed".to_string(),
            };
            println!("[{net}] {text}");
        }
    }
}
