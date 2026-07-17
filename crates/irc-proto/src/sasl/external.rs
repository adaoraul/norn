//! SASL EXTERNAL: identity proven out of band (a TLS client cert / CertFP).
//!
//! No secret goes on the wire. The response is the base64 of the authzid,
//! which is usually empty, so the exchange is just `AUTHENTICATE +`.

use super::{Mechanism, SaslError};

/// SASL EXTERNAL: an optional authorization identity.
#[derive(Debug, Clone, Default)]
pub struct External {
    /// Authorization identity; usually empty.
    pub authzid: String,
}

impl External {
    /// EXTERNAL with an empty authzid (the common case).
    pub fn new() -> Self {
        External::default()
    }

    /// EXTERNAL requesting a specific authzid.
    pub fn with_authzid(authzid: impl Into<String>) -> Self {
        External {
            authzid: authzid.into(),
        }
    }

    /// The raw response: the authzid bytes (empty for the common case).
    pub fn encode(&self) -> Vec<u8> {
        self.authzid.as_bytes().to_vec()
    }
}

impl Mechanism for External {
    fn name(&self) -> &str {
        "EXTERNAL"
    }

    fn respond(&mut self, _challenge: &[u8]) -> Result<Vec<u8>, SaslError> {
        Ok(self.encode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sasl::response_lines;

    #[test]
    fn external_empty_authzid_is_a_single_plus() {
        let mut e = External::new();
        let raw = e.respond(&[]).unwrap();
        assert!(raw.is_empty());
        assert_eq!(response_lines(&raw), vec!["AUTHENTICATE +".to_string()]);
    }

    #[test]
    fn external_includes_authzid_when_set() {
        let e = External::with_authzid("adao");
        assert_eq!(e.encode(), b"adao");
        assert_eq!(
            response_lines(&e.encode()),
            vec!["AUTHENTICATE YWRhbw==".to_string()]
        );
    }
}
