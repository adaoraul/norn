//! SASL PLAIN (rule 9): `authzid \0 authcid \0 password`, base64'd.
//!
//! authzid is usually empty, so the payload is `\0account\0password`. PLAIN
//! sends the password in the clear (post-base64), so it must be offered over
//! TLS only; that policy is enforced by the engine, not here.
//!
//! The three fields are SASLprep'd (RFC 4013) at construction, per RFC 4616.

use super::{saslprep, Mechanism, SaslError};

/// SASL PLAIN credentials.
#[derive(Debug, Clone)]
pub struct Plain {
    /// Authorization identity; usually empty.
    pub authzid: String,
    /// Authentication identity (the account name).
    pub authcid: String,
    /// The password.
    pub password: String,
}

impl Plain {
    /// PLAIN with an empty authzid (the common case).
    pub fn new(authcid: impl Into<String>, password: impl Into<String>) -> Self {
        Plain {
            authzid: String::new(),
            authcid: saslprep(&authcid.into()),
            password: saslprep(&password.into()),
        }
    }

    /// PLAIN with an explicit authzid.
    pub fn with_authzid(
        authzid: impl Into<String>,
        authcid: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Plain {
            authzid: saslprep(&authzid.into()),
            authcid: saslprep(&authcid.into()),
            password: saslprep(&password.into()),
        }
    }

    /// The raw PLAIN message: `authzid \0 authcid \0 password`.
    pub fn encode(&self) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(self.authzid.len() + self.authcid.len() + self.password.len() + 2);
        out.extend_from_slice(self.authzid.as_bytes());
        out.push(0);
        out.extend_from_slice(self.authcid.as_bytes());
        out.push(0);
        out.extend_from_slice(self.password.as_bytes());
        out
    }
}

impl Mechanism for Plain {
    fn name(&self) -> &str {
        "PLAIN"
    }

    fn respond(&mut self, _challenge: &[u8]) -> Result<Vec<u8>, SaslError> {
        Ok(self.encode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sasl::{encode_b64, response_lines};

    #[test]
    fn plain_known_answer() {
        // TESTS.md 4.1: account "adao", password "hunter2", empty authzid.
        let p = Plain::new("adao", "hunter2");
        assert_eq!(p.encode(), b"\x00adao\x00hunter2");
        assert_eq!(encode_b64(&p.encode()), "AGFkYW8AaHVudGVyMg==");
    }

    #[test]
    fn plain_respond_frames_a_single_line() {
        let mut p = Plain::new("adao", "hunter2");
        let raw = p.respond(&[]).unwrap();
        assert_eq!(
            response_lines(&raw),
            vec!["AUTHENTICATE AGFkYW8AaHVudGVyMg==".to_string()]
        );
    }

    #[test]
    fn authzid_is_included_when_set() {
        let p = Plain::with_authzid("admin", "adao", "hunter2");
        assert_eq!(p.encode(), b"admin\x00adao\x00hunter2");
    }

    #[test]
    fn saslprep_normalizes_credentials() {
        // Soft hyphen (U+00AD) maps to nothing; no-break space (U+00A0) -> space.
        let p = Plain::new("ad\u{00AD}ao", "hunter\u{00A0}2");
        assert_eq!(p.authcid, "adao");
        assert_eq!(p.password, "hunter 2");
        // Pure ASCII is untouched (SASLprep identity), so known answers hold.
        assert_eq!(
            Plain::new("adao", "hunter2").encode(),
            b"\x00adao\x00hunter2"
        );
    }
}
