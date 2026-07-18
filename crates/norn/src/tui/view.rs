//! Rendering the TUI: sidebar, message view, nicklist, input, and overlays.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::state::{App, BufferKind, Line as BufLine, Mode};
use super::theme;

const NICK_COL: usize = 9;

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
            let color = theme::nick_color(nick);
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
#[allow(unused_assignments)] // cur_w is a loop accumulator; its last write is dead
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for word in text.split(' ') {
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
            None => (' ', theme::nick_color(&m.nick)),
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
        let mut app = App::new(nets, ClientConfig::default(), Vec::new(), None);
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
        let mut app = App::new(nets, ClientConfig::default(), Vec::new(), None);
        app.mode = Mode::Switcher;
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        assert!(buffer_text(terminal.backend().buffer()).contains("go to:"));
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
        App::new(nets, ClientConfig::default(), Vec::new(), None)
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
