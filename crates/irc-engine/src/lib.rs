//! Async IRCv3 engine.
//!
//! Owns the connection, line framing, the cap/SASL bring-up state machine, and
//! semantic `Event` emission. Built on top of the pure `irc-proto` core.
//!
//! The bring-up machine ([`bringup`]) is sans-I/O and synchronous so it can be
//! driven by scripted transcripts; [`connection`] is the async driver that
//! feeds it real bytes. Batch collection, label routing, and CHATHISTORY arrive
//! in later build-order steps.

pub mod batch;
pub mod bringup;
pub mod chat;
pub mod connection;
pub mod engine;
pub mod event;
pub mod framing;
pub mod history;
pub mod labels;
pub mod stdreply;

pub use batch::{BatchCollector, BatchItem, CollectorOutput, CompletedBatch};
pub use bringup::{Action, BringupConfig, BringupMachine, SaslFailPolicy, State};
pub use chat::{ChatMessage, MessageKind};
pub use connection::Connection;
pub use engine::Engine;
pub use event::{AccountName, DisconnectReason, Event};
pub use framing::LineFramer;
pub use history::{ChatHistoryRequest, Selector};
pub use labels::{LabelRouter, LabeledResponse};
pub use stdreply::{ReplyKind, StandardReply};
