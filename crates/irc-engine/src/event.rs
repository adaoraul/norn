//! Semantic events: the engine's only output above the pipeline.
//!
//! The UI consumes these and never sees a raw line or a numeric. This module
//! currently carries the variants produced during bring-up; more arrive in
//! later build-order steps (message receipt, history, membership).

use irc_proto::{CapSet, SaslError};

use crate::batch::CompletedBatch;
use crate::chat::ChatMessage;
use crate::stdreply::StandardReply;

/// An authenticated account name.
pub type AccountName = String;

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
}
