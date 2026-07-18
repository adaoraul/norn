//! Semantic events: the engine's only output above the pipeline.
//!
//! The UI consumes these and never sees a raw line or a numeric. This module
//! currently carries the variants produced during bring-up; more arrive in
//! later build-order steps (message receipt, history, membership).

use chrono::{DateTime, Utc};
use irc_proto::{CapSet, SaslError, Source};

use crate::batch::CompletedBatch;
use crate::chat::ChatMessage;
use crate::roster::Member;
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
    /// A netsplit/netjoin (or other collapsible) batch folded into one event.
    BatchCollapsed(CompletedBatch),
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
    /// The connection was terminated.
    Disconnected(DisconnectReason),
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
