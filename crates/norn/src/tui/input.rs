//! Key handling for the TUI: input editing, commands, completion, switcher.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::state::{
    network_field_value, App, AppAction, BufferKind, Completion, Mode, NetFieldKind, NetworksFocus,
    Switcher, NETWORK_FIELDS,
};
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
    if app.mode == Mode::Help {
        handle_help(app, key);
        return Vec::new();
    }
    if app.mode == Mode::Settings {
        handle_settings(app, key);
        return Vec::new();
    }
    if app.mode == Mode::Networks {
        handle_networks(app, key);
        return Vec::new();
    }
    if app.mode == Mode::Plugins {
        handle_plugins_screen(app, key);
        return Vec::new();
    }
    if app.mode == Mode::PluginConfig {
        handle_plugin_config_screen(app, key);
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
        KeyCode::F(2) => app.open_settings(),
        KeyCode::F(3) => app.open_networks(),
        KeyCode::F(4) => app.open_plugins(),
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

/// Keys for the `/help` panel. In the list pane: type to filter, arrows move the
/// selection, Right focuses the detail. In the detail pane: arrows scroll, Left
/// returns. Enter inserts the selected command; Esc closes.
fn handle_help(app: &mut App, key: KeyEvent) {
    use crate::tui::state::HelpFocus;
    let insert_selected = |app: &mut App| {
        if let Some(doc) = crate::commands::help_commands(&app.help.query).get(app.help.sel) {
            app.input = format!("/{} ", doc.name);
            app.cursor = app.input.len();
        }
        app.mode = Mode::Normal;
    };
    match app.help.focus {
        HelpFocus::List => match key.code {
            KeyCode::Esc => app.mode = Mode::Normal,
            KeyCode::Enter => insert_selected(app),
            KeyCode::Right => {
                app.help.focus = HelpFocus::Detail;
                app.help.detail_scroll = 0;
            }
            KeyCode::Up => app.help.sel = app.help.sel.saturating_sub(1),
            KeyCode::Down => app.help.sel += 1,
            KeyCode::PageUp => app.help.sel = app.help.sel.saturating_sub(10),
            KeyCode::PageDown => app.help.sel += 10,
            KeyCode::Backspace => {
                app.help.query.pop();
                app.help.sel = 0;
            }
            KeyCode::Char(c) => {
                app.help.query.push(c);
                app.help.sel = 0;
            }
            _ => {}
        },
        HelpFocus::Detail => match key.code {
            KeyCode::Esc => app.mode = Mode::Normal,
            KeyCode::Enter => insert_selected(app),
            KeyCode::Left | KeyCode::Backspace => app.help.focus = HelpFocus::List,
            KeyCode::Up => app.help.detail_scroll = app.help.detail_scroll.saturating_sub(1),
            KeyCode::Down => app.help.detail_scroll += 1,
            KeyCode::PageUp => app.help.detail_scroll = app.help.detail_scroll.saturating_sub(5),
            KeyCode::PageDown => app.help.detail_scroll += 5,
            _ => {}
        },
    }
    // Clamp the selection to the current match count.
    let n = crate::commands::help_commands(&app.help.query).len();
    app.help.sel = app.help.sel.min(n.saturating_sub(1));
}

/// Keys for the `/settings` panel. While browsing: arrows move the selection,
/// typing filters, Enter edits/toggles the selected row, Left/Right adjust a
/// bool/enum, Delete removes an alias, Esc closes. While editing a text/int
/// setting or an alias, keys edit the inline buffer (Enter commits, Esc cancels).
fn handle_settings(app: &mut App, key: KeyEvent) {
    if app.settings.editing.is_some() {
        handle_settings_edit(app, key);
        return;
    }
    let n = app.settings_selectable().len();
    match key.code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Up => {
            app.settings.sel = app.settings.sel.saturating_sub(1);
            app.settings.msg = None;
        }
        KeyCode::Down => {
            app.settings.sel = (app.settings.sel + 1).min(n.saturating_sub(1));
            app.settings.msg = None;
        }
        KeyCode::Enter => activate_setting(app),
        KeyCode::Left => adjust_setting(app, -1),
        KeyCode::Right => adjust_setting(app, 1),
        KeyCode::Delete => delete_selected_alias(app),
        KeyCode::Backspace => {
            app.settings.filter.pop();
            app.settings.sel = 0;
            app.settings.msg = None;
        }
        KeyCode::Char(c) => {
            app.settings.filter.push(c);
            app.settings.sel = 0;
            app.settings.msg = None;
        }
        _ => {}
    }
}

/// Enter on the selected row: toggle a bool / cycle an enum in place, or begin
/// inline editing of a text/int setting or an alias.
fn activate_setting(app: &mut App) {
    use crate::settings::SettingKind;
    use crate::tui::state::SettingsRow;
    match app.selected_setting_row() {
        Some(SettingsRow::Setting(doc)) => match doc.kind {
            SettingKind::Bool | SettingKind::Enum(_) => adjust_setting(app, 1),
            SettingKind::Int { .. } | SettingKind::Str => {
                app.settings.editing = Some(app.setting_value(doc.key));
                app.settings.msg = None;
            }
        },
        Some(SettingsRow::Alias { expansion, .. }) => {
            app.settings.editing = Some(expansion);
            app.settings.msg = None;
        }
        Some(SettingsRow::AddAlias) => {
            app.settings.editing = Some(String::new());
            app.settings.msg = None;
        }
        _ => {}
    }
}

/// Adjust the selected bool (toggle) or enum (cycle by `dir`) setting in place.
fn adjust_setting(app: &mut App, dir: isize) {
    use crate::settings::SettingKind;
    use crate::tui::state::SettingsRow;
    let Some(SettingsRow::Setting(doc)) = app.selected_setting_row() else {
        return;
    };
    let result = match doc.kind {
        SettingKind::Bool => {
            let on = app.setting_value(doc.key) == "on";
            app.set_setting(doc.key, if on { "off" } else { "on" })
        }
        SettingKind::Enum(values) => {
            let cur = app.setting_value(doc.key);
            let i = values.iter().position(|&v| v == cur).unwrap_or(0);
            let next = (i as isize + dir).rem_euclid(values.len() as isize) as usize;
            app.set_setting(doc.key, values[next])
        }
        _ => return,
    };
    if let Err(err) = result {
        app.settings.msg = Some(err);
    }
}

/// Delete the alias on the selected row (a no-op on a setting row).
fn delete_selected_alias(app: &mut App) {
    use crate::tui::state::SettingsRow;
    let Some(SettingsRow::Alias { name, .. }) = app.selected_setting_row() else {
        return;
    };
    app.aliases.remove(&name);
    app.save_config();
    app.settings.msg = Some(format!("removed alias /{name}"));
    let n = app.settings_selectable().len();
    if app.settings.sel >= n {
        app.settings.sel = n.saturating_sub(1);
    }
}

/// Keys while inline-editing a settings value: type into the buffer, Enter
/// commits, Esc cancels.
fn handle_settings_edit(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.settings.editing = None;
            app.settings.msg = None;
        }
        KeyCode::Enter => commit_settings_edit(app),
        KeyCode::Backspace => {
            if let Some(buf) = app.settings.editing.as_mut() {
                buf.pop();
            }
        }
        KeyCode::Char(c) => {
            if let Some(buf) = app.settings.editing.as_mut() {
                buf.push(c);
            }
        }
        _ => {}
    }
}

/// Commit the inline edit to the selected setting or alias. On a validation
/// error, keep editing and show the message.
fn commit_settings_edit(app: &mut App) {
    use crate::tui::state::SettingsRow;
    let Some(buf) = app.settings.editing.clone() else {
        return;
    };
    match app.selected_setting_row() {
        Some(SettingsRow::Setting(doc)) => match app.set_setting(doc.key, &buf) {
            Ok(_) => {
                app.settings.editing = None;
                app.settings.msg = None;
            }
            Err(err) => app.settings.msg = Some(err),
        },
        Some(SettingsRow::Alias { name, .. }) => {
            let expansion = buf.trim().to_string();
            if expansion.is_empty() {
                app.settings.msg = Some("expansion cannot be empty".to_string());
            } else {
                app.aliases.insert(name, expansion);
                app.save_config();
                app.settings.editing = None;
                app.settings.msg = None;
            }
        }
        Some(SettingsRow::AddAlias) => {
            // The buffer is `name expansion`; split on the first space.
            let mut parts = buf.trim().splitn(2, ' ');
            let name = parts
                .next()
                .unwrap_or("")
                .trim_start_matches('/')
                .to_ascii_lowercase();
            let expansion = parts.next().unwrap_or("").trim().to_string();
            if name.is_empty() || expansion.is_empty() {
                app.settings.msg = Some("type: name expansion".to_string());
            } else if STRUCTURAL.contains(&name.as_str()) {
                app.settings.msg = Some(format!("cannot alias the built-in /{name}"));
            } else {
                app.aliases.insert(name.clone(), expansion);
                app.save_config();
                app.settings.editing = None;
                app.settings.msg = Some(format!("added alias /{name}"));
                // A new alias row was inserted above the add row; keep the
                // selection on the add row so another can be added.
                app.settings.sel = app.settings_selectable().len().saturating_sub(1);
            }
        }
        _ => app.settings.editing = None,
    }
}

/// Keys for the `/networks` manager. Routes to the list pane, the form pane, or
/// the inline field editor depending on focus and edit state.
fn handle_networks(app: &mut App, key: KeyEvent) {
    if app.networks_ui.editing.is_some() {
        handle_networks_edit(app, key);
        return;
    }
    match app.networks_ui.focus {
        NetworksFocus::List => handle_networks_list(app, key),
        NetworksFocus::Form => handle_networks_form(app, key),
    }
}

/// List pane: move over the definitions and the add row; Enter/Right opens the
/// form (or creates a network on the add row); c/d connect/disconnect; x/Delete
/// removes; Esc closes.
fn handle_networks_list(app: &mut App, key: KeyEvent) {
    let add_row = app.definitions.len(); // index of the "+ add network" row
    match key.code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Up => {
            app.networks_ui.sel = app.networks_ui.sel.saturating_sub(1);
            app.networks_ui.msg = None;
        }
        KeyCode::Down => {
            app.networks_ui.sel = (app.networks_ui.sel + 1).min(add_row);
            app.networks_ui.msg = None;
        }
        KeyCode::Enter | KeyCode::Right => {
            if app.networks_ui.sel >= add_row {
                app.add_network_definition();
            } else {
                app.networks_ui.focus = NetworksFocus::Form;
                app.networks_ui.field = 0;
                app.networks_ui.msg = None;
            }
        }
        KeyCode::Char('c') => connect_selected_network(app),
        KeyCode::Char('d') => disconnect_selected_network(app),
        KeyCode::Char('x') | KeyCode::Delete => {
            let sel = app.networks_ui.sel;
            if sel < add_row {
                app.delete_network_definition(sel);
            }
        }
        _ => {}
    }
}

/// Form pane: move between fields; Enter edits a text field or toggles/cycles a
/// bool/mech; Space/Right also toggles/cycles; Left/Esc returns to the list.
fn handle_networks_form(app: &mut App, key: KeyEvent) {
    let last = NETWORK_FIELDS.len().saturating_sub(1);
    match key.code {
        KeyCode::Esc | KeyCode::Left => {
            app.networks_ui.focus = NetworksFocus::List;
            app.networks_ui.msg = None;
        }
        KeyCode::Up => app.networks_ui.field = app.networks_ui.field.saturating_sub(1),
        KeyCode::Down => app.networks_ui.field = (app.networks_ui.field + 1).min(last),
        KeyCode::Enter => activate_network_field(app),
        KeyCode::Char(' ') | KeyCode::Right => adjust_selected_network_field(app),
        _ => {}
    }
}

/// Enter on a form field: begin editing a text field, or toggle/cycle a
/// bool/mech field in place.
fn activate_network_field(app: &mut App) {
    let (idx, field) = (app.networks_ui.sel, app.networks_ui.field);
    match NETWORK_FIELDS.get(field).map(|f| f.kind) {
        Some(NetFieldKind::Text) => {
            if let Some(cfg) = app.definitions.get(idx) {
                app.networks_ui.editing = Some(network_field_value(cfg, field));
                app.networks_ui.msg = None;
            }
        }
        Some(NetFieldKind::Toggle) | Some(NetFieldKind::Mech) => adjust_selected_network_field(app),
        None => {}
    }
}

/// Toggle/cycle the selected bool/mech field (a no-op on a text field).
fn adjust_selected_network_field(app: &mut App) {
    let (idx, field) = (app.networks_ui.sel, app.networks_ui.field);
    if let Err(err) = app.adjust_network_field(idx, field) {
        app.networks_ui.msg = Some(err);
    }
}

/// Connect the network on the selected list row.
fn connect_selected_network(app: &mut App) {
    let sel = app.networks_ui.sel;
    if let Some(name) = app.definitions.get(sel).map(|c| c.name.clone()) {
        app.connect_network(&name);
        app.networks_ui.msg = Some(format!("connecting to {name}..."));
    }
}

/// Disconnect the network on the selected list row.
fn disconnect_selected_network(app: &mut App) {
    let sel = app.networks_ui.sel;
    if let Some(name) = app.definitions.get(sel).map(|c| c.name.clone()) {
        app.disconnect_network(&name);
        app.networks_ui.msg = Some(format!("disconnecting {name}"));
    }
}

/// Keys while inline-editing a network field: type into the buffer, Enter
/// commits (keeping the editor open on a validation error), Esc cancels.
fn handle_networks_edit(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.networks_ui.editing = None;
            app.networks_ui.msg = None;
        }
        KeyCode::Enter => {
            let Some(buf) = app.networks_ui.editing.clone() else {
                return;
            };
            let (idx, field) = (app.networks_ui.sel, app.networks_ui.field);
            match app.set_network_field(idx, field, &buf) {
                Ok(()) => {
                    app.networks_ui.editing = None;
                    app.networks_ui.msg = None;
                }
                Err(err) => app.networks_ui.msg = Some(err),
            }
        }
        KeyCode::Backspace => {
            if let Some(buf) = app.networks_ui.editing.as_mut() {
                buf.pop();
            }
        }
        KeyCode::Char(c) => {
            if let Some(buf) = app.networks_ui.editing.as_mut() {
                buf.push(c);
            }
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

/// Names of structural (stateful) commands that aliases cannot shadow.
const STRUCTURAL: &[&str] = &[
    "query",
    "q",
    "close",
    "wc",
    "set",
    "settings",
    "network",
    "networks",
    "net",
    "connect",
    "server",
    "disconnect",
    "reconnect",
    "clear",
    "alias",
    "unalias",
    "trigger",
    "plugins",
];

/// Max alias-expansion recursion depth (guards cyclic aliases).
const MAX_ALIAS_DEPTH: usize = 10;

fn submit(app: &mut App) -> Vec<NetCommand> {
    let text = app.input.trim().to_string();
    app.input.clear();
    app.cursor = 0;
    app.completion = None;
    if text.is_empty() {
        return Vec::new();
    }
    app.remember_input(&text);
    run_command(app, &text, 0)
}

/// Dispatch one input line: structural TUI commands first, then user aliases
/// (which may expand and chain), then IRC verbs via `translate`. `depth` guards
/// against cyclic aliases.
fn run_command(app: &mut App, text: &str, depth: usize) -> Vec<NetCommand> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    if depth > MAX_ALIAS_DEPTH {
        app.push_active_event("alias expansion too deep (cyclic alias?)".to_string());
        return Vec::new();
    }

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
            "settings" => {
                app.open_settings();
                return Vec::new();
            }
            "network" | "net" => {
                handle_network(app, arg);
                return Vec::new();
            }
            "networks" => {
                app.open_networks();
                return Vec::new();
            }
            "connect" | "server" => {
                match arg.split_whitespace().next() {
                    Some(name) => app.connect_network(name),
                    None => app.push_active_event("usage: /connect <name>".to_string()),
                }
                return Vec::new();
            }
            "disconnect" => {
                let reason = (!arg.is_empty()).then(|| arg.to_string());
                app.disconnect_active(reason);
                return Vec::new();
            }
            "reconnect" => {
                app.reconnect_active();
                return Vec::new();
            }
            "clear" => {
                app.clear_active();
                return Vec::new();
            }
            "alias" => {
                handle_alias(app, arg);
                return Vec::new();
            }
            "unalias" => {
                handle_unalias(app, arg);
                return Vec::new();
            }
            "trigger" => {
                handle_trigger(app, arg);
                return Vec::new();
            }
            "plugins" => {
                handle_plugins(app, arg);
                return Vec::new();
            }
            "help" | "h" => {
                app.open_help(arg);
                return Vec::new();
            }
            _ => {}
        }

        // A user alias: expand (with parameters and `;` chaining) and re-dispatch.
        if let Some(template) = app.aliases.get(&cmd).cloned() {
            let args: Vec<&str> = arg.split_whitespace().collect();
            let mut out = Vec::new();
            for segment in expand_alias(&template, &args) {
                out.extend(run_command(app, &segment, depth + 1));
            }
            return out;
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
    let result = crate::input::translate(text, &mut current);
    if let Some(feedback) = result.feedback {
        app.push_active_event(feedback);
    }
    if result.quit {
        app.should_quit = true;
        return Vec::new();
    }
    result.lines.into_iter().map(NetCommand::Raw).collect()
}

/// `/alias` - list, or `/alias <name> <expansion>` to define (auto-saved).
fn handle_alias(app: &mut App, arg: &str) {
    if arg.is_empty() {
        let mut lines = vec!["aliases:".to_string()];
        if app.aliases.is_empty() {
            lines.push("  (none defined)".to_string());
        }
        for (name, expansion) in &app.aliases {
            lines.push(format!("  /{name} = {expansion}"));
        }
        for line in lines {
            app.push_console(line);
        }
        app.switch_to_console();
        return;
    }
    let mut it = arg.splitn(2, ' ');
    let name = it
        .next()
        .unwrap_or("")
        .trim_start_matches('/')
        .to_ascii_lowercase();
    let expansion = it.next().unwrap_or("").trim();
    if name.is_empty() || expansion.is_empty() {
        app.push_active_event("usage: /alias <name> <expansion>".to_string());
        return;
    }
    if STRUCTURAL.contains(&name.as_str()) {
        app.push_active_event(format!("cannot alias the built-in command /{name}"));
        return;
    }
    app.aliases.insert(name.clone(), expansion.to_string());
    app.save_config();
    app.push_active_event(format!("alias /{name} = {expansion}"));
}

/// `/unalias <name>` - remove a user alias (auto-saved).
fn handle_unalias(app: &mut App, arg: &str) {
    let name = arg
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_start_matches('/')
        .to_ascii_lowercase();
    if app.aliases.remove(&name).is_some() {
        app.save_config();
        app.push_active_event(format!("removed alias /{name}"));
    } else {
        app.push_active_event(format!("no alias /{name}"));
    }
}

/// `/trigger` - list, define, or remove addon triggers. Edits are auto-saved and
/// the live addon host is rebuilt via [`AppAction::ReloadAddons`].
fn handle_trigger(app: &mut App, arg: &str) {
    let mut it = arg.splitn(2, ' ');
    let sub = it.next().unwrap_or("").to_ascii_lowercase();
    let rest = it.next().unwrap_or("").trim();
    match sub.as_str() {
        "" | "ls" | "list" => {
            let mut lines = vec!["triggers:".to_string()];
            if app.triggers.is_empty() {
                lines.push("  (none defined)".to_string());
            }
            for (i, t) in app.triggers.iter().enumerate() {
                let off = if t.enabled { "" } else { "  (disabled)" };
                lines.push(format!("  {}. on {} = {}{off}", i + 1, t.on, t.run));
            }
            for line in lines {
                app.push_console(line);
            }
            app.switch_to_console();
        }
        "add" => {
            let Some((on, run)) = rest.split_once('=') else {
                app.push_active_event("usage: /trigger add <on> = <run>".to_string());
                return;
            };
            let (on, run) = (on.trim(), run.trim());
            if on.is_empty() || run.is_empty() {
                app.push_active_event("usage: /trigger add <on> = <run>".to_string());
                return;
            }
            if !crate::addons::matcher_is_valid(on) {
                app.push_active_event(format!(
                    "unknown trigger event '{on}' (try: highlight, message, notice, join, part, quit, nick)"
                ));
                return;
            }
            app.triggers.push(crate::config::TriggerConfig {
                on: on.to_string(),
                run: run.to_string(),
                enabled: true,
            });
            app.save_config();
            app.actions.push(AppAction::ReloadAddons);
            app.push_active_event(format!("added trigger: on {on} = {run}"));
        }
        "remove" | "rm" | "del" => {
            let Some(n) = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<usize>().ok())
            else {
                app.push_active_event("usage: /trigger rm <number>".to_string());
                return;
            };
            if n == 0 || n > app.triggers.len() {
                app.push_active_event(format!("no trigger {n} (see /trigger ls)"));
                return;
            }
            let removed = app.triggers.remove(n - 1);
            app.save_config();
            app.actions.push(AppAction::ReloadAddons);
            app.push_active_event(format!(
                "removed trigger: on {} = {}",
                removed.on, removed.run
            ));
        }
        other => app.push_active_event(format!("usage: /trigger ls|add|rm (got '{other}')")),
    }
}

/// Keys for the `/plugins` manager: arrows move the selection; Space/Enter
/// loads/unloads the selected script; Esc closes.
fn handle_plugins_screen(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Up => app.plugins_ui.sel = app.plugins_ui.sel.saturating_sub(1),
        KeyCode::Down => {
            app.plugins_ui.sel = (app.plugins_ui.sel + 1).min(app.plugins.len().saturating_sub(1));
        }
        KeyCode::Char(' ') | KeyCode::Enter => {
            let idx = app.plugins_ui.sel;
            app.toggle_plugin(idx);
        }
        KeyCode::Char('c') => app.open_plugin_config_selected(),
        _ => {}
    }
}

/// The per-plugin config editor (`Mode::PluginConfig`): navigate keys, edit a
/// value inline, save on Enter. Esc cancels an edit or closes back to plugins.
fn handle_plugin_config_screen(app: &mut App, key: KeyEvent) {
    let rows = app.plugin_cfg_schema();
    // Editing a value: capture text until Enter (commit) or Esc (cancel).
    if let Some(buf) = app.plugin_cfg.editing.clone() {
        match key.code {
            KeyCode::Esc => app.plugin_cfg.editing = None,
            KeyCode::Enter => {
                if let Some((k, default)) = rows.get(app.plugin_cfg.sel) {
                    app.set_plugin_cfg_value(k, buf.trim(), default);
                    app.plugin_cfg.msg = Some(format!("saved {k}"));
                }
                app.plugin_cfg.editing = None;
            }
            KeyCode::Backspace => {
                let mut b = buf;
                b.pop();
                app.plugin_cfg.editing = Some(b);
            }
            KeyCode::Char(c) => {
                let mut b = buf;
                b.push(c);
                app.plugin_cfg.editing = Some(b);
            }
            _ => {}
        }
        return;
    }
    match key.code {
        KeyCode::Esc => app.mode = Mode::Plugins,
        KeyCode::Up => app.plugin_cfg.sel = app.plugin_cfg.sel.saturating_sub(1),
        KeyCode::Down => {
            app.plugin_cfg.sel = (app.plugin_cfg.sel + 1).min(rows.len().saturating_sub(1));
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            if let Some((k, default)) = rows.get(app.plugin_cfg.sel) {
                app.plugin_cfg.msg = None;
                app.plugin_cfg.editing = Some(app.plugin_cfg_value(k, default));
            }
        }
        _ => {}
    }
}

/// `/plugins` - list addon scripts and their status, reload them, or
/// enable/disable one. (Bare `/plugins` opens the manager screen.)
fn handle_plugins(app: &mut App, arg: &str) {
    use crate::addons::PluginStatus;
    let mut it = arg.splitn(2, ' ');
    let sub = it.next().unwrap_or("").to_ascii_lowercase();
    let rest = it.next().unwrap_or("").trim();
    match sub.as_str() {
        "" => app.open_plugins(),
        "ls" | "list" => {
            let mut lines = vec!["plugins:".to_string()];
            if app.plugins.is_empty() {
                lines.push("  (no scripts; put *.rhai in the plugins dir)".to_string());
            }
            for p in &app.plugins {
                let status = match &p.status {
                    PluginStatus::Loaded => "on".to_string(),
                    PluginStatus::Disabled => "off".to_string(),
                    PluginStatus::Failed(err) => format!("failed: {err}"),
                };
                let version = if p.version.is_empty() {
                    String::new()
                } else {
                    format!(" v{}", p.version)
                };
                lines.push(format!(
                    "  [{status}] {}{version}  {}",
                    p.name, p.description
                ));
            }
            for line in lines {
                app.push_console(line);
            }
            app.switch_to_console();
        }
        "reload" => {
            app.actions.push(AppAction::ReloadAddons);
            app.push_active_event("reloading plugins...".to_string());
        }
        "available" => {
            let mut lines = vec!["available to install (/plugins install <name>):".to_string()];
            let mut any = false;
            for o in crate::addons::official::OFFICIAL {
                if app
                    .plugins
                    .iter()
                    .any(|p| p.name.eq_ignore_ascii_case(o.name))
                {
                    continue;
                }
                lines.push(format!("  {}  {}", o.name, o.description));
                any = true;
            }
            if !any {
                lines.push("  (all official plugins installed)".to_string());
            }
            for line in lines {
                app.push_console(line);
            }
            app.switch_to_console();
        }
        "install" => {
            let name = rest.split_whitespace().next().unwrap_or("");
            if name.is_empty() {
                app.push_active_event("usage: /plugins install <name>".to_string());
                return;
            }
            match app.install_plugin(name) {
                Ok(file) => app.push_active_event(format!("installed {file}")),
                Err(err) => app.push_active_event(err),
            }
        }
        "enable" | "disable" => {
            let enable = sub == "enable";
            let name = rest.split_whitespace().next().unwrap_or("");
            if name.is_empty() {
                app.push_active_event(format!("usage: /plugins {sub} <name>"));
                return;
            }
            match app.set_plugin_enabled(name, enable) {
                Ok(file) => app.push_active_event(format!(
                    "{} {file}",
                    if enable { "enabled" } else { "disabled" }
                )),
                Err(err) => app.push_active_event(err),
            }
        }
        "config" | "configure" => {
            let name = rest.split_whitespace().next().unwrap_or("");
            if name.is_empty() {
                app.push_active_event("usage: /plugins config <name>".to_string());
                return;
            }
            app.open_plugin_config(name);
        }
        other => app.push_active_event(format!(
            "usage: /plugins ls|available|install|reload|enable|disable|config (got '{other}')"
        )),
    }
}

/// Expand an alias template into one or more command lines. Segments split on
/// `;`. If the template has no `$` placeholder, the raw args are appended;
/// otherwise `$1..$9` and `$*` are substituted. Each result is normalized to a
/// slash-command.
fn expand_alias(template: &str, args: &[&str]) -> Vec<String> {
    let has_placeholder = template.contains('$');
    let base = if !has_placeholder && !args.is_empty() {
        format!("{template} {}", args.join(" "))
    } else {
        template.to_string()
    };
    base.split(';')
        .filter_map(|segment| {
            let segment = segment.trim();
            if segment.is_empty() {
                return None;
            }
            let expanded = if has_placeholder {
                substitute_args(segment, args)
            } else {
                segment.to_string()
            };
            Some(if expanded.starts_with('/') {
                expanded
            } else {
                format!("/{expanded}")
            })
        })
        .collect()
}

/// Substitute `$1..$9` (1-based positional) and `$*` (all args) in `segment`.
/// An out-of-range `$N` becomes empty; a `$` not forming a placeholder is kept.
fn substitute_args(segment: &str, args: &[&str]) -> String {
    let mut out = String::new();
    let mut chars = segment.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('*') => {
                chars.next();
                out.push_str(&args.join(" "));
            }
            Some(d) if d.is_ascii_digit() => {
                let n = d.to_digit(10).unwrap() as usize;
                chars.next();
                if n >= 1 {
                    if let Some(arg) = args.get(n - 1) {
                        out.push_str(arg);
                    }
                }
            }
            _ => out.push('$'),
        }
    }
    out
}

/// `/set` - view or change a client setting, validated against the settings
/// registry, applied live, and auto-saved. Shares `App::set_setting` with the
/// `/settings` screen.
fn handle_set(app: &mut App, arg: &str) {
    let mut it = arg.split_whitespace();
    let Some(key) = it.next() else {
        // No args: show all settings and their values in the console.
        app.push_console("settings:".to_string());
        for s in crate::settings::SETTINGS {
            app.push_console(format!("  {} = {}", s.key, app.setting_value(s.key)));
        }
        app.switch_to_console();
        return;
    };
    let value = it.collect::<Vec<_>>().join(" ");
    if value.is_empty() {
        app.push_active_event(format!("usage: /set {key} <value>"));
        return;
    }
    match app.set_setting(key, &value) {
        Ok(applied) => app.push_active_event(format!("set {key} = {applied}")),
        Err(err) => app.push_active_event(err),
    }
}

/// `/network list|add|remove` - manage persisted network definitions.
fn handle_network(app: &mut App, arg: &str) {
    let mut it = arg.splitn(2, ' ');
    let sub = it.next().unwrap_or("").to_ascii_lowercase();
    let rest = it.next().unwrap_or("").trim();
    match sub.as_str() {
        "list" | "ls" | "" => {
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
        "show" => {
            let name = rest.split_whitespace().next().unwrap_or("");
            let Some(net) = app
                .definitions
                .iter()
                .find(|n| n.name.eq_ignore_ascii_case(name))
                .cloned()
            else {
                app.push_active_event(format!("no network named '{name}'"));
                return;
            };
            // Live connection state, if a network of this name is connected.
            let live = app
                .networks
                .iter()
                .find(|m| m.name.eq_ignore_ascii_case(&net.name))
                .map(|m| format!("{:?}", m.state).to_lowercase());
            let mut lines = vec![
                format!("network {}:", net.name),
                format!("  host         = {}:{}", net.host, net.port),
                format!("  tls          = {}", net.tls),
                format!("  auto_connect = {}", net.auto_connect),
                format!("  identify     = {}", net.identify),
                format!("  nick         = {}", net.nick),
            ];
            if let Some(user) = &net.user {
                lines.push(format!("  user         = {user}"));
            }
            if let Some(realname) = &net.realname {
                lines.push(format!("  realname     = {realname}"));
            }
            if let Some(account) = &net.sasl_account {
                lines.push(format!(
                    "  sasl         = {account} ({})",
                    format!("{:?}", net.sasl_mech).to_lowercase()
                ));
                lines.push(format!(
                    "  password     = {}",
                    if net.password_command.is_some() {
                        "via password_command"
                    } else {
                        "via NORN_PASSWORD"
                    }
                ));
            }
            if !net.auto_join.is_empty() {
                lines.push(format!("  join         = {}", net.auto_join.join(", ")));
            }
            if let Some(state) = live {
                lines.push(format!("  state        = {state}"));
            }
            for line in lines {
                app.push_console(line);
            }
            app.switch_to_console();
        }
        other => app.push_active_event(format!("usage: /network ls|add|rm|show (got '{other}')")),
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
[password_command=\"...\"] [join=#a,#b] [auto_connect=on|off] [identify=on|off]";

    let tokens = split_args(arg);
    let mut tokens = tokens.into_iter();
    let name = tokens.next().filter(|n| !n.contains('=')).ok_or(USAGE)?;

    let (mut host, mut nick) = (None, None);
    let (mut port, mut tls) = (6697u16, true);
    let (mut user, mut realname, mut sasl_account, mut password_command) = (None, None, None, None);
    let mut sasl_mech = SaslMech::Plain;
    let mut auto_join = Vec::new();
    let mut auto_connect = true;
    let mut identify = false;

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
            "auto_connect" => {
                auto_connect =
                    parse_bool(value).ok_or_else(|| format!("bad auto_connect '{value}'"))?
            }
            "identify" => {
                identify = parse_bool(value).ok_or_else(|| format!("bad identify '{value}'"))?
            }
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
        auto_connect,
        identify,
    })
}

fn complete(app: &mut App) {
    if let Some(completion) = &app.completion {
        let idx = (completion.idx + 1) % completion.matches.len();
        apply_completion(app, idx);
        return;
    }
    // Slash-commands: complete the command name or an argument by position.
    if app.input.starts_with('/') {
        if let Some(completion) = slash_completion(app) {
            app.completion = Some(completion);
            apply_completion(app, 0);
        }
        return;
    }
    // Plain text: complete a nick from the channel roster.
    let start = word_start(&app.input, app.cursor);
    let stem = app.input[start..app.cursor].to_lowercase();
    if stem.is_empty() {
        return;
    }
    let matches: Vec<String> = nick_names(app)
        .into_iter()
        .filter(|n| n.to_lowercase().starts_with(&stem))
        .collect();
    if matches.is_empty() {
        return;
    }
    // A nick completed at the start of a line gets the configured completion
    // character and a space (e.g. `nick: `); mid-line it gets nothing.
    let suffix = if start == 0 {
        format!("{} ", app.client.completion_char)
    } else {
        String::new()
    };
    app.completion = Some(Completion {
        matches,
        idx: 0,
        start,
        suffix,
    });
    apply_completion(app, 0);
}

/// Complete a slash-command's name (first token) or its current argument. The
/// candidate set depends on the command and argument position; aliases are
/// resolved to their underlying command so their parameters complete too.
fn slash_completion(app: &App) -> Option<Completion> {
    let ws = word_start(&app.input, app.cursor);
    // The command token starts just after '/'; argument tokens at their word start.
    let (start, stem) = if ws == 0 {
        (1, app.input.get(1..app.cursor).unwrap_or("").to_string())
    } else {
        (ws, app.input[ws..app.cursor].to_string())
    };
    let stem = stem.to_lowercase();

    let before: Vec<&str> = app.input[..ws].split_whitespace().collect();
    let token_idx = before.len(); // 0 = the command name itself
    let (mut candidates, suffix): (Vec<String>, &'static str) = if token_idx == 0 {
        (command_names(app), " ")
    } else {
        let cmd = before[0].trim_start_matches('/').to_ascii_lowercase();
        arg_candidates(app, &cmd, token_idx, &before)
    };

    candidates.retain(|c| c.to_lowercase().starts_with(&stem));
    candidates.sort();
    candidates.dedup();
    if candidates.is_empty() {
        return None;
    }
    Some(Completion {
        matches: candidates,
        idx: 0,
        start,
        suffix: suffix.to_string(),
    })
}

/// All completable command names: built-ins plus the user's alias names.
fn command_names(app: &App) -> Vec<String> {
    crate::commands::COMMANDS
        .iter()
        .map(|c| c.name.to_string())
        .chain(app.aliases.keys().cloned())
        .collect()
}

/// Candidates for the `token_idx`-th token of `cmd` (1 = first argument),
/// resolved entirely from the command knowledge base. `tokens` are the
/// already-typed tokens (token 0 is the command). Aliases resolve to their
/// underlying command so their parameters complete too.
fn arg_candidates(
    app: &App,
    cmd: &str,
    token_idx: usize,
    tokens: &[&str],
) -> (Vec<String>, &'static str) {
    // Resolve a user alias to its underlying command (its first word), so e.g.
    // an alias `j = join $1` completes channels for `/j`.
    let effective = match app.aliases.get(cmd) {
        Some(template) => template
            .split_whitespace()
            .next()
            .unwrap_or(cmd)
            .trim_start_matches('/')
            .to_ascii_lowercase(),
        None => cmd.to_string(),
    };
    let Some(doc) = crate::commands::find(&effective) else {
        // Unknown command: offer nicks (mentions).
        return (nick_names(app), " ");
    };

    if !doc.subcommands.is_empty() {
        if token_idx == 1 {
            return (
                doc.subcommands.iter().map(|s| s.name.to_string()).collect(),
                " ",
            );
        }
        // Within the chosen subcommand's parameters.
        let Some(sub) = tokens
            .get(1)
            .map(|s| s.to_ascii_lowercase())
            .and_then(|name| doc.subcommands.iter().find(|s| s.name == name))
        else {
            return (Vec::new(), " ");
        };
        return sub_param_candidates(app, sub, token_idx - 2);
    }

    // No subcommands: positional parameters.
    match doc.params.get(token_idx - 1) {
        Some(param) => (kind_candidates(app, param.kind), suffix_for(param.kind)),
        None => (Vec::new(), " "),
    }
}

/// Candidates for the `param_idx`-th parameter of a subcommand. Repeatable
/// `key=` options are offered at every position; positional params by index.
fn sub_param_candidates(
    app: &App,
    sub: &crate::commands::SubDoc,
    param_idx: usize,
) -> (Vec<String>, &'static str) {
    use crate::commands::ArgKind;
    if !sub.params.is_empty() && sub.params.iter().all(|p| p.kind == ArgKind::OptionKey) {
        return (sub.params.iter().map(|p| p.name.to_string()).collect(), "=");
    }
    match sub.params.get(param_idx) {
        Some(param) => (kind_candidates(app, param.kind), suffix_for(param.kind)),
        None => (Vec::new(), " "),
    }
}

/// Resolve an `ArgKind` to its concrete completion candidates.
fn kind_candidates(app: &App, kind: crate::commands::ArgKind) -> Vec<String> {
    use crate::commands::ArgKind;
    match kind {
        ArgKind::Enum(values) => values.iter().map(|s| s.to_string()).collect(),
        ArgKind::Nick => nick_names(app),
        ArgKind::Channel => channel_names(app),
        ArgKind::Network => network_names(app),
        ArgKind::Command => crate::commands::COMMANDS
            .iter()
            .map(|c| c.name.to_string())
            .collect(),
        ArgKind::Alias => app.aliases.keys().cloned().collect(),
        ArgKind::Plugin => app.plugins.iter().map(|p| p.name.clone()).collect(),
        ArgKind::OfficialPlugin => crate::addons::official::OFFICIAL
            .iter()
            .map(|p| p.name.to_string())
            .collect(),
        ArgKind::OptionKey => Vec::new(), // handled at the subcommand level
        ArgKind::Free => nick_names(app), // freeform: offer nicks for mentions
    }
}

/// The completion suffix for a value of this kind (`=` for option keys).
fn suffix_for(kind: crate::commands::ArgKind) -> &'static str {
    match kind {
        crate::commands::ArgKind::OptionKey => "=",
        _ => " ",
    }
}

/// Defined and connected network names (for `/connect`, `/network rm|show`).
fn network_names(app: &App) -> Vec<String> {
    app.definitions
        .iter()
        .map(|n| n.name.clone())
        .chain(app.networks.iter().map(|m| m.name.clone()))
        .collect()
}

/// Nicks in the active channel (excluding our own).
fn nick_names(app: &App) -> Vec<String> {
    let my = app.my_nick().to_lowercase();
    app.active_buffer()
        .members
        .iter()
        .map(|m| m.nick.clone())
        .filter(|n| n.to_lowercase() != my)
        .collect()
}

/// Names of open channel buffers.
fn channel_names(app: &App) -> Vec<String> {
    app.buffers
        .iter()
        .filter(|b| b.kind == BufferKind::Channel)
        .map(|b| b.name.clone())
        .collect()
}

fn apply_completion(app: &mut App, idx: usize) {
    let Some(completion) = app.completion.as_mut() else {
        return;
    };
    completion.idx = idx;
    let candidate = completion.matches[idx].clone();
    let start = completion.start;
    let suffix = completion.suffix.clone();
    app.input = format!("{}{candidate}{suffix}", &app.input[..start]);
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
            away: false,
            account: None,
        }];
        let mut app = App::new(
            nets,
            ClientConfig::default(),
            Vec::new(),
            Default::default(),
            None,
        );
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
    fn tab_completes_command_names() {
        let mut app = app_with_channel();
        // A unique prefix fills the whole command with a trailing space.
        for c in "/wh".chars() {
            handle_key(&mut app, key(KeyCode::Char(c)));
        }
        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.input, "/whois ");
    }

    /// Type `text` into a cleared input (no Enter) and press Tab, returning the
    /// resulting input.
    fn tab_after(app: &mut App, text: &str) -> String {
        app.input.clear();
        app.cursor = 0;
        app.completion = None;
        for c in text.chars() {
            handle_key(app, key(KeyCode::Char(c)));
        }
        handle_key(app, key(KeyCode::Tab));
        app.input.clone()
    }

    #[test]
    fn tab_completes_network_subcommand() {
        let mut app = app_with_channel();
        assert_eq!(tab_after(&mut app, "/network a"), "/network add ");
    }

    #[test]
    fn tab_completes_network_name_argument() {
        let mut app = app_with_channel();
        run_line(
            &mut app,
            "/network add libera host=irc.libera.chat nick=svan",
        );
        assert_eq!(tab_after(&mut app, "/connect li"), "/connect libera ");
        assert_eq!(
            tab_after(&mut app, "/network show li"),
            "/network show libera "
        );
    }

    #[test]
    fn tab_completes_set_keys_and_values() {
        let mut app = app_with_channel();
        assert_eq!(tab_after(&mut app, "/set time"), "/set timestamps ");
        assert_eq!(
            tab_after(&mut app, "/set timestamps o"),
            "/set timestamps off "
        );
        assert_eq!(tab_after(&mut app, "/set theme am"), "/set theme amber ");
    }

    #[test]
    fn tab_completes_network_add_keys_with_equals() {
        let mut app = app_with_channel();
        assert_eq!(tab_after(&mut app, "/network add ho"), "/network add host=");
    }

    #[test]
    fn tab_completes_alias_parameters_via_underlying_command() {
        let mut app = app_with_channel(); // has a #rust channel buffer
        run_line(&mut app, "/alias j join $1");
        // The alias resolves to /join, so its first arg completes channels.
        assert_eq!(tab_after(&mut app, "/j #ru"), "/j #rust ");
    }

    #[test]
    fn tab_cycles_multiple_command_matches_and_includes_aliases() {
        let mut app = app_with_channel();
        run_line(&mut app, "/alias nap away napping"); // an alias starting with 'n'
        for c in "/n".chars() {
            handle_key(&mut app, key(KeyCode::Char(c)));
        }
        handle_key(&mut app, key(KeyCode::Tab));
        // Candidates (sorted): names, nap, network, nick, notice -> first is "names".
        assert_eq!(app.input, "/names ");
        let candidates = app.completion.as_ref().unwrap().matches.clone();
        assert!(candidates.contains(&"network".to_string()));
        assert!(
            candidates.contains(&"nap".to_string()),
            "aliases are offered"
        );
    }

    #[test]
    fn networks_screen_adds_and_edits_a_definition() {
        let mut app = app_with_channel();
        run_line(&mut app, "/networks");
        assert_eq!(app.mode, Mode::Networks);
        assert_eq!(app.networks_ui.focus, NetworksFocus::List);
        // No definitions yet: the add row is index 0; Enter creates one and
        // lands on the form's host field.
        handle_key(&mut app, key(KeyCode::Enter));
        assert_eq!(app.definitions.len(), 1);
        assert_eq!(app.networks_ui.focus, NetworksFocus::Form);
        assert_eq!(app.networks_ui.field, 1);
        // Edit host inline.
        handle_key(&mut app, key(KeyCode::Enter));
        for c in "irc.libera.chat".chars() {
            handle_key(&mut app, key(KeyCode::Char(c)));
        }
        handle_key(&mut app, key(KeyCode::Enter));
        assert_eq!(app.definitions[0].host, "irc.libera.chat");
        assert!(app.networks_ui.editing.is_none());
        // tls is field 3; Space toggles it.
        handle_key(&mut app, key(KeyCode::Down));
        handle_key(&mut app, key(KeyCode::Down));
        assert_eq!(app.networks_ui.field, 3);
        let tls = app.definitions[0].tls;
        handle_key(&mut app, key(KeyCode::Char(' ')));
        assert_eq!(app.definitions[0].tls, !tls);
        // Left returns to the list; Esc closes.
        handle_key(&mut app, key(KeyCode::Left));
        assert_eq!(app.networks_ui.focus, NetworksFocus::List);
        handle_key(&mut app, key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn networks_screen_connect_and_delete() {
        use crate::tui::state::AppAction;
        let mut app = app_with_channel();
        run_line(
            &mut app,
            "/network add libera host=irc.libera.chat nick=svan",
        );
        run_line(&mut app, "/networks");
        // sel 0 = libera; `c` dials it (queues an AddNetwork since it is not live).
        handle_key(&mut app, key(KeyCode::Char('c')));
        assert!(app
            .actions
            .iter()
            .any(|a| matches!(a, AppAction::AddNetwork { .. })));
        // `x` removes the definition.
        handle_key(&mut app, key(KeyCode::Char('x')));
        assert!(app.definitions.is_empty());
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
    fn settings_screen_toggles_and_cycles_live() {
        let mut app = app_with_channel();
        run_line(&mut app, "/settings");
        assert_eq!(app.mode, Mode::Settings);
        // Row 0 is `timestamps` (bool); Enter toggles it and mirrors live.
        assert!(app.client.timestamps);
        handle_key(&mut app, key(KeyCode::Enter));
        assert!(!app.client.timestamps);
        assert!(!app.timestamps, "live mirror follows the change");
        // Row 2 is `theme` (enum); Right cycles it.
        handle_key(&mut app, key(KeyCode::Down));
        handle_key(&mut app, key(KeyCode::Down));
        let before = app.client.theme.clone();
        handle_key(&mut app, key(KeyCode::Right));
        assert_ne!(app.client.theme, before, "theme cycled");
        assert_eq!(app.accent, crate::tui::theme::accent_for(&app.client.theme));
        handle_key(&mut app, key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn f2_opens_settings() {
        let mut app = app_with_channel();
        handle_key(&mut app, key(KeyCode::F(2)));
        assert_eq!(app.mode, Mode::Settings);
    }

    #[test]
    fn settings_screen_edits_a_str_setting() {
        let mut app = app_with_channel();
        run_line(&mut app, "/settings");
        // Rows: timestamps, nick_colors, theme, nicklist, completion_char, ...
        for _ in 0..4 {
            handle_key(&mut app, key(KeyCode::Down));
        }
        // Enter begins editing the current value (":").
        handle_key(&mut app, key(KeyCode::Enter));
        assert_eq!(app.settings.editing.as_deref(), Some(":"));
        // Replace it with ">" and commit.
        handle_key(&mut app, key(KeyCode::Backspace));
        handle_key(&mut app, key(KeyCode::Char('>')));
        handle_key(&mut app, key(KeyCode::Enter));
        assert!(app.settings.editing.is_none());
        assert_eq!(app.client.completion_char, ">");
    }

    #[test]
    fn settings_screen_edit_rejects_bad_value_and_keeps_editing() {
        let mut app = app_with_channel();
        run_line(&mut app, "/settings");
        // Navigate to scrollback_lines (index 6) and edit it to something invalid.
        for _ in 0..6 {
            handle_key(&mut app, key(KeyCode::Down));
        }
        handle_key(&mut app, key(KeyCode::Enter));
        // Blank the buffer and type a non-number.
        while app.settings.editing.as_deref().map(str::len).unwrap_or(0) > 0 {
            handle_key(&mut app, key(KeyCode::Backspace));
        }
        handle_key(&mut app, key(KeyCode::Char('x')));
        handle_key(&mut app, key(KeyCode::Enter));
        assert!(app.settings.editing.is_some(), "still editing after error");
        assert!(app.settings.msg.is_some(), "shows a validation message");
    }

    #[test]
    fn settings_screen_deletes_alias() {
        let mut app = app_with_channel();
        run_line(&mut app, "/alias hi msg $1 hi");
        run_line(&mut app, "/settings");
        // The last selectable row is the "add alias" row; the alias is just above.
        for _ in 0..30 {
            handle_key(&mut app, key(KeyCode::Down));
        }
        handle_key(&mut app, key(KeyCode::Up));
        assert!(
            matches!(app.selected_setting_row(), Some(crate::tui::state::SettingsRow::Alias { name, .. }) if name == "hi")
        );
        handle_key(&mut app, key(KeyCode::Delete));
        assert!(!app.aliases.contains_key("hi"));
    }

    #[test]
    fn settings_screen_adds_alias_via_add_row() {
        let mut app = app_with_channel();
        run_line(&mut app, "/settings");
        // With no aliases, the add row is the last selectable one.
        for _ in 0..30 {
            handle_key(&mut app, key(KeyCode::Down));
        }
        assert!(matches!(
            app.selected_setting_row(),
            Some(crate::tui::state::SettingsRow::AddAlias)
        ));
        handle_key(&mut app, key(KeyCode::Enter)); // begin creating
        assert_eq!(app.settings.editing.as_deref(), Some(""));
        for c in "bye quit".chars() {
            handle_key(&mut app, key(KeyCode::Char(c)));
        }
        handle_key(&mut app, key(KeyCode::Enter)); // commit
        assert_eq!(app.aliases.get("bye").map(String::as_str), Some("quit"));
        // A structural name is refused.
        handle_key(&mut app, key(KeyCode::Enter));
        for c in "set nope".chars() {
            handle_key(&mut app, key(KeyCode::Char(c)));
        }
        handle_key(&mut app, key(KeyCode::Enter));
        assert!(!app.aliases.contains_key("set"));
        assert!(app
            .settings
            .msg
            .as_deref()
            .unwrap_or("")
            .contains("built-in"));
    }

    #[test]
    fn settings_typing_filters_the_list() {
        let mut app = app_with_channel();
        run_line(&mut app, "/settings");
        for c in "theme".chars() {
            handle_key(&mut app, key(KeyCode::Char(c)));
        }
        assert_eq!(app.settings.filter, "theme");
        let rows = app.settings_selectable();
        assert!(rows
            .iter()
            .all(|r| matches!(r, crate::tui::state::SettingsRow::Setting(d) if d.key == "theme")));
    }

    #[test]
    fn set_new_settings_via_registry_and_completion_char() {
        let mut app = app_with_channel();
        run_line(&mut app, "/set nick_colors off");
        assert!(!app.client.nick_colors);
        run_line(&mut app, "/set beep_on_highlight on");
        assert!(app.client.beep_on_highlight);
        run_line(&mut app, "/set scrollback_lines 200");
        assert_eq!(app.client.scrollback_lines, 200);
        // Out-of-range and unknown keys are rejected; the value stays put.
        run_line(&mut app, "/set scrollback_lines 5");
        assert_eq!(app.client.scrollback_lines, 200);
        run_line(&mut app, "/set nope 1");
        // completion_char changes the suffix for a nick completed at line start.
        run_line(&mut app, "/set completion_char ,");
        assert_eq!(tab_after(&mut app, "al"), "alice, ");
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
    fn connect_defined_network_allocates_and_queues_add() {
        use crate::tui::state::AppAction;
        let mut app = app_with_channel(); // one live network "net" (id 0)
        run_line(
            &mut app,
            "/network add libera host=irc.libera.chat nick=svan",
        );
        run_line(&mut app, "/connect libera");
        // A new network slot (id 1) was allocated and an AddNetwork queued.
        assert_eq!(app.networks.len(), 2);
        assert_eq!(app.networks[1].name, "libera");
        assert!(matches!(
            app.actions.as_slice(),
            [AppAction::AddNetwork { id: 1, .. }]
        ));
        // We switched to the new network's server buffer.
        assert_eq!(app.active_buffer().net, 1);
    }

    #[test]
    fn connect_unknown_network_is_rejected() {
        let mut app = app_with_channel();
        run_line(&mut app, "/connect nope");
        assert_eq!(app.networks.len(), 1, "no slot allocated for unknown net");
        assert!(app.actions.is_empty());
    }

    #[test]
    fn disconnect_active_queues_disconnect() {
        use crate::session::ConnState;
        use crate::tui::state::AppAction;
        let mut app = app_with_channel();
        app.networks[0].state = ConnState::Registered { nick: "me".into() };
        // Active buffer is #rust on net 0.
        run_line(&mut app, "/disconnect bye");
        assert_eq!(
            app.actions,
            vec![AppAction::Disconnect(0, Some("bye".to_string()))]
        );
    }

    #[test]
    fn network_show_dumps_a_definition() {
        let mut app = app_with_channel();
        run_line(
            &mut app,
            "/network add libera host=irc.libera.chat nick=svan",
        );
        run_line(&mut app, "/network show libera");
        // Output lands in the console, which becomes active.
        assert_eq!(app.active_buffer().kind, BufferKind::Status);
        let text: String = app
            .active_buffer()
            .lines
            .iter()
            .filter_map(|l| match l {
                crate::tui::state::Line::Event { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("network libera"));
        assert!(text.contains("irc.libera.chat"));
    }

    #[test]
    fn clear_empties_the_active_buffer() {
        let mut app = app_with_channel();
        run_line(&mut app, "hi there"); // active is #rust; sends a PRIVMSG
        app.push_active_event("some noise".to_string());
        assert!(!app.active_buffer().lines.is_empty());
        run_line(&mut app, "/clear");
        assert!(app.active_buffer().lines.is_empty());
    }

    #[test]
    fn reconnect_queues_disconnect_then_connect() {
        use crate::session::ConnState;
        use crate::tui::state::AppAction;
        let mut app = app_with_channel();
        app.networks[0].state = ConnState::Registered { nick: "me".into() };
        run_line(&mut app, "/reconnect");
        assert_eq!(
            app.actions,
            vec![
                AppAction::Disconnect(0, Some("reconnecting".to_string())),
                AppAction::Connect(0),
            ]
        );
    }

    #[test]
    fn network_add_parses_auto_connect() {
        let mut app = app_with_channel();
        run_line(&mut app, "/network add idle host=h nick=n auto_connect=off");
        assert_eq!(app.definitions.len(), 1);
        assert!(!app.definitions[0].auto_connect);
        // Omitted, it defaults to on.
        run_line(&mut app, "/network add live host=h nick=n");
        assert!(app.definitions[1].auto_connect);
    }

    #[test]
    fn networks_form_toggles_auto_connect_independently_of_tls() {
        let mut app = app_with_channel();
        app.add_network_definition(); // appends a default network, focuses the form
        let idx = |name: &str| NETWORK_FIELDS.iter().position(|f| f.name == name).unwrap();
        assert!(app.definitions[0].tls);
        assert!(app.definitions[0].auto_connect);
        // Toggling auto_connect leaves tls alone.
        app.adjust_network_field(0, idx("auto_connect")).unwrap();
        assert!(!app.definitions[0].auto_connect);
        assert!(app.definitions[0].tls, "tls unaffected");
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
    fn expand_alias_positional_and_star_and_append() {
        // No placeholder -> append args.
        assert_eq!(expand_alias("join", &["#rust"]), vec!["/join #rust"]);
        assert_eq!(expand_alias("quit", &[]), vec!["/quit"]);
        // Positional substitution.
        assert_eq!(expand_alias("join $1", &["#rust"]), vec!["/join #rust"]);
        assert_eq!(
            expand_alias("me waves at $1", &["bob"]),
            vec!["/me waves at bob"]
        );
        // $* takes all args.
        assert_eq!(
            expand_alias("msg bob $*", &["a", "b"]),
            vec!["/msg bob a b"]
        );
        // Chaining with ; keeps a command per segment.
        assert_eq!(
            expand_alias("msg $1 hi;msg $1 there", &["bob"]),
            vec!["/msg bob hi", "/msg bob there"]
        );
        // Out-of-range placeholder is empty.
        assert_eq!(expand_alias("kick $1 $2", &["bob"]), vec!["/kick bob "]);
    }

    #[test]
    fn alias_define_expand_and_recursion_guard() {
        let mut app = app_with_channel(); // active is #rust on net 0
        run_line(&mut app, "/alias j join $1");
        assert_eq!(app.aliases.get("j").map(String::as_str), Some("join $1"));
        // Using the alias expands to a JOIN line.
        let out = run_line(&mut app, "/j #ratatui");
        assert_eq!(out, vec![NetCommand::Raw("JOIN #ratatui".to_string())]);
        // A cyclic alias must not blow the stack; it terminates with feedback.
        run_line(&mut app, "/alias loop loop");
        let out = run_line(&mut app, "/loop");
        assert!(out.is_empty());
        // /unalias removes it.
        run_line(&mut app, "/unalias j");
        assert!(!app.aliases.contains_key("j"));
    }

    #[test]
    fn trigger_add_rm_and_reload() {
        let mut app = app_with_channel();
        run_line(&mut app, "/trigger add highlight = notify $nick: $msg");
        assert_eq!(app.triggers.len(), 1);
        assert_eq!(app.triggers[0].on, "highlight");
        assert_eq!(app.triggers[0].run, "notify $nick: $msg");
        // Editing queues a live addon reload.
        assert!(app.actions.contains(&AppAction::ReloadAddons));
        // A bad event name is rejected.
        run_line(&mut app, "/trigger add bogus = notify hi");
        assert_eq!(app.triggers.len(), 1, "invalid matcher refused");
        // Remove by number.
        run_line(&mut app, "/trigger rm 1");
        assert!(app.triggers.is_empty());
        // Out-of-range removal is a no-op with feedback.
        run_line(&mut app, "/trigger rm 5");
        assert!(app.triggers.is_empty());
    }

    fn plugin(
        name: &str,
        file: &str,
        status: crate::addons::PluginStatus,
    ) -> crate::addons::PluginInfo {
        crate::addons::PluginInfo {
            name: name.into(),
            file: file.into(),
            description: "does things".into(),
            version: "1.0".into(),
            status,
            config: Vec::new(),
        }
    }

    #[test]
    fn plugins_reload_and_enable_disable() {
        use crate::addons::PluginStatus;
        let mut app = app_with_channel();
        run_line(&mut app, "/plugins reload");
        assert!(app.actions.contains(&AppAction::ReloadAddons));
        app.actions.clear();
        // A loaded plugin can be disabled by name; a reload is queued and saved.
        app.plugins = vec![plugin("urlgrab", "urlgrab.rhai", PluginStatus::Loaded)];
        run_line(&mut app, "/plugins disable urlgrab");
        assert!(app.disabled_plugins.iter().any(|f| f == "urlgrab.rhai"));
        assert!(app.actions.contains(&AppAction::ReloadAddons));
        // And re-enabled.
        run_line(&mut app, "/plugins enable urlgrab");
        assert!(app.disabled_plugins.is_empty());
        // An unknown plugin is rejected.
        run_line(&mut app, "/plugins disable nope");
        assert!(app.disabled_plugins.is_empty());
    }

    #[test]
    fn plugins_screen_space_toggles_selected() {
        use crate::addons::PluginStatus;
        use crate::tui::state::Mode;
        let mut app = app_with_channel();
        app.plugins = vec![
            plugin("nickcolor", "nickcolor.rhai", PluginStatus::Loaded),
            plugin("urlgrab", "urlgrab.rhai", PluginStatus::Loaded),
        ];
        run_line(&mut app, "/plugins"); // bare opens the screen
        assert_eq!(app.mode, Mode::Plugins);
        // Move to the second plugin and unload it with Space.
        handle_key(&mut app, key(KeyCode::Down));
        handle_key(&mut app, key(KeyCode::Char(' ')));
        assert!(app.disabled_plugins.iter().any(|f| f == "urlgrab.rhai"));
        assert!(app.actions.contains(&AppAction::ReloadAddons));
        handle_key(&mut app, key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn plugin_config_editor_edits_and_clears_override() {
        use crate::addons::{PluginInfo, PluginStatus};
        use crate::tui::state::Mode;
        let mut app = app_with_channel();
        app.plugins = vec![PluginInfo {
            name: "autoop".into(),
            file: "autoop.rhai".into(),
            description: "op trusted".into(),
            version: "1.0".into(),
            status: PluginStatus::Loaded,
            config: vec![("TRUSTED".into(), "alice".into())],
        }];
        // Open the editor via the command.
        run_line(&mut app, "/plugins config autoop");
        assert_eq!(app.mode, Mode::PluginConfig);
        // Edit the value (buffer seeds with the current "alice") and save.
        handle_key(&mut app, key(KeyCode::Enter));
        handle_key(&mut app, key(KeyCode::Char('2')));
        handle_key(&mut app, key(KeyCode::Enter));
        assert_eq!(
            app.plugin_config
                .get("autoop.rhai")
                .and_then(|m| m.get("TRUSTED"))
                .map(String::as_str),
            Some("alice2")
        );
        assert!(app.actions.contains(&AppAction::ReloadAddons));
        // Editing back to the default drops the override entirely.
        handle_key(&mut app, key(KeyCode::Enter));
        handle_key(&mut app, key(KeyCode::Backspace));
        handle_key(&mut app, key(KeyCode::Enter));
        assert!(!app.plugin_config.contains_key("autoop.rhai"));
        // Esc returns to the plugins screen.
        handle_key(&mut app, key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Plugins);
    }

    #[test]
    fn plugins_config_reports_when_no_settings() {
        use crate::addons::PluginStatus;
        use crate::tui::state::Mode;
        let mut app = app_with_channel();
        // A plugin with no CONFIG cannot be configured.
        app.plugins = vec![plugin("plain", "plain.rhai", PluginStatus::Loaded)];
        run_line(&mut app, "/plugins config plain");
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn plugins_install_writes_file_and_reloads() {
        let dir = std::env::temp_dir().join("norn-plugins-install-test");
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = app_with_channel();
        app.config_path = Some(dir.join("config.toml"));
        // Install an official plugin: writes the file and queues a reload.
        run_line(&mut app, "/plugins install autorejoin");
        assert!(dir.join("plugins/autorejoin.rhai").exists());
        assert!(app.actions.contains(&AppAction::ReloadAddons));
        // Installing again is refused (already present).
        app.actions.clear();
        run_line(&mut app, "/plugins install autorejoin");
        assert!(!app.actions.contains(&AppAction::ReloadAddons));
        // An unknown official plugin is refused.
        run_line(&mut app, "/plugins install nope");
        assert!(!dir.join("plugins/nope.rhai").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn f4_opens_plugins() {
        use crate::tui::state::Mode;
        let mut app = app_with_channel();
        handle_key(&mut app, key(KeyCode::F(4)));
        assert_eq!(app.mode, Mode::Plugins);
    }

    #[test]
    fn alias_cannot_shadow_a_structural_command() {
        let mut app = app_with_channel();
        run_line(&mut app, "/alias set something");
        assert!(!app.aliases.contains_key("set"), "structural name refused");
    }

    #[test]
    fn help_opens_filters_and_closes() {
        use crate::tui::state::{HelpFocus, Mode};
        let mut app = app_with_channel();
        // Bare /help opens the list pane.
        run_line(&mut app, "/help");
        assert_eq!(app.mode, Mode::Help);
        assert_eq!(app.help.focus, HelpFocus::List);
        // Typing filters and resets the selection.
        for c in "net".chars() {
            handle_key(&mut app, key(KeyCode::Char(c)));
        }
        assert_eq!(app.help.query, "net");
        // Down moves the selection; clamped so it never exceeds the matches.
        for _ in 0..100 {
            handle_key(&mut app, key(KeyCode::Down));
        }
        let n = crate::commands::help_commands(&app.help.query).len();
        assert!(app.help.sel < n.max(1));
        // Esc closes without inserting.
        handle_key(&mut app, key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.input.is_empty());
    }

    #[test]
    fn help_names_a_command_opens_its_detail() {
        use crate::tui::state::{HelpFocus, Mode};
        let mut app = app_with_channel();
        // /help <command> jumps straight to that command's detail.
        run_line(&mut app, "/help whois");
        assert_eq!(app.mode, Mode::Help);
        assert_eq!(app.help.focus, HelpFocus::Detail);
        let selected = crate::commands::help_commands(&app.help.query)[app.help.sel];
        assert_eq!(selected.name, "whois");
        // Enter inserts the selected command from either pane.
        handle_key(&mut app, key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.input, "/whois ");
        assert_eq!(app.cursor, app.input.len());
    }

    #[test]
    fn help_right_focuses_detail_and_left_returns() {
        use crate::tui::state::HelpFocus;
        let mut app = app_with_channel();
        run_line(&mut app, "/help");
        assert_eq!(app.help.focus, HelpFocus::List);
        handle_key(&mut app, key(KeyCode::Right));
        assert_eq!(app.help.focus, HelpFocus::Detail);
        // In detail, Down scrolls; Left returns to the list.
        handle_key(&mut app, key(KeyCode::Down));
        assert_eq!(app.help.detail_scroll, 1);
        handle_key(&mut app, key(KeyCode::Left));
        assert_eq!(app.help.focus, HelpFocus::List);
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
