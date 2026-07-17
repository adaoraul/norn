//! Async IRCv3 engine.
//!
//! Owns the connection, line framing, the cap/SASL bring-up state machine, and
//! semantic `Event` emission. Built on top of the pure `irc-proto` core.
//!
//! The bring-up machine ([`bringup`]) is sans-I/O and synchronous so it can be
//! driven by scripted transcripts; [`connection`] is the async driver that
//! feeds it real bytes. Batch collection, label routing, and CHATHISTORY arrive
//! in later build-order steps.

pub mod bringup;
pub mod connection;
pub mod event;
pub mod framing;

pub use bringup::{Action, BringupConfig, BringupMachine, SaslFailPolicy, State};
pub use connection::Connection;
pub use event::{AccountName, DisconnectReason, Event};
pub use framing::LineFramer;
