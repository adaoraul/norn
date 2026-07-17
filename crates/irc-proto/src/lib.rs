//! Pure, synchronous IRCv3 protocol core.
//!
//! This crate owns the deterministic, high-risk logic (tag codec, message
//! parse/emit, capability value parsing, SASL math). It performs no I/O and
//! uses no async, so it can be property-tested and fuzzed in isolation.
//!
//! See `ARCHITECTURE.md` for the design and `CLAUDE.md` for the hard protocol
//! rules each module must uphold.

pub mod caps;
pub mod message;
pub mod sasl;
pub mod tags;

pub use caps::{Cap, CapName, CapSet, LsAccumulator};
pub use message::{Command, Message, ParseError, Source};
pub use sasl::{External, Mechanism, Plain, SaslError, ScramSha256};
pub use tags::{TagKey, Tags};
