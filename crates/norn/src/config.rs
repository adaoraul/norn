//! Configuration: CLI args plus an optional multi-network TOML file.
//!
//! Networks come from a TOML config file (default `~/.config/norn/config.toml`,
//! or `--config <path>`); the CLI flags additionally define one ad-hoc network.
//! SASL passwords are never stored in the file: each network either names a
//! `password_command` whose stdout is the password, or falls back to the
//! `NORN_PASSWORD` environment variable.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command as ProcCommand;

use clap::{Parser, ValueEnum};
use directories::ProjectDirs;
use irc_engine::{recommended_caps, BringupConfig, SaslFailPolicy};
use irc_proto::{Mechanism, Plain, ScramSha256};
use rand::Rng;
use serde::{Deserialize, Serialize};

/// Environment variable holding a fallback SASL password.
pub const PASSWORD_ENV: &str = "NORN_PASSWORD";

/// Which SASL mechanism to authenticate with.
#[derive(ValueEnum, Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
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
fn default_theme() -> String {
    "teal".to_string()
}
fn default_completion_char() -> String {
    ":".to_string()
}
fn default_scrollback() -> usize {
    5000
}

fn default_idle_secs() -> usize {
    300
}

/// A network as declared in the TOML file (or synthesized from CLI flags).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Realname (defaults to the nick).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realname: Option<String>,
    /// SASL account, if authenticating.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sasl_account: Option<String>,
    /// SASL mechanism.
    #[serde(default)]
    pub sasl_mech: SaslMech,
    /// Shell command whose stdout is the SASL password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_command: Option<String>,
    /// Channels to auto-join.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auto_join: Vec<String>,
    /// Whether to connect this network automatically on launch. When false, it
    /// stays defined but idle until `/connect` (or the networks manager).
    #[serde(default = "default_true")]
    pub auto_connect: bool,
    /// Whether to auto-identify to NickServ when prompted, using the same
    /// resolved password as SASL. A fallback: dormant when SASL already logged in.
    #[serde(default, skip_serializing_if = "is_false")]
    pub identify: bool,
}

/// serde `skip_serializing_if` helper for a `false` bool.
fn is_false(b: &bool) -> bool {
    !*b
}

/// Client-wide UI preferences (the TOML `[client]` table). The interactive
/// `/settings` screen and `/set` command both edit these through the settings
/// registry (`crate::settings`).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ClientConfig {
    /// Whether to show message timestamps.
    #[serde(default = "default_true")]
    pub timestamps: bool,
    /// Whether to color nicks by a per-nick hue (off = a single muted color).
    #[serde(default = "default_true")]
    pub nick_colors: bool,
    /// Accent theme name (see `tui::theme::accent_for`).
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Whether the channel nicklist is shown by default.
    #[serde(default = "default_true")]
    pub nicklist: bool,
    /// The character inserted after a nick completed at the start of a line
    /// (followed by a space), e.g. `":"` -> `nick: `.
    #[serde(default = "default_completion_char")]
    pub completion_char: String,
    /// Whether to ring the terminal bell when a message highlights your nick.
    #[serde(default)]
    pub beep_on_highlight: bool,
    /// Maximum number of lines kept per buffer.
    #[serde(default = "default_scrollback")]
    pub scrollback_lines: usize,
    /// Seconds of keyboard inactivity before plugins get an `on_idle` event
    /// (0 disables idle tracking).
    #[serde(default = "default_idle_secs")]
    pub idle_secs: usize,
    /// Whether norn captures the mouse (click to switch buffers, wheel to
    /// scroll). Off hands the mouse back to the terminal for plain text
    /// selection.
    #[serde(default = "default_true")]
    pub mouse: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            timestamps: true,
            nick_colors: true,
            theme: default_theme(),
            nicklist: true,
            completion_char: default_completion_char(),
            beep_on_highlight: false,
            scrollback_lines: default_scrollback(),
            idle_secs: default_idle_secs(),
            mouse: true,
        }
    }
}

/// A declarative addon trigger (TOML `[[trigger]]`): run a command in response
/// to an event. `on` is a match spec (`"highlight"`, `"join #norn"`, ...) and
/// `run` a command template with `$nick`/`$chan`/`$msg`/`$me` variables.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TriggerConfig {
    /// The event match spec.
    pub on: String,
    /// The command template to run.
    pub run: String,
    /// Whether the trigger is active.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// The whole config file.
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Config {
    /// Client-wide UI preferences.
    #[serde(default)]
    pub client: ClientConfig,
    /// User-defined command aliases (TOML `[aliases]`), name -> expansion.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub aliases: BTreeMap<String, String>,
    /// Declared networks (TOML `[[network]]` or `[[networks]]`).
    #[serde(default, alias = "network", skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<NetworkConfig>,
    /// Declarative addon triggers (TOML `[[trigger]]`).
    #[serde(default, alias = "trigger", skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<TriggerConfig>,
    /// Addon script filenames that are installed but disabled (not loaded).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_plugins: Vec<String>,
    /// Per-plugin config overrides (TOML `[plugin_config.<file>]`): plugin
    /// filename -> key -> value. Only values differing from the plugin's declared
    /// default are stored.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub plugin_config: BTreeMap<String, BTreeMap<String, String>>,
}

/// Header prepended to a saved config (auto-save rewrites the file, so any
/// hand-written comments are lost; this reminds the user where guidance lives).
const SAVE_HEADER: &str = "\
# norn configuration (auto-generated).
# Edit in-app with /set and /network, or by hand while norn is not running.
# Passwords are never stored here: use `password_command` or NORN_PASSWORD.

";

impl Config {
    /// Serialize and write the config to `path`, creating parent dirs. Only
    /// `password_command` is ever written for a network, never a password.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let body = toml::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, format!("{SAVE_HEADER}{body}"))
    }
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
    identify: bool,
}

impl NetworkSettings {
    /// The nick this network registers with.
    pub fn nick(&self) -> &str {
        &self.nick
    }

    /// The NickServ IDENTIFY line to send when prompted, or `None` when
    /// auto-identify is off or no password is available. The password is used to
    /// build the line but is never returned above this type (so it never reaches
    /// scripts or the UI).
    pub fn identify_line(&self) -> Option<String> {
        if !self.identify {
            return None;
        }
        let password = self.password.as_ref()?;
        Some(match &self.sasl_account {
            Some(account) => format!("PRIVMSG NickServ :IDENTIFY {account} {password}"),
            None => format!("PRIVMSG NickServ :IDENTIFY {password}"),
        })
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
    /// Resolve into connect-ready settings at startup, before the TUI owns the
    /// terminal, so a `password_command` that prompts (a GPG pinentry, say) can
    /// still use it. Warnings go to stderr.
    pub fn resolve(&self) -> io::Result<NetworkSettings> {
        let (settings, warnings) = self.resolve_with_warnings()?;
        for warning in warnings {
            eprintln!("warning: {warning}");
        }
        Ok(settings)
    }

    /// Resolve into connect-ready settings, running the password command (or
    /// reading NORN_PASSWORD) once now. Problems come back as warnings instead
    /// of being printed, so a caller inside the TUI can show them properly. This
    /// blocks while the command runs: call it from `spawn_blocking`.
    pub fn resolve_with_warnings(&self) -> io::Result<(NetworkSettings, Vec<String>)> {
        // Resolve the password for SASL and/or NickServ auto-identify.
        let (password, warnings) = if self.sasl_account.is_some() || self.identify {
            resolve_password(self.password_command.as_deref())
        } else {
            (None, Vec::new())
        };
        let settings = NetworkSettings {
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
            identify: self.identify,
        };
        Ok((settings, warnings))
    }
}

/// Resolve a password from a command's stdout, else the `NORN_PASSWORD` env var.
/// Returns the password and any warnings. A warning never contains the command
/// line or anything it printed: either could be, or hold, the secret.
fn resolve_password(command: Option<&str>) -> (Option<String>, Vec<String>) {
    let mut warnings = Vec::new();
    if let Some(cmd) = command {
        match ProcCommand::new("sh").arg("-c").arg(cmd).output() {
            Ok(out) if out.status.success() => {
                let pw = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !pw.is_empty() {
                    return (Some(pw), warnings);
                }
                warnings.push("password_command produced no output".to_string());
            }
            Ok(out) => warnings.push(match out.status.code() {
                Some(code) => format!("password_command failed (exit status {code})"),
                None => "password_command was stopped by a signal".to_string(),
            }),
            Err(e) => warnings.push(format!("could not run password_command: {e}")),
        }
    }
    let from_env = std::env::var(PASSWORD_ENV).ok().filter(|p| !p.is_empty());
    (from_env, warnings)
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
            auto_connect: true,
            identify: false,
        })
    }
}

/// Everything resolved at startup: what to connect now, the persisted network
/// definitions, client preferences, and where to auto-save.
pub struct Startup {
    /// Networks to dial at launch (file networks plus any CLI ad-hoc one),
    /// resolved and ready to connect.
    pub connect: Vec<NetworkSettings>,
    /// The persisted network definitions (file only; the CLI ad-hoc network is
    /// connected but never written back). Authoritative list for in-app editing.
    pub definitions: Vec<NetworkConfig>,
    /// Client UI preferences.
    pub client: ClientConfig,
    /// User-defined command aliases.
    pub aliases: BTreeMap<String, String>,
    /// Declarative addon triggers.
    pub triggers: Vec<TriggerConfig>,
    /// Disabled addon script filenames.
    pub disabled_plugins: Vec<String>,
    /// Per-plugin config overrides (filename -> key -> value).
    pub plugin_config: BTreeMap<String, BTreeMap<String, String>>,
    /// The config file to auto-save to (`None` if no config dir is available).
    pub path: Option<PathBuf>,
}

/// Resolve startup configuration from the config file and/or CLI flags. Unlike
/// before, this never exits early: with no config and no CLI network it returns
/// an empty `connect`/`definitions`, and the TUI launches into its status
/// console where the user configures networks in-app (irssi-style).
pub fn resolve_startup(cli: &Cli) -> io::Result<Startup> {
    let path = cli.config.clone().or_else(default_config_path);
    let mut client = ClientConfig::default();
    let mut aliases = BTreeMap::new();
    let mut definitions: Vec<NetworkConfig> = Vec::new();
    let mut triggers: Vec<TriggerConfig> = Vec::new();
    let mut disabled_plugins: Vec<String> = Vec::new();
    let mut plugin_config: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();

    if let Some(path) = &path {
        if path.exists() {
            let text = std::fs::read_to_string(path)?;
            let config: Config = toml::from_str(&text)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            client = config.client;
            aliases = config.aliases;
            definitions = config.networks;
            triggers = config.triggers;
            disabled_plugins = config.disabled_plugins;
            plugin_config = config.plugin_config;
        }
    }

    // Only auto-connect networks dial at launch; the rest stay defined but idle
    // (connectable in-app). The CLI ad-hoc network always connects.
    let mut to_connect: Vec<NetworkConfig> = definitions
        .iter()
        .filter(|n| n.auto_connect)
        .cloned()
        .collect();
    if let Some(network) = cli.ad_hoc_network() {
        to_connect.push(network);
    }
    let connect = to_connect
        .iter()
        .map(NetworkConfig::resolve)
        .collect::<io::Result<Vec<_>>>()?;

    Ok(Startup {
        connect,
        definitions,
        client,
        aliases,
        triggers,
        disabled_plugins,
        plugin_config,
        path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_through_save_serialization() {
        let mut aliases = BTreeMap::new();
        aliases.insert("j".to_string(), "join $1".to_string());
        aliases.insert("exit".to_string(), "quit".to_string());
        let config = Config {
            client: ClientConfig {
                timestamps: false,
                theme: "amber".into(),
                nicklist: true,
                idle_secs: 120,
                ..ClientConfig::default()
            },
            aliases,
            networks: vec![NetworkConfig {
                name: "libera".into(),
                host: "irc.libera.chat".into(),
                port: 6697,
                tls: true,
                nick: "svan".into(),
                user: None,
                realname: None,
                sasl_account: Some("svan".into()),
                sasl_mech: SaslMech::Scram,
                password_command: Some("pass irc/libera".into()),
                auto_join: vec!["#rust".into()],
                auto_connect: true,
                identify: true,
            }],
            triggers: vec![TriggerConfig {
                on: "highlight".into(),
                run: "notify $nick: $msg".into(),
                enabled: true,
            }],
            disabled_plugins: vec!["weather.rhai".into()],
            plugin_config: {
                let mut m = BTreeMap::new();
                let mut keys = BTreeMap::new();
                keys.insert("TRUSTED".to_string(), "alice,bob".to_string());
                m.insert("autoop.rhai".to_string(), keys);
                m
            },
        };
        let text = toml::to_string_pretty(&config).unwrap();
        // Never leak a resolved password (there is no password field to leak),
        // and the password_command is preserved.
        assert!(!text.contains("password ="));
        assert!(text.contains("password_command"));
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.client.theme, "amber");
        assert!(!back.client.timestamps);
        assert_eq!(back.client.idle_secs, 120);
        assert_eq!(back.aliases.get("j").map(String::as_str), Some("join $1"));
        assert_eq!(back.aliases.get("exit").map(String::as_str), Some("quit"));
        assert_eq!(back.networks.len(), 1);
        assert_eq!(back.networks[0].sasl_mech, SaslMech::Scram);
        assert_eq!(back.networks[0].auto_join, vec!["#rust"]);
        assert!(back.networks[0].identify);
        assert_eq!(back.triggers.len(), 1);
        assert_eq!(back.triggers[0].on, "highlight");
        assert_eq!(back.triggers[0].run, "notify $nick: $msg");
        assert_eq!(back.disabled_plugins, vec!["weather.rhai"]);
        assert_eq!(
            back.plugin_config
                .get("autoop.rhai")
                .and_then(|m| m.get("TRUSTED")),
            Some(&"alice,bob".to_string())
        );
    }

    #[test]
    fn resolve_startup_respects_auto_connect() {
        // Two defined networks; only the auto-connect one dials at launch.
        let path = std::env::temp_dir().join("norn-auto-connect-test.toml");
        std::fs::write(
            &path,
            r#"
            [[network]]
            name = "auto"
            host = "ha"
            nick = "n"

            [[network]]
            name = "manual"
            host = "hb"
            nick = "n"
            auto_connect = false
            "#,
        )
        .unwrap();
        let cli = Cli::parse_from(["norn", "--config", path.to_str().unwrap()]);
        let startup = resolve_startup(&cli).unwrap();
        assert_eq!(startup.definitions.len(), 2, "both stay defined");
        assert_eq!(startup.connect.len(), 1, "only auto-connect dials");
        assert_eq!(startup.connect[0].name, "auto");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn auto_connect_defaults_true() {
        let config: Config = toml::from_str(
            r#"
            [[network]]
            name = "a"
            host = "h"
            nick = "n"
            "#,
        )
        .unwrap();
        assert!(config.networks[0].auto_connect, "defaults to true");
    }

    #[test]
    fn client_config_defaults() {
        let config: Config = toml::from_str("").unwrap();
        assert!(config.client.timestamps);
        assert!(config.client.nicklist);
        assert!(config.client.nick_colors);
        assert_eq!(config.client.theme, "teal");
        assert_eq!(config.client.completion_char, ":");
        assert!(!config.client.beep_on_highlight);
        assert_eq!(config.client.scrollback_lines, 5000);
        assert!(config.networks.is_empty());
    }

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
            auto_connect: true,
            identify: false,
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
            auto_connect: true,
            identify: false,
        };
        let settings = net.resolve().unwrap();
        assert_eq!(settings.password.as_deref(), Some("sekret"));
        assert_eq!(settings.bringup().sasl.len(), 1);
    }

    fn net_with_command(cmd: &str) -> NetworkConfig {
        NetworkConfig {
            name: "n".into(),
            host: "h".into(),
            port: 6697,
            tls: true,
            nick: "nick".into(),
            user: None,
            realname: None,
            sasl_account: Some("acct".into()),
            sasl_mech: SaslMech::Plain,
            password_command: Some(cmd.into()),
            auto_join: vec![],
            auto_connect: true,
            identify: false,
        }
    }

    #[test]
    fn a_failing_password_command_warns_without_leaking_anything() {
        // The command line itself carries a "secret", and the command prints one
        // to both streams before failing: none of it may reach a warning.
        let net = net_with_command(
            "echo cmd-secret >/dev/null; echo out-secret; echo err-secret >&2; exit 3",
        );
        let (_, warnings) = net.resolve_with_warnings().unwrap();
        assert_eq!(warnings, vec!["password_command failed (exit status 3)"]);
        for leaked in ["cmd-secret", "out-secret", "err-secret"] {
            assert!(
                warnings.iter().all(|w| !w.contains(leaked)),
                "warning leaked {leaked}"
            );
        }
    }

    #[test]
    fn an_empty_password_command_result_warns() {
        let (_, warnings) = net_with_command("true").resolve_with_warnings().unwrap();
        assert_eq!(warnings, vec!["password_command produced no output"]);
    }

    #[test]
    fn a_working_password_command_has_no_warnings() {
        let (settings, warnings) = net_with_command("printf sekret")
            .resolve_with_warnings()
            .unwrap();
        assert!(warnings.is_empty());
        assert_eq!(settings.password.as_deref(), Some("sekret"));
    }

    #[test]
    fn identify_line_uses_password_only_when_enabled() {
        let base = NetworkConfig {
            name: "n".into(),
            host: "h".into(),
            port: 6697,
            tls: true,
            nick: "nick".into(),
            user: None,
            realname: None,
            sasl_account: Some("acct".into()),
            sasl_mech: SaslMech::Plain,
            password_command: Some("printf sekret".into()),
            auto_join: vec![],
            auto_connect: true,
            identify: true,
        };
        assert_eq!(
            base.resolve().unwrap().identify_line().as_deref(),
            Some("PRIVMSG NickServ :IDENTIFY acct sekret")
        );
        // No account -> identify the current nick.
        let no_account = NetworkConfig {
            sasl_account: None,
            ..base.clone()
        };
        assert_eq!(
            no_account.resolve().unwrap().identify_line().as_deref(),
            Some("PRIVMSG NickServ :IDENTIFY sekret")
        );
        // identify off -> no line even though a password exists.
        let off = NetworkConfig {
            identify: false,
            ..base.clone()
        };
        assert!(off.resolve().unwrap().identify_line().is_none());
    }
}
