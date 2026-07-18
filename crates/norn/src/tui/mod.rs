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
use ratatui::style::Color;
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::session::{NetCommand, UiEvent};
use state::{App, NetworkMeta};

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

/// Run the TUI until the user quits or all networks close.
pub async fn run(
    mut ui_rx: mpsc::UnboundedReceiver<UiEvent>,
    cmd_txs: Vec<mpsc::UnboundedSender<NetCommand>>,
    networks: Vec<NetworkMeta>,
    timestamps: bool,
    accent: Color,
    quit: Arc<AtomicBool>,
) -> io::Result<()> {
    install_panic_hook();
    let mut guard = TerminalGuard::new()?;
    let mut app = App::new(networks, timestamps, accent);
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
                    app.apply(event);
                    // Coalesce a burst (e.g. history) into one redraw.
                    while let Ok(event) = ui_rx.try_recv() {
                        app.apply(event);
                    }
                }
            }
            term = term_events.next() => {
                match term {
                    Some(Ok(CrosstermEvent::Key(key))) if key.kind == KeyEventKind::Press => {
                        let lines = input::handle_key(&mut app, key);
                        let net = app.active_buffer().net;
                        for line in lines {
                            let _ = cmd_txs[net].send(NetCommand::Raw(line));
                        }
                    }
                    Some(Ok(CrosstermEvent::Mouse(mouse))) => {
                        let size = guard.terminal.size().unwrap_or_default();
                        input::handle_mouse(&mut app, mouse, size.width, size.height);
                    }
                    Some(Ok(CrosstermEvent::Resize(_, _))) => app.dirty = true,
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => break, // input stream ended
                }
            }
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
