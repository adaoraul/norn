//! TUI application state and engine-event routing.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use chrono::Local;
use irc_engine::{
    DisconnectReason, Event, LeaveReason, Member, MessageKind, TopicChange, WhoisInfo,
};
use irc_proto::Source;
use ratatui::style::Color;

use crate::addons::PluginInfo;
use crate::config::{ClientConfig, Config, NetworkConfig, SaslMech, TriggerConfig};
use crate::session::{ConnState, NetCommand, NetworkId, UiEvent, UiEventKind};
use crate::tui::theme;

/// How many older messages to pull per scroll-up page.
const HISTORY_PAGE: usize = 50;

/// How many submitted input lines are kept for recall (oldest dropped first).
/// History stays in memory only: it can hold passwords typed into
/// `/msg NickServ IDENTIFY ...`, and passwords never go to disk.
pub const HISTORY_MAX: usize = 500;

/// The message-pane height assumed until the first draw reports the real one.
const DEFAULT_MSG_ROWS: usize = 12;

/// How many lines one mouse-wheel notch scrolls.
pub const WHEEL_LINES: isize = 3;

/// Sentinel `NetworkId` for the global console buffer, which belongs to no
/// network. Being out of range of `networks`/`cmd_txs`, it is naturally
/// excluded from every per-network iteration and lookup.
pub const CONSOLE: NetworkId = usize::MAX;

/// The kind of a buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferKind {
    /// The global "norn" console: welcome text, settings, and network output.
    /// Not tied to any network.
    Status,
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
        /// Whether this is a CTCP ACTION (`/me`).
        action: bool,
        /// The message id, if any (for history pagination cursors).
        msgid: Option<String>,
    },
    /// A status/event line (joins, topics, notices).
    Event {
        /// Local `HH:MM` when the event was received.
        time: Option<String>,
        /// The event text.
        text: String,
        /// Whether this reports a failure (rendered in the error style).
        error: bool,
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
        error: false,
    }
}

/// Build a timestamped error line.
fn error_line(text: String) -> Line {
    Line::Event {
        time: Some(now_hm()),
        text,
        error: true,
    }
}

/// The inner text of a CTCP ACTION (`\x01ACTION ...\x01`), if `text` is one.
pub(crate) fn ctcp_action(text: &str) -> Option<&str> {
    let inner = text.strip_prefix('\u{1}')?.strip_prefix("ACTION ")?;
    Some(inner.strip_suffix('\u{1}').unwrap_or(inner))
}

/// A control-plane request from the UI to the supervisor (the `tui::run` loop),
/// which owns the tokio machinery needed to spawn and signal network tasks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppAction {
    /// Spawn and connect a task for a just-allocated network id.
    AddNetwork {
        /// The allocated id (equals `cmd_txs.len()` at spawn time).
        id: NetworkId,
        /// The network definition to resolve and dial.
        config: NetworkConfig,
    },
    /// Ask an existing idle network to (re)connect.
    Connect(NetworkId),
    /// Ask a network to disconnect (go idle), with an optional quit reason.
    Disconnect(NetworkId, Option<String>),
    /// Rebuild the addon host from `app.triggers` after an in-app edit.
    ReloadAddons,
    /// Turn terminal mouse capture on or off (the `mouse` setting).
    SetMouse(bool),
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
    /// Whether we are currently marked away (RPL_NOWAWAY/RPL_UNAWAY).
    pub away: bool,
    /// Our services account, if logged in (SASL or `account-notify`).
    pub account: Option<String>,
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
    /// Whether an older-history request is in flight (avoids duplicate loads).
    pub history_pending: bool,
    /// Whether the server has reported no older history remains (a short page),
    /// so scrolling up should stop requesting.
    pub history_exhausted: bool,
    /// Lines scrolled up from the bottom (0 = following live).
    pub scroll: usize,
    /// Maximum lines retained (the `scrollback_lines` client setting).
    pub max_lines: usize,
    /// A channel's flag modes, e.g. `+nt` (empty when unknown or none).
    pub modes: String,
    /// Whether we are in this channel. False after we part or are kicked; the
    /// buffer stays so the scrollback can still be read.
    pub joined: bool,
    /// When each nick (lowercased) last spoke here, as a rising counter, so
    /// completion can offer the people talking right now first.
    pub last_spoke: std::collections::HashMap<String, u64>,
    /// The counter behind `last_spoke`.
    speak_clock: u64,
}

impl Buffer {
    fn new(net: NetworkId, name: impl Into<String>, kind: BufferKind, max_lines: usize) -> Self {
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
            history_pending: false,
            history_exhausted: false,
            scroll: 0,
            max_lines,
            modes: String::new(),
            joined: true,
            last_spoke: std::collections::HashMap::new(),
            speak_clock: 0,
        }
    }

    /// Note that `nick` just spoke.
    fn note_spoke(&mut self, nick: &str) {
        self.speak_clock += 1;
        self.last_spoke
            .insert(nick.to_ascii_lowercase(), self.speak_clock);
    }

    fn push(&mut self, line: Line) {
        self.lines.push(line);
        if self.lines.len() > self.max_lines {
            let overflow = self.lines.len() - self.max_lines;
            self.lines.drain(0..overflow);
            if let Some(marker) = &mut self.unread_marker {
                *marker = marker.saturating_sub(overflow);
            }
        }
    }

    /// Change the retained-line cap, trimming immediately if it shrank.
    fn set_max_lines(&mut self, max_lines: usize) {
        self.max_lines = max_lines.max(1);
        if self.lines.len() > self.max_lines {
            let overflow = self.lines.len() - self.max_lines;
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

/// How long a "press again to confirm" stays armed.
pub const CONFIRM_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

/// A destructive action waiting for a second press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    /// Leave norn (Ctrl+C).
    Quit,
    /// Delete the network definition with this name.
    DeleteNetwork(String),
    /// Delete the alias with this name.
    DeleteAlias(String),
}

impl Confirm {
    /// What to tell the user while this is armed.
    pub fn prompt(&self) -> String {
        match self {
            Confirm::Quit => "press Ctrl+C again to quit".to_string(),
            Confirm::DeleteNetwork(name) => format!("press x again to delete '{name}'"),
            Confirm::DeleteAlias(name) => format!("press Delete again to remove alias /{name}"),
        }
    }
}

/// Current interaction mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Normal chat/input.
    Normal,
    /// The Ctrl+K buffer switcher is open.
    Switcher,
    /// The `/help` panel is open.
    Help,
    /// The `/settings` panel is open.
    Settings,
    /// The `/networks` manager is open.
    Networks,
    /// The `/plugins` manager is open.
    Plugins,
    /// A single plugin's config editor is open.
    PluginConfig,
    /// A multi-line paste is waiting for Enter (send) or Esc (discard).
    PasteConfirm,
}

/// Which pane of the `/help` panel has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HelpFocus {
    /// The command list (filtering and selection).
    #[default]
    List,
    /// The detail pane (scrolling the selected command's docs).
    Detail,
}

/// State of the `/help` panel: a filter, the selected command, which pane has
/// focus, and the detail pane's scroll offset.
#[derive(Debug, Clone, Default)]
pub struct HelpState {
    /// The filter query (matched against name/usage/summary).
    pub query: String,
    /// Index of the highlighted command among the current matches.
    pub sel: usize,
    /// Which pane is focused.
    pub focus: HelpFocus,
    /// First visible line of the detail pane.
    pub detail_scroll: usize,
}

/// State of the `/plugins` manager: which plugin row is selected.
#[derive(Debug, Clone, Default)]
pub struct PluginsState {
    /// Index of the highlighted plugin (into `App.plugins`).
    pub sel: usize,
}

/// State of a single plugin's config editor (`/plugins config <name>`).
#[derive(Debug, Clone, Default)]
pub struct PluginConfigState {
    /// The plugin's filename (key into `plugin_config`).
    pub file: String,
    /// The plugin's display name (for the header).
    pub name: String,
    /// Index of the highlighted config key.
    pub sel: usize,
    /// When editing a value, the in-progress text.
    pub editing: Option<String>,
    /// The cursor within `editing` (a byte offset).
    pub edit_cursor: usize,
    /// A transient status line (e.g. "saved").
    pub msg: Option<String>,
}

impl PluginConfigState {
    /// Start editing `text`, with the cursor at its end.
    pub fn begin_edit(&mut self, text: String) {
        self.edit_cursor = text.len();
        self.editing = Some(text);
    }
}

/// State of the `/settings` panel: a live filter, the selected row, an optional
/// inline edit buffer, and a transient message (e.g. a validation error).
#[derive(Debug, Clone, Default)]
pub struct SettingsState {
    /// The filter query (matched against key/category/description and aliases).
    pub filter: String,
    /// Index of the highlighted row among the selectable (non-header) rows.
    pub sel: usize,
    /// When editing a text/int setting or an alias, the in-progress value.
    pub editing: Option<String>,
    /// The cursor within `editing` (a byte offset).
    pub edit_cursor: usize,
    /// A transient status line (validation error or confirmation).
    pub msg: Option<String>,
}

impl SettingsState {
    /// Start editing `text`, with the cursor at its end.
    pub fn begin_edit(&mut self, text: String) {
        self.edit_cursor = text.len();
        self.editing = Some(text);
    }
}

/// One rendered row of the `/settings` panel: a category header, a client
/// setting, or a user alias. Built by [`App::settings_rows`] and consumed by both
/// the view and the key handler so the two agree on ordering.
#[derive(Debug, Clone)]
pub enum SettingsRow {
    /// A group heading (not selectable).
    Header(&'static str),
    /// A client preference from the settings registry.
    Setting(&'static crate::settings::SettingDoc),
    /// A user-defined command alias.
    Alias {
        /// The alias name.
        name: String,
        /// What it expands to.
        expansion: String,
    },
    /// The row that starts creating a new alias.
    AddAlias,
}

/// Which pane of the `/networks` manager has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NetworksFocus {
    /// The list of network definitions (plus the add row).
    #[default]
    List,
    /// The edit form for the selected definition.
    Form,
}

/// State of the `/networks` manager: the selected list row, which pane has
/// focus, the selected form field, an optional inline edit buffer, and a
/// transient message.
#[derive(Debug, Clone, Default)]
pub struct NetworksState {
    /// Index into the left list (definitions, then the add row).
    pub sel: usize,
    /// Which pane is focused.
    pub focus: NetworksFocus,
    /// Selected form field index (into [`NETWORK_FIELDS`]).
    pub field: usize,
    /// When editing a text field, the in-progress value.
    pub editing: Option<String>,
    /// The cursor within `editing` (a byte offset).
    pub edit_cursor: usize,
    /// A transient status line (validation error or confirmation).
    pub msg: Option<String>,
}

impl NetworksState {
    /// Start editing `text`, with the cursor at its end.
    pub fn begin_edit(&mut self, text: String) {
        self.edit_cursor = text.len();
        self.editing = Some(text);
    }
}

/// How a network form field is edited and rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetFieldKind {
    /// Freeform text (parsed on commit for `port`).
    Text,
    /// A boolean, toggled in place and shown as a checkbox.
    Toggle,
    /// The SASL mechanism, cycled in place.
    Mech,
}

/// One field of the network edit form. Deliberately has no plaintext password
/// field: authentication is configured through `password_command` only.
#[derive(Debug, Clone, Copy)]
pub struct NetField {
    /// The field name (matches the `NetworkConfig` field / `/network add` key).
    pub name: &'static str,
    /// How it is edited and rendered.
    pub kind: NetFieldKind,
}

/// The editable network fields, in form order.
pub const NETWORK_FIELDS: &[NetField] = &[
    NetField {
        name: "name",
        kind: NetFieldKind::Text,
    },
    NetField {
        name: "host",
        kind: NetFieldKind::Text,
    },
    NetField {
        name: "port",
        kind: NetFieldKind::Text,
    },
    NetField {
        name: "tls",
        kind: NetFieldKind::Toggle,
    },
    NetField {
        name: "auto_connect",
        kind: NetFieldKind::Toggle,
    },
    NetField {
        name: "identify",
        kind: NetFieldKind::Toggle,
    },
    NetField {
        name: "nick",
        kind: NetFieldKind::Text,
    },
    NetField {
        name: "user",
        kind: NetFieldKind::Text,
    },
    NetField {
        name: "realname",
        kind: NetFieldKind::Text,
    },
    NetField {
        name: "sasl_account",
        kind: NetFieldKind::Text,
    },
    NetField {
        name: "sasl_mech",
        kind: NetFieldKind::Mech,
    },
    NetField {
        name: "password_command",
        kind: NetFieldKind::Text,
    },
    NetField {
        name: "auto_join",
        kind: NetFieldKind::Text,
    },
];

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
    /// Candidate completions (nicks or command names).
    pub matches: Vec<String>,
    /// Selected candidate.
    pub idx: usize,
    /// Byte offset in the input where the replaced token starts.
    pub start: usize,
    /// Text appended after the inserted candidate (e.g. `": "` for a leading
    /// nick, `" "` for a command, `""` mid-line).
    pub suffix: String,
    /// What followed the cursor when completion began; kept, so completing in
    /// the middle of a line does not eat the rest of it.
    pub tail: String,
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
    /// Help-panel state.
    pub help: HelpState,
    /// Settings-panel state.
    pub settings: SettingsState,
    /// Networks-manager state.
    pub networks_ui: NetworksState,
    /// Plugins-manager state.
    pub plugins_ui: PluginsState,
    /// Tab-completion state.
    pub completion: Option<Completion>,
    /// Whether the nicklist is shown.
    pub nicklist_visible: bool,
    /// Whether timestamps are shown.
    pub timestamps: bool,
    /// Accent color.
    pub accent: Color,
    /// Client preferences (persisted `[client]` section).
    pub client: ClientConfig,
    /// The persisted network definitions (authoritative for `/network`).
    pub definitions: Vec<NetworkConfig>,
    /// User-defined command aliases (name -> expansion template).
    pub aliases: BTreeMap<String, String>,
    /// Declarative addon triggers (persisted; the live host is rebuilt from these
    /// when they change).
    pub triggers: Vec<TriggerConfig>,
    /// Disabled addon-script filenames (persisted).
    pub disabled_plugins: Vec<String>,
    /// Per-plugin config overrides (persisted): filename -> key -> value.
    pub plugin_config:
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    /// State of the per-plugin config editor.
    pub plugin_cfg: PluginConfigState,
    /// Discovered addon scripts with status/metadata for `/plugins` (runtime).
    pub plugins: Vec<PluginInfo>,
    /// Whether any loaded plugin uses a presence accessor (skips the per-event
    /// presence snapshot when false).
    pub needs_presence: bool,
    /// Where to auto-save config (`None` if no config dir is available).
    pub config_path: Option<PathBuf>,
    /// Pending control-plane actions for the supervisor to execute.
    pub actions: Vec<AppAction>,
    /// Previously submitted input lines (for recall).
    pub history: Vec<String>,
    /// Position while navigating history (`None` = at the live draft).
    pub history_pos: Option<usize>,
    /// The in-progress input saved when navigating history.
    pub draft: String,
    /// Set when a redraw is needed.
    pub dirty: bool,
    /// Set when a highlight arrived and `beep_on_highlight` is on; the run loop
    /// rings the terminal bell and clears it.
    pub bell: bool,
    /// Set when the app should exit.
    pub should_quit: bool,
    /// The reason given to `/quit`, sent with the QUIT line.
    pub quit_reason: Option<String>,
    /// A channel we just asked to join; the buffer becomes active once our own
    /// JOIN arrives.
    pub pending_switch: Option<(NetworkId, String)>,
    /// Channels whose next NAMES reply was asked for by `/names` and should be
    /// printed (lowercased).
    pub pending_names: HashSet<(NetworkId, String)>,
    /// Networks whose server echoes our own messages (`echo-message`); on the
    /// rest we echo locally.
    pub echo_nets: HashSet<NetworkId>,
    /// The latest PING round-trip time per network.
    pub lag: std::collections::HashMap<NetworkId, std::time::Duration>,
    /// A destructive action waiting for its second press, and when it was armed.
    pub armed: Option<(Confirm, std::time::Instant)>,
    /// The lines of a multi-line paste awaiting confirmation (`Mode::PasteConfirm`).
    pub paste: Vec<String>,
    /// How many rows the message pane had when last drawn; PageUp/PageDown move
    /// by about this much.
    pub msg_rows: usize,
}

impl App {
    /// Build an app with the global console (buffer 0) plus a server buffer per
    /// network. Client preferences seed the live UI toggles.
    pub fn new(
        networks: Vec<NetworkMeta>,
        client: ClientConfig,
        definitions: Vec<NetworkConfig>,
        aliases: BTreeMap<String, String>,
        config_path: Option<PathBuf>,
    ) -> Self {
        let cap = client.scrollback_lines;
        let mut console = Buffer::new(CONSOLE, "norn", BufferKind::Status, cap);
        for line in welcome_lines(networks.is_empty(), &definitions) {
            console.lines.push(event_line(line));
        }
        let mut buffers = vec![console];
        buffers.extend(
            networks
                .iter()
                .enumerate()
                .map(|(id, _)| Buffer::new(id, "*", BufferKind::Server, cap)),
        );
        // Land on the console when nothing is configured; otherwise the first
        // network's server buffer (index 1, right after the console).
        let active = if networks.is_empty() { 0 } else { 1 };
        App {
            networks,
            buffers,
            active,
            input: String::new(),
            cursor: 0,
            mode: Mode::Normal,
            switcher: Switcher::default(),
            help: HelpState::default(),
            settings: SettingsState::default(),
            networks_ui: NetworksState::default(),
            plugins_ui: PluginsState::default(),
            completion: None,
            nicklist_visible: client.nicklist,
            timestamps: client.timestamps,
            accent: theme::accent_for(&client.theme),
            client,
            definitions,
            aliases,
            triggers: Vec::new(),
            disabled_plugins: Vec::new(),
            plugin_config: std::collections::BTreeMap::new(),
            plugin_cfg: PluginConfigState::default(),
            plugins: Vec::new(),
            needs_presence: false,
            config_path,
            actions: Vec::new(),
            history: Vec::new(),
            history_pos: None,
            draft: String::new(),
            dirty: true,
            bell: false,
            should_quit: false,
            quit_reason: None,
            pending_switch: None,
            pending_names: HashSet::new(),
            echo_nets: HashSet::new(),
            lag: std::collections::HashMap::new(),
            armed: None,
            paste: Vec::new(),
            msg_rows: DEFAULT_MSG_ROWS,
        }
    }

    /// Ask for a second press. Returns `true` (and disarms) when `action` was
    /// already armed within [`CONFIRM_WINDOW`]; otherwise arms it and returns
    /// `false`, so the caller shows [`App::armed_prompt`] instead of acting.
    pub fn confirm(&mut self, action: Confirm) -> bool {
        let confirmed = matches!(
            &self.armed,
            Some((armed, at)) if *armed == action && at.elapsed() < CONFIRM_WINDOW
        );
        self.armed = if confirmed {
            None
        } else {
            Some((action, std::time::Instant::now()))
        };
        self.dirty = true;
        confirmed
    }

    /// The question shown while a multi-line paste waits for Enter or Esc.
    pub fn paste_prompt(&self) -> Option<String> {
        if self.mode != Mode::PasteConfirm {
            return None;
        }
        let n = self.paste.len();
        let commands = self.paste.iter().filter(|l| l.starts_with('/')).count();
        let target = match self.active_buffer().kind {
            BufferKind::Channel | BufferKind::Query => self.active_buffer().name.clone(),
            _ => "this buffer".to_string(),
        };
        let warn = if commands > 0 {
            format!(" ({commands} start with / and will run as commands)")
        } else {
            String::new()
        };
        Some(format!(
            "paste {n} lines to {target}{warn}? Enter sends · Esc cancels"
        ))
    }

    /// The prompt for a live armed action, if one is still within its window.
    pub fn armed_prompt(&self) -> Option<String> {
        match &self.armed {
            Some((action, at)) if at.elapsed() < CONFIRM_WINDOW => Some(action.prompt()),
            _ => None,
        }
    }

    /// Whether commands can reach `net` right now. `Err` carries what to tell the
    /// user (the console belongs to no network; an unregistered network drops
    /// every line).
    pub fn send_ready(&self, net: NetworkId) -> Result<(), String> {
        let Some(meta) = self.networks.get(net) else {
            return Err("not on a network; switch to one first, or /connect <name>".to_string());
        };
        match meta.state {
            ConnState::Registered { .. } => Ok(()),
            ConnState::Connecting => Err(format!("{} is still connecting", meta.name)),
            _ => Err(format!("{0} is not connected (/connect {0})", meta.name)),
        }
    }

    /// Show a message we sent in its buffer when the server will not echo it
    /// back. Never counts as unread.
    pub fn echo_local(&mut self, net: NetworkId, out: &crate::input::Outgoing) {
        if self.echo_nets.contains(&net) {
            return;
        }
        let kind = if is_channel_name(&out.target) {
            BufferKind::Channel
        } else {
            BufferKind::Query
        };
        let idx = self.ensure_buffer(net, &out.target, kind);
        let nick = self
            .networks
            .get(net)
            .map(|m| m.my_nick.clone())
            .unwrap_or_default();
        self.buffers[idx].push(Line::Chat {
            time: Some(now_hm()),
            nick,
            text: out.text.clone(),
            notice: out.notice,
            mention: false,
            action: out.action,
            msgid: None,
        });
        self.dirty = true;
    }

    /// The active buffer.
    pub fn active_buffer(&self) -> &Buffer {
        &self.buffers[self.active]
    }

    /// The nick we use on the active buffer's network (falls back to "norn" on
    /// the console, which belongs to no network).
    pub fn my_nick(&self) -> &str {
        self.networks
            .get(self.active_buffer().net)
            .map(|n| n.my_nick.as_str())
            .unwrap_or("norn")
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
        self.buffers.insert(
            pos,
            Buffer::new(net, name, kind, self.client.scrollback_lines),
        );
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
            UiEventKind::Lag(lag) => {
                self.lag.insert(event.net, lag);
            }
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
            ConnState::Disconnected => "disconnected".to_string(),
            ConnState::Closed => "connection closed".to_string(),
        };
        // A measurement from a dead connection says nothing about the next one.
        if !matches!(state, ConnState::Registered { .. }) {
            self.lag.remove(&net);
        }
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
                } else if sender.eq_ignore_ascii_case(&my_nick) && !is_channel_name(&msg.target) {
                    // The echo of a message we sent to a nick belongs in that
                    // nick's query, not a channel buffer named after them.
                    (msg.target.clone(), BufferKind::Query)
                } else {
                    (msg.target.clone(), BufferKind::Channel)
                };
                let (text, action) = match ctcp_action(&msg.text) {
                    Some(inner) => (inner.to_string(), true),
                    None => (msg.text.clone(), false),
                };
                let mention = mentions(&text, &my_nick);
                let spoke = sender.clone();
                let line = Line::Chat {
                    // Fall back to the local receipt time when the message has no
                    // server-time tag (e.g. NickServ notices during connect).
                    time: Some(msg.time.map(local_hm).unwrap_or_else(now_hm)),
                    nick: sender,
                    text,
                    notice: msg.kind == MessageKind::Notice,
                    mention,
                    action,
                    msgid: msg.msgid.clone(),
                };
                self.push_to(net, &target, kind, line, mention);
                // Remember who is talking, so completion offers them first.
                if kind == BufferKind::Channel {
                    if let Some(i) = self.buffer_index(net, &target) {
                        self.buffers[i].note_spoke(&spoke);
                    }
                }
            }
            Event::HistoryLoaded {
                target,
                messages,
                complete,
            } => {
                let idx = self.ensure_buffer(net, &target, BufferKind::Channel);
                let mut lines: Vec<Line> = messages
                    .iter()
                    .map(|m| {
                        let (text, action) = match ctcp_action(&m.text) {
                            Some(inner) => (inner.to_string(), true),
                            None => (m.text.clone(), false),
                        };
                        Line::Chat {
                            time: m.time.map(local_hm),
                            nick: m
                                .sender
                                .as_ref()
                                .map(source_nick)
                                .unwrap_or("?")
                                .to_string(),
                            text,
                            notice: m.kind == MessageKind::Notice,
                            mention: false,
                            action,
                            msgid: m.msgid.clone(),
                        }
                    })
                    .collect();
                // Prepend older messages. The view is anchored from the bottom,
                // so the visible window stays put; the user scrolls further up to
                // reach the newly loaded lines.
                let added = lines.len();
                lines.append(&mut self.buffers[idx].lines);
                self.buffers[idx].lines = lines;
                // Lines were inserted above the divider, so it moves down with them.
                if let Some(marker) = self.buffers[idx].unread_marker.as_mut() {
                    *marker += added;
                }
                self.buffers[idx].history_pending = false;
                // A short page (complete) means there is nothing older; stop
                // paging so scrolling up does not re-request the same top.
                if complete {
                    self.buffers[idx].history_exhausted = true;
                }
            }
            Event::NamesLoaded { target, members } => {
                let asked = self
                    .pending_names
                    .remove(&(net, target.to_ascii_lowercase()));
                if asked {
                    let list: Vec<String> = members
                        .iter()
                        .map(|m| {
                            let prefix = m.highest().map(|p| p.symbol().to_string());
                            format!("{}{}", prefix.unwrap_or_default(), m.nick)
                        })
                        .collect();
                    let text = format!("names {target} ({}): {}", list.len(), list.join(" "));
                    let at = if self.buffers[self.active].net == net {
                        self.active
                    } else {
                        self.server_buffer(net)
                    };
                    self.buffers[at].push(event_line(text));
                }
                // A NAMES for a channel we are not in must not conjure a buffer.
                let idx = match self.buffer_index(net, &target) {
                    Some(i) => i,
                    None if asked => return,
                    None => self.ensure_buffer(net, &target, BufferKind::Channel),
                };
                self.buffers[idx].members = members;
            }
            Event::CapabilitiesChanged { enabled, .. } => {
                if enabled.contains("echo-message") {
                    self.echo_nets.insert(net);
                } else {
                    self.echo_nets.remove(&net);
                }
            }
            Event::TopicChanged {
                target,
                change,
                set_by,
                ..
            } => {
                let idx = self.ensure_buffer(net, &target, BufferKind::Channel);
                // Set/clear the stored text; a metadata-only 333 leaves it alone.
                match &change {
                    TopicChange::Set(t) => self.buffers[idx].topic = Some(t.clone()),
                    TopicChange::Cleared => self.buffers[idx].topic = None,
                    TopicChange::Unchanged => {}
                }
                if let Some(text) = topic_line(&target, &change, &set_by) {
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
                        away: false,
                    });
                }
                self.buffers[idx].push(event_line(format!("{} joined {target}", who.nick)));
                // Our own JOIN completes a /join: show that channel.
                let ours = self
                    .networks
                    .get(net)
                    .is_some_and(|m| m.my_nick.eq_ignore_ascii_case(&who.nick));
                if ours {
                    self.buffers[idx].joined = true;
                }
                if ours
                    && self
                        .pending_switch
                        .as_ref()
                        .is_some_and(|(n, c)| *n == net && c.eq_ignore_ascii_case(&target))
                {
                    self.pending_switch = None;
                    self.switch_to(idx);
                }
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
                    // A quit has no channel: show it where the nick was visible
                    // (shared channels and an open query), then drop them.
                    for b in self.buffers.iter_mut().filter(|b| b.net == net) {
                        let was_member = b
                            .members
                            .iter()
                            .any(|m| m.nick.eq_ignore_ascii_case(&who.nick));
                        let is_query =
                            b.kind == BufferKind::Query && b.name.eq_ignore_ascii_case(&who.nick);
                        if was_member || is_query {
                            b.push(event_line(text.clone()));
                        }
                        b.members
                            .retain(|m| !m.nick.eq_ignore_ascii_case(&who.nick));
                    }
                } else if let Some(idx) = self.buffer_index(net, &target) {
                    self.buffers[idx]
                        .members
                        .retain(|m| !m.nick.eq_ignore_ascii_case(&who.nick));
                    // Leaving (or being kicked) ourselves keeps the buffer but
                    // marks it as no longer joined.
                    let me = self
                        .networks
                        .get(net)
                        .is_some_and(|m| m.my_nick.eq_ignore_ascii_case(&who.nick));
                    if me {
                        self.buffers[idx].joined = false;
                    }
                    self.buffers[idx].push(event_line(text));
                }
            }
            Event::NickChanged { old, new } => {
                let mine = self
                    .networks
                    .get(net)
                    .is_some_and(|m| m.my_nick.eq_ignore_ascii_case(&old));
                if mine {
                    self.networks[net].my_nick = new.clone();
                }
                let has_new_query = self.buffers.iter().any(|b| {
                    b.net == net && b.kind == BufferKind::Query && b.name.eq_ignore_ascii_case(&new)
                });
                let text = if mine {
                    format!("you are now known as {new}")
                } else {
                    format!("{old} is now known as {new}")
                };
                for b in self.buffers.iter_mut().filter(|b| b.net == net) {
                    let mut seen = false;
                    for m in &mut b.members {
                        if m.nick.eq_ignore_ascii_case(&old) {
                            m.nick = new.clone();
                            seen = true;
                        }
                    }
                    let is_query = b.kind == BufferKind::Query && b.name.eq_ignore_ascii_case(&old);
                    if is_query && !has_new_query {
                        b.name = new.clone();
                    }
                    if seen || is_query || (mine && b.kind == BufferKind::Server) {
                        b.push(event_line(text.clone()));
                    }
                }
            }
            // `away-notify` is network-wide: reflect it in every channel where we
            // share membership so the nicklist can dim away nicks.
            Event::AwayChanged { nick, message } => {
                let away = message.is_some();
                for b in self.buffers.iter_mut().filter(|b| b.net == net) {
                    for m in &mut b.members {
                        if m.nick.eq_ignore_ascii_case(&nick) {
                            m.away = away;
                        }
                    }
                }
            }
            // Our own away state, from RPL_NOWAWAY/RPL_UNAWAY.
            Event::AwayStatus(now_away) => {
                if let Some(meta) = self.networks.get_mut(net) {
                    meta.away = now_away;
                }
                let text = if now_away {
                    "you are now marked away"
                } else {
                    "you are no longer away"
                };
                let i = self.server_buffer(net);
                self.buffers[i].push(event_line(text.to_string()));
            }
            // Track our own services account (member accounts are not stored).
            Event::AccountChanged { nick, account } => {
                let mine = self
                    .networks
                    .get(net)
                    .is_some_and(|m| m.my_nick.eq_ignore_ascii_case(&nick));
                if mine {
                    if let Some(meta) = self.networks.get_mut(net) {
                        meta.account = account;
                    }
                }
            }
            Event::StandardReply(reply) => {
                let i = self.server_buffer(net);
                self.buffers[i].push(event_line(format!(
                    "{} {} {}: {}",
                    reply.kind.label(),
                    reply.command,
                    reply.code,
                    reply.description
                )));
            }
            // The server refused something: say so where the user is looking.
            Event::CommandError {
                error,
                target,
                message,
            } => {
                let text = error.describe(target.as_deref(), &message);
                let idx = target
                    .as_deref()
                    .and_then(|t| self.buffer_index(net, t))
                    .or_else(|| (self.buffers[self.active].net == net).then_some(self.active))
                    .unwrap_or_else(|| self.server_buffer(net));
                self.buffers[idx].push(error_line(text));
            }
            Event::Motd(lines) => {
                let i = self.server_buffer(net);
                if lines.is_empty() {
                    self.buffers[i].push(event_line("no message of the day".to_string()));
                } else {
                    self.buffers[i].push(event_line("message of the day:".to_string()));
                    for line in lines {
                        self.buffers[i].push(event_line(format!("  {line}")));
                    }
                }
            }
            Event::ServerInfo(text) => {
                let i = self.server_buffer(net);
                self.buffers[i].push(event_line(text));
            }
            Event::ModeChanged {
                target,
                by,
                modes,
                args,
                prefix_changes,
            } => {
                let text = crate::render::mode_line(&target, by.as_deref(), &modes, &args);
                let idx = match self.buffer_index(net, &target) {
                    Some(i) if self.buffers[i].kind == BufferKind::Channel => i,
                    _ => self.server_buffer(net),
                };
                // Op/voice/... changes move the nick in the nicklist.
                for change in &prefix_changes {
                    for member in &mut self.buffers[idx].members {
                        if member.nick.eq_ignore_ascii_case(&change.nick) {
                            member.prefixes.retain(|p| *p != change.prefix);
                            if change.granted {
                                member.prefixes.push(change.prefix);
                                member.prefixes.sort_by_key(|p| p.rank());
                            }
                        }
                    }
                }
                if self.buffers[idx].kind == BufferKind::Channel {
                    let current = std::mem::take(&mut self.buffers[idx].modes);
                    self.buffers[idx].modes = apply_flag_modes(&current, &modes);
                }
                self.buffers[idx].push(event_line(text));
            }
            Event::ChannelModes { target, modes } => {
                let idx = match self.buffer_index(net, &target) {
                    Some(i) => i,
                    None => self.server_buffer(net),
                };
                if self.buffers[idx].kind == BufferKind::Channel {
                    // "+ntk secret": the flags are the first word.
                    let flags = modes.split_whitespace().next().unwrap_or("");
                    self.buffers[idx].modes = apply_flag_modes("", flags);
                }
                self.buffers[idx].push(event_line(format!("modes for {target}: {modes}")));
            }
            // A split is one line per affected channel, not one quit per user.
            Event::Netsplit { servers, users } => {
                let servers = servers.join(" ");
                for b in self.buffers.iter_mut().filter(|b| b.net == net) {
                    let gone: Vec<String> = users
                        .iter()
                        .filter(|u| {
                            b.members
                                .iter()
                                .any(|m| m.nick.eq_ignore_ascii_case(&u.nick))
                        })
                        .map(|u| u.nick.clone())
                        .collect();
                    if gone.is_empty() {
                        continue;
                    }
                    b.members
                        .retain(|m| !gone.iter().any(|n| n.eq_ignore_ascii_case(&m.nick)));
                    b.push(event_line(format!(
                        "netsplit {servers}: {} left ({})",
                        gone.len(),
                        nick_summary(&gone)
                    )));
                }
            }
            Event::Netjoin { servers, joins } => {
                let servers = servers.join(" ");
                let mut by_channel: BTreeMap<String, Vec<String>> = BTreeMap::new();
                for (channel, user) in joins {
                    by_channel
                        .entry(channel.to_ascii_lowercase())
                        .or_default()
                        .push(user.nick);
                }
                for (channel, nicks) in by_channel {
                    let Some(idx) = self.buffer_index(net, &channel) else {
                        continue;
                    };
                    for nick in &nicks {
                        let present = self.buffers[idx]
                            .members
                            .iter()
                            .any(|m| m.nick.eq_ignore_ascii_case(nick));
                        if !present {
                            self.buffers[idx].members.push(Member {
                                nick: nick.clone(),
                                prefixes: Vec::new(),
                                away: false,
                            });
                        }
                    }
                    self.buffers[idx].push(event_line(format!(
                        "netjoin {servers}: {} back ({})",
                        nicks.len(),
                        nick_summary(&nicks)
                    )));
                }
            }
            Event::Disconnected(reason) => {
                let text = match reason {
                    DisconnectReason::NickUnavailable => {
                        "all nicknames are in use; registration abandoned"
                    }
                    DisconnectReason::SaslAbortedByPolicy => {
                        "SASL authentication failed; connection aborted by policy"
                    }
                };
                let i = self.server_buffer(net);
                self.buffers[i].push(event_line(text.to_string()));
            }
            Event::AuthResult(result) => {
                let text = match &result {
                    Ok(account) => {
                        if let Some(meta) = self.networks.get_mut(net) {
                            meta.account = Some(account.clone());
                        }
                        format!("authenticated as {account}")
                    }
                    Err(err) => format!("authentication failed: {err}"),
                };
                let i = self.server_buffer(net);
                self.buffers[i].push(event_line(text));
            }
            // Show the whois where the user is looking (the active buffer on this
            // network), else the server buffer.
            Event::WhoisReceived(info) => {
                let idx = if self.buffers[self.active].net == net {
                    self.active
                } else {
                    self.server_buffer(net)
                };
                for line in whois_lines(&info) {
                    self.buffers[idx].push(event_line(line));
                }
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
        if mention && self.client.beep_on_highlight {
            self.bell = true;
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

    /// Scroll the active buffer by `pages` (positive = up/older). Returns a
    /// history-request command to send when scrolling reaches the top of a
    /// channel (to load older messages).
    pub fn scroll(&mut self, pages: isize) -> Option<NetCommand> {
        // A page is the pane height less two rows, so the last lines of one page
        // are still visible at the top of the next.
        let page = self.msg_rows.saturating_sub(2).max(1) as isize;
        self.scroll_by(page * pages)
    }

    /// Scroll the active buffer by `lines` (positive = up/older), asking for
    /// older history when that reaches the top of a channel.
    pub fn scroll_by(&mut self, lines: isize) -> Option<NetCommand> {
        self.dirty = true;
        let idx = self.active;
        let buffer = &mut self.buffers[idx];
        let max = buffer.lines.len() as isize;
        let requested = buffer.scroll as isize + lines;
        buffer.scroll = requested.clamp(0, max) as usize;
        // Reached (or pushed past) the top while scrolling up: pull older history.
        if lines > 0 && requested >= max {
            return self.request_older_history();
        }
        None
    }

    /// Build a paging request for the active channel's oldest known message,
    /// unless one is already in flight, the history is exhausted, or there is
    /// no msgid cursor to page from.
    fn request_older_history(&mut self) -> Option<NetCommand> {
        let idx = self.active;
        let buffer = &self.buffers[idx];
        if buffer.kind != BufferKind::Channel || buffer.history_pending || buffer.history_exhausted
        {
            return None;
        }
        let target = buffer.name.clone();
        let before = buffer.lines.iter().find_map(|l| match l {
            Line::Chat {
                msgid: Some(id), ..
            } => Some(id.clone()),
            _ => None,
        })?;
        self.buffers[idx].history_pending = true;
        Some(NetCommand::RequestHistory {
            target,
            before,
            limit: HISTORY_PAGE,
        })
    }

    /// Push a local feedback/status line into the active buffer.
    pub fn push_active_event(&mut self, text: String) {
        let idx = self.active;
        self.buffers[idx].push(event_line(text));
        self.dirty = true;
    }

    /// Push a failure/usage message into the active buffer, in the error style.
    pub fn push_error(&mut self, text: String) {
        let idx = self.active;
        self.buffers[idx].push(error_line(text));
        self.dirty = true;
    }

    /// The global console buffer index.
    fn console(&self) -> usize {
        self.buffers
            .iter()
            .position(|b| b.kind == BufferKind::Status)
            .unwrap_or(0)
    }

    /// Push a line into the console buffer (not necessarily the active one).
    pub fn push_console(&mut self, text: String) {
        let idx = self.console();
        self.buffers[idx].push(event_line(text));
        self.dirty = true;
    }

    /// Show an addon notification: a console line tagged with the network, plus
    /// the terminal bell.
    pub fn push_notice(&mut self, net: NetworkId, text: String) {
        let prefix = self
            .networks
            .get(net)
            .map(|n| format!("[{}] ", n.name))
            .unwrap_or_default();
        self.push_console(format!("{prefix}{text}"));
        self.bell = true;
        self.dirty = true;
    }

    /// Switch to the console buffer.
    pub fn switch_to_console(&mut self) {
        let idx = self.console();
        self.switch_to(idx);
    }

    /// Open the `/help` panel. If `query` names a command exactly, open straight
    /// to its detail; otherwise filter the list by `query`.
    pub fn open_help(&mut self, query: &str) {
        let query = query.trim();
        self.mode = Mode::Help;
        self.help = match crate::commands::find(query) {
            Some(doc) => {
                let sel = crate::commands::help_commands("")
                    .iter()
                    .position(|c| c.name == doc.name)
                    .unwrap_or(0);
                HelpState {
                    query: String::new(),
                    sel,
                    focus: HelpFocus::Detail,
                    detail_scroll: 0,
                }
            }
            None => HelpState {
                query: query.to_string(),
                sel: 0,
                focus: HelpFocus::List,
                detail_scroll: 0,
            },
        };
        self.dirty = true;
    }

    /// Open the `/settings` panel, reset to no filter and the first row.
    pub fn open_settings(&mut self) {
        self.mode = Mode::Settings;
        self.settings = SettingsState::default();
        self.dirty = true;
    }

    /// The rows shown on the `/settings` panel for the current filter: the
    /// registry settings (grouped by category) then an `aliases` section, each
    /// preceded by a header. Headers appear only when the group has a match.
    pub fn settings_rows(&self) -> Vec<SettingsRow> {
        let filter = self.settings.filter.trim().to_lowercase();
        let mut rows = Vec::new();
        let mut category = "";
        for doc in crate::settings::SETTINGS {
            if !crate::settings::matches(doc, &filter) {
                continue;
            }
            if doc.category != category {
                rows.push(SettingsRow::Header(doc.category));
                category = doc.category;
            }
            rows.push(SettingsRow::Setting(doc));
        }
        // Aliases are always their own section (with an "add" row) when the
        // filter is empty or targets "aliases"; a filter matching specific alias
        // names/expansions shows just those.
        let include = |name: &str, exp: &str| {
            name.to_lowercase().contains(&filter) || exp.to_lowercase().contains(&filter)
        };
        let section_targeted = filter.is_empty() || "aliases".contains(filter.as_str());
        let any_match = self.aliases.iter().any(|(n, e)| include(n, e));
        if section_targeted || any_match {
            rows.push(SettingsRow::Header("aliases"));
            for (name, expansion) in &self.aliases {
                if section_targeted || include(name, expansion) {
                    rows.push(SettingsRow::Alias {
                        name: name.clone(),
                        expansion: expansion.clone(),
                    });
                }
            }
            // The add row only shows when not narrowing to specific aliases.
            if filter.is_empty() {
                rows.push(SettingsRow::AddAlias);
            }
        }
        rows
    }

    /// The selectable (non-header) rows, in display order. The `sel` index refers
    /// into this list.
    pub fn settings_selectable(&self) -> Vec<SettingsRow> {
        self.settings_rows()
            .into_iter()
            .filter(|r| !matches!(r, SettingsRow::Header(_)))
            .collect()
    }

    /// The currently selected settings row, if any.
    pub fn selected_setting_row(&self) -> Option<SettingsRow> {
        self.settings_selectable()
            .into_iter()
            .nth(self.settings.sel)
    }

    /// Re-derive the live UI mirror (accent, timestamps, nicklist) from
    /// `self.client`, mark dirty, and persist. The single apply-and-save path
    /// shared by `/set` and the Settings screen, so the two never drift.
    pub fn apply_client_change(&mut self) {
        self.timestamps = self.client.timestamps;
        self.nicklist_visible = self.client.nicklist;
        self.accent = theme::accent_for(&self.client.theme);
        self.dirty = true;
        self.save_config();
    }

    /// The current value of a setting, rendered for display.
    pub fn setting_value(&self, key: &str) -> String {
        let on_off = |b: bool| if b { "on" } else { "off" }.to_string();
        match key {
            "timestamps" => on_off(self.client.timestamps),
            "nick_colors" => on_off(self.client.nick_colors),
            "theme" => self.client.theme.clone(),
            "nicklist" => on_off(self.client.nicklist),
            "completion_char" => self.client.completion_char.clone(),
            "beep_on_highlight" => on_off(self.client.beep_on_highlight),
            "mouse" => on_off(self.client.mouse),
            "scrollback_lines" => self.client.scrollback_lines.to_string(),
            "idle_secs" => self.client.idle_secs.to_string(),
            _ => String::new(),
        }
    }

    /// Validate and apply a setting from raw text (shared by `/set` and the
    /// `/settings` screen). Returns the applied value on success, or a message on
    /// invalid input. Applies live and auto-saves via [`Self::apply_client_change`].
    pub fn set_setting(&mut self, key: &str, raw: &str) -> Result<String, String> {
        use crate::settings::{self, SettingKind};
        let doc = settings::find(key).ok_or_else(|| format!("unknown setting '{key}'"))?;
        let raw = raw.trim();
        let want_bool = || parse_bool(raw).ok_or_else(|| format!("expected on/off, got '{raw}'"));
        match doc.key {
            "timestamps" => self.client.timestamps = want_bool()?,
            "nick_colors" => self.client.nick_colors = want_bool()?,
            "nicklist" => self.client.nicklist = want_bool()?,
            "beep_on_highlight" => self.client.beep_on_highlight = want_bool()?,
            "mouse" => {
                let want = want_bool()?;
                if want != self.client.mouse {
                    self.client.mouse = want;
                    // Capturing the mouse is a terminal mode, not UI state: the
                    // run loop flips it.
                    self.actions.push(AppAction::SetMouse(want));
                }
            }
            "theme" => {
                let v = raw.to_ascii_lowercase();
                if !theme::THEME_NAMES.contains(&v.as_str()) {
                    return Err(format!(
                        "expected one of: {}",
                        theme::THEME_NAMES.join(", ")
                    ));
                }
                self.client.theme = v;
            }
            "completion_char" => {
                if raw.is_empty() {
                    return Err("value cannot be empty".to_string());
                }
                self.client.completion_char = raw.to_string();
            }
            "scrollback_lines" => {
                let SettingKind::Int { min, max } = doc.kind else {
                    unreachable!("scrollback_lines is an int setting")
                };
                let n: usize = raw
                    .parse()
                    .map_err(|_| format!("expected a number, got '{raw}'"))?;
                if !(min..=max).contains(&n) {
                    return Err(format!("must be between {min} and {max}"));
                }
                self.set_scrollback(n);
            }
            "idle_secs" => {
                let SettingKind::Int { min, max } = doc.kind else {
                    unreachable!("idle_secs is an int setting")
                };
                let n: usize = raw
                    .parse()
                    .map_err(|_| format!("expected a number, got '{raw}'"))?;
                if !(min..=max).contains(&n) {
                    return Err(format!("must be between {min} and {max}"));
                }
                self.client.idle_secs = n;
            }
            other => return Err(format!("unknown setting '{other}'")),
        }
        self.apply_client_change();
        Ok(self.setting_value(doc.key))
    }

    /// Change the per-buffer scrollback cap, trimming every buffer to fit.
    fn set_scrollback(&mut self, n: usize) {
        self.client.scrollback_lines = n;
        for buffer in &mut self.buffers {
            buffer.set_max_lines(n);
        }
    }

    /// Persist current client prefs and network definitions to the config file.
    /// A missing path or write error is reported into the active buffer.
    pub fn save_config(&mut self) {
        let Some(path) = self.config_path.clone() else {
            self.push_error("no config file path; change not saved".to_string());
            return;
        };
        let config = Config {
            client: self.client.clone(),
            aliases: self.aliases.clone(),
            networks: self.definitions.clone(),
            triggers: self.triggers.clone(),
            disabled_plugins: self.disabled_plugins.clone(),
            plugin_config: self.plugin_config.clone(),
        };
        if let Err(err) = config.save(&path) {
            self.push_error(format!("save failed: {err}"));
        }
    }

    /// Open the `/plugins` manager, clamping the selection into range.
    pub fn open_plugins(&mut self) {
        self.mode = Mode::Plugins;
        self.plugins_ui.sel = self
            .plugins_ui
            .sel
            .min(self.plugins.len().saturating_sub(1));
        self.dirty = true;
    }

    /// Toggle the enabled state of the plugin at `idx` (saves + reloads).
    pub fn toggle_plugin(&mut self, idx: usize) {
        let Some(plugin) = self.plugins.get(idx) else {
            return;
        };
        let (file, enabled) = (plugin.file.clone(), self.plugin_enabled(&plugin.file));
        self.set_plugin_file_enabled(&file, !enabled);
    }

    /// Whether a plugin (by filename) is currently enabled.
    pub fn plugin_enabled(&self, file: &str) -> bool {
        !self
            .disabled_plugins
            .iter()
            .any(|f| f.eq_ignore_ascii_case(file))
    }

    /// Enable or disable a plugin by name (its display name or filename), saving
    /// and queuing a live reload. Returns the resolved filename, or an error if
    /// no such plugin.
    pub fn set_plugin_enabled(&mut self, name: &str, enabled: bool) -> Result<String, String> {
        let file = self
            .plugins
            .iter()
            .find(|p| {
                p.name.eq_ignore_ascii_case(name)
                    || p.file.eq_ignore_ascii_case(name)
                    || p.file.eq_ignore_ascii_case(&format!("{name}.rhai"))
            })
            .map(|p| p.file.clone())
            .ok_or_else(|| format!("no plugin '{name}'"))?;
        self.set_plugin_file_enabled(&file, enabled);
        Ok(file)
    }

    /// Enable/disable a plugin by exact filename, then save and reload.
    pub fn set_plugin_file_enabled(&mut self, file: &str, enabled: bool) {
        if enabled {
            self.disabled_plugins
                .retain(|f| !f.eq_ignore_ascii_case(file));
        } else if self.plugin_enabled(file) {
            self.disabled_plugins.push(file.to_string());
        }
        self.save_config();
        self.actions.push(AppAction::ReloadAddons);
    }

    /// The plugins folder (`<config-dir>/plugins`), if a config path is known.
    pub fn plugins_dir(&self) -> Option<PathBuf> {
        self.config_path
            .as_deref()
            .and_then(|p| p.parent())
            .map(|d| d.join("plugins"))
    }

    /// Install a bundled official plugin into the plugins folder (unless already
    /// present), then queue a reload. Returns the written filename.
    pub fn install_plugin(&mut self, name: &str) -> Result<String, String> {
        let plugin = crate::addons::official::find(name)
            .ok_or_else(|| format!("no official plugin '{name}'"))?;
        let dir = self.plugins_dir().ok_or("no config dir to install into")?;
        let file = format!("{}.rhai", plugin.name);
        let path = dir.join(&file);
        if path.exists() {
            return Err(format!("{file} already installed"));
        }
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(&path, plugin.source).map_err(|e| e.to_string())?;
        self.actions.push(AppAction::ReloadAddons);
        Ok(file)
    }

    /// Open the config editor for the plugin selected on the `/plugins` screen.
    /// Reports (without opening) if that plugin declares no config.
    pub fn open_plugin_config_selected(&mut self) {
        let idx = self.plugins_ui.sel;
        let Some(p) = self.plugins.get(idx) else {
            return;
        };
        let (file, name, empty) = (p.file.clone(), p.name.clone(), p.config.is_empty());
        if empty {
            self.push_error(format!("{name} has no configurable settings"));
            return;
        }
        self.plugin_cfg = PluginConfigState {
            file,
            name,
            sel: 0,
            editing: None,
            edit_cursor: 0,
            msg: None,
        };
        self.mode = Mode::PluginConfig;
    }

    /// Open the config editor for a plugin by name/file/stem (`/plugins config`).
    pub fn open_plugin_config(&mut self, name: &str) {
        let Some(idx) = self.plugins.iter().position(|p| {
            p.name.eq_ignore_ascii_case(name)
                || p.file.eq_ignore_ascii_case(name)
                || p.file.eq_ignore_ascii_case(&format!("{name}.rhai"))
        }) else {
            self.push_error(format!("no plugin '{name}'"));
            return;
        };
        self.plugins_ui.sel = idx;
        self.open_plugin_config_selected();
    }

    /// The config schema (key, default) of the plugin being edited.
    pub fn plugin_cfg_schema(&self) -> Vec<(String, String)> {
        self.plugins
            .iter()
            .find(|p| p.file == self.plugin_cfg.file)
            .map(|p| p.config.clone())
            .unwrap_or_default()
    }

    /// The current value of a config key: the user override, else the default.
    pub fn plugin_cfg_value(&self, key: &str, default: &str) -> String {
        self.plugin_config
            .get(&self.plugin_cfg.file)
            .and_then(|m| m.get(key))
            .cloned()
            .unwrap_or_else(|| default.to_string())
    }

    /// Set a config override for the edited plugin (dropping it when equal to the
    /// default so the config file stays clean), then save and reload.
    pub fn set_plugin_cfg_value(&mut self, key: &str, value: &str, default: &str) {
        let file = self.plugin_cfg.file.clone();
        if value == default {
            if let Some(m) = self.plugin_config.get_mut(&file) {
                m.remove(key);
                if m.is_empty() {
                    self.plugin_config.remove(&file);
                }
            }
        } else {
            self.plugin_config
                .entry(file)
                .or_default()
                .insert(key.to_string(), value.to_string());
        }
        self.save_config();
        self.actions.push(AppAction::ReloadAddons);
    }

    /// Connect a network by name: revive an existing idle one, or spawn a new
    /// task from its definition. Queues an [`AppAction`] for the supervisor.
    pub fn connect_network(&mut self, name: &str) {
        // Already a live network with this name?
        if let Some(id) = self
            .networks
            .iter()
            .position(|n| n.name.eq_ignore_ascii_case(name))
        {
            match self.networks[id].state {
                ConnState::Connecting | ConnState::Registered { .. } => {
                    self.push_active_event(format!("{name} is already connected"));
                }
                _ => {
                    self.networks[id].state = ConnState::Connecting;
                    self.actions.push(AppAction::Connect(id));
                }
            }
            return;
        }
        // Otherwise dial a defined network.
        let Some(config) = self
            .definitions
            .iter()
            .find(|n| n.name.eq_ignore_ascii_case(name))
            .cloned()
        else {
            self.push_error(format!("no network '{name}' (define it with /network add)"));
            return;
        };
        if let Err(err) = check_dialable(&config) {
            self.push_error(err);
            return;
        }
        let id = self.networks.len();
        self.networks.push(NetworkMeta {
            name: config.name.clone(),
            my_nick: config.nick.clone(),
            state: ConnState::Connecting,
            away: false,
            account: None,
        });
        let idx = self.ensure_buffer(id, "*", BufferKind::Server);
        self.actions.push(AppAction::AddNetwork { id, config });
        self.switch_to(idx);
    }

    /// Disconnect the active buffer's network (keeping the task idle so it can be
    /// reconnected). No-op on the console.
    pub fn disconnect_active(&mut self, reason: Option<String>) {
        let net = self.active_buffer().net;
        let Some(meta) = self.networks.get(net) else {
            self.push_error("no network here to disconnect".to_string());
            return;
        };
        if matches!(meta.state, ConnState::Disconnected | ConnState::Closed) {
            self.push_error(format!("{} is not connected", meta.name));
            return;
        }
        self.actions.push(AppAction::Disconnect(net, reason));
    }

    /// Reconnect the active buffer's network: drop it then redial. If it is not
    /// currently connected, just connect. No-op on the console.
    pub fn reconnect_active(&mut self) {
        let net = self.active_buffer().net;
        let Some(meta) = self.networks.get(net) else {
            self.push_error("no network here to reconnect".to_string());
            return;
        };
        if !matches!(meta.state, ConnState::Disconnected | ConnState::Closed) {
            self.actions
                .push(AppAction::Disconnect(net, Some("reconnecting".to_string())));
        }
        self.actions.push(AppAction::Connect(net));
    }

    /// Open the `/networks` manager, reset to the list pane.
    pub fn open_networks(&mut self) {
        self.mode = Mode::Networks;
        self.networks_ui = NetworksState::default();
        self.dirty = true;
    }

    /// Append a new network definition with safe defaults, select it, and focus
    /// the form (on the host field, since the name is prefilled). Auto-saved.
    pub fn add_network_definition(&mut self) {
        let nick = self
            .definitions
            .first()
            .map(|d| d.nick.clone())
            .or_else(|| self.networks.first().map(|n| n.my_nick.clone()))
            .unwrap_or_else(|| "norn".to_string());
        // A unique default name so a second "new-network" does not collide.
        let mut name = "new-network".to_string();
        let mut n = 2;
        while self
            .definitions
            .iter()
            .any(|d| d.name.eq_ignore_ascii_case(&name))
        {
            name = format!("new-network-{n}");
            n += 1;
        }
        self.definitions.push(NetworkConfig {
            name,
            host: String::new(),
            port: 6697,
            tls: true,
            nick,
            user: None,
            realname: None,
            sasl_account: None,
            sasl_mech: SaslMech::Plain,
            password_command: None,
            auto_join: Vec::new(),
            auto_connect: true,
            identify: false,
        });
        self.save_config();
        self.networks_ui.sel = self.definitions.len() - 1;
        self.networks_ui.focus = NetworksFocus::Form;
        self.networks_ui.field = 1;
        self.networks_ui.editing = None;
        self.networks_ui.msg =
            Some("new network — set host and nick, then press c to connect".into());
    }

    /// Remove the definition at `idx`, disconnecting a live network of that name
    /// first. Auto-saved.
    pub fn delete_network_definition(&mut self, idx: usize) {
        if idx >= self.definitions.len() {
            return;
        }
        let name = self.definitions[idx].name.clone();
        self.disconnect_network(&name);
        self.definitions.remove(idx);
        self.save_config();
        self.networks_ui.msg = Some(format!("removed network '{name}'"));
        // The list shrank; keep the selection in range (add row = len).
        if self.networks_ui.sel > self.definitions.len() {
            self.networks_ui.sel = self.definitions.len();
        }
    }

    /// Disconnect a live network by name (a no-op if it is not connected).
    pub fn disconnect_network(&mut self, name: &str) {
        if let Some(id) = self
            .networks
            .iter()
            .position(|n| n.name.eq_ignore_ascii_case(name))
        {
            if !matches!(
                self.networks[id].state,
                ConnState::Disconnected | ConnState::Closed
            ) {
                self.actions
                    .push(AppAction::Disconnect(id, Some("disconnected".into())));
            }
        }
    }

    /// Validate and apply a network form field from raw text, then auto-save.
    /// Returns a message on invalid input. Passwords are never a field here.
    pub fn set_network_field(&mut self, idx: usize, field: usize, raw: &str) -> Result<(), String> {
        let raw = raw.trim();
        let opt = |s: &str| (!s.is_empty()).then(|| s.to_string());
        let cfg = self.definitions.get_mut(idx).ok_or("no such network")?;
        match NETWORK_FIELDS.get(field).map(|f| f.name) {
            Some("name") => {
                if raw.is_empty() {
                    return Err("name cannot be empty".into());
                }
                cfg.name = raw.to_string();
            }
            Some("host") => {
                if raw.is_empty() {
                    return Err("host cannot be empty".into());
                }
                cfg.host = raw.to_string();
            }
            Some("port") => cfg.port = raw.parse().map_err(|_| format!("bad port '{raw}'"))?,
            Some("tls") => {
                cfg.tls = parse_bool(raw).ok_or_else(|| format!("expected on/off, got '{raw}'"))?
            }
            Some("auto_connect") => {
                cfg.auto_connect =
                    parse_bool(raw).ok_or_else(|| format!("expected on/off, got '{raw}'"))?
            }
            Some("identify") => {
                cfg.identify =
                    parse_bool(raw).ok_or_else(|| format!("expected on/off, got '{raw}'"))?
            }
            Some("nick") => {
                if raw.is_empty() {
                    return Err("nick cannot be empty".into());
                }
                cfg.nick = raw.to_string();
            }
            Some("user") => cfg.user = opt(raw),
            Some("realname") => cfg.realname = opt(raw),
            Some("sasl_account") => cfg.sasl_account = opt(raw),
            Some("sasl_mech") => {
                cfg.sasl_mech = match raw.to_ascii_lowercase().as_str() {
                    "plain" => SaslMech::Plain,
                    "scram" => SaslMech::Scram,
                    _ => return Err("expected plain|scram".into()),
                }
            }
            Some("password_command") => cfg.password_command = opt(raw),
            Some("auto_join") => {
                cfg.auto_join = raw
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect()
            }
            _ => return Err("unknown field".into()),
        }
        self.save_config();
        Ok(())
    }

    /// Toggle the `tls` field or cycle `sasl_mech` on the definition at `idx`,
    /// then auto-save. Returns a message on error (e.g. a non-toggle field).
    pub fn adjust_network_field(&mut self, idx: usize, field: usize) -> Result<(), String> {
        let cfg = self.definitions.get(idx).ok_or("no such network")?;
        match NETWORK_FIELDS.get(field).map(|f| (f.name, f.kind)) {
            Some((_, NetFieldKind::Toggle)) => {
                let on = network_field_value(cfg, field) == "on";
                self.set_network_field(idx, field, if on { "off" } else { "on" })
            }
            Some(("sasl_mech", _)) => {
                let next = match cfg.sasl_mech {
                    SaslMech::Plain => "scram",
                    SaslMech::Scram => "plain",
                };
                self.set_network_field(idx, field, next)
            }
            _ => Ok(()),
        }
    }

    /// The live connection state of a defined network, if a network of that name
    /// exists (for the list markers).
    pub fn network_live_state(&self, name: &str) -> Option<ConnState> {
        self.networks
            .iter()
            .find(|n| n.name.eq_ignore_ascii_case(name))
            .map(|n| n.state.clone())
    }

    /// Clear the active buffer's scrollback.
    pub fn clear_active(&mut self) {
        let idx = self.active;
        let buffer = &mut self.buffers[idx];
        buffer.lines.clear();
        buffer.scroll = 0;
        buffer.unread = 0;
        buffer.unread_marker = None;
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
        if matches!(
            self.active_buffer().kind,
            BufferKind::Server | BufferKind::Status
        ) {
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
            if self.history.len() > HISTORY_MAX {
                self.history.remove(0);
            }
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

/// The display value of a network form field (an empty string for an unset
/// optional). `tls` renders as `on`/`off`; callers show it as a checkbox.
pub fn network_field_value(cfg: &NetworkConfig, field: usize) -> String {
    match NETWORK_FIELDS.get(field).map(|f| f.name) {
        Some("name") => cfg.name.clone(),
        Some("host") => cfg.host.clone(),
        Some("port") => cfg.port.to_string(),
        Some("tls") => if cfg.tls { "on" } else { "off" }.to_string(),
        Some("auto_connect") => if cfg.auto_connect { "on" } else { "off" }.to_string(),
        Some("identify") => if cfg.identify { "on" } else { "off" }.to_string(),
        Some("nick") => cfg.nick.clone(),
        Some("user") => cfg.user.clone().unwrap_or_default(),
        Some("realname") => cfg.realname.clone().unwrap_or_default(),
        Some("sasl_account") => cfg.sasl_account.clone().unwrap_or_default(),
        Some("sasl_mech") => format!("{:?}", cfg.sasl_mech).to_lowercase(),
        Some("password_command") => cfg.password_command.clone().unwrap_or_default(),
        Some("auto_join") => cfg.auto_join.join(","),
        _ => String::new(),
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

/// Whether `text` mentions `nick` as a whole word.
pub(crate) fn mentions(text: &str, nick: &str) -> bool {
    if nick.is_empty() {
        return false;
    }
    let lower = text.to_lowercase();
    let nick = nick.to_lowercase();
    lower
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .any(|word| word == nick)
}

/// Fold a mode string into a channel's displayed flag set. Only flag modes are
/// tracked: membership prefixes (`o v h a q`) and list modes (`b e I`) are not
/// channel state, and arguments (key, limit) are dropped, leaving just `k`/`l`.
fn apply_flag_modes(current: &str, change: &str) -> String {
    let mut flags: Vec<char> = current.chars().filter(|c| *c != '+').collect();
    let mut adding = true;
    for c in change.chars() {
        match c {
            '+' => adding = true,
            '-' => adding = false,
            'o' | 'v' | 'h' | 'a' | 'q' | 'b' | 'e' | 'I' => {}
            c if adding => {
                if !flags.contains(&c) {
                    flags.push(c);
                }
            }
            c => flags.retain(|f| *f != c),
        }
    }
    flags.sort_unstable();
    if flags.is_empty() {
        String::new()
    } else {
        format!("+{}", flags.into_iter().collect::<String>())
    }
}

/// "alice, bob, carol, +11 more": a short list for a bulk event.
fn nick_summary(nicks: &[String]) -> String {
    const SHOWN: usize = 3;
    let mut out = nicks
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if nicks.len() > SHOWN {
        out.push_str(&format!(", +{} more", nicks.len() - SHOWN));
    }
    out
}

/// Why a network definition cannot be dialled yet, if it cannot.
pub fn check_dialable(config: &NetworkConfig) -> Result<(), String> {
    if config.host.trim().is_empty() {
        return Err(format!(
            "{0} has no host yet; set one in /networks before connecting",
            config.name
        ));
    }
    if config.nick.trim().is_empty() {
        return Err(format!(
            "{0} has no nick yet; set one in /networks before connecting",
            config.name
        ));
    }
    Ok(())
}

/// Whether `name` is a channel (vs a nick) by its leading character.
pub fn is_channel_name(name: &str) -> bool {
    name.starts_with(['#', '&', '+', '!'])
}

/// The console's opening lines. Point the user at the discoverable commands;
/// when nothing is configured yet, also show the quickest path to a connection.
fn welcome_lines(no_networks: bool, defined: &[NetworkConfig]) -> Vec<String> {
    let mut lines = vec![
        "welcome to norn".to_string(),
        "/help      browse every command (Tab also completes as you type)".to_string(),
        "/settings  configure the client (theme, timestamps, aliases, ...)".to_string(),
        "/networks  add, edit, connect, and disconnect networks".to_string(),
        format!("keys      {}", crate::keys::welcome_line()),
    ];
    if no_networks && defined.is_empty() {
        lines.push("no networks yet — open /networks and press Enter on the add row,".to_string());
        lines.push("or run /network add <name> host=<server> nick=<you> then /connect".to_string());
    } else if no_networks {
        lines.push(format!(
            "{} network(s) defined but not connected — click one in the sidebar, or /connect <name>",
            defined.len()
        ));
    }
    lines
}

/// Render a completed whois into buffer lines (a header plus indented details).
fn whois_lines(info: &WhoisInfo) -> Vec<String> {
    if info.not_found {
        return vec![format!("no such nick: {}", info.nick)];
    }
    let mut lines = Vec::new();
    match (&info.user, &info.host) {
        (Some(user), Some(host)) => lines.push(format!("{} is {user}@{host}", info.nick)),
        _ => lines.push(format!("whois {}", info.nick)),
    }
    if let Some(realname) = &info.realname {
        lines.push(format!("  realname: {realname}"));
    }
    if let Some(account) = &info.account {
        lines.push(format!("  account: {account}"));
    }
    if let Some(server) = &info.server {
        lines.push(format!("  server: {server}"));
    }
    if let Some(channels) = &info.channels {
        lines.push(format!("  channels: {channels}"));
    }
    if info.is_operator {
        lines.push("  is an IRC operator".to_string());
    }
    if info.secure {
        lines.push("  using a secure connection".to_string());
    }
    if let Some(away) = &info.away {
        lines.push(format!("  away: {away}"));
    }
    if let Some(idle) = info.idle_secs {
        let mut line = format!("  idle {}", fmt_idle(idle));
        if let Some(signon) = info.signon {
            line.push_str(&format!(
                ", signon {}",
                signon.with_timezone(&Local).format("%Y-%m-%d %H:%M")
            ));
        }
        lines.push(line);
    }
    lines
}

/// Format an idle duration in seconds compactly (e.g. `1h5m`, `30s`).
fn fmt_idle(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h{m}m")
    } else if m > 0 {
        format!("{m}m{s}s")
    } else {
        format!("{s}s")
    }
}

pub(crate) fn source_nick(source: &Source) -> &str {
    match source {
        Source::User { nick, .. } => nick,
        Source::Server(name) => name,
    }
}

fn topic_line(target: &str, change: &TopicChange, set_by: &Option<String>) -> Option<String> {
    match (change, set_by) {
        (TopicChange::Set(t), _) => Some(format!("topic for {target}: {t}")),
        (TopicChange::Unchanged, Some(by)) => Some(format!("topic set by {by}")),
        (TopicChange::Unchanged, None) => None,
        (TopicChange::Cleared, _) => Some(format!("{target} has no topic")),
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
                away: false,
                account: None,
            },
            NetworkMeta {
                name: "netb".into(),
                my_nick: "me2".into(),
                state: ConnState::Connecting,
                away: false,
                account: None,
            },
        ];
        App::new(
            nets,
            ClientConfig::default(),
            Vec::new(),
            Default::default(),
            None,
        )
    }

    fn engine(net: NetworkId, event: Event) -> UiEvent {
        UiEvent {
            net,
            kind: UiEventKind::Engine(event),
        }
    }

    #[test]
    fn console_plus_a_server_buffer_per_network() {
        let a = app();
        // Buffer 0 is the global console; then one server buffer per network.
        assert_eq!(a.buffers.len(), 3);
        assert_eq!(a.buffers[0].kind, BufferKind::Status);
        assert!(a.buffers[1..].iter().all(|b| b.kind == BufferKind::Server));
        // With networks present, we land on the first server buffer, not the console.
        assert_eq!(a.active, 1);
    }

    #[test]
    fn zero_networks_lands_on_console() {
        let a = App::new(
            Vec::new(),
            ClientConfig::default(),
            Vec::new(),
            Default::default(),
            None,
        );
        assert_eq!(a.buffers.len(), 1);
        assert_eq!(a.buffers[0].kind, BufferKind::Status);
        assert_eq!(a.active, 0);
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
    fn ctcp_action_becomes_an_action_line() {
        let mut a = app();
        a.apply(engine(
            0,
            Event::MessageReceived(chat("#rust", "alice", "\u{1}ACTION waves\u{1}")),
        ));
        let idx = a.buffer_index(0, "#rust").unwrap();
        match &a.buffers[idx].lines[0] {
            Line::Chat { action, text, .. } => {
                assert!(*action);
                assert_eq!(text, "waves");
            }
            other => panic!("expected an action chat line, got {other:?}"),
        }
    }

    #[test]
    fn message_without_server_time_gets_a_receipt_timestamp() {
        let mut a = app();
        // `chat` builds a message with time: None (like a NickServ notice).
        a.apply(engine(
            0,
            Event::MessageReceived(chat("me", "NickServ", "registered")),
        ));
        let idx = a.buffer_index(0, "NickServ").unwrap();
        match &a.buffers[idx].lines[0] {
            Line::Chat { time, .. } => assert!(time.is_some(), "gets a fallback time"),
            other => panic!("expected a chat line, got {other:?}"),
        }
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
    fn away_notify_marks_member_away_across_channels() {
        let mut a = app();
        a.apply(engine(
            0,
            Event::NamesLoaded {
                target: "#rust".into(),
                members: vec![member("@alice"), member("bob")],
            },
        ));
        let idx = a.buffer_index(0, "#rust").unwrap();
        // Going away sets the flag on the matching member only.
        a.apply(engine(
            0,
            Event::AwayChanged {
                nick: "alice".into(),
                message: Some("lunch".into()),
            },
        ));
        let alice = |a: &App| {
            a.buffers[idx]
                .members
                .iter()
                .find(|m| m.nick == "alice")
                .unwrap()
                .away
        };
        assert!(alice(&a));
        assert!(
            !a.buffers[idx]
                .members
                .iter()
                .find(|m| m.nick == "bob")
                .unwrap()
                .away
        );
        // Coming back clears it.
        a.apply(engine(
            0,
            Event::AwayChanged {
                nick: "alice".into(),
                message: None,
            },
        ));
        assert!(!alice(&a));
    }

    #[test]
    fn self_away_and_account_track_on_network() {
        let mut a = app();
        assert!(!a.networks[0].away);
        assert!(a.networks[0].account.is_none());
        a.apply(engine(0, Event::AwayStatus(true)));
        assert!(a.networks[0].away);
        a.apply(engine(0, Event::AwayStatus(false)));
        assert!(!a.networks[0].away);
        // SASL success records our account.
        a.apply(engine(0, Event::AuthResult(Ok("myacct".into()))));
        assert_eq!(a.networks[0].account.as_deref(), Some("myacct"));
        // account-notify for our own nick updates it; a logout clears it.
        a.apply(engine(
            0,
            Event::AccountChanged {
                nick: "me".into(),
                account: Some("other".into()),
            },
        ));
        assert_eq!(a.networks[0].account.as_deref(), Some("other"));
        a.apply(engine(
            0,
            Event::AccountChanged {
                nick: "me".into(),
                account: None,
            },
        ));
        assert!(a.networks[0].account.is_none());
        // A different nick's account-notify does not touch ours.
        a.apply(engine(0, Event::AuthResult(Ok("mine".into()))));
        a.apply(engine(
            0,
            Event::AccountChanged {
                nick: "bob".into(),
                account: Some("bobacct".into()),
            },
        ));
        assert_eq!(a.networks[0].account.as_deref(), Some("mine"));
    }

    #[test]
    fn topic_sets_and_metadata_does_not_clear() {
        let mut a = app();
        a.apply(engine(
            0,
            Event::TopicChanged {
                target: "#rust".into(),
                change: TopicChange::Set("hello".into()),
                set_by: None,
                set_at: None,
            },
        ));
        let idx = a.buffer_index(0, "#rust").unwrap();
        assert_eq!(a.buffers[idx].topic.as_deref(), Some("hello"));
        // 333-style metadata: Unchanged -> keep the topic text.
        a.apply(engine(
            0,
            Event::TopicChanged {
                target: "#rust".into(),
                change: TopicChange::Unchanged,
                set_by: Some("op".into()),
                set_at: None,
            },
        ));
        assert_eq!(a.buffers[idx].topic.as_deref(), Some("hello"));
        // A genuine live clear (Cleared) removes the stored topic, even though
        // it too has a setter -- the case the old Option model could not tell
        // apart from 333 metadata.
        a.apply(engine(
            0,
            Event::TopicChanged {
                target: "#rust".into(),
                change: TopicChange::Cleared,
                set_by: Some("op".into()),
                set_at: None,
            },
        ));
        assert_eq!(a.buffers[idx].topic, None);
    }

    #[test]
    fn scroll_to_top_requests_older_history_once() {
        let mut a = app();
        let mut msg = chat("#rust", "alice", "hello");
        msg.msgid = Some("m1".into());
        a.apply(engine(0, Event::MessageReceived(msg)));
        let idx = a.buffer_index(0, "#rust").unwrap();
        a.switch_to(idx);

        // Scrolling up to the top requests older history as a structured command.
        let cmd = a.scroll(1);
        assert_eq!(
            cmd,
            Some(NetCommand::RequestHistory {
                target: "#rust".into(),
                before: "m1".into(),
                limit: HISTORY_PAGE,
            })
        );
        assert!(a.buffers[idx].history_pending);
        // A second scroll does not re-request while one is in flight.
        assert!(a.scroll(1).is_none());
        // A short (complete) page clears pending and marks history exhausted.
        a.apply(engine(
            0,
            Event::HistoryLoaded {
                target: "#rust".into(),
                messages: vec![],
                complete: true,
            },
        ));
        assert!(!a.buffers[idx].history_pending);
        assert!(a.buffers[idx].history_exhausted);
        // Once exhausted, scrolling to the top requests nothing more.
        assert!(a.scroll(1).is_none());
    }

    #[test]
    fn set_setting_scrollback_trims_and_validates() {
        let mut a = app();
        let i = a.server_buffer(0);
        for n in 0..120 {
            a.buffers[i].push(event_line(format!("line {n}")));
        }
        // The default cap (5000) keeps all 120.
        assert_eq!(a.buffers[i].lines.len(), 120);
        // Lowering the cap trims existing buffers immediately.
        a.set_setting("scrollback_lines", "100").unwrap();
        assert_eq!(a.buffers[i].lines.len(), 100);
        assert_eq!(a.client.scrollback_lines, 100);
        // And new pushes stay capped.
        for n in 0..10 {
            a.buffers[i].push(event_line(format!("more {n}")));
        }
        assert_eq!(a.buffers[i].lines.len(), 100);
        // Validation: out of range, unknown key, and a bad enum are all errors.
        assert!(a.set_setting("scrollback_lines", "0").is_err());
        assert!(a.set_setting("nope", "x").is_err());
        assert!(a.set_setting("theme", "chartreuse").is_err());
        a.set_setting("theme", "amber").unwrap();
        assert_eq!(a.client.theme, "amber");
    }

    #[test]
    fn beep_flag_set_only_when_enabled() {
        let mut a = app();
        a.client.beep_on_highlight = true;
        // A mention in a non-active buffer raises the bell flag.
        a.apply(engine(
            0,
            Event::MessageReceived(chat("#rust", "x", "hey me!")),
        ));
        assert!(a.bell);
        a.bell = false;
        a.client.beep_on_highlight = false;
        a.apply(engine(
            0,
            Event::MessageReceived(chat("#rust", "x", "me again")),
        ));
        assert!(!a.bell, "no bell when the setting is off");
    }

    #[test]
    fn network_form_has_no_password_field() {
        // The hard rule: the form never exposes a plaintext password, only
        // password_command.
        assert!(NETWORK_FIELDS
            .iter()
            .all(|f| f.name != "password" && f.name != "pass"));
        assert!(NETWORK_FIELDS.iter().any(|f| f.name == "password_command"));
    }

    #[test]
    fn network_field_edit_toggle_and_cycle() {
        let mut a = app();
        a.definitions.push(NetworkConfig {
            name: "libera".into(),
            host: "h".into(),
            port: 6697,
            tls: true,
            nick: "n".into(),
            user: None,
            realname: None,
            sasl_account: None,
            sasl_mech: SaslMech::Plain,
            password_command: None,
            auto_join: vec![],
            auto_connect: true,
            identify: false,
        });
        let idx = |name: &str| NETWORK_FIELDS.iter().position(|f| f.name == name).unwrap();
        // port: rejects non-numeric, accepts a number.
        assert!(a.set_network_field(0, idx("port"), "nope").is_err());
        a.set_network_field(0, idx("port"), "6667").unwrap();
        assert_eq!(a.definitions[0].port, 6667);
        // tls toggles; sasl_mech cycles.
        a.adjust_network_field(0, idx("tls")).unwrap();
        assert!(!a.definitions[0].tls);
        a.adjust_network_field(0, idx("sasl_mech")).unwrap();
        assert_eq!(a.definitions[0].sasl_mech, SaslMech::Scram);
        // Optional fields clear to None on empty; auto_join splits on commas.
        a.set_network_field(0, idx("user"), "bob").unwrap();
        assert_eq!(a.definitions[0].user.as_deref(), Some("bob"));
        a.set_network_field(0, idx("user"), "").unwrap();
        assert_eq!(a.definitions[0].user, None);
        a.set_network_field(0, idx("auto_join"), "#a, #b").unwrap();
        assert_eq!(a.definitions[0].auto_join, vec!["#a", "#b"]);
        // Required fields refuse to blank.
        assert!(a.set_network_field(0, idx("host"), "").is_err());
    }

    #[test]
    fn delete_network_definition_removes_and_clamps() {
        let mut a = app();
        for name in ["a", "b"] {
            a.definitions.push(NetworkConfig {
                name: name.into(),
                host: "h".into(),
                port: 6697,
                tls: true,
                nick: "n".into(),
                user: None,
                realname: None,
                sasl_account: None,
                sasl_mech: SaslMech::Plain,
                password_command: None,
                auto_join: vec![],
                auto_connect: true,
                identify: false,
            });
        }
        a.networks_ui.sel = 2; // the add row
        a.delete_network_definition(1);
        assert_eq!(a.definitions.len(), 1);
        assert_eq!(a.definitions[0].name, "a");
        // The add row is now index 1; the selection clamps to it.
        assert_eq!(a.networks_ui.sel, 1);
    }

    #[test]
    fn whois_renders_into_the_active_buffer() {
        let mut a = app(); // active is net 0's server buffer
        a.apply(engine(
            0,
            Event::WhoisReceived(WhoisInfo {
                nick: "alice".into(),
                user: Some("~u".into()),
                host: Some("host.example".into()),
                realname: Some("Alice A".into()),
                server: Some("irc.example.net".into()),
                account: Some("acct".into()),
                channels: Some("#rust".into()),
                idle_secs: Some(65),
                signon: None,
                is_operator: false,
                secure: true,
                away: None,
                not_found: false,
            }),
        ));
        let text: String = a
            .active_buffer()
            .lines
            .iter()
            .filter_map(|l| match l {
                Line::Event { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("alice is ~u@host.example"));
        assert!(text.contains("account: acct"));
        assert!(text.contains("using a secure connection"));
        assert!(text.contains("idle 1m5s"));
    }

    #[test]
    fn whois_not_found_renders_no_such_nick() {
        let mut a = app();
        a.apply(engine(
            0,
            Event::WhoisReceived(WhoisInfo {
                nick: "ghost".into(),
                not_found: true,
                ..Default::default()
            }),
        ));
        let text: String = a
            .active_buffer()
            .lines
            .iter()
            .filter_map(|l| match l {
                Line::Event { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("no such nick: ghost"));
    }

    fn event_texts(buffer: &Buffer) -> Vec<String> {
        buffer
            .lines
            .iter()
            .filter_map(|l| match l {
                Line::Event { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    fn join_channel(a: &mut App, chan: &str, nicks: &[&str]) -> usize {
        a.apply(engine(
            0,
            Event::NamesLoaded {
                target: chan.into(),
                members: nicks.iter().map(|n| member(n)).collect(),
            },
        ));
        a.buffer_index(0, chan).unwrap()
    }

    #[test]
    fn quit_is_shown_in_shared_channels_and_query_only() {
        let mut a = app();
        let rust = join_channel(&mut a, "#rust", &["alice", "bob"]);
        let other = join_channel(&mut a, "#other", &["carol"]);
        a.apply(engine(0, Event::MessageReceived(chat("me", "alice", "hi"))));
        let query = a.buffer_index(0, "alice").unwrap();
        a.apply(engine(
            0,
            Event::MemberLeft {
                target: String::new(),
                who: irc_engine::User::nick("alice"),
                reason: LeaveReason::Quit("bye".into()),
            },
        ));
        assert_eq!(event_texts(&a.buffers[rust]), vec!["alice quit (bye)"]);
        assert_eq!(event_texts(&a.buffers[query]), vec!["alice quit (bye)"]);
        assert!(event_texts(&a.buffers[other]).is_empty());
        assert_eq!(a.buffers[rust].members.len(), 1);
    }

    #[test]
    fn own_nick_change_updates_my_nick_and_is_announced() {
        let mut a = app();
        let rust = join_channel(&mut a, "#rust", &["me", "bob"]);
        a.apply(engine(
            0,
            Event::NickChanged {
                old: "me".into(),
                new: "me_".into(),
            },
        ));
        assert_eq!(a.networks[0].my_nick, "me_");
        assert_eq!(
            event_texts(&a.buffers[rust]),
            vec!["you are now known as me_"]
        );
        // A DM to the new nick now opens a query, not a channel buffer.
        a.apply(engine(0, Event::MessageReceived(chat("me_", "bob", "yo"))));
        let q = a.buffer_index(0, "bob").unwrap();
        assert_eq!(a.buffers[q].kind, BufferKind::Query);
    }

    #[test]
    fn other_nick_change_renames_member_and_query() {
        let mut a = app();
        let rust = join_channel(&mut a, "#rust", &["alice"]);
        a.apply(engine(0, Event::MessageReceived(chat("me", "alice", "hi"))));
        a.apply(engine(
            0,
            Event::NickChanged {
                old: "alice".into(),
                new: "alicia".into(),
            },
        ));
        assert_eq!(a.buffers[rust].members[0].nick, "alicia");
        assert_eq!(
            event_texts(&a.buffers[rust]),
            vec!["alice is now known as alicia"]
        );
        assert!(a.buffer_index(0, "alicia").is_some());
        assert!(a.buffer_index(0, "alice").is_none());
    }

    #[test]
    fn away_status_and_disconnect_reasons_reach_the_server_buffer() {
        let mut a = app();
        a.apply(engine(0, Event::AwayStatus(true)));
        a.apply(engine(0, Event::AwayStatus(false)));
        a.apply(engine(
            0,
            Event::Disconnected(DisconnectReason::NickUnavailable),
        ));
        let server = a.buffer_index(0, "*").unwrap();
        let texts = event_texts(&a.buffers[server]);
        assert!(texts.contains(&"you are now marked away".to_string()));
        assert!(texts.contains(&"you are no longer away".to_string()));
        assert!(texts.iter().any(|t| t.contains("nicknames are in use")));
    }

    #[test]
    fn standard_reply_uses_the_wire_keyword() {
        let mut a = app();
        a.apply(engine(
            0,
            Event::StandardReply(irc_engine::StandardReply {
                kind: irc_engine::ReplyKind::Fail,
                command: "CHATHISTORY".into(),
                code: "INVALID_PARAMS".into(),
                context: Vec::new(),
                description: "bad".into(),
            }),
        ));
        let server = a.buffer_index(0, "*").unwrap();
        assert_eq!(
            event_texts(&a.buffers[server]),
            vec!["FAIL CHATHISTORY INVALID_PARAMS: bad"]
        );
    }

    #[test]
    fn history_prepend_keeps_the_unread_marker_on_the_same_line() {
        let mut a = app();
        let idx = join_channel(&mut a, "#rust", &["alice"]);
        a.apply(engine(
            0,
            Event::MessageReceived(chat("#rust", "alice", "one")),
        ));
        a.buffers[idx].unread_marker = Some(1);
        a.apply(engine(
            0,
            Event::HistoryLoaded {
                target: "#rust".into(),
                messages: vec![
                    chat("#rust", "alice", "old1"),
                    chat("#rust", "alice", "old2"),
                ],
                complete: false,
            },
        ));
        assert_eq!(a.buffers[idx].unread_marker, Some(3));
    }

    fn last_line(buffer: &Buffer) -> &Line {
        buffer.lines.last().expect("a line")
    }

    #[test]
    fn command_error_goes_to_the_named_buffer_as_an_error() {
        let mut a = app();
        let rust = join_channel(&mut a, "#rust", &["me"]);
        a.apply(engine(
            0,
            Event::CommandError {
                error: irc_engine::ServerError::CannotSend,
                target: Some("#rust".into()),
                message: "Cannot send to channel".into(),
            },
        ));
        match last_line(&a.buffers[rust]) {
            Line::Event { text, error, .. } => {
                assert!(*error);
                assert_eq!(text, "#rust: Cannot send to channel");
            }
            other => panic!("expected an error line, got {other:?}"),
        }
    }

    #[test]
    fn command_error_without_a_buffer_lands_where_the_user_is() {
        let mut a = app(); // active: net 0's server buffer
        a.apply(engine(
            0,
            Event::CommandError {
                error: irc_engine::ServerError::NickInUse,
                target: Some("bob".into()),
                message: "Nickname is already in use".into(),
            },
        ));
        let text = event_texts(a.active_buffer());
        assert_eq!(
            text,
            vec!["nick bob is already in use; pick another with /nick <name>"]
        );
        // An error on another network does not leak into the active buffer.
        let before = a.active_buffer().lines.len();
        a.apply(engine(
            1,
            Event::CommandError {
                error: irc_engine::ServerError::NoPrivileges,
                target: None,
                message: "Permission Denied".into(),
            },
        ));
        assert_eq!(a.active_buffer().lines.len(), before);
        let other = a.buffer_index(1, "*").unwrap();
        assert_eq!(event_texts(&a.buffers[other]), vec!["Permission Denied"]);
    }

    #[test]
    fn motd_and_server_info_go_to_the_server_buffer() {
        let mut a = app();
        a.apply(engine(
            0,
            Event::Motd(vec!["Welcome".into(), "Be nice".into()]),
        ));
        a.apply(engine(0, Event::ServerInfo("There are 5 users".into())));
        let server = a.buffer_index(0, "*").unwrap();
        assert_eq!(
            event_texts(&a.buffers[server]),
            vec![
                "message of the day:",
                "  Welcome",
                "  Be nice",
                "There are 5 users"
            ]
        );
    }

    #[test]
    fn mode_change_moves_prefixes_in_the_nicklist() {
        let mut a = app();
        let rust = join_channel(&mut a, "#rust", &["alice", "bob"]);
        a.apply(engine(
            0,
            Event::ModeChanged {
                target: "#rust".into(),
                by: Some("alice".into()),
                modes: "+o".into(),
                args: vec!["bob".into()],
                prefix_changes: vec![irc_engine::PrefixChange {
                    nick: "bob".into(),
                    prefix: irc_engine::MemberPrefix::Op,
                    granted: true,
                }],
            },
        ));
        let bob = |a: &App| {
            a.buffers[rust]
                .members
                .iter()
                .find(|m| m.nick == "bob")
                .unwrap()
                .prefixes
                .clone()
        };
        assert_eq!(bob(&a), vec![irc_engine::MemberPrefix::Op]);
        assert_eq!(
            event_texts(&a.buffers[rust]),
            vec!["alice sets mode +o bob on #rust"]
        );
        a.apply(engine(
            0,
            Event::ModeChanged {
                target: "#rust".into(),
                by: None,
                modes: "-o".into(),
                args: vec!["bob".into()],
                prefix_changes: vec![irc_engine::PrefixChange {
                    nick: "bob".into(),
                    prefix: irc_engine::MemberPrefix::Op,
                    granted: false,
                }],
            },
        ));
        assert!(bob(&a).is_empty());
    }

    #[test]
    fn user_modes_are_shown_in_the_server_buffer() {
        let mut a = app();
        a.apply(engine(
            0,
            Event::ModeChanged {
                target: "me".into(),
                by: None,
                modes: "+i".into(),
                args: Vec::new(),
                prefix_changes: Vec::new(),
            },
        ));
        let server = a.buffer_index(0, "*").unwrap();
        assert_eq!(event_texts(&a.buffers[server]), vec!["mode +i on me"]);
    }

    #[test]
    fn netsplit_is_one_summary_line_per_channel_and_removes_members() {
        let mut a = app();
        let rust = join_channel(&mut a, "#rust", &["a", "b", "c", "d", "e"]);
        let other = join_channel(&mut a, "#other", &["zed"]);
        let users = ["a", "b", "c", "d"].map(irc_engine::User::nick).to_vec();
        a.apply(engine(
            0,
            Event::Netsplit {
                servers: vec!["irc.a".into(), "irc.b".into()],
                users,
            },
        ));
        assert_eq!(
            event_texts(&a.buffers[rust]),
            vec!["netsplit irc.a irc.b: 4 left (a, b, c, +1 more)"]
        );
        assert_eq!(a.buffers[rust].members.len(), 1);
        assert!(
            event_texts(&a.buffers[other]).is_empty(),
            "unaffected channel"
        );
    }

    #[test]
    fn netjoin_restores_members_with_one_line_per_channel() {
        let mut a = app();
        let rust = join_channel(&mut a, "#rust", &["me"]);
        a.apply(engine(
            0,
            Event::Netjoin {
                servers: vec!["irc.a".into()],
                joins: vec![
                    ("#rust".into(), irc_engine::User::nick("a")),
                    ("#rust".into(), irc_engine::User::nick("b")),
                ],
            },
        ));
        assert_eq!(a.buffers[rust].members.len(), 3);
        assert_eq!(
            event_texts(&a.buffers[rust]),
            vec!["netjoin irc.a: 2 back (a, b)"]
        );
    }

    #[test]
    fn every_registered_setting_can_be_read_and_written_back() {
        // Catches a setting added to the registry but not wired into
        // `setting_value` / `set_setting` (it would read as empty or be refused).
        let mut a = app();
        for doc in crate::settings::SETTINGS {
            let value = a.setting_value(doc.key);
            assert!(!value.is_empty(), "{} reads as empty", doc.key);
            assert_eq!(
                a.set_setting(doc.key, &value).as_deref(),
                Ok(value.as_str()),
                "{} does not accept its own value",
                doc.key
            );
        }
    }

    #[test]
    fn input_history_is_capped_and_drops_the_oldest() {
        let mut a = app();
        for i in 0..(HISTORY_MAX + 25) {
            a.remember_input(&format!("line {i}"));
        }
        assert_eq!(a.history.len(), HISTORY_MAX);
        assert_eq!(a.history.first().map(String::as_str), Some("line 25"));
        assert_eq!(
            a.history.last().map(String::as_str),
            Some(format!("line {}", HISTORY_MAX + 24).as_str())
        );
        // Recall still walks back from the newest.
        a.history_prev();
        assert_eq!(a.input, format!("line {}", HISTORY_MAX + 24));
    }

    #[test]
    fn page_keys_scroll_by_the_pane_height_and_the_wheel_by_a_few_lines() {
        let mut a = app();
        let idx = join_channel(&mut a, "#rust", &["me"]);
        a.switch_to(idx);
        for i in 0..200 {
            a.buffers[idx].push(event_line(format!("line {i}")));
        }
        a.msg_rows = 20;
        a.scroll(1);
        assert_eq!(
            a.buffers[idx].scroll, 18,
            "a page is the pane less two rows"
        );
        a.scroll(1);
        assert_eq!(a.buffers[idx].scroll, 36);
        a.scroll(-1);
        assert_eq!(a.buffers[idx].scroll, 18);
        a.scroll_by(WHEEL_LINES);
        assert_eq!(a.buffers[idx].scroll, 21);
        a.scroll_by(-1000);
        assert_eq!(a.buffers[idx].scroll, 0, "cannot scroll below the live end");
        // A tiny pane still moves.
        a.msg_rows = 1;
        a.scroll(1);
        assert_eq!(a.buffers[idx].scroll, 1);
    }

    #[test]
    fn flag_modes_accumulate_and_ignore_prefixes_and_lists() {
        assert_eq!(apply_flag_modes("", "+nt"), "+nt");
        assert_eq!(apply_flag_modes("+nt", "+k-t"), "+kn");
        // Op/voice and ban lists are not channel state.
        assert_eq!(apply_flag_modes("+n", "+ov-b"), "+n");
        assert_eq!(apply_flag_modes("+n", "-n"), "");
    }

    #[test]
    fn lag_is_recorded_and_forgotten_when_the_connection_drops() {
        let mut a = app();
        a.apply(UiEvent {
            net: 0,
            kind: UiEventKind::Lag(std::time::Duration::from_millis(120)),
        });
        assert_eq!(a.lag.get(&0), Some(&std::time::Duration::from_millis(120)));
        a.apply(UiEvent {
            net: 0,
            kind: UiEventKind::ConnState(ConnState::Reconnecting {
                delay: std::time::Duration::from_secs(1),
            }),
        });
        assert!(!a.lag.contains_key(&0));
    }

    #[test]
    fn parting_marks_the_channel_not_joined_until_we_rejoin() {
        let mut a = app();
        let rust = join_channel(&mut a, "#rust", &["me", "bob"]);
        assert!(a.buffers[rust].joined);
        // Someone else leaving does not change our own membership.
        a.apply(engine(
            0,
            Event::MemberLeft {
                target: "#rust".into(),
                who: irc_engine::User::nick("bob"),
                reason: LeaveReason::Part(String::new()),
            },
        ));
        assert!(a.buffers[rust].joined);
        a.apply(engine(
            0,
            Event::MemberLeft {
                target: "#rust".into(),
                who: irc_engine::User::nick("me"),
                reason: LeaveReason::Kicked {
                    by: "op".into(),
                    reason: "bye".into(),
                },
            },
        ));
        assert!(!a.buffers[rust].joined);
        a.apply(engine(
            0,
            Event::MemberJoined {
                target: "#rust".into(),
                who: irc_engine::User::nick("me"),
                account: None,
            },
        ));
        assert!(a.buffers[rust].joined);
    }

    #[test]
    fn the_console_cannot_be_closed() {
        let mut a = app();
        a.switch_to(0);
        assert_eq!(a.active_buffer().kind, BufferKind::Status);
        let before = a.buffers.len();
        assert!(a.close_active().is_none());
        assert_eq!(a.buffers.len(), before);
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
