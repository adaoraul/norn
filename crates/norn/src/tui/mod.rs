//! The ratatui terminal UI: a network-grouped buffer client.
//!
//! `run` owns the terminal (raw mode + alternate screen, restored on drop or
//! panic) and the async loop: it selects between UI events from the network
//! tasks and key events from the terminal, redrawing when state changes.

pub mod input;
pub mod state;
pub mod theme;
pub mod view;

use std::collections::HashSet;
use std::io::{self, Stdout, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

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

use crate::addons::{
    build_addon_host, AddonCtx, AddonEvent, AddonEventKind, AddonHost, PluginInfo, PluginStatus,
    Presence, Reaction,
};
use crate::config::{ClientConfig, NetworkConfig, TriggerConfig};
use crate::session::{ConnState, NetCommand, NetworkId, UiEvent, UiEventKind};
use state::{App, AppAction, BufferKind, NetworkMeta};

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
    disabled_plugins: Vec<String>,
    plugin_config: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    config_path: Option<PathBuf>,
    quit: Arc<AtomicBool>,
) -> io::Result<()> {
    install_panic_hook();
    // `ui_tx` is held (and cloned when spawning) so `ui_rx` stays open even with
    // zero networks; `cmd_txs` grows as networks are added at runtime.
    let mut cmd_txs = cmd_txs;
    let mut guard = TerminalGuard::new()?;
    let mut app = App::new(networks, client, definitions, aliases, config_path);
    // Plugin scripts live next to the config file (`<config-dir>/plugins`).
    let plugins_dir: Option<PathBuf> = app
        .config_path
        .as_deref()
        .and_then(Path::parent)
        .map(|d| d.join("plugins"));
    // The addon host reacts to engine events: declarative triggers plus scripts.
    app.triggers = triggers;
    app.disabled_plugins = disabled_plugins;
    app.plugin_config = plugin_config;
    let disabled: HashSet<String> = app.disabled_plugins.iter().cloned().collect();
    let report = build_addon_host(
        &app.triggers,
        plugins_dir.as_deref(),
        &disabled,
        &app.plugin_config,
    );
    let mut host = report.host;
    report_addon_load(&mut app, report.plugins, report.needs_presence);
    let mut term_events = EventStream::new();
    // Keystroke-idle tracking for the on_idle/on_active plugin hooks.
    let mut last_activity = Instant::now();
    let mut is_idle = false;
    let mut idle_tick = tokio::time::interval(Duration::from_secs(5));

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
                        last_activity = Instant::now();
                        if is_idle {
                            is_idle = false;
                            fire_synthetic(&mut app, host.as_mut(), &cmd_txs, || AddonEventKind::Active);
                        }
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
            _ = idle_tick.tick() => {
                // Persist any buffered plugin state (debounced to this ~5s cadence).
                host.flush();
                let idle_secs = app.client.idle_secs as u64;
                if !is_idle && idle_secs > 0 {
                    let elapsed = last_activity.elapsed().as_secs();
                    if elapsed >= idle_secs {
                        is_idle = true;
                        fire_synthetic(&mut app, host.as_mut(), &cmd_txs,
                            || AddonEventKind::Idle { seconds: elapsed });
                    }
                }
            }
        }

        // Execute any control-plane actions the input handlers queued.
        drain_actions(
            &mut app,
            &mut cmd_txs,
            &mut host,
            plugins_dir.as_deref(),
            &ui_tx,
            &quit,
        );

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

    // Persist any buffered plugin state before exiting (covers every break path).
    host.flush();
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
    let reactions = addon_reactions(app, host, &event);
    enact_reactions(app, cmd_txs, reactions);
    app.apply(event);
}

/// Enact host reactions: send raw lines, show local notices, or raise desktop
/// notifications.
fn enact_reactions(
    app: &mut App,
    cmd_txs: &[mpsc::UnboundedSender<NetCommand>],
    reactions: Vec<Reaction>,
) {
    for reaction in reactions {
        match reaction {
            Reaction::Send { net, lines } => {
                if let Some(tx) = cmd_txs.get(net) {
                    for line in lines {
                        let _ = tx.send(NetCommand::Raw(line));
                    }
                }
            }
            Reaction::Notify { net, text } => app.push_notice(net, text),
            Reaction::Desktop { text } => desktop_notify(text),
        }
    }
}

/// Raise an OS desktop notification via `notify-send`, fire-and-forget. Errors
/// (missing binary, no desktop) are ignored; this never blocks the UI loop.
fn desktop_notify(text: String) {
    let _ = tokio::process::Command::new("notify-send")
        .arg("norn")
        .arg(text)
        .spawn();
}

/// Store the discovered plugins on the app and report any load failures to the
/// console.
fn report_addon_load(app: &mut App, plugins: Vec<PluginInfo>, needs_presence: bool) {
    for plugin in &plugins {
        if let PluginStatus::Failed(err) = &plugin.status {
            app.push_console(format!("plugin error: {}: {err}", plugin.file));
        }
    }
    app.plugins = plugins;
    app.needs_presence = needs_presence;
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
    // Only snapshot presence when a loaded plugin actually reads it.
    let presence = if app.needs_presence {
        network_presence(app, event.net)
    } else {
        Presence::default()
    };
    let ctx = AddonCtx {
        my_nick: &my_nick,
        network: &network,
        presence: &presence,
    };
    host.on_event(&addon_event, &ctx)
}

/// Fire a synthetic (non-engine) addon event on every registered network and
/// enact the reactions. Used for idle/active events, which are client-local.
fn fire_synthetic(
    app: &mut App,
    host: &mut dyn AddonHost,
    cmd_txs: &[mpsc::UnboundedSender<NetCommand>],
    make_kind: impl Fn() -> AddonEventKind,
) {
    let targets: Vec<usize> = app
        .networks
        .iter()
        .enumerate()
        .filter(|(_, m)| matches!(m.state, ConnState::Registered { .. }))
        .map(|(i, _)| i)
        .collect();
    for net in targets {
        let (my_nick, network) = {
            let m = &app.networks[net];
            (m.my_nick.clone(), m.name.clone())
        };
        let presence = if app.needs_presence {
            network_presence(app, net)
        } else {
            Presence::default()
        };
        let ev = AddonEvent {
            net,
            kind: make_kind(),
        };
        let ctx = AddonCtx {
            my_nick: &my_nick,
            network: &network,
            presence: &presence,
        };
        let reactions = host.on_event(&ev, &ctx);
        enact_reactions(app, cmd_txs, reactions);
    }
}

/// Snapshot our presence on `net`: away/account plus each channel's roster.
fn network_presence(app: &App, net: NetworkId) -> Presence {
    let (away, account) = app
        .networks
        .get(net)
        .map(|m| (m.away, m.account.clone()))
        .unwrap_or((false, None));
    let channels = app
        .buffers
        .iter()
        .filter(|b| b.net == net && b.kind == BufferKind::Channel)
        .map(|b| {
            let members = b
                .members
                .iter()
                .map(|m| (m.nick.clone(), member_is_op(m)))
                .collect();
            (b.name.clone(), members)
        })
        .collect();
    Presence {
        away,
        account,
        channels,
    }
}

/// Whether a channel member holds op or higher (op, admin, or owner).
fn member_is_op(m: &irc_engine::Member) -> bool {
    use irc_engine::MemberPrefix::{Admin, Op, Owner};
    m.prefixes.iter().any(|p| matches!(p, Owner | Admin | Op))
}

/// Perform the supervisor's queued actions: spawn new network tasks and signal
/// existing ones to connect or disconnect.
fn drain_actions(
    app: &mut App,
    cmd_txs: &mut Vec<mpsc::UnboundedSender<NetCommand>>,
    host: &mut Box<dyn AddonHost>,
    plugins_dir: Option<&Path>,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
    quit: &Arc<AtomicBool>,
) {
    for action in std::mem::take(&mut app.actions) {
        match action {
            AppAction::ReloadAddons => {
                let disabled: HashSet<String> = app.disabled_plugins.iter().cloned().collect();
                let report =
                    build_addon_host(&app.triggers, plugins_dir, &disabled, &app.plugin_config);
                *host = report.host;
                report_addon_load(app, report.plugins, report.needs_presence);
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
