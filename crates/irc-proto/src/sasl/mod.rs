//! SASL: mechanism trait, base64 helpers, and AUTHENTICATE framing.
//!
//! Mechanism-agnostic framing lives here; mechanism-specific payloads live in
//! the submodules. The exchange runs over repeated `AUTHENTICATE` lines: the
//! client sends `AUTHENTICATE <MECH>`, the server replies `AUTHENTICATE +`
//! (an empty challenge), and base64 blobs flow in each direction.
//!
//! The one subtlety worth pinning down is the 400-byte chunking (rule 8),
//! implemented in [`authenticate_lines`].

pub mod external;
pub mod plain;
pub mod scram;

use base64::{engine::general_purpose::STANDARD, Engine as _};

pub use external::External;
pub use plain::Plain;
pub use scram::ScramSha256;

/// Maximum base64 payload bytes per `AUTHENTICATE` line (rule 8).
pub const CHUNK_LEN: usize = 400;

/// Errors from the SASL exchange. The numeric-driven ones map to the failure
/// replies; [`SaslError::ServerSignatureMismatch`] is the SCRAM verification
/// failure (rule 10).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SaslError {
    /// `904`: authentication failed.
    #[error("authentication failed")]
    Failed,
    /// `905`: the AUTHENTICATE message was too long.
    #[error("authentication message too long")]
    TooLong,
    /// `906`: authentication was aborted.
    #[error("authentication aborted")]
    Aborted,
    /// SCRAM server signature (`v=`) did not verify.
    #[error("server signature did not verify")]
    ServerSignatureMismatch,
    /// A server challenge was not valid base64.
    #[error("invalid base64 in SASL exchange")]
    InvalidBase64,
}

/// A SASL mechanism drives the AUTHENTICATE exchange.
///
/// Given a decoded server challenge, it yields the next raw response payload
/// (before base64 and chunking). Single-message mechanisms (PLAIN, EXTERNAL)
/// respond once to the initial empty challenge.
///
/// `Send` is required so a boxed mechanism can move into a per-connection task
/// on a multi-threaded runtime.
pub trait Mechanism: Send {
    /// The mechanism name as sent in `AUTHENTICATE <name>`.
    fn name(&self) -> &str;

    /// Produce the next response to a decoded server challenge. The server's
    /// initial `AUTHENTICATE +` decodes to an empty challenge.
    fn respond(&mut self, challenge: &[u8]) -> Result<Vec<u8>, SaslError>;
}

/// Apply SASLprep (RFC 4013) to a username or password, as PLAIN (RFC 4616)
/// and SCRAM (RFC 5802) require, so a client and server that type the same
/// credential differently (non-ASCII spaces, compatibility forms, combining
/// marks) still agree after normalization.
///
/// SASLprep is the identity on ASCII printable strings, so the common case is
/// untouched. On a prohibited-output error (control characters, etc.) we fall
/// back to the original string rather than failing the exchange here: that is
/// no worse than sending the raw credential, and it keeps this a pure,
/// infallible transform at the mechanism boundary.
pub(crate) fn saslprep(input: &str) -> String {
    stringprep::saslprep(input)
        .map(|prepped| prepped.into_owned())
        .unwrap_or_else(|_| input.to_string())
}

/// base64-encode a raw payload (standard alphabet, padded).
pub fn encode_b64(raw: &[u8]) -> String {
    STANDARD.encode(raw)
}

/// Decode a base64 server challenge.
pub fn decode_b64(payload: &str) -> Result<Vec<u8>, SaslError> {
    STANDARD
        .decode(payload)
        .map_err(|_| SaslError::InvalidBase64)
}

/// Frame a base64 payload into the sequence of `AUTHENTICATE` lines (rule 8).
///
/// - An empty payload is a single `AUTHENTICATE +`.
/// - Otherwise the payload is split into chunks of at most [`CHUNK_LEN`] bytes.
///   Completion is signaled to the server by a chunk SHORTER than [`CHUNK_LEN`].
/// - If the payload length is an exact multiple of [`CHUNK_LEN`], an extra
///   `AUTHENTICATE +` is appended, or the peer waits forever.
pub fn authenticate_lines(payload: &str) -> Vec<String> {
    if payload.is_empty() {
        return vec!["AUTHENTICATE +".to_string()];
    }

    // base64 is ASCII, so byte offsets are valid char boundaries.
    let mut lines: Vec<String> = payload
        .as_bytes()
        .chunks(CHUNK_LEN)
        .map(|chunk| {
            let text = std::str::from_utf8(chunk).expect("base64 is ascii");
            format!("AUTHENTICATE {text}")
        })
        .collect();

    // A payload that is an exact multiple of CHUNK_LEN ends on a full chunk, so
    // the server cannot tell we are done: send an explicit empty continuation.
    if payload.len().is_multiple_of(CHUNK_LEN) {
        lines.push("AUTHENTICATE +".to_string());
    }

    lines
}

/// Convenience: base64-encode a raw response and frame it into AUTHENTICATE
/// lines in one step.
pub fn response_lines(raw: &[u8]) -> Vec<String> {
    authenticate_lines(&encode_b64(raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A base64-shaped payload of `n` ASCII chars.
    fn payload(n: usize) -> String {
        "A".repeat(n)
    }

    const PREFIX: usize = "AUTHENTICATE ".len();

    #[test]
    fn empty_payload_is_single_plus() {
        assert_eq!(authenticate_lines(""), vec!["AUTHENTICATE +".to_string()]);
    }

    #[test]
    fn payload_399_is_one_short_line() {
        let lines = authenticate_lines(&payload(399));
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), PREFIX + 399);
    }

    #[test]
    fn payload_400_appends_empty_continuation() {
        let lines = authenticate_lines(&payload(400));
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), PREFIX + 400);
        assert_eq!(lines[1], "AUTHENTICATE +");
    }

    #[test]
    fn payload_401_splits_400_then_1() {
        let lines = authenticate_lines(&payload(401));
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), PREFIX + 400);
        assert_eq!(lines[1].len(), PREFIX + 1);
    }

    #[test]
    fn payload_800_is_two_chunks_then_empty() {
        let lines = authenticate_lines(&payload(800));
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].len(), PREFIX + 400);
        assert_eq!(lines[1].len(), PREFIX + 400);
        assert_eq!(lines[2], "AUTHENTICATE +");
    }

    #[test]
    fn base64_roundtrips() {
        let raw = b"\x00abc\x00secret";
        assert_eq!(decode_b64(&encode_b64(raw)).unwrap(), raw);
    }

    #[test]
    fn decode_rejects_invalid_base64() {
        assert_eq!(decode_b64("not valid!!"), Err(SaslError::InvalidBase64));
    }
}
