//! Rendering the TUI: sidebar, message view, nicklist, input, and overlays.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
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

    let center = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Min(1),    // messages
        Constraint::Length(1), // activity bar
        Constraint::Length(1), // input
    ])
    .split(cols[1]);

    draw_sidebar(f, cols[0], app);
    draw_header(f, center[0], app);
    draw_messages(f, center[1], app);
    draw_activity(f, center[2], app);
    draw_input(f, center[3], app);
    if nick_w > 0 {
        draw_nicklist(f, cols[2], app);
    }

    if app.mode == Mode::Switcher {
        draw_switcher(f, area, app);
    } else if let Some(completion) = &app.completion {
        draw_completion(f, center[3], completion);
    }
}

fn draw_sidebar(f: &mut Frame, area: Rect, app: &App) {
    f.render_widget(
        Block::default().style(Style::default().bg(theme::PANEL)),
        area,
    );
    let mut lines: Vec<Line> = Vec::new();
    let inner_w = area.width.saturating_sub(2) as usize;

    for (net_id, net) in app.networks.iter().enumerate() {
        // The network name header is the server/status buffer's entry.
        let server_active = app
            .buffers
            .get(app.active)
            .is_some_and(|b| b.net == net_id && b.kind == BufferKind::Server);
        let bar = if server_active { "▎" } else { " " };
        let name = truncate(&net.name.to_uppercase(), inner_w.saturating_sub(1));
        let pad = " ".repeat(inner_w.saturating_sub(1 + name.width()));
        let header_style = if server_active {
            Style::default().fg(theme::BRIGHT).bg(theme::ACTIVE_BG)
        } else {
            Style::default().fg(theme::DIM2)
        };
        lines.push(Line::from(vec![
            Span::styled(bar, Style::default().fg(app.accent)),
            Span::styled(format!("{name}{pad}"), header_style),
        ]));

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
            let bar = if active { "▎" } else { " " };
            let name_w = inner_w.saturating_sub(1 + badge.width());
            let name = truncate(&label, name_w);
            let pad = " ".repeat(name_w.saturating_sub(name.width()));
            let base_fg = if active {
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
            let mut style = Style::default().fg(base_fg);
            if active {
                style = style.bg(theme::ACTIVE_BG);
            }
            lines.push(Line::from(vec![
                Span::styled(bar, Style::default().fg(app.accent)),
                Span::styled(format!("{name}{pad}"), style),
                Span::styled(badge, Style::default().fg(badge_fg)),
            ]));
        }
    }
    f.render_widget(Paragraph::new(lines), inset(area));
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

fn draw_messages(f: &mut Frame, area: Rect, app: &App) {
    let buffer = app.active_buffer();
    let height = area.height as usize;
    let total = buffer.lines.len();
    let end = total.saturating_sub(buffer.scroll);
    let start = end.saturating_sub(height);
    let lines: Vec<Line> = buffer.lines[start..end]
        .iter()
        .map(|l| render_buf_line(app, l))
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

fn render_buf_line<'a>(app: &App, line: &'a BufLine) -> Line<'a> {
    match line {
        BufLine::Event(text) => Line::from(vec![
            Span::styled(pad_left("-!-", NICK_COL), Style::default().fg(theme::EVENT)),
            Span::styled(" │ ", Style::default().fg(theme::FAINT)),
            Span::styled(text.clone(), Style::default().fg(theme::EVENT)),
        ]),
        BufLine::Chat {
            time,
            nick,
            text,
            notice,
            mention,
        } => {
            let mut spans: Vec<Span> = Vec::new();
            if app.timestamps {
                let t = time.clone().unwrap_or_default();
                spans.push(Span::styled(
                    format!("{t:5} "),
                    Style::default().fg(theme::DIM2),
                ));
            }
            spans.push(Span::styled(
                pad_left(nick, NICK_COL),
                Style::default().fg(theme::nick_color(nick)),
            ));
            spans.push(Span::styled(" │ ", Style::default().fg(theme::FAINT)));
            let text_style = if *mention {
                Style::default().fg(theme::GOLD).bg(theme::HL_BG)
            } else if *notice {
                Style::default().fg(theme::DIM)
            } else {
                Style::default().fg(theme::TEXT)
            };
            spans.push(Span::styled(text.clone(), text_style));
            Line::from(spans)
        }
    }
}

fn draw_nicklist(f: &mut Frame, area: Rect, app: &App) {
    f.render_widget(
        Block::default().style(Style::default().bg(theme::PANEL)),
        area,
    );
    let buffer = app.active_buffer();
    let members = buffer.sorted_members();
    let mut lines = vec![Line::from(Span::styled(
        format!("{} nicks", members.len()),
        Style::default().fg(theme::DIM2),
    ))];
    for m in members {
        let (sym, color) = match m.highest() {
            Some(p) if p.symbol() == '@' => ('@', theme::GOLD),
            Some(p) if p.symbol() == '+' => ('+', theme::ACCENT),
            Some(p) => (p.symbol(), theme::TEXT),
            None => (' ', theme::nick_color(&m.nick)),
        };
        lines.push(Line::from(Span::styled(
            format!("{sym}{}", m.nick),
            Style::default().fg(color),
        )));
    }
    f.render_widget(Paragraph::new(lines), inset(area));
}

fn draw_activity(f: &mut Frame, area: Rect, app: &App) {
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
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::ACTIVE_BG)),
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
        let label = if b.kind == BufferKind::Server {
            format!("{} (status)", app.networks[b.net].name)
        } else {
            format!("{}  {}", b.name, app.networks[b.net].name)
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
    use crate::session::{ConnState, UiEvent, UiEventKind};
    use crate::tui::state::{App, NetworkMeta};
    use irc_engine::Event;
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
        let mut app = App::new(nets, true, theme::ACCENT);
        app.apply(UiEvent {
            net: 0,
            kind: UiEventKind::Engine(Event::TopicChanged {
                target: "#ratatui".into(),
                topic: Some("Rust TUI library".into()),
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

        // The input line sits inside the center column: the sidebar area (cols
        // 0..24) on the last row is blank, and the prompt is to its right.
        let last = 23u16;
        let sidebar: String = (0..24)
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
        let mut app = App::new(nets, true, theme::ACCENT);
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
}
