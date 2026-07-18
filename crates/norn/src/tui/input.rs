//! Key handling for the TUI: input editing, commands, completion, switcher.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::state::{App, BufferKind, Completion, Mode, Switcher};

/// Width of the sidebar column.
const SIDEBAR_W: u16 = 24;
/// Width of the nicklist column.
const NICKLIST_W: u16 = 18;

/// Handle a mouse event: click the sidebar to switch buffers, click a nick to
/// open a query, or scroll the message view. Returns any lines to send (a scroll
/// to the top may request older history).
pub fn handle_mouse(app: &mut App, event: MouseEvent, width: u16, _height: u16) -> Vec<String> {
    match event.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // Sidebar and nicklist span the full height; out-of-range rows map to
            // None, so no vertical guard is needed.
            if event.column < SIDEBAR_W {
                if let Some(idx) = sidebar_buffer_at(app, event.row) {
                    app.switch_to(idx);
                }
            } else if app.nicklist_visible
                && app.active_buffer().kind == BufferKind::Channel
                && event.column >= width.saturating_sub(NICKLIST_W)
            {
                if let Some(nick) = nicklist_nick_at(app, event.row) {
                    let net = app.active_buffer().net;
                    app.open_query(net, &nick);
                }
            }
        }
        MouseEventKind::ScrollUp => return app.scroll(1).into_iter().collect(),
        MouseEventKind::ScrollDown => return app.scroll(-1).into_iter().collect(),
        _ => {}
    }
    Vec::new()
}

/// Which buffer index is at sidebar row `y` (matches `view::draw_sidebar`).
/// The network header row selects that network's server/status buffer.
fn sidebar_buffer_at(app: &App, y: u16) -> Option<usize> {
    let mut row = 0u16;
    for net_id in 0..app.networks.len() {
        if row == y {
            return app
                .buffers
                .iter()
                .position(|b| b.net == net_id && b.kind == BufferKind::Server);
        }
        row += 1;
        for (idx, buffer) in app.buffers.iter().enumerate() {
            if buffer.net != net_id || buffer.kind == BufferKind::Server {
                continue;
            }
            if row == y {
                return Some(idx);
            }
            row += 1;
        }
    }
    None
}

/// Which nick is at nicklist row `y` (row 0 is the count header).
fn nicklist_nick_at(app: &App, y: u16) -> Option<String> {
    if y == 0 {
        return None;
    }
    app.active_buffer()
        .sorted_members()
        .get((y - 1) as usize)
        .map(|m| m.nick.clone())
}

/// Handle one key. Mutates `app` and returns raw lines to send to the active
/// buffer's network (empty for local-only keys). Sets `app.should_quit` on quit.
pub fn handle_key(app: &mut App, key: KeyEvent) -> Vec<String> {
    app.dirty = true;
    if app.mode == Mode::Switcher {
        handle_switcher(app, key);
        return Vec::new();
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    match key.code {
        KeyCode::Char('c') if ctrl => app.should_quit = true,
        KeyCode::Char('k') if ctrl => {
            app.mode = Mode::Switcher;
            app.switcher = Switcher::default();
        }
        KeyCode::F(9) => app.nicklist_visible = !app.nicklist_visible,
        KeyCode::Left if alt => switch_relative(app, -1),
        KeyCode::Right if alt => switch_relative(app, 1),
        KeyCode::Char(c) if alt && c.is_ascii_digit() => {
            let n = c.to_digit(10).unwrap() as usize;
            if n >= 1 {
                app.switch_to(n - 1);
            }
        }
        KeyCode::Esc => app.completion = None,
        KeyCode::Tab => complete(app),
        KeyCode::Enter => return submit(app),
        KeyCode::Backspace => {
            backspace(app);
            app.completion = None;
        }
        KeyCode::Left => move_left(app),
        KeyCode::Right => move_right(app),
        KeyCode::Home => app.cursor = 0,
        KeyCode::End => app.cursor = app.input.len(),
        KeyCode::Up => app.history_prev(),
        KeyCode::Down => app.history_next(),
        KeyCode::PageUp => return app.scroll(1).into_iter().collect(),
        KeyCode::PageDown => return app.scroll(-1).into_iter().collect(),
        KeyCode::Char(c) if !ctrl && !alt => {
            app.input.insert(app.cursor, c);
            app.cursor += c.len_utf8();
            app.completion = None;
        }
        _ => {}
    }
    Vec::new()
}

fn handle_switcher(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Enter => {
            let matches = switcher_matches(app);
            if let Some(&idx) = matches.get(app.switcher.sel) {
                app.switch_to(idx);
            }
            app.mode = Mode::Normal;
        }
        KeyCode::Up => app.switcher.sel = app.switcher.sel.saturating_sub(1),
        KeyCode::Down => {
            let n = switcher_matches(app).len();
            app.switcher.sel = (app.switcher.sel + 1).min(n.saturating_sub(1));
        }
        KeyCode::Backspace => {
            app.switcher.query.pop();
            app.switcher.sel = 0;
        }
        KeyCode::Char(c) => {
            app.switcher.query.push(c);
            app.switcher.sel = 0;
        }
        _ => {}
    }
}

/// Buffer indices matching the switcher query.
pub fn switcher_matches(app: &App) -> Vec<usize> {
    let q = app.switcher.query.to_lowercase();
    app.buffers
        .iter()
        .enumerate()
        .filter(|(_, b)| {
            let label = if b.kind == BufferKind::Server {
                format!("{} status", app.networks[b.net].name)
            } else {
                b.name.clone()
            };
            label.to_lowercase().contains(&q)
        })
        .map(|(i, _)| i)
        .collect()
}

fn switch_relative(app: &mut App, delta: isize) {
    if app.buffers.is_empty() {
        return;
    }
    let n = app.buffers.len() as isize;
    let next = (app.active as isize + delta).rem_euclid(n) as usize;
    app.switch_to(next);
}

fn submit(app: &mut App) -> Vec<String> {
    let text = app.input.trim().to_string();
    app.input.clear();
    app.cursor = 0;
    app.completion = None;
    if text.is_empty() {
        return Vec::new();
    }
    app.remember_input(&text);

    // TUI-local commands that open/close buffers.
    if let Some(rest) = text.strip_prefix('/') {
        let mut it = rest.splitn(2, ' ');
        let cmd = it.next().unwrap_or("").to_ascii_lowercase();
        let arg = it.next().unwrap_or("").trim();
        match cmd.as_str() {
            "query" | "q" => {
                match arg.split_whitespace().next() {
                    Some(nick) => {
                        let net = app.active_buffer().net;
                        app.open_query(net, nick);
                    }
                    None => app.push_active_event("usage: /query <nick>".to_string()),
                }
                return Vec::new();
            }
            "close" | "wc" => {
                return match app.close_active() {
                    Some(channel) => vec![format!("PART {channel}")],
                    None => Vec::new(),
                };
            }
            _ => {}
        }
    }

    let buffer = app.active_buffer();
    let target = match buffer.kind {
        BufferKind::Server => None,
        _ => Some(buffer.name.clone()),
    };
    // Plain text needs a target; a server buffer only takes commands.
    if target.is_none() && !text.starts_with('/') {
        app.push_active_event("no target here; join a channel or /query <nick>".to_string());
        return Vec::new();
    }

    let mut current = target;
    let result = crate::input::translate(&text, &mut current);
    if let Some(feedback) = result.feedback {
        app.push_active_event(feedback);
    }
    if result.quit {
        app.should_quit = true;
        return Vec::new();
    }
    result.lines
}

fn complete(app: &mut App) {
    if let Some(completion) = &app.completion {
        let idx = (completion.idx + 1) % completion.matches.len();
        apply_completion(app, idx);
        return;
    }
    let start = word_start(&app.input, app.cursor);
    let stem = app.input[start..app.cursor].to_lowercase();
    if stem.is_empty() {
        return;
    }
    let my_nick = app.my_nick().to_lowercase();
    let matches: Vec<String> = app
        .active_buffer()
        .members
        .iter()
        .map(|m| m.nick.clone())
        .filter(|n| n.to_lowercase().starts_with(&stem) && n.to_lowercase() != my_nick)
        .collect();
    if matches.is_empty() {
        return;
    }
    app.completion = Some(Completion {
        matches,
        idx: 0,
        start,
    });
    apply_completion(app, 0);
}

fn apply_completion(app: &mut App, idx: usize) {
    let Some(completion) = app.completion.as_mut() else {
        return;
    };
    completion.idx = idx;
    let nick = completion.matches[idx].clone();
    let start = completion.start;
    let suffix = if start == 0 { ": " } else { "" };
    app.input = format!("{}{nick}{suffix}", &app.input[..start]);
    app.cursor = app.input.len();
}

fn word_start(input: &str, cursor: usize) -> usize {
    input[..cursor].rfind(' ').map(|i| i + 1).unwrap_or(0)
}

fn backspace(app: &mut App) {
    if app.cursor == 0 {
        return;
    }
    let prev = app.input[..app.cursor]
        .chars()
        .next_back()
        .map(char::len_utf8)
        .unwrap_or(1);
    let start = app.cursor - prev;
    app.input.replace_range(start..app.cursor, "");
    app.cursor = start;
}

fn move_left(app: &mut App) {
    if app.cursor > 0 {
        let prev = app.input[..app.cursor]
            .chars()
            .next_back()
            .map(char::len_utf8)
            .unwrap_or(1);
        app.cursor -= prev;
    }
}

fn move_right(app: &mut App) {
    if app.cursor < app.input.len() {
        let next = app.input[app.cursor..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1);
        app.cursor += next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::ConnState;
    use crate::tui::state::{App, NetworkMeta};
    use ratatui::style::Color;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn app_with_channel() -> App {
        let nets = vec![NetworkMeta {
            name: "net".into(),
            my_nick: "me".into(),
            state: ConnState::Connecting,
        }];
        let mut app = App::new(nets, true, Color::Reset);
        app.apply(crate::session::UiEvent {
            net: 0,
            kind: crate::session::UiEventKind::Engine(irc_engine::Event::NamesLoaded {
                target: "#rust".into(),
                members: vec![nick_member("alice"), nick_member("albert")],
            }),
        });
        let idx = app.buffers.iter().position(|b| b.name == "#rust").unwrap();
        app.switch_to(idx);
        app
    }

    fn nick_member(n: &str) -> irc_engine::Member {
        irc_engine::Member {
            nick: n.into(),
            prefixes: vec![],
            away: false,
        }
    }

    #[test]
    fn typing_and_backspace() {
        let mut app = app_with_channel();
        handle_key(&mut app, key(KeyCode::Char('h')));
        handle_key(&mut app, key(KeyCode::Char('i')));
        assert_eq!(app.input, "hi");
        handle_key(&mut app, key(KeyCode::Backspace));
        assert_eq!(app.input, "h");
    }

    #[test]
    fn enter_sends_privmsg_to_active_channel() {
        let mut app = app_with_channel();
        for c in "hello".chars() {
            handle_key(&mut app, key(KeyCode::Char(c)));
        }
        let out = handle_key(&mut app, key(KeyCode::Enter));
        assert_eq!(out, vec!["PRIVMSG #rust :hello".to_string()]);
        assert!(app.input.is_empty());
    }

    #[test]
    fn tab_completes_and_cycles() {
        let mut app = app_with_channel();
        handle_key(&mut app, key(KeyCode::Char('a')));
        handle_key(&mut app, key(KeyCode::Char('l')));
        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.input, "alice: "); // first match, at line start
        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.input, "albert: "); // cycles to next
    }

    #[test]
    fn ctrl_k_opens_switcher_and_filters() {
        let mut app = app_with_channel();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.mode, Mode::Switcher);
        handle_key(&mut app, key(KeyCode::Char('r')));
        let matches = switcher_matches(&app);
        assert!(matches.iter().any(|&i| app.buffers[i].name == "#rust"));
    }

    #[test]
    fn f9_toggles_nicklist() {
        let mut app = app_with_channel();
        assert!(app.nicklist_visible);
        handle_key(&mut app, key(KeyCode::F(9)));
        assert!(!app.nicklist_visible);
    }

    #[test]
    fn unknown_command_shows_feedback_in_buffer_and_sends_nothing() {
        let mut app = app_with_channel();
        for c in "/nope".chars() {
            handle_key(&mut app, key(KeyCode::Char(c)));
        }
        let out = handle_key(&mut app, key(KeyCode::Enter));
        assert!(out.is_empty(), "nothing is sent to the server");
        let has_feedback = app.active_buffer().lines.iter().any(|l| {
            matches!(l, crate::tui::state::Line::Event { text, .. } if text.contains("unknown command"))
        });
        assert!(has_feedback, "feedback shows in the active buffer");
    }

    #[test]
    fn up_arrow_recalls_history() {
        let mut app = app_with_channel();
        for c in "hello".chars() {
            handle_key(&mut app, key(KeyCode::Char(c)));
        }
        handle_key(&mut app, key(KeyCode::Enter));
        assert!(app.input.is_empty());
        handle_key(&mut app, key(KeyCode::Up));
        assert_eq!(app.input, "hello");
        handle_key(&mut app, key(KeyCode::Down));
        assert_eq!(app.input, ""); // back to the (empty) draft
    }

    #[test]
    fn query_command_opens_a_query_buffer() {
        let mut app = app_with_channel();
        for c in "/query bob".chars() {
            handle_key(&mut app, key(KeyCode::Char(c)));
        }
        handle_key(&mut app, key(KeyCode::Enter));
        assert_eq!(app.active_buffer().name, "bob");
        assert_eq!(
            app.active_buffer().kind,
            crate::tui::state::BufferKind::Query
        );
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn clicking_sidebar_switches_buffer() {
        // Sidebar rows: 0=network header (server buffer), 1=#rust.
        let mut app = app_with_channel();
        let click = MouseEventKind::Down(MouseButton::Left);
        handle_mouse(&mut app, mouse(click, 5, 0), 100, 24);
        assert_eq!(app.active_buffer().kind, BufferKind::Server);
        handle_mouse(&mut app, mouse(click, 5, 1), 100, 24);
        assert_eq!(app.active_buffer().name, "#rust");
    }

    #[test]
    fn clicking_a_nick_opens_a_query() {
        // Nicklist rows: 0=header, 1=albert, 2=alice (sorted). Width 100 -> the
        // nick column starts at 82.
        let mut app = app_with_channel();
        let click = MouseEventKind::Down(MouseButton::Left);
        handle_mouse(&mut app, mouse(click, 90, 1), 100, 24);
        assert_eq!(app.active_buffer().name, "albert");
        assert_eq!(app.active_buffer().kind, BufferKind::Query);
    }
}
