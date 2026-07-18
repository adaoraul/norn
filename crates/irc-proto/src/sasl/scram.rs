//! SASL SCRAM-SHA-256 (rule 10): salted challenge-response, password never on
//! the wire, and the server is verified too.
//!
//! Four messages over the AUTHENTICATE framing:
//!
//! 1. client-first: `n,,n=<user>,r=<client-nonce>`
//! 2. server-first: `r=<combined-nonce>,s=<salt>,i=<iterations>`
//! 3. client-final: `c=biws,r=<combined-nonce>,p=<ClientProof>`
//! 4. server-final: `v=<ServerSignature>` (the client MUST verify this before
//!    trusting the login; a mismatch is [`SaslError::ServerSignatureMismatch`]).
//!
//! The client nonce is supplied by the caller so this stays pure and the
//! known-answer test is deterministic; in real use the engine generates a fresh
//! random nonce per attempt. Username and password are SASLprep'd (RFC 4013)
//! at construction, then the username is escaped (`=` -> `=3D`, `,` -> `=2C`).

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use super::{decode_b64, encode_b64, saslprep, Mechanism, SaslError};

type HmacSha256 = Hmac<Sha256>;

fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    let out = mac.finalize().into_bytes();
    let mut result = [0u8; 32];
    result.copy_from_slice(&out);
    result
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let out = hasher.finalize();
    let mut result = [0u8; 32];
    result.copy_from_slice(&out);
    result
}

/// PBKDF2-HMAC-SHA256 with `dkLen == hLen`, so a single output block.
fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    // U1 = HMAC(password, salt || INT(1)); Uj = HMAC(password, U(j-1)).
    let mut block = salt.to_vec();
    block.extend_from_slice(&1u32.to_be_bytes());
    let mut u = hmac(password, &block);
    let mut result = u;
    for _ in 1..iterations {
        u = hmac(password, &u);
        for (r, x) in result.iter_mut().zip(u.iter()) {
            *r ^= *x;
        }
    }
    result
}

/// Escape a SCRAM username: `=` and `,` are reserved in the attribute list.
fn escape_username(name: &str) -> String {
    name.replace('=', "=3D").replace(',', "=2C")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScramState {
    Initial,
    AwaitingServerFirst,
    AwaitingServerFinal,
    Done,
}

/// SASL SCRAM-SHA-256 driver.
#[derive(Debug, Clone)]
pub struct ScramSha256 {
    username: String,
    password: String,
    client_nonce: String,
    state: ScramState,
    client_first_bare: String,
    server_signature: [u8; 32],
}

impl ScramSha256 {
    /// Build a SCRAM-SHA-256 attempt. `client_nonce` must be a fresh random
    /// string (the engine supplies one); it is fixed here only for tests.
    pub fn new(
        username: impl Into<String>,
        password: impl Into<String>,
        client_nonce: impl Into<String>,
    ) -> Self {
        ScramSha256 {
            // SASLprep both credentials (RFC 5802 §5.1): the SCRAM proof binds
            // the normalized password, and `n=` carries the normalized username.
            username: saslprep(&username.into()),
            password: saslprep(&password.into()),
            client_nonce: client_nonce.into(),
            state: ScramState::Initial,
            client_first_bare: String::new(),
            server_signature: [0u8; 32],
        }
    }

    fn client_first(&mut self) -> Vec<u8> {
        self.client_first_bare = format!(
            "n={},r={}",
            escape_username(&self.username),
            self.client_nonce
        );
        // GS2 header "n,," (no channel binding, no authzid).
        format!("n,,{}", self.client_first_bare).into_bytes()
    }

    fn client_final(&mut self, server_first: &str) -> Result<Vec<u8>, SaslError> {
        let (nonce, salt, iterations) =
            parse_server_first(server_first).ok_or(SaslError::Failed)?;

        // The combined nonce must begin with our client nonce (RFC 5802).
        if !nonce.starts_with(&self.client_nonce) {
            return Err(SaslError::Failed);
        }

        let salted = pbkdf2_sha256(self.password.as_bytes(), &salt, iterations);
        let client_key = hmac(&salted, b"Client Key");
        let stored_key = sha256(&client_key);

        // c=biws is base64 of the GS2 header "n,,".
        let client_final_bare = format!("c=biws,r={nonce}");
        let auth_message = format!(
            "{},{},{}",
            self.client_first_bare, server_first, client_final_bare
        );

        let client_signature = hmac(&stored_key, auth_message.as_bytes());
        let mut proof = client_key;
        for (p, s) in proof.iter_mut().zip(client_signature.iter()) {
            *p ^= *s;
        }

        let server_key = hmac(&salted, b"Server Key");
        self.server_signature = hmac(&server_key, auth_message.as_bytes());

        Ok(format!("{client_final_bare},p={}", encode_b64(&proof)).into_bytes())
    }

    fn verify_server_final(&mut self, server_final: &str) -> Result<Vec<u8>, SaslError> {
        let signature =
            parse_server_final(server_final).ok_or(SaslError::ServerSignatureMismatch)?;
        if signature.as_slice() == self.server_signature.as_slice() {
            // Verified: acknowledge with an empty response (AUTHENTICATE +).
            Ok(Vec::new())
        } else {
            Err(SaslError::ServerSignatureMismatch)
        }
    }
}

impl Mechanism for ScramSha256 {
    fn name(&self) -> &str {
        "SCRAM-SHA-256"
    }

    fn respond(&mut self, challenge: &[u8]) -> Result<Vec<u8>, SaslError> {
        match self.state {
            ScramState::Initial => {
                let out = self.client_first();
                self.state = ScramState::AwaitingServerFirst;
                Ok(out)
            }
            ScramState::AwaitingServerFirst => {
                let server_first = std::str::from_utf8(challenge).map_err(|_| SaslError::Failed)?;
                let out = self.client_final(server_first)?;
                self.state = ScramState::AwaitingServerFinal;
                Ok(out)
            }
            ScramState::AwaitingServerFinal => {
                let server_final = std::str::from_utf8(challenge)
                    .map_err(|_| SaslError::ServerSignatureMismatch)?;
                let out = self.verify_server_final(server_final)?;
                self.state = ScramState::Done;
                Ok(out)
            }
            ScramState::Done => Ok(Vec::new()),
        }
    }
}

/// Parse `r=<nonce>,s=<b64 salt>,i=<iterations>` from a server-first message.
fn parse_server_first(message: &str) -> Option<(String, Vec<u8>, u32)> {
    let mut nonce = None;
    let mut salt = None;
    let mut iterations = None;
    for field in message.split(',') {
        if let Some(v) = field.strip_prefix("r=") {
            nonce = Some(v.to_string());
        } else if let Some(v) = field.strip_prefix("s=") {
            salt = decode_b64(v).ok();
        } else if let Some(v) = field.strip_prefix("i=") {
            iterations = v.parse().ok();
        }
    }
    Some((nonce?, salt?, iterations?))
}

/// Parse the `v=<b64 signature>` from a server-final message (or `None` if the
/// server sent an error `e=...` instead).
fn parse_server_final(message: &str) -> Option<Vec<u8>> {
    for field in message.split(',') {
        if let Some(v) = field.strip_prefix("v=") {
            return decode_b64(v).ok();
        }
        if field.starts_with("e=") {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 7677 known-answer vector: user "user", password "pencil".
    const CLIENT_NONCE: &str = "rOprNGfwEbeRWgbNEkqO";
    const SERVER_FIRST: &str =
        "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096";
    const CLIENT_FINAL: &str = "c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ=";
    const SERVER_FINAL: &str = "v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=";

    #[test]
    fn known_answer_proof_and_verification() {
        let mut scram = ScramSha256::new("user", "pencil", CLIENT_NONCE);

        let client_first = scram.respond(b"").unwrap();
        assert_eq!(
            String::from_utf8(client_first).unwrap(),
            "n,,n=user,r=rOprNGfwEbeRWgbNEkqO"
        );

        let client_final = scram.respond(SERVER_FIRST.as_bytes()).unwrap();
        assert_eq!(String::from_utf8(client_final).unwrap(), CLIENT_FINAL);

        // Correct server signature: client accepts, acknowledging with empty.
        let ack = scram.respond(SERVER_FINAL.as_bytes()).unwrap();
        assert!(ack.is_empty());
    }

    #[test]
    fn rejects_a_wrong_server_signature() {
        let mut scram = ScramSha256::new("user", "pencil", CLIENT_NONCE);
        scram.respond(b"").unwrap();
        scram.respond(SERVER_FIRST.as_bytes()).unwrap();

        let wrong = format!("v={}", encode_b64(&[0u8; 32]));
        assert_eq!(
            scram.respond(wrong.as_bytes()),
            Err(SaslError::ServerSignatureMismatch)
        );
    }

    #[test]
    fn rejects_server_error_in_final() {
        let mut scram = ScramSha256::new("user", "pencil", CLIENT_NONCE);
        scram.respond(b"").unwrap();
        scram.respond(SERVER_FIRST.as_bytes()).unwrap();
        assert_eq!(
            scram.respond(b"e=other-error"),
            Err(SaslError::ServerSignatureMismatch)
        );
    }

    #[test]
    fn rejects_server_nonce_not_extending_client_nonce() {
        let mut scram = ScramSha256::new("user", "pencil", CLIENT_NONCE);
        scram.respond(b"").unwrap();
        // Server nonce that does not start with our client nonce.
        let bad = "r=DIFFERENTNONCE,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096";
        assert_eq!(scram.respond(bad.as_bytes()), Err(SaslError::Failed));
    }

    #[test]
    fn username_special_characters_are_escaped() {
        let mut scram = ScramSha256::new("a,b=c", "pw", "nonce123");
        let first = String::from_utf8(scram.respond(b"").unwrap()).unwrap();
        assert_eq!(first, "n,,n=a=2Cb=3Dc,r=nonce123");
    }

    #[test]
    fn saslprep_normalizes_username_and_password() {
        // Soft hyphen maps to nothing, no-break space maps to a plain space:
        // both credentials are SASLprep'd before use (RFC 5802 §5.1).
        let scram = ScramSha256::new("us\u{00AD}er", "pen\u{00A0}cil", "nonce");
        assert_eq!(scram.username, "user");
        assert_eq!(scram.password, "pen cil");
    }
}
