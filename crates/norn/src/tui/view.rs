//! Rendering the TUI: sidebar, message view, nicklist, input, and overlays.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::state::{
    network_field_value, App, BufferKind, Line as BufLine, Mode, NetFieldKind, NetworksFocus,
    SettingsRow, NETWORK_FIELDS,
};
use super::theme;
use crate::addons::PluginStatus;
use crate::session::ConnState;

const NICK_COL: usize = 9;

/// A nick's display color, honoring the `nick_colors` setting (off = one muted
/// color for every nick).
fn nick_color(app: &App, nick: &str) -> ratatui::style::Color {
    if app.client.nick_colors {
        theme::nick_color(nick)
    } else {
        theme::DIM
    }
}

/// Draw the whole UI.
pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    f.render_widget(Block::default().style(Style::default().bg(theme::BG)), area);

    // Sidebar and nicklist run the full height; the input area lives inside the
    // center column.
    let nick_w = if app.nicklist_visible && app.active_buffer().kind == BufferKind::Channel {
        18
    } else {
        0
    };
    let cols = Layout::horizontal([
        Constraint::Length(24),
        Constraint::Min(10),
        Constraint::Length(nick_w),
    ])
    .split(area);

    // Indent the center content from the panes (the design has inner padding).
    let center_area = Rect {
        x: cols[1].x + 1,
        y: cols[1].y,
        width: cols[1].width.saturating_sub(2),
        height: cols[1].height,
    };
    let center = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Length(1), // header rule
        Constraint::Min(1),    // messages
        Constraint::Length(1), // activity bar
        Constraint::Length(1), // input
    ])
    .split(center_area);

    draw_sidebar(f, cols[0], app);
    draw_header(f, center[0], app);
    draw_rule(f, center[1]);
    let lines_above = draw_messages(f, center[2], app);
    draw_activity(f, center[3], app, lines_above);
    draw_input(f, center[4], app);
    if nick_w > 0 {
        draw_nicklist(f, cols[2], app);
    }

    if app.mode == Mode::Switcher {
        draw_switcher(f, area, app);
    } else if app.mode == Mode::Help {
        draw_help(f, area, app);
    } else if app.mode == Mode::Settings {
        draw_settings(f, area, app);
    } else if app.mode == Mode::Networks {
        draw_networks(f, area, app);
    } else if app.mode == Mode::Plugins {
        draw_plugins(f, area, app);
    } else if app.mode == Mode::PluginConfig {
        draw_plugin_config(f, area, app);
    } else if let Some(completion) = &app.completion {
        draw_completion(f, center[4], completion);
    }
}

/// A horizontal separator rule.
fn draw_rule(f: &mut Frame, area: Rect) {
    let rule = "─".repeat(area.width as usize);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            rule,
            Style::default().fg(theme::BORDER),
        )))
        .style(Style::default().bg(theme::BG)),
        area,
    );
}

fn draw_sidebar(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(theme::BORDER))
        .style(Style::default().bg(theme::PANEL));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let inner_w = inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();

    // The digit that reaches each buffer with Alt+N (the console is Alt+0).
    let numbered = numbered_buffers(app);
    let number_of = |idx: usize| numbered.iter().position(|&b| b == idx).map(|p| p + 1);

    for row in sidebar_rows(app) {
        match row {
            // The global console is the first row, above every network.
            SidebarRow::Console => {
                let active = app
                    .buffers
                    .get(app.active)
                    .is_some_and(|b| b.kind == BufferKind::Status);
                lines.push(sidebar_row(
                    app.accent,
                    &SidebarCell {
                        active,
                        num: Some(0),
                        lead: None,
                        label: "norn",
                        fg: if active { theme::BRIGHT } else { theme::DIM2 },
                        badge: String::new(),
                        badge_style: Style::default(),
                    },
                    inner_w,
                ));
            }
            // The network header is the server buffer's entry, led by a glyph
            // for the connection state and followed by the lag when it is high.
            SidebarRow::Network(net_id) => {
                let active = app
                    .buffers
                    .get(app.active)
                    .is_some_and(|b| b.net == net_id && b.kind == BufferKind::Server);
                let (glyph, glyph_fg) = state_glyph(&app.networks[net_id].state);
                let badge = match app.lag.get(&net_id) {
                    Some(lag) if lag.as_millis() >= 1000 => format!(" {}", format_lag(*lag)),
                    _ => String::new(),
                };
                let server_idx = app
                    .buffers
                    .iter()
                    .position(|b| b.net == net_id && b.kind == BufferKind::Server);
                lines.push(sidebar_row(
                    app.accent,
                    &SidebarCell {
                        active,
                        num: server_idx.and_then(number_of),
                        lead: Some((glyph, glyph_fg)),
                        label: &app.networks[net_id].name.to_uppercase(),
                        fg: if active { theme::BRIGHT } else { theme::DIM2 },
                        badge,
                        badge_style: Style::default().fg(theme::GOLD),
                    },
                    inner_w,
                ));
            }
            SidebarRow::Buffer(idx) => {
                let buffer = &app.buffers[idx];
                let active = idx == app.active;
                let label = match buffer.kind {
                    BufferKind::Query => format!("@ {}", buffer.name),
                    _ => buffer.name.clone(),
                };
                // A mention reads `!N` in bold gold; plain unread is just `N`.
                // The `!` carries the meaning, so it survives without colour.
                let (badge, badge_style) = if buffer.unread == 0 {
                    (String::new(), Style::default())
                } else if buffer.mentioned {
                    (
                        format!(" !{}", buffer.unread),
                        Style::default()
                            .fg(theme::GOLD)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    (
                        format!(" {}", buffer.unread),
                        Style::default().fg(theme::TEXT),
                    )
                };
                let fg = if active {
                    theme::BRIGHT
                } else if buffer.kind == BufferKind::Query {
                    theme::nick_color(&buffer.name)
                } else if buffer.joined {
                    theme::TEXT
                } else {
                    // Parted/kicked: still readable, but visibly not live.
                    theme::DIM2
                };
                lines.push(sidebar_row(
                    app.accent,
                    &SidebarCell {
                        active,
                        num: number_of(idx),
                        lead: None,
                        label: &label,
                        fg,
                        badge,
                        badge_style,
                    },
                    inner_w,
                ));
            }
            // A defined network with no live connection: dim, click to connect.
            SidebarRow::Idle(def) => {
                lines.push(sidebar_row(
                    app.accent,
                    &SidebarCell {
                        active: false,
                        num: None,
                        lead: Some(("○", theme::DIM2)),
                        label: &app.definitions[def].name.to_uppercase(),
                        fg: theme::DIM2,
                        badge: String::new(),
                        badge_style: Style::default(),
                    },
                    inner_w,
                ));
            }
        }
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// One row of the sidebar, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarRow {
    /// The global console.
    Console,
    /// A live network's header (its server buffer).
    Network(usize),
    /// A channel or query buffer, by index into `App.buffers`.
    Buffer(usize),
    /// A defined network with no live connection, by index into
    /// `App.definitions`.
    Idle(usize),
}

/// The sidebar's rows, top to bottom. Drawing and mouse hit-testing both use
/// this, so a click always lands on the row the user sees.
pub fn sidebar_rows(app: &App) -> Vec<SidebarRow> {
    let mut rows = vec![SidebarRow::Console];
    for net_id in 0..app.networks.len() {
        rows.push(SidebarRow::Network(net_id));
        rows.extend(
            app.buffers
                .iter()
                .enumerate()
                .filter(|(_, b)| b.net == net_id && b.kind != BufferKind::Server)
                .map(|(idx, _)| SidebarRow::Buffer(idx)),
        );
    }
    rows.extend(
        app.definitions
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                !app.networks
                    .iter()
                    .any(|n| n.name.eq_ignore_ascii_case(&d.name))
            })
            .map(|(i, _)| SidebarRow::Idle(i)),
    );
    rows
}

/// What one sidebar row shows.
struct SidebarCell<'a> {
    /// Whether this is the active buffer (accent bar and highlight).
    active: bool,
    /// The Alt+N digit that jumps here (`0` for the console), if it has one.
    num: Option<usize>,
    /// An optional leading glyph and its colour (connection state).
    lead: Option<(&'a str, ratatui::style::Color)>,
    /// The row's text.
    label: &'a str,
    /// The label colour.
    fg: ratatui::style::Color,
    /// Right-aligned text (unread count, lag); empty for none.
    badge: String,
    /// How the badge is drawn.
    badge_style: Style,
}

/// The buffers Alt+1..9 reach, in the order the sidebar lists them: each live
/// network's status buffer, then its channels and queries. The console is Alt+0
/// and idle (unconnected) networks have no buffer to jump to.
pub fn numbered_buffers(app: &App) -> Vec<usize> {
    sidebar_rows(app)
        .into_iter()
        .filter_map(|row| match row {
            SidebarRow::Network(net_id) => app
                .buffers
                .iter()
                .position(|b| b.net == net_id && b.kind == BufferKind::Server),
            SidebarRow::Buffer(idx) => Some(idx),
            SidebarRow::Console | SidebarRow::Idle(_) => None,
        })
        .take(9)
        .collect()
}

/// Build one sidebar row: an accent bar, an optional state glyph, a padded
/// label, and a right badge, with a full-width active-row background.
fn sidebar_row(accent: ratatui::style::Color, cell: &SidebarCell, inner_w: usize) -> Line<'static> {
    let bg = if cell.active {
        theme::ACTIVE_BG
    } else {
        theme::PANEL
    };
    let bar = if cell.active { "▎" } else { " " };
    // Layout: accent bar (1) + jump digit (1) + a space + [glyph + space] + label
    // + padding + badge.
    let lead_w = cell.lead.map_or(0, |(g, _)| g.width() + 1);
    let name_w = inner_w.saturating_sub(3 + lead_w + cell.badge.width());
    let name = truncate(cell.label, name_w);
    let pad = " ".repeat(name_w.saturating_sub(name.width()));
    // The jump digit sits in the cell after the bar, where a plain space was.
    let digit = match cell.num {
        Some(n) if n <= 9 => char::from_digit(n as u32, 10).unwrap_or(' ').to_string(),
        _ => " ".to_string(),
    };
    let mut spans = vec![
        Span::styled(bar, Style::default().fg(accent).bg(bg)),
        Span::styled(digit, Style::default().fg(theme::DIM2).bg(bg)),
        Span::styled(" ", Style::default().bg(bg)),
    ];
    if let Some((glyph, glyph_fg)) = cell.lead {
        spans.push(Span::styled(
            format!("{glyph} "),
            Style::default().fg(glyph_fg).bg(bg),
        ));
    }
    spans.push(Span::styled(
        format!("{name}{pad}"),
        Style::default().fg(cell.fg).bg(bg),
    ));
    spans.push(Span::styled(cell.badge.clone(), cell.badge_style.bg(bg)));
    Line::from(spans)
}

/// The glyph and colour for a connection state. The shape differs per state, so
/// the meaning does not depend on colour alone.
fn state_glyph(state: &ConnState) -> (&'static str, ratatui::style::Color) {
    match state {
        ConnState::Registered { .. } => ("●", theme::GOLD),
        ConnState::Connecting | ConnState::Reconnecting { .. } => ("◐", theme::ACCENT),
        ConnState::Disconnected => ("○", theme::DIM2),
        ConnState::Closed => ("✕", theme::RED),
    }
}

/// A round-trip time for display: `0.3s`, `2.1s`, `15s`.
fn format_lag(lag: std::time::Duration) -> String {
    let secs = lag.as_secs_f64();
    if secs < 10.0 {
        format!("{secs:.1}s")
    } else {
        format!("{secs:.0}s")
    }
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let buffer = app.active_buffer();
    // `network · #channel +nt`, so the network is never ambiguous when several
    // are open; the console and status buffers stand alone.
    let net_name = app.networks.get(buffer.net).map(|n| n.name.as_str());
    let mut title = match (buffer.kind, net_name) {
        (BufferKind::Server, Some(net)) => net.to_string(),
        (BufferKind::Status, _) | (_, None) => buffer.name.clone(),
        (_, Some(net)) => format!("{net} · {}", buffer.name),
    };
    if buffer.kind == BufferKind::Channel {
        if !buffer.modes.is_empty() {
            title.push(' ');
            title.push_str(&buffer.modes);
        }
        if !buffer.joined {
            title.push_str(" (not joined)");
        }
    }
    let right = if buffer.kind == BufferKind::Channel {
        format!("{} nicks · F9", buffer.members.len())
    } else {
        String::new()
    };
    let head_w = area.width as usize;
    // Topic gets whatever room is left after the title and the right label.
    let sub_room = head_w.saturating_sub(title.width() + 2 + right.width() + 1);
    let sub = truncate(&buffer.topic.clone().unwrap_or_default(), sub_room);
    let used = title.width() + 2 + sub.width() + right.width();
    let pad = " ".repeat(head_w.saturating_sub(used));
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(title, Style::default().fg(theme::BRIGHT2)),
            Span::styled(format!("  {sub}"), Style::default().fg(theme::DIM)),
            Span::raw(pad),
            Span::styled(right, Style::default().fg(theme::DIM2)),
        ]))
        .style(Style::default().bg(theme::BG)),
        area,
    );
}

/// The fixed left gutter width: timestamp + nick column + `" │ "`.
fn prefix_width(app: &App) -> usize {
    (if app.timestamps { 6 } else { 0 }) + NICK_COL + 3
}

/// Draw the message list, bottom-anchored and word-wrapped, with the unread
/// divider. Returns the number of raw lines above the top of the viewport (for
/// the `↑ N more` hint).
fn draw_messages(f: &mut Frame, area: Rect, app: &App) -> usize {
    let buffer = app.active_buffer();
    let width = area.width as usize;
    let height = area.height as usize;
    let want = height + buffer.scroll;

    // Build wrapped visual lines from the bottom up, tagged with their source
    // line index; insert the unread divider above the marked line.
    let mut visual: Vec<(usize, Line)> = Vec::new();
    for (i, line) in buffer.lines.iter().enumerate().rev() {
        for vl in wrap_buf_line(app, line, width).into_iter().rev() {
            visual.push((i, vl));
        }
        if buffer.unread_marker == Some(i) {
            visual.push((i, divider_line(width, app.accent)));
        }
        if visual.len() >= want {
            break;
        }
    }
    let vis: Vec<(usize, Line)> = visual.into_iter().rev().collect();
    let n = vis.len();
    let end = n.saturating_sub(buffer.scroll);
    let start = end.saturating_sub(height);
    let lines_above = vis.get(start).map(|(i, _)| *i).unwrap_or(0);
    let shown: Vec<Line> = vis[start..end].iter().map(|(_, l)| l.clone()).collect();
    f.render_widget(Paragraph::new(shown), area);
    lines_above
}

/// The `──── unread ────` divider line.
fn divider_line(width: usize, accent: ratatui::style::Color) -> Line<'static> {
    let label = " unread ";
    let dashes = width.saturating_sub(label.width());
    let left = dashes / 2;
    let right = dashes - left;
    // The rule is decoration (FAINT); the label is text and must stay readable.
    Line::from(vec![
        Span::styled("─".repeat(left), Style::default().fg(theme::FAINT)),
        Span::styled(label, Style::default().fg(accent)),
        Span::styled("─".repeat(right), Style::default().fg(theme::FAINT)),
    ])
}

/// Wrap one buffer line into its visual (possibly multiple) lines.
fn wrap_buf_line(app: &App, line: &BufLine, width: usize) -> Vec<Line<'static>> {
    let pw = prefix_width(app);
    let text_w = width.saturating_sub(pw).max(1);
    let (time, gutter_nick, nick_color, base, mention_nick, text) = match line {
        // Failures get their own marker and colour; the `!!` keeps them
        // distinguishable from joins and parts without relying on colour.
        BufLine::Event {
            time,
            text,
            error: true,
        } => (
            time.clone(),
            "!!".to_string(),
            theme::RED,
            Style::default().fg(theme::RED),
            None,
            text.clone(),
        ),
        BufLine::Event { time, text, .. } => (
            time.clone(),
            "-!-".to_string(),
            theme::EVENT,
            Style::default().fg(theme::EVENT),
            None,
            text.clone(),
        ),
        BufLine::Chat {
            time,
            nick,
            text,
            notice,
            mention,
            action,
            ..
        } => {
            let color = nick_color(app, nick);
            if *action {
                // `* nick does something`, all in the sender's color.
                (
                    time.clone(),
                    "*".to_string(),
                    color,
                    Style::default().fg(color),
                    mention.then(|| app.my_nick().to_string()),
                    format!("{nick} {text}"),
                )
            } else {
                (
                    time.clone(),
                    nick.clone(),
                    color,
                    if *notice {
                        Style::default().fg(theme::DIM)
                    } else {
                        Style::default().fg(theme::TEXT)
                    },
                    mention.then(|| app.my_nick().to_string()),
                    text.clone(),
                )
            }
        }
    };

    let chunks = wrap_text(&text, text_w);
    let mut out = Vec::with_capacity(chunks.len());
    for (i, chunk) in chunks.iter().enumerate() {
        let mut spans: Vec<Span> = Vec::new();
        if i == 0 {
            if app.timestamps {
                spans.push(Span::styled(
                    format!("{:5} ", time.clone().unwrap_or_default()),
                    Style::default().fg(theme::DIM2),
                ));
            }
            spans.push(Span::styled(
                pad_left(&gutter_nick, NICK_COL),
                Style::default().fg(nick_color),
            ));
            // A line that mentions us gets a gold bar in the gutter, so it can
            // be found while scrolling without relying on the highlight colour.
            if mention_nick.is_some() {
                spans.push(Span::styled(" ▌ ", Style::default().fg(theme::GOLD)));
            } else {
                spans.push(Span::styled(" │ ", Style::default().fg(theme::FAINT)));
            }
        } else {
            spans.push(Span::raw(" ".repeat(pw)));
        }
        spans.extend(styled_chunk(chunk, base, mention_nick.as_deref()));
        out.push(Line::from(spans));
    }
    out
}

/// Style a text chunk, highlighting a whole-word mention of `nick` if present.
fn styled_chunk(chunk: &str, base: Style, mention: Option<&str>) -> Vec<Span<'static>> {
    if let Some(nick) = mention {
        if let Some((start, end)) = find_word(chunk, nick) {
            return vec![
                Span::styled(chunk[..start].to_string(), base),
                Span::styled(
                    chunk[start..end].to_string(),
                    Style::default().fg(theme::GOLD).bg(theme::HL_BG),
                ),
                Span::styled(chunk[end..].to_string(), base),
            ];
        }
    }
    vec![Span::styled(chunk.to_string(), base)]
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Find the byte range of a whole-word, ASCII-case-insensitive `word` in `text`.
fn find_word(text: &str, word: &str) -> Option<(usize, usize)> {
    if word.is_empty() {
        return None;
    }
    let (tb, wb, wl) = (text.as_bytes(), word.as_bytes(), word.len());
    let mut i = 0;
    while i + wl <= text.len() {
        if tb[i..i + wl].eq_ignore_ascii_case(wb)
            && (i == 0 || !is_word_byte(tb[i - 1]))
            && (i + wl == text.len() || !is_word_byte(tb[i + wl]))
            && text.is_char_boundary(i)
            && text.is_char_boundary(i + wl)
        {
            return Some((i, i + wl));
        }
        i += 1;
    }
    None
}

/// Greedy word-wrap `text` to `width` display columns, hard-breaking long words.
/// A leading-space indent is preserved on the first line (the greedy pass below
/// otherwise collapses it), so indented lines keep their indent.
#[allow(unused_assignments)] // cur_w is a loop accumulator; its last write is dead
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let indent_len = text.len() - text.trim_start_matches(' ').len();
    let (indent, body) = text.split_at(indent_len);
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for word in body.split(' ') {
        let ww = word.width();
        if ww > width {
            if cur_w > 0 {
                out.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            let mut piece_w = 0;
            for ch in word.chars() {
                let cw = ch.to_string().width();
                if piece_w + cw > width {
                    out.push(std::mem::take(&mut cur));
                    piece_w = 0;
                }
                cur.push(ch);
                piece_w += cw;
            }
            cur_w = piece_w;
            continue;
        }
        if cur_w > 0 && cur_w + 1 + ww > width {
            out.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        if cur_w > 0 {
            cur.push(' ');
            cur_w += 1;
        }
        cur.push_str(word);
        cur_w += ww;
    }
    out.push(cur);
    // Restore the leading indent on the first visual line.
    if !indent.is_empty() {
        if let Some(first) = out.first_mut() {
            first.insert_str(0, indent);
        }
    }
    out
}

fn draw_nicklist(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(theme::BORDER))
        .style(Style::default().bg(theme::PANEL));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let area = inner;

    let buffer = app.active_buffer();
    let members = buffer.sorted_members();
    let total = members.len();
    let mut lines = vec![Line::from(Span::styled(
        format!("{total} · F9"),
        Style::default().fg(theme::DIM2),
    ))];

    // Reserve the last row for a "…N more" line when the list overflows.
    let rows = (area.height as usize).saturating_sub(1);
    let (show, overflow) = if total > rows {
        (rows.saturating_sub(1), total - rows + 1)
    } else {
        (total, 0)
    };
    for m in members.into_iter().take(show) {
        let (sym, color) = match m.highest() {
            Some(p) if p.symbol() == '@' => ('@', theme::GOLD),
            Some(p) if p.symbol() == '+' => ('+', theme::ACCENT),
            Some(p) => (p.symbol(), theme::TEXT),
            None => (' ', nick_color(app, &m.nick)),
        };
        // Away members render dimmed while keeping their hue identity.
        let style = if m.away {
            Style::default().fg(theme::DIM2).add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(color)
        };
        lines.push(Line::from(Span::styled(format!("{sym}{}", m.nick), style)));
    }
    if overflow > 0 {
        lines.push(Line::from(Span::styled(
            format!("…{overflow} more"),
            Style::default().fg(theme::DIM2),
        )));
    }
    f.render_widget(Paragraph::new(lines), inset(area));
}

fn draw_activity(f: &mut Frame, area: Rect, app: &App, lines_above: usize) {
    let buffer = app.active_buffer();
    let mut spans = vec![Span::styled(
        format!(" {} ", app.my_nick()),
        Style::default().fg(theme::BRIGHT2),
    )];
    let meta = app.networks.get(buffer.net);
    if meta.is_some_and(|m| m.away) {
        spans.push(Span::styled("[away] ", Style::default().fg(theme::GOLD)));
    }
    if let Some(lag) = app.lag.get(&buffer.net) {
        spans.push(Span::styled(
            format!("lag {} ", format_lag(*lag)),
            Style::default().fg(theme::DIM2),
        ));
    }
    // Buffers with unread messages. With several networks the name carries the
    // network (`libera/#rust`); a mention gets a `!`, so it stands out without
    // relying on colour.
    let qualify = app.networks.len() > 1;
    let active: Vec<(String, bool)> = app
        .buffers
        .iter()
        .filter(|b| b.unread > 0)
        .map(|b| {
            let name = match app.networks.get(b.net) {
                Some(n) if qualify => format!("{}/{}", n.name, b.name),
                _ => b.name.clone(),
            };
            (name, b.mentioned)
        })
        .collect();
    if !active.is_empty() {
        spans.push(Span::styled("[Act: ", Style::default().fg(theme::TEXT)));
        for (i, (name, mentioned)) in active.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(", ", Style::default().fg(theme::TEXT)));
            }
            if *mentioned {
                spans.push(Span::styled(
                    format!("!{name}"),
                    Style::default()
                        .fg(theme::GOLD)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(name.clone(), Style::default().fg(theme::TEXT)));
            }
        }
        spans.push(Span::styled("]", Style::default().fg(theme::TEXT)));
    }
    // A pending "press again" (quit): spelled out so it cannot be missed.
    if let Some(prompt) = app.armed_prompt() {
        spans.push(Span::styled(
            format!(" {prompt}"),
            Style::default()
                .fg(theme::GOLD)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let mut left_line = Line::from(spans);
    if lines_above > 0 {
        let hint = format!("↑ {lines_above} more ");
        let used: usize = left_line.spans.iter().map(|s| s.content.width()).sum();
        let pad = (area.width as usize).saturating_sub(used + hint.width());
        left_line.spans.push(Span::raw(" ".repeat(pad)));
        left_line
            .spans
            .push(Span::styled(hint, Style::default().fg(theme::DIM2)));
    }
    f.render_widget(
        Paragraph::new(left_line).style(Style::default().bg(theme::ACTIVE_BG)),
        area,
    );
}

fn draw_input(f: &mut Frame, area: Rect, app: &App) {
    let buffer = app.active_buffer();
    let prompt = match buffer.kind {
        BufferKind::Server => "> ".to_string(),
        _ => format!("{} > ", buffer.name),
    };
    let line = Line::from(vec![
        Span::styled(prompt.clone(), Style::default().fg(theme::DIM2)),
        Span::styled(app.input.clone(), Style::default().fg(theme::BRIGHT)),
    ]);
    f.render_widget(Paragraph::new(line), area);

    if app.mode == Mode::Normal {
        let x = area.x + (prompt.width() + app.input[..app.cursor].width()) as u16;
        f.set_cursor_position((x.min(area.right().saturating_sub(1)), area.y));
    }
}

fn draw_completion(f: &mut Frame, input_area: Rect, completion: &super::state::Completion) {
    let h = (completion.matches.len() as u16).min(6);
    if h == 0 {
        return;
    }
    let w = completion
        .matches
        .iter()
        .map(|m| m.width() as u16)
        .max()
        .unwrap_or(4)
        + 2;
    let area = Rect {
        x: input_area.x + 2,
        y: input_area.y.saturating_sub(h),
        width: w.min(input_area.width),
        height: h,
    };
    f.render_widget(Clear, area);
    let lines: Vec<Line> = completion
        .matches
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let style = if i == completion.idx {
                Style::default().fg(theme::BRIGHT).bg(theme::BORDER_BRIGHT)
            } else {
                Style::default().fg(theme::TEXT)
            };
            Line::from(Span::styled(format!(" {m}"), style))
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme::ACTIVE_BG)),
        area,
    );
}

fn draw_switcher(f: &mut Frame, area: Rect, app: &App) {
    let w = 50.min(area.width.saturating_sub(4));
    let h = 12.min(area.height.saturating_sub(4));
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + 4,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    f.render_widget(
        Block::default().style(Style::default().bg(theme::PANEL).fg(theme::TEXT)),
        rect,
    );

    let matches = super::input::switcher_matches(app);
    let mut lines = vec![Line::from(vec![
        Span::styled("go to: ", Style::default().fg(theme::DIM2)),
        Span::styled(
            app.switcher.query.clone(),
            Style::default().fg(theme::BRIGHT),
        ),
    ])];
    // Room for the query line above and the hint line below; scroll the list so
    // the selection is always on screen.
    let visible = (h as usize).saturating_sub(2).max(1);
    let start = (app.switcher.sel + 1).saturating_sub(visible);
    for (i, &idx) in matches.iter().enumerate().skip(start).take(visible) {
        let b = &app.buffers[idx];
        let net_name = app
            .networks
            .get(b.net)
            .map(|n| n.name.as_str())
            .unwrap_or("");
        let label = match b.kind {
            BufferKind::Status => b.name.clone(),
            BufferKind::Server => format!("{net_name} (status)"),
            _ => format!("{}  {net_name}", b.name),
        };
        let style = if i == app.switcher.sel {
            Style::default().fg(theme::BRIGHT).bg(theme::ACTIVE_BG)
        } else {
            Style::default().fg(theme::TEXT)
        };
        lines.push(Line::from(Span::styled(format!(" {label}"), style)));
    }
    if matches.is_empty() {
        lines.push(Line::from(Span::styled(
            " no matching buffer",
            Style::default().fg(theme::DIM2),
        )));
    }
    let list_area = inset(rect);
    f.render_widget(Paragraph::new(lines), list_area);
    if h >= 3 {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " type to filter · ↑↓ · Enter switch · Esc cancel",
                Style::default().fg(theme::DIM2),
            ))),
            Rect {
                y: list_area.y + h - 1,
                height: 1,
                ..list_area
            },
        );
    }
}

/// The `/settings` panel: a categorized, typed, filterable list of client
/// preferences (and the user's aliases). A header line, the row list with a
/// centered selection, then a per-row detail line and the live filter.
fn draw_settings(f: &mut Frame, area: Rect, app: &App) {
    let w = 96.min(area.width.saturating_sub(2));
    let h = 30.min(area.height.saturating_sub(2));
    let rect = Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BORDER_BRIGHT))
        .style(Style::default().bg(theme::PANEL));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height < 5 || inner.width < 24 {
        return;
    }

    let rows = app.settings_rows();
    let count = rows
        .iter()
        .filter(|r| matches!(r, SettingsRow::Setting(_) | SettingsRow::Alias { .. }))
        .count();
    let width = inner.width as usize;

    // Header line: title + hint on the left, match count on the right.
    let left = "settings";
    let hint = "  /set · type to filter · Enter to edit";
    let right = format!("{count} matches");
    let pad = width.saturating_sub(left.width() + hint.width() + right.width());
    let header = Line::from(vec![
        Span::styled(left, Style::default().fg(theme::BRIGHT)),
        Span::styled(hint, Style::default().fg(theme::DIM2)),
        Span::raw(" ".repeat(pad)),
        Span::styled(right, Style::default().fg(theme::DIM2)),
    ]);
    f.render_widget(Paragraph::new(header), Rect { height: 1, ..inner });
    draw_rule(
        f,
        Rect {
            y: inner.y + 1,
            height: 1,
            ..inner
        },
    );

    // Body: the scrollable row list, between the rule and the two footer lines.
    let body = Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(4),
        ..inner
    };
    draw_settings_body(f, body, app, &rows);

    // Detail line: a validation message, an edit hint, or the selected row's doc.
    let detail = settings_detail_line(app);
    f.render_widget(
        Paragraph::new(detail).style(Style::default().bg(theme::PANEL)),
        Rect {
            y: inner.y + inner.height - 2,
            height: 1,
            ..inner
        },
    );

    // Filter line (cursor shown only while browsing, not editing).
    let cursor = if app.settings.editing.is_none() {
        "\u{2588}"
    } else {
        ""
    };
    let filter = Line::from(vec![
        Span::styled("filter: ", Style::default().fg(theme::DIM2)),
        Span::styled(
            format!("{}{cursor}", app.settings.filter),
            Style::default().fg(theme::BRIGHT),
        ),
    ]);
    f.render_widget(
        Paragraph::new(filter).style(Style::default().bg(theme::PANEL)),
        Rect {
            y: inner.y + inner.height - 1,
            height: 1,
            ..inner
        },
    );
}

/// Render the `/settings` row list into `area`, keeping the selection centered.
fn draw_settings_body(f: &mut Frame, area: Rect, app: &App, rows: &[SettingsRow]) {
    let width = area.width as usize;
    let list_h = area.height as usize;
    let mut body: Vec<Line> = Vec::new();
    let mut ordinal = 0usize;
    let mut sel_line = 0usize;
    for row in rows {
        match row {
            SettingsRow::Header(category) => body.push(Line::from(Span::styled(
                format!(" {}", category.to_uppercase()),
                Style::default().fg(theme::GOLD),
            ))),
            _ => {
                let selected = ordinal == app.settings.sel;
                if selected {
                    sel_line = body.len();
                }
                ordinal += 1;
                body.push(settings_row_line(app, row, selected, width));
            }
        }
    }
    if body.is_empty() {
        body.push(Line::from(Span::styled(
            " no matches",
            Style::default().fg(theme::DIM2),
        )));
    }
    let scroll = if body.len() <= list_h {
        0
    } else {
        sel_line.saturating_sub(list_h / 2).min(body.len() - list_h)
    };
    let shown: Vec<Line> = body.into_iter().skip(scroll).take(list_h).collect();
    f.render_widget(Paragraph::new(shown), area);
}

/// Build one selectable settings/alias row: `▎ key … value  type`.
fn settings_row_line(app: &App, row: &SettingsRow, selected: bool, width: usize) -> Line<'static> {
    let bg = if selected {
        theme::ACTIVE_BG
    } else {
        theme::PANEL
    };
    let bar = if selected { "▎" } else { " " };
    let label_fg = if selected {
        theme::BRIGHT
    } else {
        theme::BRIGHT2
    };

    // The "add alias" row: `＋ add alias`, or the edit buffer while creating one.
    if let SettingsRow::AddAlias = row {
        let (text, fg) = match (selected, &app.settings.editing) {
            (true, Some(buf)) => (format!("＋ {buf}\u{2588}"), theme::BRIGHT),
            _ => ("＋ add alias".to_string(), theme::DIM),
        };
        let text = truncate(&text, width.saturating_sub(2));
        let pad = width.saturating_sub(2 + text.width());
        return Line::from(vec![
            Span::styled(bar, Style::default().fg(app.accent).bg(bg)),
            Span::styled(format!(" {text}"), Style::default().fg(fg).bg(bg)),
            Span::styled(" ".repeat(pad), Style::default().bg(bg)),
        ]);
    }

    let (label, value, typ) = match row {
        SettingsRow::Setting(doc) => (
            doc.key.to_string(),
            app.setting_value(doc.key),
            doc.kind.label(),
        ),
        SettingsRow::Alias { name, expansion } => (name.clone(), expansion.clone(), "alias"),
        SettingsRow::Header(_) | SettingsRow::AddAlias => (String::new(), String::new(), ""),
    };

    // The selected row shows its inline edit buffer (with a cursor) as the value.
    let (value, value_fg) = match (selected, &app.settings.editing) {
        (true, Some(buf)) => (format!("{buf}\u{2588}"), theme::BRIGHT),
        _ => (value, theme::GOLD),
    };

    let right_w = value.width() + 2 + typ.width();
    let label = truncate(&label, width.saturating_sub(3 + right_w));
    let pad = width.saturating_sub(2 + label.width() + right_w + 1);
    Line::from(vec![
        Span::styled(bar, Style::default().fg(app.accent).bg(bg)),
        Span::styled(format!(" {label}"), Style::default().fg(label_fg).bg(bg)),
        Span::styled(" ".repeat(pad), Style::default().bg(bg)),
        Span::styled(value, Style::default().fg(value_fg).bg(bg)),
        Span::styled("  ", Style::default().bg(bg)),
        Span::styled(typ.to_string(), Style::default().fg(theme::DIM2).bg(bg)),
        Span::styled(" ", Style::default().bg(bg)),
    ])
}

/// The `/settings` detail line: a transient message, an edit hint, or the
/// selected row's description and default.
fn settings_detail_line(app: &App) -> Line<'static> {
    if let Some(msg) = &app.settings.msg {
        return Line::from(Span::styled(msg.clone(), Style::default().fg(theme::GOLD)));
    }
    if app.settings.editing.is_some() {
        let hint = match app.selected_setting_row() {
            Some(SettingsRow::AddAlias) => {
                "new alias: type  name expansion  · Enter saves · Esc cancels"
            }
            _ => "editing · Enter saves · Esc cancels",
        };
        return Line::from(Span::styled(hint, Style::default().fg(theme::DIM)));
    }
    let text = match app.selected_setting_row() {
        Some(SettingsRow::Setting(doc)) => {
            format!("{} — {} · default {}", doc.key, doc.desc, doc.default)
        }
        Some(SettingsRow::Alias { name, .. }) => {
            format!("alias /{name} · Enter edits · Delete removes")
        }
        Some(SettingsRow::AddAlias) => "create a new alias (name and expansion)".to_string(),
        _ => String::new(),
    };
    Line::from(Span::styled(text, Style::default().fg(theme::DIM)))
}

/// The `/networks` manager: a master-detail view. The left pane lists network
/// definitions (with a live-state marker) plus an add row; the right pane is the
/// selected definition's edit form. There is deliberately no password field.
fn draw_networks(f: &mut Frame, area: Rect, app: &App) {
    let w = 96.min(area.width.saturating_sub(2));
    let h = 30.min(area.height.saturating_sub(2));
    let rect = Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BORDER_BRIGHT))
        .title(Span::styled(
            " networks ",
            Style::default().fg(theme::BRIGHT),
        ))
        .style(Style::default().bg(theme::PANEL));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height < 4 || inner.width < 24 {
        return;
    }

    // Split: list | divider | form. Two footer lines (message + hint) sit below.
    let panes_h = inner.height.saturating_sub(2);
    let list_w = 26.min(inner.width / 2);
    let list_rect = Rect {
        width: list_w,
        height: panes_h,
        ..inner
    };
    let divider_x = inner.x + list_w;
    let form_rect = Rect {
        x: divider_x + 1,
        width: inner.width - list_w - 1,
        height: panes_h,
        ..inner
    };
    let list_focused = app.networks_ui.focus == NetworksFocus::List;

    draw_networks_list(f, list_rect, app, list_focused);
    for y in inner.y..inner.y + panes_h {
        f.render_widget(
            Paragraph::new(Span::styled("│", Style::default().fg(theme::BORDER))),
            Rect {
                x: divider_x,
                y,
                width: 1,
                height: 1,
            },
        );
    }
    draw_networks_form(f, form_rect, app, !list_focused);

    // Message line (a transient status), then the context-sensitive hint.
    let msg = app
        .networks_ui
        .msg
        .clone()
        .unwrap_or_else(|| networks_context(app));
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            msg,
            Style::default().fg(theme::GOLD),
        )))
        .style(Style::default().bg(theme::PANEL)),
        Rect {
            y: inner.y + inner.height - 2,
            height: 1,
            ..inner
        },
    );
    let hint = if app.networks_ui.editing.is_some() {
        "type · Enter saves · Esc cancels"
    } else if list_focused {
        "↑↓ select · → edit · c connect · d disconnect · x delete (twice) · Esc closes"
    } else {
        "↑↓ field · Enter edit · Space toggle · ← / Esc back"
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().fg(theme::DIM2),
        )))
        .style(Style::default().bg(theme::PANEL)),
        Rect {
            y: inner.y + inner.height - 1,
            height: 1,
            ..inner
        },
    );
}

/// A non-message context line for the `/networks` footer: the selected
/// definition's address and live state, or the add-row prompt.
fn networks_context(app: &App) -> String {
    match app.definitions.get(app.networks_ui.sel) {
        Some(cfg) => {
            let state = app
                .network_live_state(&cfg.name)
                .map(|s| format!(" · {}", conn_state_label(&s)))
                .unwrap_or_default();
            format!("{}:{}{}", cfg.host, cfg.port, state)
        }
        None => "press Enter to create a new network".to_string(),
    }
}

/// A short label for a connection state.
fn conn_state_label(state: &ConnState) -> &'static str {
    match state {
        ConnState::Connecting => "connecting",
        ConnState::Registered { .. } => "connected",
        ConnState::Reconnecting { .. } => "reconnecting",
        ConnState::Disconnected => "disconnected",
        ConnState::Closed => "closed",
    }
}

/// The left pane of `/networks`: definition names with a live-state marker, plus
/// the add row, with the selection bar-highlighted.
fn draw_networks_list(f: &mut Frame, area: Rect, app: &App, focused: bool) {
    let width = area.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    for (i, cfg) in app.definitions.iter().enumerate() {
        let selected = i == app.networks_ui.sel;
        let (marker, marker_fg) = match app.network_live_state(&cfg.name) {
            Some(state) => state_glyph(&state),
            None => ("·", theme::DIM2),
        };
        let bg = if selected && focused {
            theme::ACTIVE_BG
        } else {
            theme::PANEL
        };
        let bar = if selected { "▎" } else { " " };
        let name_fg = if selected { theme::BRIGHT } else { theme::TEXT };
        let name = truncate(&cfg.name, width.saturating_sub(4));
        lines.push(Line::from(vec![
            Span::styled(bar, Style::default().fg(app.accent).bg(bg)),
            Span::styled(format!(" {marker} "), Style::default().fg(marker_fg).bg(bg)),
            Span::styled(name, Style::default().fg(name_fg).bg(bg)),
        ]));
    }
    // The add row.
    let add_selected = app.networks_ui.sel >= app.definitions.len();
    let bg = if add_selected && focused {
        theme::ACTIVE_BG
    } else {
        theme::PANEL
    };
    let bar = if add_selected { "▎" } else { " " };
    lines.push(Line::from(vec![
        Span::styled(bar, Style::default().fg(app.accent).bg(bg)),
        Span::styled(" ＋ add network", Style::default().fg(theme::DIM).bg(bg)),
    ]));
    f.render_widget(Paragraph::new(lines), area);
}

/// The right pane of `/networks`: the selected definition's edit form (or the
/// add-row prompt). Bools render as checkboxes; the field under edit shows the
/// inline buffer. Passwords are never shown - only `password_command`.
fn draw_networks_form(f: &mut Frame, area: Rect, app: &App, focused: bool) {
    let width = area.width as usize;
    let Some(cfg) = app.definitions.get(app.networks_ui.sel) else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "＋ press Enter to create a new network",
                Style::default().fg(theme::DIM),
            ))),
            area,
        );
        return;
    };

    let mut lines: Vec<Line> = Vec::new();
    for (i, field) in NETWORK_FIELDS.iter().enumerate() {
        let selected = focused && i == app.networks_ui.field;
        let bg = if selected {
            theme::ACTIVE_BG
        } else {
            theme::PANEL
        };
        let bar = if selected { "▎" } else { " " };

        // Value: a checkbox for a bool, the edit buffer while editing, else text.
        let editing_here = selected && app.networks_ui.editing.is_some();
        let value = if field.kind == NetFieldKind::Toggle {
            if network_field_value(cfg, i) == "on" {
                "[x]".to_string()
            } else {
                "[ ]".to_string()
            }
        } else if editing_here {
            format!(
                "{}\u{2588}",
                app.networks_ui.editing.as_deref().unwrap_or("")
            )
        } else {
            let v = network_field_value(cfg, i);
            if v.is_empty() {
                "-".to_string()
            } else {
                v
            }
        };
        let value_fg = if editing_here {
            theme::BRIGHT
        } else if value == "-" {
            theme::DIM2
        } else {
            theme::GOLD
        };

        let label = format!("{:<16}", field.name);
        let value = truncate(&value, width.saturating_sub(2 + label.width()));
        lines.push(Line::from(vec![
            Span::styled(bar, Style::default().fg(app.accent).bg(bg)),
            Span::styled(format!(" {label}"), Style::default().fg(theme::TEXT).bg(bg)),
            Span::styled(value, Style::default().fg(value_fg).bg(bg)),
        ]));
    }
    f.render_widget(Paragraph::new(lines), area);
}

/// The `/plugins` manager: a floating list of addon scripts grouped LOADED /
/// AVAILABLE, with a status dot, name, description, and version. `Space`
/// loads/unloads the selected one.
fn draw_plugins(f: &mut Frame, area: Rect, app: &App) {
    let w = 96.min(area.width.saturating_sub(2));
    let h = 30.min(area.height.saturating_sub(2));
    let rect = Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BORDER_BRIGHT))
        .style(Style::default().bg(theme::PANEL));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height < 4 || inner.width < 24 {
        return;
    }

    let width = inner.width as usize;
    // Header: title + hint on the left, install count on the right.
    let left = "plugins";
    let hint = "  Space load/unload · c config · Esc closes";
    let right = format!("{} installed", app.plugins.len());
    let pad = width.saturating_sub(left.width() + hint.width() + right.width());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(left, Style::default().fg(theme::BRIGHT)),
            Span::styled(hint, Style::default().fg(theme::DIM2)),
            Span::raw(" ".repeat(pad)),
            Span::styled(right, Style::default().fg(theme::DIM2)),
        ])),
        Rect { height: 1, ..inner },
    );
    draw_rule(
        f,
        Rect {
            y: inner.y + 1,
            height: 1,
            ..inner
        },
    );
    let body = Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(2),
        ..inner
    };
    draw_plugins_body(f, body, app);
}

/// The `/plugins` row list: LOADED then AVAILABLE sections, selection centered.
fn draw_plugins_body(f: &mut Frame, area: Rect, app: &App) {
    let width = area.width as usize;
    let list_h = area.height as usize;
    if app.plugins.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " no plugins installed (put *.rhai in the plugins folder)",
                Style::default().fg(theme::DIM2),
            ))),
            area,
        );
        return;
    }

    let sel = app.plugins_ui.sel.min(app.plugins.len() - 1);
    let mut body: Vec<Line> = Vec::new();
    let mut sel_line = 0usize;
    let (mut shown_loaded, mut shown_available) = (false, false);
    for (i, plugin) in app.plugins.iter().enumerate() {
        let loaded = matches!(plugin.status, PluginStatus::Loaded);
        if loaded && !shown_loaded {
            body.push(section_header("LOADED"));
            shown_loaded = true;
        } else if !loaded && !shown_available {
            body.push(section_header("AVAILABLE"));
            shown_available = true;
        }
        if i == sel {
            sel_line = body.len();
        }
        body.push(plugin_row_line(app, plugin, i == sel, width));
    }
    let scroll = if body.len() <= list_h {
        0
    } else {
        sel_line.saturating_sub(list_h / 2).min(body.len() - list_h)
    };
    let shown: Vec<Line> = body.into_iter().skip(scroll).take(list_h).collect();
    f.render_widget(Paragraph::new(shown), area);
}

/// A GOLD uppercase section header line.
fn section_header(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!(" {text}"),
        Style::default().fg(theme::GOLD),
    ))
}

/// One plugin row: `▎ ● name  description … vX.Y`, colored by status.
fn plugin_row_line(
    app: &App,
    plugin: &crate::addons::PluginInfo,
    selected: bool,
    width: usize,
) -> Line<'static> {
    let bg = if selected {
        theme::ACTIVE_BG
    } else {
        theme::PANEL
    };
    let bar = if selected { "▎" } else { " " };
    let (dot, dot_fg) = match &plugin.status {
        PluginStatus::Loaded => ("●", app.accent),
        PluginStatus::Disabled => ("○", theme::DIM2),
        PluginStatus::Failed(_) => ("×", theme::RED),
    };
    let name_fg = if selected {
        theme::BRIGHT
    } else {
        theme::BRIGHT2
    };
    let (desc, desc_fg) = match &plugin.status {
        PluginStatus::Failed(err) => (format!("failed to load: {err}"), theme::RED),
        _ => (plugin.description.clone(), theme::DIM),
    };
    let version = if plugin.version.is_empty() {
        String::new()
    } else {
        format!("v{}", plugin.version)
    };

    // Fixed left: bar(1) + sp(1) + dot(1) + sp(1) + name(14) + gap(2) = 20.
    let name = truncate(&plugin.name, 14);
    let name_pad = " ".repeat(14usize.saturating_sub(name.width()));
    let left_w = 4 + 14 + 2;
    let desc_room = width.saturating_sub(left_w + version.width() + 1);
    let desc = truncate(&desc, desc_room);
    let gap = width.saturating_sub(left_w + desc.width() + version.width() + 1);
    Line::from(vec![
        Span::styled(bar, Style::default().fg(app.accent).bg(bg)),
        Span::styled(format!(" {dot} "), Style::default().fg(dot_fg).bg(bg)),
        Span::styled(
            format!("{name}{name_pad}  "),
            Style::default().fg(name_fg).bg(bg),
        ),
        Span::styled(desc, Style::default().fg(desc_fg).bg(bg)),
        Span::styled(" ".repeat(gap), Style::default().bg(bg)),
        Span::styled(version, Style::default().fg(theme::DIM2).bg(bg)),
        Span::styled(" ", Style::default().bg(bg)),
    ])
}

/// The per-plugin config editor: a floating list of the plugin's declared keys
/// with their current value (override or default). Enter edits/saves a value.
fn draw_plugin_config(f: &mut Frame, area: Rect, app: &App) {
    let w = 96.min(area.width.saturating_sub(2));
    let h = 30.min(area.height.saturating_sub(2));
    let rect = Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BORDER_BRIGHT))
        .style(Style::default().bg(theme::PANEL));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height < 4 || inner.width < 24 {
        return;
    }

    let width = inner.width as usize;
    let editing = app.plugin_cfg.editing.is_some();
    let left = format!("configure {}", app.plugin_cfg.name);
    let hint = if editing {
        "  Enter save · Esc cancel"
    } else {
        "  Enter edit · Esc back"
    };
    let right = app.plugin_cfg.msg.clone().unwrap_or_default();
    let pad = width.saturating_sub(left.width() + hint.width() + right.width());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(left, Style::default().fg(theme::BRIGHT)),
            Span::styled(hint, Style::default().fg(theme::DIM2)),
            Span::raw(" ".repeat(pad)),
            Span::styled(right, Style::default().fg(theme::GOLD)),
        ])),
        Rect { height: 1, ..inner },
    );
    draw_rule(
        f,
        Rect {
            y: inner.y + 1,
            height: 1,
            ..inner
        },
    );
    let body = Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(2),
        ..inner
    };

    let schema = app.plugin_cfg_schema();
    if schema.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " this plugin has no configurable settings",
                Style::default().fg(theme::DIM2),
            ))),
            body,
        );
        return;
    }
    let sel = app.plugin_cfg.sel.min(schema.len() - 1);
    let list_h = body.height as usize;
    let mut lines: Vec<Line> = Vec::new();
    for (i, (key, default)) in schema.iter().enumerate() {
        lines.push(plugin_cfg_row_line(app, key, default, i == sel, width));
    }
    let scroll = if lines.len() <= list_h {
        0
    } else {
        sel.saturating_sub(list_h / 2).min(lines.len() - list_h)
    };
    let shown: Vec<Line> = lines.into_iter().skip(scroll).take(list_h).collect();
    f.render_widget(Paragraph::new(shown), body);
}

/// One config row: `▎ key    value` (value in the accent when overridden). When
/// the selected row is being edited, its value shows the edit buffer + a caret.
fn plugin_cfg_row_line(
    app: &App,
    key: &str,
    default: &str,
    selected: bool,
    width: usize,
) -> Line<'static> {
    let bg = if selected {
        theme::ACTIVE_BG
    } else {
        theme::PANEL
    };
    let editing = selected && app.plugin_cfg.editing.is_some();
    let overridden = app
        .plugin_config
        .get(&app.plugin_cfg.file)
        .and_then(|m| m.get(key))
        .is_some();
    let value = if editing {
        format!("{}_", app.plugin_cfg.editing.clone().unwrap_or_default())
    } else {
        app.plugin_cfg_value(key, default)
    };
    let val_fg = if editing {
        theme::BRIGHT
    } else if overridden {
        app.accent
    } else {
        theme::DIM
    };
    let name = truncate(key, 18);
    let name_pad = " ".repeat(18usize.saturating_sub(name.width()));
    let left_w = 1 + 1 + 18 + 2; // bar + sp + key + gap
    let val_room = width.saturating_sub(left_w + 1);
    let value = truncate(&value, val_room);
    let gap = width.saturating_sub(left_w + value.width() + 1);
    Line::from(vec![
        Span::styled(
            if selected { "▎" } else { " " },
            Style::default().fg(app.accent).bg(bg),
        ),
        Span::styled(
            format!(" {name}{name_pad}  "),
            Style::default()
                .fg(if selected {
                    theme::BRIGHT
                } else {
                    theme::BRIGHT2
                })
                .bg(bg),
        ),
        Span::styled(value, Style::default().fg(val_fg).bg(bg)),
        Span::styled(" ".repeat(gap), Style::default().bg(bg)),
        Span::styled(" ", Style::default().bg(bg)),
    ])
}

/// The `/help` panel: a master-detail command reference. The left pane is a
/// searchable, categorized command list; the right pane shows the selected
/// command's full documentation. `→` focuses the detail to scroll it.
fn draw_help(f: &mut Frame, area: Rect, app: &App) {
    use crate::tui::state::HelpFocus;

    let w = 96.min(area.width.saturating_sub(2));
    let h = 30.min(area.height.saturating_sub(2));
    let rect = Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BORDER_BRIGHT))
        .title(Span::styled(
            " norn help ",
            Style::default().fg(theme::BRIGHT),
        ))
        .style(Style::default().bg(theme::PANEL));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height < 4 || inner.width < 20 {
        return;
    }

    // Split: list | divider | detail. The detail takes the larger share.
    let list_w = 28.min(inner.width / 2);
    let list_rect = Rect {
        width: list_w,
        ..inner
    };
    let divider_x = inner.x + list_w;
    let detail_rect = Rect {
        x: divider_x + 1,
        width: inner.width - list_w - 1,
        ..inner
    };

    let selected = crate::commands::help_commands(&app.help.query)
        .get(app.help.sel)
        .copied();
    let list_focused = app.help.focus == HelpFocus::List;

    draw_help_list(f, list_rect, app, list_focused);
    // Vertical divider.
    for y in inner.y..inner.y + inner.height - 1 {
        f.render_widget(
            Paragraph::new(Span::styled("│", Style::default().fg(theme::BORDER))),
            Rect {
                x: divider_x,
                y,
                width: 1,
                height: 1,
            },
        );
    }
    draw_help_detail(f, detail_rect, app, selected, !list_focused);

    // Footer hint spanning the full width.
    let footer = Rect {
        x: inner.x,
        y: inner.y + inner.height - 1,
        width: inner.width,
        height: 1,
    };
    let hint = if list_focused {
        "↓↑ select · → details · Enter inserts · Esc closes"
    } else {
        "↓↑ scroll · ← back · Enter inserts · Esc closes"
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().fg(theme::DIM2),
        )))
        .style(Style::default().bg(theme::PANEL)),
        footer,
    );
}

/// The left pane of `/help`: a filter line, a rule, and the categorized command
/// list with the selection centered in view.
fn draw_help_list(f: &mut Frame, area: Rect, app: &App, focused: bool) {
    use crate::commands::HelpRow;
    let width = area.width as usize;
    let list_h = (area.height as usize).saturating_sub(3); // search + rule + footer

    let query = if app.help.query.is_empty() {
        Span::styled("filter…", Style::default().fg(theme::DIM2))
    } else {
        Span::styled(app.help.query.clone(), Style::default().fg(theme::BRIGHT))
    };
    let mut out = vec![
        Line::from(vec![
            Span::styled("/", Style::default().fg(theme::DIM2)),
            query,
        ]),
        Line::from(Span::styled(
            "─".repeat(width),
            Style::default().fg(theme::BORDER),
        )),
    ];

    let mut body: Vec<Line> = Vec::new();
    let mut ordinal = 0usize;
    let mut sel_line = 0usize;
    for row in crate::commands::help_rows(&app.help.query) {
        match row {
            HelpRow::Header(category) => body.push(Line::from(Span::styled(
                format!(" {}", category.to_uppercase()),
                Style::default().fg(theme::GOLD),
            ))),
            HelpRow::Command(doc) => {
                let is_sel = ordinal == app.help.sel;
                if is_sel {
                    sel_line = body.len();
                }
                ordinal += 1;
                let bg = if is_sel && focused {
                    theme::ACTIVE_BG
                } else {
                    theme::PANEL
                };
                let bar = if is_sel { "▎" } else { " " };
                let fg = if is_sel {
                    theme::BRIGHT
                } else {
                    theme::BRIGHT2
                };
                let name = truncate(doc.name, width.saturating_sub(2));
                body.push(Line::from(vec![
                    Span::styled(bar, Style::default().fg(app.accent).bg(bg)),
                    Span::styled(format!(" {name}"), Style::default().fg(fg).bg(bg)),
                ]));
            }
        }
    }
    if body.is_empty() {
        body.push(Line::from(Span::styled(
            " no matches",
            Style::default().fg(theme::DIM2),
        )));
    }
    let scroll = if body.len() <= list_h {
        0
    } else {
        sel_line.saturating_sub(list_h / 2).min(body.len() - list_h)
    };
    out.extend(body.into_iter().skip(scroll).take(list_h));
    f.render_widget(Paragraph::new(out), area);
}

/// The right pane of `/help`: the selected command's full documentation,
/// wrapped and scrolled by `app.help.detail_scroll`.
fn draw_help_detail(
    f: &mut Frame,
    area: Rect,
    app: &App,
    doc: Option<&crate::commands::CommandDoc>,
    focused: bool,
) {
    let width = area.width as usize;
    let avail = (area.height as usize).saturating_sub(1); // leave the footer row
    let Some(doc) = doc else {
        return;
    };
    let lines = help_detail_lines(doc, width);
    let max_scroll = lines.len().saturating_sub(avail);
    let scroll = app.help.detail_scroll.min(max_scroll);
    let shown: Vec<Line> = lines.into_iter().skip(scroll).take(avail).collect();
    f.render_widget(Paragraph::new(shown), area);

    // A faint "▸ scroll" affordance while the detail has focus and overflows.
    if focused && max_scroll > 0 {
        let tag = Rect {
            x: area.x + area.width.saturating_sub(3),
            y: area.y,
            width: 3,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Span::styled("▾", Style::default().fg(theme::ACCENT))),
            tag,
        );
    }
}

/// Build the wrapped detail lines for one command: usage, description,
/// subcommands (with their params), options, and examples.
fn help_detail_lines(doc: &crate::commands::CommandDoc, width: usize) -> Vec<Line<'static>> {
    use crate::commands::ArgKind;
    let mut out: Vec<Line> = Vec::new();
    let plain =
        |s: String, c: ratatui::style::Color| Line::from(Span::styled(s, Style::default().fg(c)));

    out.push(plain(doc.usage.to_string(), theme::BRIGHT));
    out.push(Line::from(""));
    for l in wrap_text(doc.description, width) {
        out.push(plain(l, theme::TEXT));
    }

    // A parameter row: `name  desc (required)`, indented and wrapped.
    let param_lines = |out: &mut Vec<Line>, p: &crate::commands::ParamDoc, indent: usize| {
        let key = if p.kind == ArgKind::OptionKey {
            format!("{}=", p.name)
        } else {
            p.name.to_string()
        };
        let req = if p.required { " (required)" } else { "" };
        let text = format!("{key}  {}{req}", p.desc);
        for (i, l) in wrap_text(&text, width.saturating_sub(indent))
            .into_iter()
            .enumerate()
        {
            let pad = " ".repeat(if i == 0 { indent } else { indent + 2 });
            out.push(plain(format!("{pad}{l}"), theme::DIM));
        }
    };

    // Reference tables generated from their registries, so the help can never
    // list a key or setting that does not exist (or miss one that does).
    if doc.name == "keys" {
        let mut context = "";
        for k in crate::keys::KEYS {
            if k.context != context {
                context = k.context;
                out.push(Line::from(""));
                out.push(plain(context.to_uppercase(), theme::GOLD));
            }
            let text = format!("{:<22} {}", k.keys, k.action);
            for (i, l) in wrap_text(&text, width.saturating_sub(2))
                .into_iter()
                .enumerate()
            {
                let pad = if i == 0 {
                    "  "
                } else {
                    "                          "
                };
                out.push(plain(format!("{pad}{l}"), theme::BRIGHT2));
            }
        }
    }
    if doc.name == "set" {
        out.push(Line::from(""));
        out.push(plain("SETTINGS".to_string(), theme::GOLD));
        for s in crate::settings::SETTINGS {
            out.push(plain(
                format!("  {}  ({}, default {})", s.key, s.kind.label(), s.default),
                theme::BRIGHT2,
            ));
            for l in wrap_text(s.desc, width.saturating_sub(4)) {
                out.push(plain(format!("    {l}"), theme::DIM));
            }
        }
    }

    if !doc.subcommands.is_empty() {
        out.push(Line::from(""));
        out.push(plain("SUBCOMMANDS".to_string(), theme::GOLD));
        for sub in doc.subcommands {
            out.push(plain(format!("  {}", sub.usage), theme::BRIGHT2));
            for l in wrap_text(sub.desc, width.saturating_sub(4)) {
                out.push(plain(format!("    {l}"), theme::DIM));
            }
            for p in sub.params {
                param_lines(&mut out, p, 6);
            }
            for ex in sub.examples {
                for l in wrap_text(ex, width.saturating_sub(6)) {
                    out.push(plain(format!("      {l}"), theme::DIM2));
                }
            }
        }
    }

    if !doc.params.is_empty() {
        out.push(Line::from(""));
        out.push(plain("PARAMETERS".to_string(), theme::GOLD));
        for p in doc.params {
            param_lines(&mut out, p, 2);
        }
    }

    if !doc.examples.is_empty() {
        out.push(Line::from(""));
        out.push(plain("EXAMPLES".to_string(), theme::GOLD));
        for ex in doc.examples {
            for l in wrap_text(ex, width.saturating_sub(2)) {
                out.push(plain(format!("  {l}"), theme::ACCENT));
            }
        }
    }
    out
}

/// Shrink a rect by a one-cell horizontal inset.
fn inset(area: Rect) -> Rect {
    Rect {
        x: area.x + 1,
        y: area.y,
        width: area.width.saturating_sub(2),
        height: area.height,
    }
}

/// Right-align `s` into `width` columns (pad left; truncate if too wide).
fn pad_left(s: &str, width: usize) -> String {
    let w = s.width();
    if w >= width {
        truncate(s, width)
    } else {
        format!("{}{}", " ".repeat(width - w), s)
    }
}

/// Truncate `s` to at most `width` display columns.
fn truncate(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let cw = ch.to_string().width();
        if used + cw > width {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ClientConfig;
    use crate::session::{ConnState, UiEvent, UiEventKind};
    use crate::tui::state::{App, NetworkMeta};
    use irc_engine::{Event, TopicChange};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// Render a populated app to a TestBackend and confirm it doesn't panic and
    /// shows expected content. This exercises the layout/slicing math.
    #[test]
    fn renders_without_panicking() {
        let nets = vec![NetworkMeta {
            name: "libera".into(),
            my_nick: "svan".into(),
            state: ConnState::Registered {
                nick: "svan".into(),
            },
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
        app.apply(UiEvent {
            net: 0,
            kind: UiEventKind::Engine(Event::TopicChanged {
                target: "#ratatui".into(),
                change: TopicChange::Set("Rust TUI library".into()),
                set_by: None,
                set_at: None,
            }),
        });
        app.apply(UiEvent {
            net: 0,
            kind: UiEventKind::Engine(Event::MessageReceived(chat(
                "#ratatui",
                "orhun",
                "hello svan",
            ))),
        });
        let idx = app
            .buffers
            .iter()
            .position(|b| b.name == "#ratatui")
            .unwrap();
        app.switch_to(idx);

        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let backend = terminal.backend();
        let content = buffer_text(backend.buffer());
        assert!(content.contains("LIBERA"));
        assert!(content.contains("#ratatui"));
        assert!(content.contains("orhun"));
        assert!(content.contains("hello svan"));

        // The input line sits inside the center column: the sidebar interior
        // (cols 0..23, before its right border) on the last row is blank, and the
        // prompt is to its right.
        let last = 23u16;
        let sidebar: String = (0..23)
            .map(|x| backend.buffer()[(x, last)].symbol())
            .collect();
        assert!(
            sidebar.trim().is_empty(),
            "sidebar under the input is blank"
        );
        let input_row: String = (0..100)
            .map(|x| backend.buffer()[(x, last)].symbol())
            .collect();
        assert!(
            input_row.contains("#ratatui >"),
            "prompt renders in the center"
        );
    }

    #[test]
    fn switcher_overlay_renders() {
        let nets = vec![NetworkMeta {
            name: "libera".into(),
            my_nick: "svan".into(),
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
        app.mode = Mode::Switcher;
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        assert!(buffer_text(terminal.backend().buffer()).contains("go to:"));
    }

    #[test]
    fn help_panel_renders_list_and_detail() {
        let mut app = one_net_app();
        // Open straight to /network's detail so the pane shows subcommands.
        app.open_help("network");
        let mut terminal = Terminal::new(TestBackend::new(90, 30)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("norn help"));
        // Left list shows command names; right detail shows the subcommand + option.
        assert!(text.contains("network"));
        assert!(text.contains("SUBCOMMANDS"));
        assert!(text.contains("host"));
        assert!(text.contains("Enter inserts"));
    }

    #[test]
    fn settings_panel_renders() {
        let mut app = one_net_app();
        app.open_settings();
        let mut terminal = Terminal::new(TestBackend::new(90, 28)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("settings"));
        assert!(text.contains("matches"));
        // Categorized, typed rows.
        assert!(text.contains("LOOK & FEEL"));
        assert!(text.contains("timestamps"));
        assert!(text.contains("theme"));
        assert!(text.contains("scrollback_lines"));
        assert!(text.contains("filter:"));
    }

    #[test]
    fn plugins_panel_renders() {
        use crate::addons::{PluginInfo, PluginStatus};
        let mut app = one_net_app();
        app.plugins = vec![
            PluginInfo {
                name: "nickcolor".into(),
                file: "nickcolor.rhai".into(),
                description: "deterministic nick colors".into(),
                version: "1.4".into(),
                status: PluginStatus::Loaded,
                config: Vec::new(),
            },
            PluginInfo {
                name: "weather".into(),
                file: "weather.rhai".into(),
                description: String::new(),
                version: "0.2".into(),
                status: PluginStatus::Failed("missing dep".into()),
                config: Vec::new(),
            },
        ];
        app.open_plugins();
        let mut terminal = Terminal::new(TestBackend::new(90, 28)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("plugins"));
        assert!(text.contains("2 installed"));
        assert!(text.contains("LOADED"));
        assert!(text.contains("AVAILABLE"));
        assert!(text.contains("nickcolor"));
        assert!(text.contains("deterministic nick colors"));
        assert!(text.contains("v1.4"));
        assert!(text.contains("failed to load: missing dep"));
    }

    #[test]
    fn plugin_config_panel_renders() {
        use crate::addons::{PluginInfo, PluginStatus};
        let mut app = one_net_app();
        app.plugins = vec![PluginInfo {
            name: "autoop".into(),
            file: "autoop.rhai".into(),
            description: "op trusted".into(),
            version: "1.0".into(),
            status: PluginStatus::Loaded,
            config: vec![
                ("CHANNELS".into(), "#norn".into()),
                ("TRUSTED".into(), "alice".into()),
            ],
        }];
        app.open_plugin_config("autoop");
        let mut terminal = Terminal::new(TestBackend::new(90, 28)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("configure autoop"));
        assert!(text.contains("CHANNELS"));
        assert!(text.contains("TRUSTED"));
        assert!(text.contains("alice"));
    }

    #[test]
    fn networks_panel_renders() {
        let mut app = one_net_app();
        app.definitions.push(crate::config::NetworkConfig {
            name: "libera".into(),
            host: "irc.libera.chat".into(),
            port: 6697,
            tls: true,
            nick: "svan".into(),
            user: None,
            realname: None,
            sasl_account: None,
            sasl_mech: crate::config::SaslMech::Plain,
            password_command: None,
            auto_join: vec![],
            auto_connect: true,
            identify: false,
        });
        app.open_networks();
        let mut terminal = Terminal::new(TestBackend::new(90, 28)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("networks"));
        assert!(text.contains("libera"));
        assert!(text.contains("add network"));
        // The form shows fields including password_command, never a bare password.
        assert!(text.contains("host"));
        assert!(text.contains("password_command"));
    }

    #[test]
    fn error_lines_get_a_red_double_bang_marker() {
        let mut app = one_net_app();
        app.push_error("usage: /nope".to_string());
        app.push_active_event("carol joined #rust".to_string());
        let mut terminal = Terminal::new(TestBackend::new(90, 24)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buf = terminal.backend().buffer();
        let text = buffer_text(buf);
        assert!(text.contains("!! │ usage: /nope"), "error marker: {text}");
        assert!(text.contains("-!- │ carol joined"), "info marker: {text}");
        let red_marker = (0..buf.area.height).any(|y| {
            (0..buf.area.width - 1).any(|x| {
                buf[(x, y)].symbol() == "!"
                    && buf[(x + 1, y)].symbol() == "!"
                    && buf[(x, y)].fg == theme::RED
            })
        });
        assert!(red_marker, "the !! marker is drawn in the error colour");
    }

    #[test]
    fn network_form_checkboxes_reflect_their_own_field() {
        let mut app = one_net_app();
        app.definitions.push(crate::config::NetworkConfig {
            name: "libera".into(),
            host: "irc.libera.chat".into(),
            port: 6697,
            tls: true,
            nick: "svan".into(),
            user: None,
            realname: None,
            sasl_account: None,
            sasl_mech: crate::config::SaslMech::Plain,
            password_command: None,
            auto_join: vec![],
            auto_connect: false,
            identify: false,
        });
        app.open_networks();
        let mut terminal = Terminal::new(TestBackend::new(90, 28)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = buffer_text(terminal.backend().buffer());
        let row = |name: &str| -> String {
            text.lines()
                .find(|l| l.contains(name))
                .unwrap_or_else(|| panic!("no row for {name}"))
                .to_string()
        };
        assert!(row("tls").contains("[x]"), "tls is on");
        assert!(row("auto_connect").contains("[ ]"), "auto_connect is off");
    }

    fn chat(target: &str, from: &str, text: &str) -> irc_engine::ChatMessage {
        irc_engine::ChatMessage {
            time: None,
            msgid: None,
            account: None,
            sender: Some(irc_proto::Source::User {
                nick: from.into(),
                user: None,
                host: None,
            }),
            target: target.into(),
            text: text.into(),
            kind: irc_engine::MessageKind::Privmsg,
        }
    }

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn wrap_text_wraps_words_and_hard_breaks() {
        let wrapped = wrap_text("the quick brown fox jumps", 9);
        assert!(wrapped.len() > 1);
        assert!(wrapped.iter().all(|l| l.width() <= 9));
        // A word longer than the width is hard-broken.
        let broken = wrap_text("supercalifragilistic", 5);
        assert!(broken.len() > 1);
        assert!(broken.iter().all(|l| l.width() <= 5));
    }

    #[test]
    fn wrap_text_preserves_leading_indent() {
        // A leading-space indent (e.g. whois detail lines) survives the wrap.
        let wrapped = wrap_text("  realname: Alice A", 40);
        assert_eq!(wrapped[0], "  realname: Alice A");
    }

    #[test]
    fn find_word_matches_whole_word_case_insensitively() {
        assert_eq!(find_word("hey Svan!", "svan"), Some((4, 8)));
        assert_eq!(find_word("svansong", "svan"), None); // not a whole word
        assert_eq!(find_word("no match", "svan"), None);
    }

    fn one_net_app() -> App {
        let nets = vec![NetworkMeta {
            name: "libera".into(),
            my_nick: "svan".into(),
            state: ConnState::Registered {
                nick: "svan".into(),
            },
            away: false,
            account: None,
        }];
        App::new(
            nets,
            ClientConfig::default(),
            Vec::new(),
            Default::default(),
            None,
        )
    }

    fn draw_text(app: &App, w: u16, h: u16) -> (ratatui::buffer::Buffer, String) {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text = buffer_text(&buf);
        (buf, text)
    }

    fn engine(app: &mut App, event: Event) {
        app.apply(UiEvent {
            net: 0,
            kind: UiEventKind::Engine(event),
        });
    }

    /// The text of the first screen row containing `needle`.
    fn row_with<'a>(text: &'a str, needle: &str) -> &'a str {
        text.lines()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no row containing {needle:?} in:\n{text}"))
    }

    #[test]
    fn sidebar_network_row_shows_a_state_glyph_per_state() {
        let mut app = one_net_app();
        let (_, text) = draw_text(&app, 80, 20);
        assert!(row_with(&text, "LIBERA").contains("● LIBERA"), "registered");
        app.networks[0].state = ConnState::Connecting;
        let (_, text) = draw_text(&app, 80, 20);
        assert!(row_with(&text, "LIBERA").contains("◐ LIBERA"), "connecting");
        app.networks[0].state = ConnState::Disconnected;
        let (_, text) = draw_text(&app, 80, 20);
        assert!(
            row_with(&text, "LIBERA").contains("○ LIBERA"),
            "disconnected"
        );
        app.networks[0].state = ConnState::Closed;
        let (_, text) = draw_text(&app, 80, 20);
        assert!(row_with(&text, "LIBERA").contains("✕ LIBERA"), "closed");
    }

    #[test]
    fn lag_is_shown_in_the_sidebar_only_when_high_and_always_in_the_activity_bar() {
        let mut app = one_net_app();
        app.lag.insert(0, std::time::Duration::from_millis(300));
        let (_, text) = draw_text(&app, 80, 20);
        assert!(
            !row_with(&text, "LIBERA").contains("0.3s"),
            "low lag stays quiet"
        );
        assert!(text.contains("lag 0.3s"), "activity bar has it: {text}");
        app.lag.insert(0, std::time::Duration::from_millis(2100));
        let (_, text) = draw_text(&app, 80, 20);
        assert!(
            row_with(&text, "LIBERA").contains("2.1s"),
            "high lag is flagged"
        );
    }

    #[test]
    fn unread_badges_tell_mentions_from_plain_unread() {
        let mut app = one_net_app();
        engine(&mut app, Event::MessageReceived(chat("#quiet", "a", "hi")));
        engine(&mut app, Event::MessageReceived(chat("#quiet", "a", "hi")));
        engine(&mut app, Event::MessageReceived(chat("#quiet", "a", "hi")));
        engine(
            &mut app,
            Event::MessageReceived(chat("#loud", "b", "hey svan")),
        );
        let (buf, text) = draw_text(&app, 80, 20);
        // Three ordinary messages are just a count; one mention gets the `!`.
        // (Only the sidebar's 23 columns: the rest of the row is the chat pane.)
        let sidebar = |needle: &str| -> String {
            row_with(&text, needle).chars().take(23).collect::<String>()
        };
        assert!(sidebar("#quiet").trim_end().ends_with(" 3"), "{text}");
        assert!(sidebar("#loud").contains("!1"), "{text}");
        // The mention badge is bold gold.
        let y = text.lines().position(|l| l.contains("#loud")).unwrap() as u16;
        let x = (0..buf.area.width)
            .find(|x| buf[(*x, y)].symbol() == "!")
            .unwrap();
        assert_eq!(buf[(x, y)].fg, theme::GOLD);
        assert!(buf[(x, y)].modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn header_names_the_network_modes_and_membership() {
        let mut app = one_net_app();
        engine(
            &mut app,
            Event::NamesLoaded {
                target: "#rust".into(),
                members: Vec::new(),
            },
        );
        engine(
            &mut app,
            Event::ChannelModes {
                target: "#rust".into(),
                modes: "+ntk secret".into(),
            },
        );
        let rust = app.buffers.iter().position(|b| b.name == "#rust").unwrap();
        app.switch_to(rust);
        let (_, text) = draw_text(&app, 100, 20);
        let header = text.lines().next().unwrap();
        assert!(header.contains("libera · #rust +knt"), "{header}");
        assert!(!header.contains("not joined"));
        // Parting ourselves keeps the buffer but says so.
        engine(
            &mut app,
            Event::MemberLeft {
                target: "#rust".into(),
                who: irc_engine::User::nick("svan"),
                reason: irc_engine::LeaveReason::Part(String::new()),
            },
        );
        let (_, text) = draw_text(&app, 100, 20);
        assert!(text.lines().next().unwrap().contains("(not joined)"));
    }

    #[test]
    fn activity_bar_qualifies_names_and_marks_mentions() {
        let mut app = one_net_app();
        engine(&mut app, Event::MessageReceived(chat("#quiet", "a", "hi")));
        engine(
            &mut app,
            Event::MessageReceived(chat("#loud", "b", "hey svan")),
        );
        app.networks[0].away = true;
        let (_, text) = draw_text(&app, 100, 20);
        let bar = row_with(&text, "[Act:");
        assert!(bar.contains("[away]"), "{bar}");
        assert!(bar.contains("[Act: #quiet, !#loud]"), "{bar}");
        // A second network makes the names carry their network.
        app.networks.push(NetworkMeta {
            name: "oftc".into(),
            my_nick: "svan".into(),
            state: ConnState::Disconnected,
            away: false,
            account: None,
        });
        let (_, text) = draw_text(&app, 100, 20);
        assert!(row_with(&text, "[Act:").contains("libera/#quiet"));
    }

    #[test]
    fn the_sidebar_shows_the_alt_digit_beside_each_buffer() {
        let mut app = one_net_app();
        engine(&mut app, Event::MessageReceived(chat("#rust", "bob", "hi")));
        engine(&mut app, Event::MessageReceived(chat("bob", "bob", "psst")));
        let (_, text) = draw_text(&app, 80, 20);
        let sidebar = |needle: &str| -> String {
            row_with(&text, needle).chars().take(23).collect::<String>()
        };
        // The digit is the second cell (after the accent bar, which is `▎` on
        // the active row). 0 = console, then the network (1), then its buffers
        // in listed order.
        let digit = |needle: &str| sidebar(needle).chars().nth(1);
        assert_eq!(digit("norn"), Some('0'), "{text}");
        assert_eq!(digit("LIBERA"), Some('1'), "{text}");
        assert_eq!(digit("#rust"), Some('2'), "{text}");
        assert_eq!(digit("bob"), Some('3'), "{text}");
    }

    #[test]
    fn numbered_buffers_skip_the_console_and_idle_networks_and_stop_at_nine() {
        let mut app = one_net_app();
        for i in 0..12 {
            engine(
                &mut app,
                Event::MessageReceived(chat(&format!("#c{i}"), "bob", "hi")),
            );
        }
        let numbered = numbered_buffers(&app);
        assert_eq!(numbered.len(), 9);
        assert_eq!(app.buffers[numbered[0]].kind, BufferKind::Server);
        assert!(numbered
            .iter()
            .all(|&i| app.buffers[i].kind != BufferKind::Status));
    }

    #[test]
    fn welcome_text_points_at_the_keys() {
        let app = one_net_app();
        let console: Vec<String> = app.buffers[0]
            .lines
            .iter()
            .filter_map(|l| match l {
                BufLine::Event { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        let keys_line = console.iter().find(|l| l.starts_with("keys")).unwrap();
        assert!(keys_line.contains("F1 keys"), "{keys_line}");
        assert!(keys_line.contains("Ctrl+K switch buffers"), "{keys_line}");
    }

    #[test]
    fn help_detail_lists_every_key_and_every_setting_from_the_registries() {
        let mut app = one_net_app();
        app.open_help("keys");
        // Scroll through the whole detail pane, collecting what was drawn.
        let mut seen = String::new();
        for scroll in (0..200).step_by(10) {
            app.help.detail_scroll = scroll;
            seen.push_str(&draw_text(&app, 120, 40).1);
        }
        for k in crate::keys::KEYS {
            assert!(seen.contains(k.keys), "help is missing the key {}", k.keys);
        }
        app.open_help("set");
        let mut seen = String::new();
        for scroll in (0..200).step_by(10) {
            app.help.detail_scroll = scroll;
            seen.push_str(&draw_text(&app, 120, 40).1);
        }
        for s in crate::settings::SETTINGS {
            assert!(
                seen.contains(s.key),
                "help is missing the setting {}",
                s.key
            );
        }
    }

    #[test]
    fn switcher_and_panel_footers_show_their_keys() {
        let mut app = one_net_app();
        app.mode = Mode::Switcher;
        let (_, text) = draw_text(&app, 100, 30);
        assert!(text.contains("type to filter"), "{text}");
        assert!(text.contains("Esc cancel"), "{text}");

        let mut app = one_net_app();
        app.open_plugins();
        let (_, text) = draw_text(&app, 100, 30);
        assert!(text.contains("c config"), "plugins hint: {text}");

        let mut app = one_net_app();
        app.add_network_definition();
        app.mode = Mode::Networks;
        let (_, text) = draw_text(&app, 100, 30);
        assert!(text.contains("← / Esc back"), "networks form hint: {text}");
    }

    #[test]
    fn the_switcher_keeps_the_selection_on_screen() {
        let mut app = one_net_app();
        for i in 0..30 {
            engine(
                &mut app,
                Event::MessageReceived(chat(&format!("#room{i:02}"), "bob", "hi")),
            );
        }
        app.mode = Mode::Switcher;
        app.switcher.sel = 25;
        let (_, text) = draw_text(&app, 100, 30);
        // Row 25 of the list is the 26th buffer: console, status, then #room00...
        assert!(
            text.contains("#room23"),
            "selection scrolled into view: {text}"
        );
    }

    #[test]
    fn a_pending_quit_is_spelled_out_in_the_activity_bar() {
        let mut app = one_net_app();
        let (_, text) = draw_text(&app, 100, 20);
        assert!(!text.contains("press Ctrl+C again"));
        assert!(!app.confirm(crate::tui::state::Confirm::Quit));
        let (_, text) = draw_text(&app, 100, 20);
        assert!(text.contains("press Ctrl+C again to quit"), "{text}");
    }

    #[test]
    fn mention_lines_get_a_gold_gutter_bar() {
        let mut app = one_net_app();
        engine(
            &mut app,
            Event::MessageReceived(chat("#rust", "bob", "hey svan")),
        );
        engine(
            &mut app,
            Event::MessageReceived(chat("#rust", "bob", "plain")),
        );
        let rust = app.buffers.iter().position(|b| b.name == "#rust").unwrap();
        app.switch_to(rust);
        let (buf, text) = draw_text(&app, 100, 20);
        assert!(row_with(&text, "hey svan").contains("▌"), "{text}");
        assert!(!row_with(&text, "plain").contains("▌"));
        let y = text.lines().position(|l| l.contains("hey svan")).unwrap() as u16;
        let x = (0..buf.area.width)
            .find(|x| buf[(*x, y)].symbol() == "▌")
            .unwrap();
        assert_eq!(buf[(x, y)].fg, theme::GOLD);
    }

    #[test]
    fn unread_divider_appears_after_leaving_and_returning() {
        let mut app = one_net_app();
        app.apply(UiEvent {
            net: 0,
            kind: UiEventKind::Engine(Event::MessageReceived(chat("#rust", "a", "first"))),
        });
        let rust = app.buffers.iter().position(|b| b.name == "#rust").unwrap();
        app.switch_to(rust);
        app.switch_to(0); // leave #rust -> marker set at its length
        app.apply(UiEvent {
            net: 0,
            kind: UiEventKind::Engine(Event::MessageReceived(chat("#rust", "b", "second"))),
        });
        app.switch_to(rust); // return -> divider before the unread line

        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        assert!(buffer_text(terminal.backend().buffer()).contains("unread"));
    }

    #[test]
    fn event_line_renders_with_marker() {
        let mut app = one_net_app();
        app.apply(UiEvent {
            net: 0,
            kind: UiEventKind::Engine(Event::MemberJoined {
                target: "#rust".into(),
                who: irc_engine::User::nick("kex"),
                account: None,
            }),
        });
        let rust = app.buffers.iter().position(|b| b.name == "#rust").unwrap();
        app.switch_to(rust);
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let content = buffer_text(terminal.backend().buffer());
        assert!(content.contains("-!-"));
        assert!(content.contains("kex joined"));
    }
}
