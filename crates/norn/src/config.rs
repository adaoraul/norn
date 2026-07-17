//! Command-line configuration for the norn client.
//!
//! Everything but the password comes from CLI flags. The SASL password is read
//! from the `NORN_PASSWORD` environment variable rather than a flag, so it never
//! lands in argv or shell history.

use clap::{Parser, ValueEnum};
use irc_engine::{recommended_caps, BringupConfig, SaslFailPolicy};
use irc_proto::{Mechanism, Plain, ScramSha256};
use rand::Rng;

/// Environment variable holding the SASL password.
pub const PASSWORD_ENV: &str = "NORN_PASSWORD";

/// Which SASL mechanism to authenticate with.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaslMech {
    /// PLAIN: password base64'd, TLS only.
    Plain,
    /// SCRAM-SHA-256: salted challenge-response, password never sent.
    Scram,
}

/// A fresh random SCRAM client nonce (printable ASCII, no reserved chars).
fn random_nonce() -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..24)
        .map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char)
        .collect()
}

/// Parsed command-line arguments.
#[derive(Parser, Debug)]
#[command(name = "norn", about = "A terminal IRCv3 client.")]
pub struct Cli {
    /// Server hostname to connect to.
    #[arg(long)]
    pub server: String,

    /// Server port.
    #[arg(long, default_value_t = 6697)]
    pub port: u16,

    /// Nickname to register.
    #[arg(long)]
    pub nick: String,

    /// Username/ident (defaults to the nick).
    #[arg(long)]
    pub user: Option<String>,

    /// Realname / GECOS (defaults to the nick).
    #[arg(long)]
    pub realname: Option<String>,

    /// SASL account name. When set, authentication is attempted using the
    /// password from the NORN_PASSWORD environment variable.
    #[arg(long)]
    pub sasl_account: Option<String>,

    /// SASL mechanism to use with --sasl-account.
    #[arg(long, value_enum, default_value_t = SaslMech::Plain)]
    pub sasl_mech: SaslMech,

    /// Disable TLS and connect in plaintext (for local test servers).
    #[arg(long, default_value_t = false)]
    pub no_tls: bool,

    /// Channels to auto-join on connect (comma-separated, e.g. #rust,#tokio).
    #[arg(long, value_delimiter = ',')]
    pub join: Vec<String>,
}

/// Where and how to open the socket.
#[derive(Debug, Clone)]
pub struct ConnConfig {
    /// Server hostname.
    pub host: String,
    /// Server port.
    pub port: u16,
    /// Whether to wrap the connection in TLS.
    pub tls: bool,
}

/// Fully resolved settings: transport plus the engine bring-up config.
pub struct Settings {
    /// Transport parameters.
    pub conn: ConnConfig,
    /// Engine bring-up configuration.
    pub bringup: BringupConfig,
    /// Channels to auto-join once registered.
    pub auto_join: Vec<String>,
}

impl Cli {
    /// Resolve CLI args (and the password env var) into [`Settings`].
    pub fn into_settings(self) -> Settings {
        let tls = !self.no_tls;
        let user = self.user.clone().unwrap_or_else(|| self.nick.clone());
        let realname = self.realname.clone().unwrap_or_else(|| self.nick.clone());

        let mut sasl: Vec<Box<dyn Mechanism>> = Vec::new();
        if let Some(account) = &self.sasl_account {
            match std::env::var(PASSWORD_ENV) {
                Ok(password) if !password.is_empty() => {
                    let mechanism: Box<dyn Mechanism> = match self.sasl_mech {
                        SaslMech::Plain => Box::new(Plain::new(account.clone(), password)),
                        SaslMech::Scram => {
                            Box::new(ScramSha256::new(account.clone(), password, random_nonce()))
                        }
                    };
                    sasl.push(mechanism);
                }
                _ => {
                    eprintln!(
                        "warning: --sasl-account set but {PASSWORD_ENV} is empty; \
                         connecting without SASL"
                    );
                }
            }
        }

        let bringup = BringupConfig {
            nick: self.nick.clone(),
            user,
            realname,
            cap_groups: recommended_caps(),
            sasl,
            tls,
            sasl_fail_policy: SaslFailPolicy::Continue,
        };

        Settings {
            conn: ConnConfig {
                host: self.server,
                port: self.port,
                tls,
            },
            bringup,
            auto_join: self.join,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::random_nonce;

    #[test]
    fn nonce_is_well_formed_and_varies() {
        let a = random_nonce();
        let b = random_nonce();
        assert_eq!(a.len(), 24);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(a, b, "two nonces should differ");
    }
}
