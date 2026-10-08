//! Semantic events: the engine's only output above the pipeline.
//!
//! The UI consumes these and never sees a raw line or a numeric. This module
//! currently carries the variants produced during bring-up; more arrive in
//! later build-order steps (message receipt, history, membership).

use chrono::{DateTime, Utc};
use irc_proto::{CapSet, SaslError, Source};

use crate::chat::ChatMessage;
use crate::roster::{Member, MemberPrefix};
use crate::stdreply::StandardReply;

/// An authenticated account name.
pub type AccountName = String;

/// A user identity from a message prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    /// The nickname.
    pub nick: String,
    /// The user/ident, if known.
    pub user: Option<String>,
    /// The host, if known.
    pub host: Option<String>,
}

impl User {
    /// A user known only by nick (e.g. the target of a KICK).
    pub fn nick(nick: impl Into<String>) -> Self {
        User {
            nick: nick.into(),
            user: None,
            host: None,
        }
    }

    /// Build from a message source; `None` for a server prefix.
    pub fn from_source(source: &Source) -> Option<User> {
        match source {
            Source::User { nick, user, host } => Some(User {
                nick: nick.clone(),
                user: user.clone(),
                host: host.clone(),
            }),
            Source::Server(_) => None,
        }
    }
}

/// What a [`Event::TopicChanged`] asserts about the topic text, kept distinct
/// so a genuine clear is never confused with a metadata-only update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopicChange {
    /// The topic text is now this (RPL_TOPIC 332, or a live `TOPIC` with text).
    Set(String),
    /// The topic was cleared or there is none (RPL_NOTOPIC 331, or a live
    /// `TOPIC` with an empty trailing param).
    Cleared,
    /// This event carries only who/when metadata (RPL_TOPICWHOTIME 333); the
    /// topic text is unchanged and must not be overwritten.
    Unchanged,
}

/// The accumulated result of a `WHOIS`, assembled from its reply numerics and
/// emitted as one unit at end-of-whois (318). Fields are `None`/empty when the
/// server did not supply them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WhoisInfo {
    /// The nick queried.
    pub nick: String,
    /// User/ident (311).
    pub user: Option<String>,
    /// Host (311).
    pub host: Option<String>,
    /// Realname / GECOS (311).
    pub realname: Option<String>,
    /// The server they are on (312).
    pub server: Option<String>,
    /// Their authenticated account (330).
    pub account: Option<String>,
    /// The channels they are on, as the server's prefixed list (319).
    pub channels: Option<String>,
    /// Idle time in seconds (317).
    pub idle_secs: Option<u64>,
    /// Signon time (317).
    pub signon: Option<DateTime<Utc>>,
    /// Whether they are an IRC operator (313).
    pub is_operator: bool,
    /// Whether they are on a secure (TLS) connection (671).
    pub secure: bool,
    /// Their away message, if away (301, only when part of a whois).
    pub away: Option<String>,
    /// The nick does not exist (ERR_NOSUCHNICK 401); the other fields are unset.
    pub not_found: bool,
}

/// Why a member left a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaveReason {
    /// `PART`, with its reason.
    Part(String),
    /// `QUIT`, with its reason (applies to every shared channel).
    Quit(String),
    /// `KICK`, by whom and why.
    Kicked {
        /// The nick that issued the kick.
        by: String,
        /// The kick reason.
        reason: String,
    },
}

/// A semantic event emitted by the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Registration completed (`001` received after `CAP END`).
    Registered {
        /// The nick the server confirmed.
        nick: String,
    },
    /// The capability menu changed: negotiated at bring-up, or via cap-notify
    /// (`CAP NEW`/`CAP DEL`) afterwards.
    CapabilitiesChanged {
        /// Everything the server advertises.
        available: CapSet,
        /// What we have successfully enabled.
        enabled: CapSet,
    },
    /// The result of a SASL attempt: the account on success, or the failure.
    AuthResult(Result<AccountName, SaslError>),
    /// A chat message (PRIVMSG/NOTICE) was received, with original metadata.
    MessageReceived(ChatMessage),
    /// A CHATHISTORY response resolved into an ordered page of messages, each
    /// carrying its original `time`/`msgid`. `complete` is false when the
    /// server returned exactly the requested limit (there may be more).
    HistoryLoaded {
        /// The conversation the history is for.
        target: String,
        /// The messages, in receive order.
        messages: Vec<ChatMessage>,
        /// Whether this is all there is (fewer than the limit returned).
        complete: bool,
    },
    /// A netsplit: these users left together because servers lost contact. The
    /// UI shows one summary instead of a quit per user.
    Netsplit {
        /// The servers involved (the batch parameters).
        servers: Vec<String>,
        /// Everyone who dropped off.
        users: Vec<User>,
    },
    /// A netjoin: these users came back after a netsplit healed.
    Netjoin {
        /// The servers involved (the batch parameters).
        servers: Vec<String>,
        /// Who rejoined which channel.
        joins: Vec<(String, User)>,
    },
    /// The full member list for a channel (emitted at end-of-NAMES, 366).
    NamesLoaded {
        /// The channel.
        target: String,
        /// Its members, with prefixes.
        members: Vec<Member>,
    },
    /// A channel topic, or its metadata, changed.
    ///
    /// `change` distinguishes a topic set, a clear, and a metadata-only update
    /// (RPL_TOPICWHOTIME 333) so a live clear is never mistaken for stale
    /// metadata. `set_by`/`set_at` carry who/when when the server supplies them.
    TopicChanged {
        /// The channel.
        target: String,
        /// What happened to the topic text.
        change: TopicChange,
        /// Who set the topic, if known.
        set_by: Option<String>,
        /// When the topic was set, if known.
        set_at: Option<DateTime<Utc>>,
    },
    /// A member joined a channel. `account` is present with `extended-join`.
    MemberJoined {
        /// The channel joined.
        target: String,
        /// Who joined.
        who: User,
        /// Their account, if `extended-join` supplied one.
        account: Option<String>,
    },
    /// A member left a channel (PART/QUIT/KICK).
    MemberLeft {
        /// The channel left (empty for QUIT, which is channel-agnostic).
        target: String,
        /// Who left.
        who: User,
        /// How they left.
        reason: LeaveReason,
    },
    /// A user changed nick (`NICK`).
    NickChanged {
        /// Previous nick.
        old: String,
        /// New nick.
        new: String,
    },
    /// A user's account changed (`account-notify`): `Some` on login, `None` on
    /// logout.
    AccountChanged {
        /// The affected nick.
        nick: String,
        /// The new account, or `None` if logged out.
        account: Option<String>,
    },
    /// A user's user/host changed (`chghost`).
    HostChanged {
        /// The affected nick.
        nick: String,
        /// New user/ident.
        user: String,
        /// New host.
        host: String,
    },
    /// A user's away state changed (`away-notify`): `Some(message)` when away,
    /// `None` when back.
    AwayChanged {
        /// The affected nick.
        nick: String,
        /// The away message, or `None` if no longer away.
        message: Option<String>,
    },
    /// Our own away state, from `RPL_NOWAWAY` (306, now away) / `RPL_UNAWAY`
    /// (305, back). `true` means we are now marked away.
    AwayStatus(bool),
    /// A user changed their realname (`setname`).
    RealnameChanged {
        /// The affected nick.
        nick: String,
        /// The new realname.
        realname: String,
    },
    /// A completed `WHOIS`, assembled from its reply numerics (311-319/330/671),
    /// emitted as one unit at end-of-whois (318).
    WhoisReceived(WhoisInfo),
    /// A `FAIL`/`WARN`/`NOTE` standard reply (rule 15).
    StandardReply(StandardReply),
    /// The server rejected something we did (cannot send, nick in use, not an
    /// operator, ...). The numeric stays inside the engine.
    CommandError {
        /// What went wrong.
        error: ServerError,
        /// The channel, nick or command it concerns, when the server names one.
        target: Option<String>,
        /// The server's human-readable explanation.
        message: String,
    },
    /// The message of the day, complete (empty when the server has none).
    Motd(Vec<String>),
    /// Informational server text with no dedicated event (LUSERS counts and the
    /// like), so nothing the server says is silently dropped.
    ServerInfo(String),
    /// A mode change on a channel (or on a nick, for user modes).
    ModeChanged {
        /// The channel or nick the modes apply to.
        target: String,
        /// Who changed them, if a user did (servers set modes too).
        by: Option<String>,
        /// The mode string as sent, e.g. `+o-v`.
        modes: String,
        /// The mode arguments as sent, in order.
        args: Vec<String>,
        /// The membership-prefix changes this implies (op, voice, ...).
        prefix_changes: Vec<PrefixChange>,
    },
    /// The server answered one of our `PING`s. The caller owns the clock: it
    /// matches `token` against the ping it sent and times the round trip.
    Pong {
        /// The token echoed back.
        token: String,
    },
    /// A channel's current modes, from the reply to a bare `MODE #chan`.
    ChannelModes {
        /// The channel.
        target: String,
        /// The modes, with their arguments (e.g. `+nt` or `+k secret`).
        modes: String,
    },
    /// The connection was terminated.
    Disconnected(DisconnectReason),
}

/// One membership-prefix change implied by a channel mode (`+o bob`, `-v alice`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefixChange {
    /// Whose prefix changed.
    pub nick: String,
    /// Which prefix.
    pub prefix: MemberPrefix,
    /// Whether it was granted (`true`) or removed (`false`).
    pub granted: bool,
}

/// Why the server rejected a command. `Other` carries only the server's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerError {
    /// The channel does not exist.
    NoSuchChannel,
    /// We cannot send to that channel or nick.
    CannotSend,
    /// We are in too many channels.
    TooManyChannels,
    /// The server does not know the command.
    UnknownCommand,
    /// The nickname is not valid.
    ErroneousNick,
    /// The nickname is already taken.
    NickInUse,
    /// The target is not on the channel.
    UserNotInChannel,
    /// We are not on the channel.
    NotOnChannel,
    /// The command lacked parameters.
    NeedMoreParams,
    /// The channel is full.
    ChannelFull,
    /// The channel is invite-only.
    InviteOnly,
    /// We are banned from the channel.
    Banned,
    /// The channel key was missing or wrong.
    BadKey,
    /// We need channel operator status.
    NotOperator,
    /// We lack the privileges for this.
    NoPrivileges,
    /// Anything else the server rejected.
    Other,
}

impl ServerError {
    /// A one-line explanation for the user: the server's own words, prefixed by
    /// what they concern, with a next step where there is an obvious one.
    pub fn describe(&self, target: Option<&str>, message: &str) -> String {
        match (self, target) {
            (ServerError::NickInUse, Some(nick)) => {
                format!("nick {nick} is already in use; pick another with /nick <name>")
            }
            (ServerError::BadKey, Some(channel)) => {
                format!("{channel}: {message} (try /join {channel} <key>)")
            }
            (_, Some(target)) => format!("{target}: {message}"),
            (_, None) => message.to_string(),
        }
    }
}

/// Why the engine tore down the connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisconnectReason {
    /// SASL failed and the configured policy was to abort rather than continue
    /// unauthenticated.
    SaslAbortedByPolicy,
    /// Every candidate nickname was already in use; registration was abandoned.
    NickUnavailable,
}
