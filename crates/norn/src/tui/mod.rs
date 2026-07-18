//! The ratatui terminal UI: a network-grouped buffer client.
//!
//! `run` owns the terminal (raw mode + alternate screen, restored on drop or
//! panic) and the async loop: it selects between UI events from the network
//! tasks and key events from the terminal, redrawing when state changes.

pub mod input;
pub mod state;
pub mod theme;
pub mod view;

use std::io::{self, Stdout, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event as CrosstermEvent, EventStream, KeyEventKind,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::addons::{AddonCtx, AddonEvent, AddonHost, Reaction, Triggers};
use crate::config::{ClientConfig, NetworkConfig, TriggerConfig};
use crate::session::{NetCommand, UiEvent, UiEventKind};
use state::{App, AppAction, NetworkMeta};

/// A terminal in raw/alternate-screen mode, restored on drop.
struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        crossterm::execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(TerminalGuard { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(
            self.terminal.backend_mut(),
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}

/// Route commands to the active buffer's network. The console (and any buffer
/// on a network with no live task) has no sender, so its commands are dropped.
fn send_to_active(app: &App, cmd_txs: &[mpsc::UnboundedSender<NetCommand>], cmds: Vec<NetCommand>) {
    let net = app.active_buffer().net;
    if let Some(tx) = cmd_txs.get(net) {
        for cmd in cmds {
            let _ = tx.send(cmd);
        }
    }
}

/// Restore the terminal on panic before running the default hook.
fn install_panic_hook() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
        let _ = io::stdout().flush();
        hook(info);
    }));
}

/// Run the TUI until the user quits.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    mut ui_rx: mpsc::UnboundedReceiver<UiEvent>,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
    cmd_txs: Vec<mpsc::UnboundedSender<NetCommand>>,
    networks: Vec<NetworkMeta>,
    definitions: Vec<NetworkConfig>,
    client: ClientConfig,
    aliases: std::collections::BTreeMap<String, String>,
    triggers: Vec<TriggerConfig>,
    config_path: Option<PathBuf>,
    quit: Arc<AtomicBool>,
) -> io::Result<()> {
    install_panic_hook();
    // `ui_tx` is held (and cloned when spawning) so `ui_rx` stays open even with
    // zero networks; `cmd_txs` grows as networks are added at runtime.
    let mut cmd_txs = cmd_txs;
    let mut guard = TerminalGuard::new()?;
    let mut app = App::new(networks, client, definitions, aliases, config_path);
    // The addon host reacts to engine events; triggers are its first backend.
    let mut host: Box<dyn AddonHost> = Box::new(Triggers::from_configs(&triggers));
    app.triggers = triggers;
    let mut term_events = EventStream::new();

    loop {
        if app.dirty {
            guard.terminal.draw(|f| view::draw(f, &app))?;
            app.dirty = false;
        }

        tokio::select! {
            ui = ui_rx.recv() => {
                // `None` means all networks ended; stay until the user quits.
                if let Some(event) = ui {
                    process_ui_event(&mut app, host.as_mut(), &cmd_txs, event);
                    // Coalesce a burst (e.g. history) into one redraw.
                    while let Ok(event) = ui_rx.try_recv() {
                        process_ui_event(&mut app, host.as_mut(), &cmd_txs, event);
                    }
                }
            }
            term = term_events.next() => {
                match term {
                    Some(Ok(CrosstermEvent::Key(key))) if key.kind == KeyEventKind::Press => {
                        let cmds = input::handle_key(&mut app, key);
                        send_to_active(&app, &cmd_txs, cmds);
                    }
                    Some(Ok(CrosstermEvent::Mouse(mouse))) => {
                        let size = guard.terminal.size().unwrap_or_default();
                        let cmds = input::handle_mouse(&mut app, mouse, size.width, size.height);
                        send_to_active(&app, &cmd_txs, cmds);
                    }
                    Some(Ok(CrosstermEvent::Resize(_, _))) => app.dirty = true,
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => break, // input stream ended
                }
            }
        }

        // Execute any control-plane actions the input handlers queued.
        drain_actions(&mut app, &mut cmd_txs, &mut host, &ui_tx, &quit);

        // Ring the terminal bell if a highlight arrived and beeping is enabled.
        if app.bell {
            let _ = write!(io::stdout(), "\x07");
            let _ = io::stdout().flush();
            app.bell = false;
        }

        if app.should_quit {
            quit.store(true, Ordering::SeqCst);
            for tx in &cmd_txs {
                let _ = tx.send(NetCommand::Quit(Some("norn".to_string())));
            }
            break;
        }
    }

    Ok(())
}

/// Run one UI event through the addon host, enact its reactions (send lines /
/// show notices), then apply the event to the app state.
fn process_ui_event(
    app: &mut App,
    host: &mut dyn AddonHost,
    cmd_txs: &[mpsc::UnboundedSender<NetCommand>],
    event: UiEvent,
) {
    for reaction in addon_reactions(app, host, &event) {
        match reaction {
            Reaction::Send { net, lines } => {
                if let Some(tx) = cmd_txs.get(net) {
                    for line in lines {
                        let _ = tx.send(NetCommand::Raw(line));
                    }
                }
            }
            Reaction::Notify { net, text } => app.push_notice(net, text),
        }
    }
    app.apply(event);
}

/// Ask the addon host for reactions to an engine event (nothing for non-engine
/// events or unknown networks).
fn addon_reactions(app: &App, host: &mut dyn AddonHost, event: &UiEvent) -> Vec<Reaction> {
    let UiEventKind::Engine(engine_event) = &event.kind else {
        return Vec::new();
    };
    let Some(meta) = app.networks.get(event.net) else {
        return Vec::new();
    };
    let (my_nick, network) = (meta.my_nick.clone(), meta.name.clone());
    let Some(addon_event) = AddonEvent::from_engine(engine_event, event.net, &my_nick) else {
        return Vec::new();
    };
    let ctx = AddonCtx {
        my_nick: &my_nick,
        network: &network,
    };
    host.on_event(&addon_event, &ctx)
}

/// Perform the supervisor's queued actions: spawn new network tasks and signal
/// existing ones to connect or disconnect.
fn drain_actions(
    app: &mut App,
    cmd_txs: &mut Vec<mpsc::UnboundedSender<NetCommand>>,
    host: &mut Box<dyn AddonHost>,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
    quit: &Arc<AtomicBool>,
) {
    for action in std::mem::take(&mut app.actions) {
        match action {
            AppAction::ReloadAddons => {
                *host = Box::new(Triggers::from_configs(&app.triggers));
            }
            AppAction::AddNetwork { id, config } => match config.resolve() {
                Ok(settings) => {
                    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
                    // The id was allocated as `networks.len()`, kept in lockstep
                    // with `cmd_txs`, so a plain push lands it at index `id`.
                    debug_assert_eq!(id, cmd_txs.len());
                    cmd_txs.push(cmd_tx);
                    tokio::spawn(crate::session::run_network(
                        id,
                        settings,
                        ui_tx.clone(),
                        cmd_rx,
                        quit.clone(),
                    ));
                }
                Err(err) => app.push_active_event(format!("connect failed: {err}")),
            },
            AppAction::Connect(id) => {
                if let Some(tx) = cmd_txs.get(id) {
                    let _ = tx.send(NetCommand::Connect);
                }
            }
            AppAction::Disconnect(id, reason) => {
                if let Some(tx) = cmd_txs.get(id) {
                    let _ = tx.send(NetCommand::Disconnect(reason));
                }
            }
        }
    }
}
