//! TUI application state and engine-event routing.

use chrono::Local;
use irc_engine::{Event, LeaveReason, Member, MessageKind};
use irc_proto::Source;
use ratatui::style::Color;

use crate::session::{ConnState, NetworkId, UiEvent, UiEventKind};

/// Max lines kept per buffer.
const MAX_LINES: usize = 5000;

/// The kind of a buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferKind {
    /// A network's status/server buffer.
    Server,
    /// A channel.
    Channel,
    /// A private query.
    Query,
}

/// One rendered line in a buffer.
#[derive(Debug, Clone)]
pub enum Line {
    /// A chat message.
    Chat {
        /// Formatted `HH:MM`, if the message carried a time.
        time: Option<String>,
        /// The sender nick.
        nick: String,
        /// The message text.
        text: String,
        /// Whether this is a NOTICE (vs PRIVMSG).
        notice: bool,
        /// Whether it mentions us.
        mention: bool,
    },
    /// A status/event line (joins, topics, notices).
    Event {
        /// Local `HH:MM` when the event was received.
        time: Option<String>,
        /// The event text.
        text: String,
    },
}

/// Current local time as `HH:MM`.
fn now_hm() -> String {
    Local::now().format("%H:%M").to_string()
}

/// Format a UTC message time as local `HH:MM`.
fn local_hm(time: chrono::DateTime<chrono::Utc>) -> String {
    time.with_timezone(&Local).format("%H:%M").to_string()
}

/// Build a timestamped event line.
fn event_line(text: String) -> Line {
    Line::Event {
        time: Some(now_hm()),
        text,
    }
}

/// A network's metadata.
#[derive(Debug, Clone)]
pub struct NetworkMeta {
    /// Display name.
    pub name: String,
    /// The nick we registered with.
    pub my_nick: String,
    /// Connection status.
    pub state: ConnState,
}

/// A single buffer (server status, channel, or query).
#[derive(Debug, Clone)]
pub struct Buffer {
    /// Owning network.
    pub net: NetworkId,
    /// Buffer name (`*` for server; channel or nick otherwise).
    pub name: String,
    /// Kind.
    pub kind: BufferKind,
    /// Rendered lines.
    pub lines: Vec<Line>,
    /// Channel members (for the nicklist).
    pub members: Vec<Member>,
    /// Channel topic.
    pub topic: Option<String>,
    /// Unread line count since last active.
    pub unread: usize,
    /// Whether an unread line mentions us.
    pub mentioned: bool,
    /// Line index where reading last stopped (for the unread divider).
    pub unread_marker: Option<usize>,
    /// Lines scrolled up from the bottom (0 = following live).
    pub scroll: usize,
}

impl Buffer {
    fn new(net: NetworkId, name: impl Into<String>, kind: BufferKind) -> Self {
        Buffer {
            net,
            name: name.into(),
            kind,
            lines: Vec::new(),
            members: Vec::new(),
            topic: None,
            unread: 0,
            mentioned: false,
            unread_marker: None,
            scroll: 0,
        }
    }

    fn push(&mut self, line: Line) {
        self.lines.push(line);
        if self.lines.len() > MAX_LINES {
            let overflow = self.lines.len() - MAX_LINES;
            self.lines.drain(0..overflow);
            if let Some(marker) = &mut self.unread_marker {
                *marker = marker.saturating_sub(overflow);
            }
        }
    }

    /// Members sorted for display: by prefix rank, then nick.
    pub fn sorted_members(&self) -> Vec<Member> {
        let mut members = self.members.clone();
        members.sort_by(|a, b| {
            let ra = a.highest().map(|p| p.rank()).unwrap_or(u8::MAX);
            let rb = b.highest().map(|p| p.rank()).unwrap_or(u8::MAX);
            ra.cmp(&rb)
                .then_with(|| a.nick.to_lowercase().cmp(&b.nick.to_lowercase()))
        });
        members
    }
}

/// Current interaction mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Normal chat/input.
    Normal,
    /// The Ctrl+K buffer switcher is open.
    Switcher,
}

/// Buffer-switcher overlay state.
#[derive(Debug, Clone, Default)]
pub struct Switcher {
    /// The filter query.
    pub query: String,
    /// Selected index into the filtered list.
    pub sel: usize,
}

/// In-progress tab completion.
#[derive(Debug, Clone)]
pub struct Completion {
    /// Candidate nicks.
    pub matches: Vec<String>,
    /// Selected candidate.
    pub idx: usize,
    /// Byte range in the input being replaced.
    pub start: usize,
}

/// The whole TUI application state.
pub struct App {
    /// Networks, indexed by `NetworkId`.
    pub networks: Vec<NetworkMeta>,
    /// All buffers, grouped by network in display order.
    pub buffers: Vec<Buffer>,
    /// Index of the active buffer.
    pub active: usize,
    /// The input line.
    pub input: String,
    /// Cursor position (byte offset into `input`).
    pub cursor: usize,
    /// Interaction mode.
    pub mode: Mode,
    /// Switcher state.
    pub switcher: Switcher,
    /// Tab-completion state.
    pub completion: Option<Completion>,
    /// Whether the nicklist is shown.
    pub nicklist_visible: bool,
    /// Whether timestamps are shown.
    pub timestamps: bool,
    /// Accent color.
    pub accent: Color,
    /// Previously submitted input lines (for recall).
    pub history: Vec<String>,
    /// Position while navigating history (`None` = at the live draft).
    pub history_pos: Option<usize>,
    /// The in-progress input saved when navigating history.
    pub draft: String,
    /// Set when a redraw is needed.
    pub dirty: bool,
    /// Set when the app should exit.
    pub should_quit: bool,
}

impl App {
    /// Build an app with a server buffer per network.
    pub fn new(networks: Vec<NetworkMeta>, timestamps: bool, accent: Color) -> Self {
        let buffers = networks
            .iter()
            .enumerate()
            .map(|(id, _)| Buffer::new(id, "*", BufferKind::Server))
            .collect();
        App {
            networks,
            buffers,
            active: 0,
            input: String::new(),
            cursor: 0,
            mode: Mode::Normal,
            switcher: Switcher::default(),
            completion: None,
            nicklist_visible: true,
            timestamps,
            accent,
            history: Vec::new(),
            history_pos: None,
            draft: String::new(),
            dirty: true,
            should_quit: false,
        }
    }

    /// The active buffer.
    pub fn active_buffer(&self) -> &Buffer {
        &self.buffers[self.active]
    }

    /// The nick we use on the active buffer's network.
    pub fn my_nick(&self) -> &str {
        &self.networks[self.active_buffer().net].my_nick
    }

    fn buffer_index(&self, net: NetworkId, name: &str) -> Option<usize> {
        self.buffers
            .iter()
            .position(|b| b.net == net && b.name.eq_ignore_ascii_case(name))
    }

    /// Find or create a buffer, keeping buffers grouped by network. Returns its
    /// index.
    fn ensure_buffer(&mut self, net: NetworkId, name: &str, kind: BufferKind) -> usize {
        if let Some(i) = self.buffer_index(net, name) {
            return i;
        }
        let pos = self
            .buffers
            .iter()
            .rposition(|b| b.net == net)
            .map(|i| i + 1)
            .unwrap_or(self.buffers.len());
        self.buffers.insert(pos, Buffer::new(net, name, kind));
        if pos <= self.active && self.buffers.len() > 1 {
            self.active += 1;
        }
        pos
    }

    /// Apply a network-tagged UI event.
    pub fn apply(&mut self, event: UiEvent) {
        self.dirty = true;
        match event.kind {
            UiEventKind::ConnState(state) => self.apply_conn_state(event.net, state),
            UiEventKind::Info(text) => {
                let i = self.server_buffer(event.net);
                self.buffers[i].push(event_line(text));
            }
            UiEventKind::Engine(engine_event) => self.apply_engine(event.net, engine_event),
        }
    }

    fn server_buffer(&mut self, net: NetworkId) -> usize {
        self.ensure_buffer(net, "*", BufferKind::Server)
    }

    fn apply_conn_state(&mut self, net: NetworkId, state: ConnState) {
        if let ConnState::Registered { nick } = &state {
            self.networks[net].my_nick = nick.clone();
        }
        let text = match &state {
            ConnState::Connecting => "connecting...".to_string(),
            ConnState::Registered { nick } => format!("registered as {nick}"),
            ConnState::Reconnecting { delay } => {
                format!("reconnecting in {}s...", delay.as_secs())
            }
            ConnState::Closed => "connection closed".to_string(),
        };
        self.networks[net].state = state;
        let i = self.server_buffer(net);
        self.buffers[i].push(event_line(text));
    }

    fn apply_engine(&mut self, net: NetworkId, event: Event) {
        match event {
            Event::MessageReceived(msg) => {
                let my_nick = self.networks[net].my_nick.clone();
                let sender = msg
                    .sender
                    .as_ref()
                    .map(source_nick)
                    .unwrap_or("?")
                    .to_string();
                // A message to us personally opens a query with the sender.
                let (target, kind) = if msg.target.eq_ignore_ascii_case(&my_nick) {
                    (sender.clone(), BufferKind::Query)
                } else {
                    (msg.target.clone(), BufferKind::Channel)
                };
                let mention = mentions(&msg.text, &my_nick);
                let line = Line::Chat {
                    time: msg.time.map(local_hm),
                    nick: sender,
                    text: msg.text.clone(),
                    notice: msg.kind == MessageKind::Notice,
                    mention,
                };
                self.push_to(net, &target, kind, line, mention);
            }
            Event::HistoryLoaded {
                target, messages, ..
            } => {
                let idx = self.ensure_buffer(net, &target, BufferKind::Channel);
                let mut lines: Vec<Line> = messages
                    .iter()
                    .map(|m| Line::Chat {
                        time: m.time.map(local_hm),
                        nick: m
                            .sender
                            .as_ref()
                            .map(source_nick)
                            .unwrap_or("?")
                            .to_string(),
                        text: m.text.clone(),
                        notice: m.kind == MessageKind::Notice,
                        mention: false,
                    })
                    .collect();
                lines.append(&mut self.buffers[idx].lines);
                self.buffers[idx].lines = lines;
            }
            Event::NamesLoaded { target, members } => {
                let idx = self.ensure_buffer(net, &target, BufferKind::Channel);
                self.buffers[idx].members = members;
            }
            Event::TopicChanged {
                target,
                topic,
                set_by,
                ..
            } => {
                let idx = self.ensure_buffer(net, &target, BufferKind::Channel);
                // Set text when present; a bare 331 (no setter) clears it; a
                // metadata-only 333 (setter, no text) leaves the text alone.
                if topic.is_some() {
                    self.buffers[idx].topic = topic.clone();
                } else if set_by.is_none() {
                    self.buffers[idx].topic = None;
                }
                if let Some(text) = topic_line(&target, &topic, &set_by) {
                    self.buffers[idx].push(event_line(text));
                }
            }
            Event::MemberJoined { target, who, .. } => {
                let idx = self.ensure_buffer(net, &target, BufferKind::Channel);
                if !self.buffers[idx]
                    .members
                    .iter()
                    .any(|m| m.nick.eq_ignore_ascii_case(&who.nick))
                {
                    self.buffers[idx].members.push(Member {
                        nick: who.nick.clone(),
                        prefixes: Vec::new(),
                    });
                }
                self.buffers[idx].push(event_line(format!("{} joined {target}", who.nick)));
            }
            Event::MemberLeft {
                target,
                who,
                reason,
            } => {
                let text = match &reason {
                    LeaveReason::Part(r) => format!("{} left {target} ({r})", who.nick),
                    LeaveReason::Quit(r) => format!("{} quit ({r})", who.nick),
                    LeaveReason::Kicked { by, reason } => {
                        format!("{} was kicked by {by} ({reason})", who.nick)
                    }
                };
                if matches!(reason, LeaveReason::Quit(_)) {
                    for b in self.buffers.iter_mut().filter(|b| b.net == net) {
                        b.members
                            .retain(|m| !m.nick.eq_ignore_ascii_case(&who.nick));
                    }
                } else if let Some(idx) = self.buffer_index(net, &target) {
                    self.buffers[idx]
                        .members
                        .retain(|m| !m.nick.eq_ignore_ascii_case(&who.nick));
                    self.buffers[idx].push(event_line(text));
                }
            }
            Event::NickChanged { old, new } => {
                for b in self.buffers.iter_mut().filter(|b| b.net == net) {
                    for m in &mut b.members {
                        if m.nick.eq_ignore_ascii_case(&old) {
                            m.nick = new.clone();
                        }
                    }
                }
            }
            Event::StandardReply(reply) => {
                let i = self.server_buffer(net);
                self.buffers[i].push(event_line(format!(
                    "{:?} {} {}: {}",
                    reply.kind, reply.command, reply.code, reply.description
                )));
            }
            Event::AuthResult(result) => {
                let text = match result {
                    Ok(account) => format!("authenticated as {account}"),
                    Err(err) => format!("authentication failed: {err}"),
                };
                let i = self.server_buffer(net);
                self.buffers[i].push(event_line(text));
            }
            // Registered / caps / other state: surfaced via ConnState + server
            // buffer; ignore here to avoid duplicate lines.
            _ => {}
        }
    }

    /// Append a chat line to a buffer, bumping unread if it is not active.
    fn push_to(&mut self, net: NetworkId, name: &str, kind: BufferKind, line: Line, mention: bool) {
        let idx = self.ensure_buffer(net, name, kind);
        self.buffers[idx].push(line);
        if idx != self.active {
            self.buffers[idx].unread += 1;
            if mention {
                self.buffers[idx].mentioned = true;
            }
        }
    }

    /// Switch to a buffer by index, clearing its unread state and marking where
    /// reading stopped in the buffer we leave (for the unread divider).
    pub fn switch_to(&mut self, index: usize) {
        if index >= self.buffers.len() {
            return;
        }
        let old = self.active;
        if old != index {
            let len = self.buffers[old].lines.len();
            self.buffers[old].unread_marker = Some(len);
        }
        self.active = index;
        self.buffers[index].unread = 0;
        self.buffers[index].mentioned = false;
        self.buffers[index].scroll = 0;
        self.dirty = true;
    }

    /// Push a local feedback/status line into the active buffer.
    pub fn push_active_event(&mut self, text: String) {
        let idx = self.active;
        self.buffers[idx].push(event_line(text));
        self.dirty = true;
    }

    /// Open (or focus) a query buffer with `nick` on `net`.
    pub fn open_query(&mut self, net: NetworkId, nick: &str) {
        let idx = self.ensure_buffer(net, nick, BufferKind::Query);
        self.switch_to(idx);
    }

    /// Close the active buffer (unless it is a server buffer). Returns the
    /// channel name if a channel was closed (so the caller can PART it).
    pub fn close_active(&mut self) -> Option<String> {
        if self.active_buffer().kind == BufferKind::Server {
            return None;
        }
        let buffer = self.buffers.remove(self.active);
        if self.active >= self.buffers.len() {
            self.active = self.buffers.len().saturating_sub(1);
        }
        self.dirty = true;
        (buffer.kind == BufferKind::Channel).then_some(buffer.name)
    }

    /// Record a submitted input line for history recall.
    pub fn remember_input(&mut self, line: &str) {
        self.history_pos = None;
        if line.is_empty() {
            return;
        }
        if self.history.last().map(String::as_str) != Some(line) {
            self.history.push(line.to_string());
        }
    }

    /// Recall the previous history entry into the input.
    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let pos = match self.history_pos {
            None => {
                self.draft = self.input.clone();
                self.history.len() - 1
            }
            Some(p) => p.saturating_sub(1),
        };
        self.history_pos = Some(pos);
        self.input = self.history[pos].clone();
        self.cursor = self.input.len();
        self.dirty = true;
    }

    /// Recall the next history entry (or restore the draft) into the input.
    pub fn history_next(&mut self) {
        match self.history_pos {
            Some(p) if p + 1 < self.history.len() => {
                self.history_pos = Some(p + 1);
                self.input = self.history[p + 1].clone();
            }
            Some(_) => {
                self.history_pos = None;
                self.input = std::mem::take(&mut self.draft);
            }
            None => return,
        }
        self.cursor = self.input.len();
        self.dirty = true;
    }
}

/// Whether `text` mentions `nick` as a whole word.
fn mentions(text: &str, nick: &str) -> bool {
    if nick.is_empty() {
        return false;
    }
    let lower = text.to_lowercase();
    let nick = nick.to_lowercase();
    lower
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .any(|word| word == nick)
}

fn source_nick(source: &Source) -> &str {
    match source {
        Source::User { nick, .. } => nick,
        Source::Server(name) => name,
    }
}

fn topic_line(target: &str, topic: &Option<String>, set_by: &Option<String>) -> Option<String> {
    match (topic, set_by) {
        (Some(t), _) => Some(format!("topic for {target}: {t}")),
        (None, Some(by)) => Some(format!("topic set by {by}")),
        (None, None) => Some(format!("{target} has no topic")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let nets = vec![
            NetworkMeta {
                name: "neta".into(),
                my_nick: "me".into(),
                state: ConnState::Connecting,
            },
            NetworkMeta {
                name: "netb".into(),
                my_nick: "me2".into(),
                state: ConnState::Connecting,
            },
        ];
        App::new(nets, true, Color::Reset)
    }

    fn engine(net: NetworkId, event: Event) -> UiEvent {
        UiEvent {
            net,
            kind: UiEventKind::Engine(event),
        }
    }

    #[test]
    fn server_buffers_exist_per_network() {
        let a = app();
        assert_eq!(a.buffers.len(), 2);
        assert!(a.buffers.iter().all(|b| b.kind == BufferKind::Server));
    }

    #[test]
    fn message_creates_channel_buffer_grouped_by_network() {
        let mut a = app();
        a.apply(engine(
            0,
            Event::MessageReceived(chat("#rust", "alice", "hi me!")),
        ));
        let idx = a.buffer_index(0, "#rust").unwrap();
        assert_eq!(a.buffers[idx].kind, BufferKind::Channel);
        // Unread bumped (not active) and mention detected.
        assert_eq!(a.buffers[idx].unread, 1);
        assert!(a.buffers[idx].mentioned);
        // The net-1 server buffer stays after net-0's buffers.
        assert!(a.buffers.last().unwrap().net == 1);
    }

    #[test]
    fn direct_message_opens_query_with_sender() {
        let mut a = app();
        a.apply(engine(0, Event::MessageReceived(chat("me", "bob", "yo"))));
        assert!(a.buffer_index(0, "bob").is_some());
    }

    #[test]
    fn names_and_join_and_part_update_members() {
        let mut a = app();
        a.apply(engine(
            0,
            Event::NamesLoaded {
                target: "#rust".into(),
                members: vec![member("@alice"), member("bob")],
            },
        ));
        let idx = a.buffer_index(0, "#rust").unwrap();
        assert_eq!(a.buffers[idx].members.len(), 2);
        a.apply(engine(
            0,
            Event::MemberJoined {
                target: "#rust".into(),
                who: irc_engine::User::nick("carol"),
                account: None,
            },
        ));
        assert_eq!(a.buffers[idx].members.len(), 3);
    }

    #[test]
    fn topic_sets_and_metadata_does_not_clear() {
        let mut a = app();
        a.apply(engine(
            0,
            Event::TopicChanged {
                target: "#rust".into(),
                topic: Some("hello".into()),
                set_by: None,
                set_at: None,
            },
        ));
        let idx = a.buffer_index(0, "#rust").unwrap();
        assert_eq!(a.buffers[idx].topic.as_deref(), Some("hello"));
        // 333-style metadata: setter present, no text -> keep the topic.
        a.apply(engine(
            0,
            Event::TopicChanged {
                target: "#rust".into(),
                topic: None,
                set_by: Some("op".into()),
                set_at: None,
            },
        ));
        assert_eq!(a.buffers[idx].topic.as_deref(), Some("hello"));
    }

    #[test]
    fn switch_clears_unread() {
        let mut a = app();
        a.apply(engine(0, Event::MessageReceived(chat("#rust", "x", "hi"))));
        let idx = a.buffer_index(0, "#rust").unwrap();
        a.switch_to(idx);
        assert_eq!(a.active, idx);
        assert_eq!(a.buffers[idx].unread, 0);
    }

    fn chat(target: &str, from: &str, text: &str) -> irc_engine::ChatMessage {
        irc_engine::ChatMessage {
            time: None,
            msgid: None,
            account: None,
            sender: Some(Source::User {
                nick: from.into(),
                user: None,
                host: None,
            }),
            target: target.into(),
            text: text.into(),
            kind: MessageKind::Privmsg,
        }
    }

    fn member(token: &str) -> Member {
        let mut r = irc_engine::Roster::new();
        r.apply_names_reply(token);
        r.snapshot().pop().unwrap()
    }
}
