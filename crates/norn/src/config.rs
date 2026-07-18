//! Configuration: CLI args plus an optional multi-network TOML file.
//!
//! Networks come from a TOML config file (default `~/.config/norn/config.toml`,
//! or `--config <path>`); the CLI flags additionally define one ad-hoc network.
//! SASL passwords are never stored in the file: each network either names a
//! `password_command` whose stdout is the password, or falls back to the
//! `NORN_PASSWORD` environment variable.

use std::io;
use std::path::PathBuf;
use std::process::Command as ProcCommand;

use clap::{Parser, ValueEnum};
use directories::ProjectDirs;
use irc_engine::{recommended_caps, BringupConfig, SaslFailPolicy};
use irc_proto::{Mechanism, Plain, ScramSha256};
use rand::Rng;
use serde::Deserialize;

/// Environment variable holding a fallback SASL password.
pub const PASSWORD_ENV: &str = "NORN_PASSWORD";

/// Which SASL mechanism to authenticate with.
#[derive(ValueEnum, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SaslMech {
    /// PLAIN: password base64'd, TLS only.
    #[default]
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
#[derive(Parser, Debug, Clone)]
#[command(name = "norn", about = "A terminal IRCv3 client.")]
pub struct Cli {
    /// Path to a TOML config file (default: ~/.config/norn/config.toml).
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Use the plain line renderer instead of the TUI.
    #[arg(long, default_value_t = false)]
    pub plain: bool,

    /// Server hostname (defines a single ad-hoc network).
    #[arg(long)]
    pub server: Option<String>,

    /// Server port.
    #[arg(long, default_value_t = 6697)]
    pub port: u16,

    /// Nickname to register.
    #[arg(long)]
    pub nick: Option<String>,

    /// Username/ident (defaults to the nick).
    #[arg(long)]
    pub user: Option<String>,

    /// Realname / GECOS (defaults to the nick).
    #[arg(long)]
    pub realname: Option<String>,

    /// SASL account name. Uses NORN_PASSWORD for the password.
    #[arg(long)]
    pub sasl_account: Option<String>,

    /// SASL mechanism to use with --sasl-account.
    #[arg(long, value_enum, default_value_t = SaslMech::Plain)]
    pub sasl_mech: SaslMech,

    /// Disable TLS and connect in plaintext (for local test servers).
    #[arg(long, default_value_t = false)]
    pub no_tls: bool,

    /// Channels to auto-join on connect (comma-separated).
    #[arg(long, value_delimiter = ',')]
    pub join: Vec<String>,
}

fn default_true() -> bool {
    true
}
fn default_port() -> u16 {
    6697
}

/// A network as declared in the TOML file (or synthesized from CLI flags).
#[derive(Deserialize, Debug, Clone)]
pub struct NetworkConfig {
    /// Display name for the network.
    pub name: String,
    /// Server hostname.
    pub host: String,
    /// Server port.
    #[serde(default = "default_port")]
    pub port: u16,
    /// Whether to use TLS.
    #[serde(default = "default_true")]
    pub tls: bool,
    /// Nickname.
    pub nick: String,
    /// Username/ident (defaults to the nick).
    #[serde(default)]
    pub user: Option<String>,
    /// Realname (defaults to the nick).
    #[serde(default)]
    pub realname: Option<String>,
    /// SASL account, if authenticating.
    #[serde(default)]
    pub sasl_account: Option<String>,
    /// SASL mechanism.
    #[serde(default)]
    pub sasl_mech: SaslMech,
    /// Shell command whose stdout is the SASL password.
    #[serde(default)]
    pub password_command: Option<String>,
    /// Channels to auto-join.
    #[serde(default)]
    pub auto_join: Vec<String>,
}

/// The whole config file.
#[derive(Deserialize, Debug, Default)]
pub struct Config {
    /// Declared networks (TOML `[[network]]` or `[[networks]]`).
    #[serde(default, alias = "network")]
    pub networks: Vec<NetworkConfig>,
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

/// A network with its password resolved and ready to connect.
pub struct NetworkSettings {
    /// Display name.
    pub name: String,
    /// Transport parameters.
    pub conn: ConnConfig,
    /// Channels to auto-join once registered.
    pub auto_join: Vec<String>,
    nick: String,
    user: String,
    realname: String,
    sasl_account: Option<String>,
    sasl_mech: SaslMech,
    password: Option<String>,
}

impl NetworkSettings {
    /// The nick this network registers with.
    pub fn nick(&self) -> &str {
        &self.nick
    }

    /// Build a fresh `BringupConfig` for a connection attempt (new SCRAM nonce
    /// and mechanism instances each time).
    pub fn bringup(&self) -> BringupConfig {
        let mut sasl: Vec<Box<dyn Mechanism>> = Vec::new();
        if let (Some(account), Some(password)) = (&self.sasl_account, &self.password) {
            let mechanism: Box<dyn Mechanism> = match self.sasl_mech {
                SaslMech::Plain => Box::new(Plain::new(account.clone(), password.clone())),
                SaslMech::Scram => Box::new(ScramSha256::new(
                    account.clone(),
                    password.clone(),
                    random_nonce(),
                )),
            };
            sasl.push(mechanism);
        }
        BringupConfig {
            nick: self.nick.clone(),
            user: self.user.clone(),
            realname: self.realname.clone(),
            cap_groups: recommended_caps(),
            sasl,
            tls: self.conn.tls,
            sasl_fail_policy: SaslFailPolicy::Continue,
        }
    }
}

impl NetworkConfig {
    /// Resolve into connect-ready settings, running the password command (or
    /// reading NORN_PASSWORD) once now.
    pub fn resolve(&self) -> io::Result<NetworkSettings> {
        let password = if self.sasl_account.is_some() {
            resolve_password(self.password_command.as_deref())
        } else {
            None
        };
        Ok(NetworkSettings {
            name: self.name.clone(),
            conn: ConnConfig {
                host: self.host.clone(),
                port: self.port,
                tls: self.tls,
            },
            auto_join: self.auto_join.clone(),
            nick: self.nick.clone(),
            user: self.user.clone().unwrap_or_else(|| self.nick.clone()),
            realname: self.realname.clone().unwrap_or_else(|| self.nick.clone()),
            sasl_account: self.sasl_account.clone(),
            sasl_mech: self.sasl_mech,
            password,
        })
    }
}

/// Resolve a password from a command's stdout, else the `NORN_PASSWORD` env var.
fn resolve_password(command: Option<&str>) -> Option<String> {
    if let Some(cmd) = command {
        match ProcCommand::new("sh").arg("-c").arg(cmd).output() {
            Ok(out) if out.status.success() => {
                let pw = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !pw.is_empty() {
                    return Some(pw);
                }
                eprintln!("warning: password_command produced no output");
            }
            Ok(_) => eprintln!("warning: password_command failed: {cmd}"),
            Err(e) => eprintln!("warning: could not run password_command: {e}"),
        }
    }
    std::env::var(PASSWORD_ENV).ok().filter(|p| !p.is_empty())
}

/// The default config path, `~/.config/norn/config.toml`.
fn default_config_path() -> Option<PathBuf> {
    ProjectDirs::from("", "", "norn").map(|d| d.config_dir().join("config.toml"))
}

impl Cli {
    /// Synthesize a single ad-hoc `NetworkConfig` from the CLI flags, if
    /// `--server` and `--nick` were given.
    fn ad_hoc_network(&self) -> Option<NetworkConfig> {
        let (host, nick) = (self.server.clone()?, self.nick.clone()?);
        Some(NetworkConfig {
            name: host.clone(),
            host,
            port: self.port,
            tls: !self.no_tls,
            nick,
            user: self.user.clone(),
            realname: self.realname.clone(),
            sasl_account: self.sasl_account.clone(),
            sasl_mech: self.sasl_mech,
            password_command: None,
            auto_join: self.join.clone(),
        })
    }
}

/// Load and resolve all networks from the config file and/or CLI flags.
pub fn load_networks(cli: &Cli) -> io::Result<Vec<NetworkSettings>> {
    let mut configs: Vec<NetworkConfig> = Vec::new();

    let path = cli.config.clone().or_else(default_config_path);
    if let Some(path) = &path {
        if path.exists() {
            let text = std::fs::read_to_string(path)?;
            let config: Config = toml::from_str(&text)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            configs.extend(config.networks);
        }
    }

    if let Some(network) = cli.ad_hoc_network() {
        configs.push(network);
    }

    if configs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no networks configured: pass --server/--nick or create a config file \
             (~/.config/norn/config.toml)",
        ));
    }

    configs.iter().map(NetworkConfig::resolve).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonce_is_well_formed_and_varies() {
        let a = random_nonce();
        let b = random_nonce();
        assert_eq!(a.len(), 24);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(a, b, "two nonces should differ");
    }

    #[test]
    fn toml_parses_networks() {
        let toml = r##"
            [[network]]
            name = "libera"
            host = "irc.libera.chat"
            nick = "svan"
            sasl_account = "svan"
            sasl_mech = "scram"
            auto_join = ["#rust", "#ratatui"]

            [[network]]
            name = "local"
            host = "127.0.0.1"
            port = 6667
            tls = false
            nick = "tester"
        "##;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.networks.len(), 2);
        assert_eq!(config.networks[0].sasl_mech, SaslMech::Scram);
        assert_eq!(config.networks[0].port, 6697); // default
        assert!(config.networks[0].tls); // default
        assert_eq!(config.networks[1].port, 6667);
        assert!(!config.networks[1].tls);
        assert_eq!(config.networks[0].auto_join, vec!["#rust", "#ratatui"]);
    }

    #[test]
    fn resolve_without_sasl_has_no_password() {
        let net = NetworkConfig {
            name: "n".into(),
            host: "h".into(),
            port: 6667,
            tls: false,
            nick: "nick".into(),
            user: None,
            realname: None,
            sasl_account: None,
            sasl_mech: SaslMech::Plain,
            password_command: None,
            auto_join: vec![],
        };
        let settings = net.resolve().unwrap();
        assert_eq!(settings.nick(), "nick");
        assert!(settings.bringup().sasl.is_empty());
    }

    #[test]
    fn password_command_is_captured() {
        let net = NetworkConfig {
            name: "n".into(),
            host: "h".into(),
            port: 6697,
            tls: true,
            nick: "nick".into(),
            user: None,
            realname: None,
            sasl_account: Some("acct".into()),
            sasl_mech: SaslMech::Plain,
            password_command: Some("printf 'sekret'".into()),
            auto_join: vec![],
        };
        let settings = net.resolve().unwrap();
        assert_eq!(settings.password.as_deref(), Some("sekret"));
        assert_eq!(settings.bringup().sasl.len(), 1);
    }
}
