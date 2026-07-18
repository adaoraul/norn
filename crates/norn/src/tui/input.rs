//! Key handling for the TUI: input editing, commands, completion, switcher.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::state::{App, BufferKind, Completion, Mode, Switcher};
use crate::session::NetCommand;

/// Width of the sidebar column.
const SIDEBAR_W: u16 = 24;
/// Width of the nicklist column.
const NICKLIST_W: u16 = 18;

/// Handle a mouse event: click the sidebar to switch buffers, click a nick to
/// open a query, or scroll the message view. Returns any commands to send (a
/// scroll to the top may request older history).
pub fn handle_mouse(app: &mut App, event: MouseEvent, width: u16, _height: u16) -> Vec<NetCommand> {
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
/// Row 0 is the console; each network header row selects that network's server
/// buffer, followed by its channels/queries.
fn sidebar_buffer_at(app: &App, y: u16) -> Option<usize> {
    let mut row = 0u16;
    // Row 0: the global console.
    if y == row {
        return app
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Status);
    }
    row += 1;
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

/// Handle one key. Mutates `app` and returns commands to send to the active
/// buffer's network (empty for local-only keys). Sets `app.should_quit` on quit.
pub fn handle_key(app: &mut App, key: KeyEvent) -> Vec<NetCommand> {
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

fn submit(app: &mut App) -> Vec<NetCommand> {
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
                    Some(channel) => vec![NetCommand::Raw(format!("PART {channel}"))],
                    None => Vec::new(),
                };
            }
            "set" => {
                handle_set(app, arg);
                return Vec::new();
            }
            "network" | "net" => {
                handle_network(app, arg);
                return Vec::new();
            }
            _ => {}
        }
    }

    let buffer = app.active_buffer();
    let target = match buffer.kind {
        BufferKind::Status | BufferKind::Server => None,
        _ => Some(buffer.name.clone()),
    };
    // Plain text needs a target; the console and server buffers only take commands.
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
    result.lines.into_iter().map(NetCommand::Raw).collect()
}

/// `/set` - view or change a client setting, applied live and auto-saved.
fn handle_set(app: &mut App, arg: &str) {
    let mut it = arg.split_whitespace();
    let Some(key) = it.next() else {
        // No args: show the current settings in the console.
        app.push_console("settings:".to_string());
        app.push_console(format!("  timestamps = {}", app.client.timestamps));
        app.push_console(format!("  nicklist   = {}", app.client.nicklist));
        app.push_console(format!(
            "  theme      = {}   ({})",
            app.client.theme,
            crate::tui::theme::THEME_NAMES.join(", ")
        ));
        app.switch_to_console();
        return;
    };
    let value = it.collect::<Vec<_>>().join(" ");
    if value.is_empty() {
        app.push_active_event(format!("usage: /set {key} <value>"));
        return;
    }
    match key {
        "timestamps" => match parse_bool(&value) {
            Some(on) => {
                app.timestamps = on;
                app.client.timestamps = on;
            }
            None => return app.push_active_event(format!("expected on/off, got '{value}'")),
        },
        "nicklist" => match parse_bool(&value) {
            Some(on) => {
                app.nicklist_visible = on;
                app.client.nicklist = on;
            }
            None => return app.push_active_event(format!("expected on/off, got '{value}'")),
        },
        "theme" => {
            if !crate::tui::theme::THEME_NAMES.contains(&value.as_str()) {
                return app.push_active_event(format!(
                    "unknown theme '{value}' (try: {})",
                    crate::tui::theme::THEME_NAMES.join(", ")
                ));
            }
            app.accent = crate::tui::theme::accent_for(&value);
            app.client.theme = value.clone();
        }
        other => return app.push_active_event(format!("unknown setting '{other}'")),
    }
    app.save_config();
    app.push_active_event(format!("set {key} = {value}"));
}

/// `/network list|add|remove` - manage persisted network definitions.
fn handle_network(app: &mut App, arg: &str) {
    let mut it = arg.splitn(2, ' ');
    let sub = it.next().unwrap_or("").to_ascii_lowercase();
    let rest = it.next().unwrap_or("").trim();
    match sub.as_str() {
        "list" | "" => {
            let mut lines = vec!["networks:".to_string()];
            if app.definitions.is_empty() {
                lines.push("  (none defined)".to_string());
            }
            for net in &app.definitions {
                let sasl = net
                    .sasl_account
                    .as_deref()
                    .map(|a| format!(" sasl={a}/{:?}", net.sasl_mech).to_lowercase())
                    .unwrap_or_default();
                lines.push(format!(
                    "  {}  {}:{}{}{}",
                    net.name,
                    net.host,
                    net.port,
                    if net.tls { "" } else { " (no tls)" },
                    sasl
                ));
            }
            for line in lines {
                app.push_console(line);
            }
            app.switch_to_console();
        }
        "add" => match parse_network_add(rest) {
            Ok(net) => {
                let name = net.name.clone();
                if let Some(slot) = app
                    .definitions
                    .iter_mut()
                    .find(|n| n.name.eq_ignore_ascii_case(&name))
                {
                    *slot = net;
                    app.save_config();
                    app.push_active_event(format!("updated network '{name}'"));
                } else {
                    app.definitions.push(net);
                    app.save_config();
                    app.push_active_event(format!("added network '{name}' (/connect to dial it)"));
                }
            }
            Err(msg) => app.push_active_event(msg),
        },
        "remove" | "rm" | "del" => {
            let name = rest.split_whitespace().next().unwrap_or("");
            let before = app.definitions.len();
            app.definitions
                .retain(|n| !n.name.eq_ignore_ascii_case(name));
            if app.definitions.len() == before {
                app.push_active_event(format!("no network named '{name}'"));
            } else {
                app.save_config();
                app.push_active_event(format!("removed network '{name}'"));
            }
        }
        other => app.push_active_event(format!("usage: /network list|add|remove (got '{other}')")),
    }
}

/// Parse `on|off|true|false|yes|no|1|0` into a bool.
fn parse_bool(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Some(true),
        "off" | "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

/// Split a command argument string into tokens, honoring double quotes so a
/// value like `password_command="pass irc/libera"` stays one token.
fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut has_token = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                has_token = true;
            }
            c if c.is_whitespace() && !in_quote => {
                if has_token {
                    out.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            c => {
                cur.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        out.push(cur);
    }
    out
}

/// Parse a `/network add` argument list into a `NetworkConfig`. Passwords are
/// never accepted inline; only `password_command` (a shell command).
fn parse_network_add(arg: &str) -> Result<crate::config::NetworkConfig, String> {
    use crate::config::SaslMech;
    const USAGE: &str = "usage: /network add <name> host=<server> nick=<you> \
[port=] [tls=on|off] [user=] [realname=] [sasl_account=] [sasl_mech=plain|scram] \
[password_command=\"...\"] [join=#a,#b]";

    let tokens = split_args(arg);
    let mut tokens = tokens.into_iter();
    let name = tokens.next().filter(|n| !n.contains('=')).ok_or(USAGE)?;

    let (mut host, mut nick) = (None, None);
    let (mut port, mut tls) = (6697u16, true);
    let (mut user, mut realname, mut sasl_account, mut password_command) = (None, None, None, None);
    let mut sasl_mech = SaslMech::Plain;
    let mut auto_join = Vec::new();

    for token in tokens {
        let (key, value) = token
            .split_once('=')
            .ok_or_else(|| format!("expected key=value, got '{token}'"))?;
        match key {
            "host" => host = Some(value.to_string()),
            "nick" => nick = Some(value.to_string()),
            "port" => port = value.parse().map_err(|_| format!("bad port '{value}'"))?,
            "tls" => tls = parse_bool(value).ok_or_else(|| format!("bad tls '{value}'"))?,
            "user" => user = Some(value.to_string()),
            "realname" => realname = Some(value.to_string()),
            "sasl_account" => sasl_account = Some(value.to_string()),
            "sasl_mech" => {
                sasl_mech = match value.to_ascii_lowercase().as_str() {
                    "plain" => SaslMech::Plain,
                    "scram" => SaslMech::Scram,
                    other => return Err(format!("bad sasl_mech '{other}' (plain|scram)")),
                }
            }
            "password_command" => password_command = Some(value.to_string()),
            "join" => auto_join = value.split(',').map(str::to_string).collect(),
            "password" | "pass" => {
                return Err(
                    "passwords are never stored; use password_command or NORN_PASSWORD".into(),
                )
            }
            other => return Err(format!("unknown key '{other}'")),
        }
    }

    Ok(crate::config::NetworkConfig {
        name,
        host: host.ok_or("host= is required")?,
        port,
        tls,
        nick: nick.ok_or("nick= is required")?,
        user,
        realname,
        sasl_account,
        sasl_mech,
        password_command,
        auto_join,
    })
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
    use crate::config::ClientConfig;
    use crate::session::ConnState;
    use crate::tui::state::{App, BufferKind, NetworkMeta};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn app_with_channel() -> App {
        let nets = vec![NetworkMeta {
            name: "net".into(),
            my_nick: "me".into(),
            state: ConnState::Connecting,
        }];
        let mut app = App::new(nets, ClientConfig::default(), Vec::new(), None);
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
        assert_eq!(
            out,
            vec![NetCommand::Raw("PRIVMSG #rust :hello".to_string())]
        );
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
        // Sidebar rows: 0=console, 1=network header (server buffer), 2=#rust.
        let mut app = app_with_channel();
        let click = MouseEventKind::Down(MouseButton::Left);
        handle_mouse(&mut app, mouse(click, 5, 0), 100, 24);
        assert_eq!(app.active_buffer().kind, BufferKind::Status);
        handle_mouse(&mut app, mouse(click, 5, 1), 100, 24);
        assert_eq!(app.active_buffer().kind, BufferKind::Server);
        handle_mouse(&mut app, mouse(click, 5, 2), 100, 24);
        assert_eq!(app.active_buffer().name, "#rust");
    }

    /// Type `text` into `app` and press Enter, returning the produced commands.
    fn run_line(app: &mut App, text: &str) -> Vec<NetCommand> {
        for c in text.chars() {
            handle_key(app, key(KeyCode::Char(c)));
        }
        handle_key(app, key(KeyCode::Enter))
    }

    #[test]
    fn set_theme_updates_accent_and_client() {
        let mut app = app_with_channel();
        assert_eq!(app.client.theme, "teal");
        let out = run_line(&mut app, "/set theme amber");
        assert!(out.is_empty(), "no server traffic for /set");
        assert_eq!(app.client.theme, "amber");
        assert_eq!(app.accent, crate::tui::theme::accent_for("amber"));
    }

    #[test]
    fn set_timestamps_toggles_live_state() {
        let mut app = app_with_channel();
        run_line(&mut app, "/set timestamps off");
        assert!(!app.timestamps);
        assert!(!app.client.timestamps);
        run_line(&mut app, "/set timestamps on");
        assert!(app.timestamps);
    }

    #[test]
    fn set_rejects_unknown_theme() {
        let mut app = app_with_channel();
        run_line(&mut app, "/set theme chartreuse");
        assert_eq!(app.client.theme, "teal", "invalid theme is not applied");
    }

    #[test]
    fn network_add_defines_and_lists() {
        let mut app = app_with_channel();
        run_line(
            &mut app,
            "/network add libera host=irc.libera.chat nick=svan sasl_account=svan sasl_mech=scram join=#rust,#ratatui",
        );
        assert_eq!(app.definitions.len(), 1);
        let net = &app.definitions[0];
        assert_eq!(net.name, "libera");
        assert_eq!(net.host, "irc.libera.chat");
        assert_eq!(net.sasl_mech, crate::config::SaslMech::Scram);
        assert_eq!(net.auto_join, vec!["#rust", "#ratatui"]);
        // remove drops it.
        run_line(&mut app, "/network remove libera");
        assert!(app.definitions.is_empty());
    }

    #[test]
    fn network_add_rejects_inline_password_and_missing_host() {
        let mut app = app_with_channel();
        run_line(&mut app, "/network add x nick=n password=secret");
        assert!(app.definitions.is_empty(), "inline password refused");
        run_line(&mut app, "/network add y nick=n");
        assert!(app.definitions.is_empty(), "host is required");
    }

    #[test]
    fn split_args_honors_quotes() {
        let parts = split_args(r#"a host=irc.x password_command="pass irc/x""#);
        assert_eq!(
            parts,
            vec!["a", "host=irc.x", "password_command=pass irc/x"]
        );
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
