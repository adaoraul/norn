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

    // The global console is the first row, above every network.
    let console_active = app
        .buffers
        .get(app.active)
        .is_some_and(|b| b.kind == BufferKind::Status);
    lines.push(sidebar_row(
        app.accent,
        console_active,
        "norn",
        "",
        if console_active {
            theme::BRIGHT
        } else {
            theme::DIM2
        },
        theme::DIM2,
        inner_w,
    ));

    for (net_id, net) in app.networks.iter().enumerate() {
        // The network name header is the server/status buffer's entry.
        let server_active = app
            .buffers
            .get(app.active)
            .is_some_and(|b| b.net == net_id && b.kind == BufferKind::Server);
        let fg = if server_active {
            theme::BRIGHT
        } else {
            theme::DIM2
        };
        lines.push(sidebar_row(
            app.accent,
            server_active,
            &net.name.to_uppercase(),
            "",
            fg,
            theme::DIM2,
            inner_w,
        ));

        for (idx, buffer) in app.buffers.iter().enumerate() {
            if buffer.net != net_id || buffer.kind == BufferKind::Server {
                continue;
            }
            let active = idx == app.active;
            let label = match buffer.kind {
                BufferKind::Query => format!("@ {}", buffer.name),
                _ => buffer.name.clone(),
            };
            let badge = if buffer.unread > 0 {
                format!(" {}", buffer.unread)
            } else {
                String::new()
            };
            let fg = if active {
                theme::BRIGHT
            } else if buffer.kind == BufferKind::Query {
                theme::nick_color(&buffer.name)
            } else {
                theme::TEXT
            };
            let badge_fg = if buffer.mentioned || buffer.unread > 2 {
                theme::GOLD
            } else {
                theme::DIM2
            };
            lines.push(sidebar_row(
                app.accent, active, &label, &badge, fg, badge_fg, inner_w,
            ));
        }
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// Build one sidebar row: an accent bar, a padded label, and a right badge, with
/// a full-width active-row background.
#[allow(clippy::too_many_arguments)]
fn sidebar_row(
    accent: ratatui::style::Color,
    active: bool,
    label: &str,
    badge: &str,
    fg: ratatui::style::Color,
    badge_fg: ratatui::style::Color,
    inner_w: usize,
) -> Line<'static> {
    // Layout: accent bar (1) + a space + label + padding + badge.
    let name_w = inner_w.saturating_sub(2 + badge.width());
    let name = truncate(label, name_w);
    let pad = " ".repeat(name_w.saturating_sub(name.width()));
    let bg = if active {
        theme::ACTIVE_BG
    } else {
        theme::PANEL
    };
    let bar = if active { "▎" } else { " " };
    Line::from(vec![
        Span::styled(bar, Style::default().fg(accent).bg(bg)),
        Span::styled(format!(" {name}{pad}"), Style::default().fg(fg).bg(bg)),
        Span::styled(badge.to_string(), Style::default().fg(badge_fg).bg(bg)),
    ])
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let buffer = app.active_buffer();
    let title = match buffer.kind {
        BufferKind::Server => app.networks[buffer.net].name.clone(),
        _ => buffer.name.clone(),
    };
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
            visual.push((i, divider_line(width)));
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
fn divider_line(width: usize) -> Line<'static> {
    let label = " unread ";
    let dashes = width.saturating_sub(label.width());
    let left = dashes / 2;
    let right = dashes - left;
    Line::from(vec![
        Span::styled("─".repeat(left), Style::default().fg(theme::FAINT)),
        Span::styled(label, Style::default().fg(theme::FAINT)),
        Span::styled("─".repeat(right), Style::default().fg(theme::FAINT)),
    ])
}

/// Wrap one buffer line into its visual (possibly multiple) lines.
fn wrap_buf_line(app: &App, line: &BufLine, width: usize) -> Vec<Line<'static>> {
    let pw = prefix_width(app);
    let text_w = width.saturating_sub(pw).max(1);
    let (time, gutter_nick, nick_color, base, mention_nick, text) = match line {
        BufLine::Event { time, text } => (
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
            spans.push(Span::styled(" │ ", Style::default().fg(theme::FAINT)));
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
    let act: Vec<&str> = app
        .buffers
        .iter()
        .filter(|b| b.unread > 0)
        .map(|b| b.name.as_str())
        .collect();
    let mut spans = vec![Span::styled(
        format!(" {} ", app.my_nick()),
        Style::default().fg(theme::BRIGHT2),
    )];
    if !act.is_empty() {
        spans.push(Span::styled(
            format!("[Act: {}]", act.join(", ")),
            Style::default().fg(theme::GOLD),
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
    for (i, &idx) in matches.iter().enumerate() {
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
    f.render_widget(Paragraph::new(lines), inset(rect));
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
        "↑↓ select · → edit · c connect · d disconnect · x delete · Esc closes"
    } else {
        "↑↓ field · Enter edit · Space toggle · ← back · Esc closes"
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
            Some(ConnState::Registered { .. }) => ("●", theme::GOLD),
            Some(ConnState::Connecting | ConnState::Reconnecting { .. }) => ("◐", theme::ACCENT),
            Some(_) => ("○", theme::DIM2),
            None => ("·", theme::FAINT),
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
            if cfg.tls {
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
    let hint = "  Space load/unload · Esc closes";
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
            },
            PluginInfo {
                name: "weather".into(),
                file: "weather.rhai".into(),
                description: String::new(),
                version: "0.2".into(),
                status: PluginStatus::Failed("missing dep".into()),
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
        }];
        App::new(
            nets,
            ClientConfig::default(),
            Vec::new(),
            Default::default(),
            None,
        )
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
