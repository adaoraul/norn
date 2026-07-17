//! Command-line configuration for the norn client.
//!
//! Everything but the password comes from CLI flags. The SASL password is read
//! from the `NORN_PASSWORD` environment variable rather than a flag, so it never
//! lands in argv or shell history.

use clap::Parser;
use irc_engine::{recommended_caps, BringupConfig, SaslFailPolicy};
use irc_proto::{Mechanism, Plain};

/// Environment variable holding the SASL password.
pub const PASSWORD_ENV: &str = "NORN_PASSWORD";

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

    /// SASL account name. When set, PLAIN authentication is attempted using the
    /// password from the NORN_PASSWORD environment variable.
    #[arg(long)]
    pub sasl_account: Option<String>,

    /// Disable TLS and connect in plaintext (for local test servers).
    #[arg(long, default_value_t = false)]
    pub no_tls: bool,
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
    /// The SASL account name, retained for display only.
    pub sasl_account: Option<String>,
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
                    sasl.push(Box::new(Plain::new(account.clone(), password)));
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
            sasl_account: self.sasl_account,
        }
    }
}
