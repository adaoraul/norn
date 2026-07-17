//! `Message`: the universal currency above framing.
//!
//! A wire line has the shape `[@tags] [:source] command [params...] [:trailing]`.
//! Parsing peels those pieces off left to right; tag unescaping happens once,
//! inside the tag codec (see [`crate::tags`]). Emit is the inverse. Structural
//! equality (`parse(emit(m)) == m`) is what round-trips, not the exact bytes,
//! since tag order and the trailing-colon choice are not significant.

use chrono::{DateTime, Utc};

use crate::tags::Tags;

/// A parsed IRC message. Every layer above framing produces or consumes one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Parsed, unescaped tags.
    pub tags: Tags,
    /// Message source, if a `:prefix` was present.
    pub source: Option<Source>,
    /// The command verb or numeric.
    pub command: Command,
    /// Positional params with the trailing param already merged in.
    pub params: Vec<String>,
}

/// The origin of a message: a server name or a user prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A bare server name (no `!`/`@` in the prefix).
    Server(String),
    /// A user prefix `nick[!user][@host]`.
    User {
        /// The nickname.
        nick: String,
        /// The user/ident, if present.
        user: Option<String>,
        /// The host, if present.
        host: Option<String>,
    },
}

/// A command: either a three-digit numeric reply or a named verb.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// A numeric reply such as `001` or `903`.
    Numeric(u16),
    /// A named command such as `PRIVMSG`, `CAP`, or `BATCH`.
    Named(String),
}

/// Failure modes of [`Message::parse`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// The line had tags and/or a source but no command token.
    #[error("message had no command")]
    MissingCommand,
}

impl Message {
    /// Parse a single line (CRLF framing already done; a trailing CR/LF is
    /// tolerated). Tag values are unescaped as part of parsing.
    pub fn parse(line: &str) -> Result<Message, ParseError> {
        let mut rest = line.trim_end_matches(['\r', '\n']);

        let tags = if let Some(after) = rest.strip_prefix('@') {
            let (section, remainder) = split_token(after);
            rest = remainder;
            Tags::parse(section)
        } else {
            Tags::new()
        };

        let source = if let Some(after) = rest.strip_prefix(':') {
            let (token, remainder) = split_token(after);
            rest = remainder;
            Some(parse_source(token))
        } else {
            None
        };

        let (command_token, remainder) = split_token(rest);
        if command_token.is_empty() {
            return Err(ParseError::MissingCommand);
        }
        let command = parse_command(command_token);
        let params = parse_params(remainder);

        Ok(Message {
            tags,
            source,
            command,
            params,
        })
    }

    /// Render the message back to a wire line (without the trailing CRLF).
    pub fn emit(&self) -> String {
        let mut out = String::new();

        if !self.tags.is_empty() {
            out.push('@');
            out.push_str(&self.tags.emit());
            out.push(' ');
        }

        if let Some(source) = &self.source {
            out.push(':');
            out.push_str(&source.emit());
            out.push(' ');
        }

        out.push_str(&self.command.emit());

        let last = self.params.len().wrapping_sub(1);
        for (i, param) in self.params.iter().enumerate() {
            out.push(' ');
            // The trailing param carries a `:` when it is empty, holds a space,
            // or itself starts with `:`, since only then does it need it.
            if i == last && (param.is_empty() || param.contains(' ') || param.starts_with(':')) {
                out.push(':');
            }
            out.push_str(param);
        }

        out
    }

    /// The `time` tag parsed as a UTC timestamp (rule 13: prefer this over
    /// local receipt time when rendering history).
    pub fn server_time(&self) -> Option<DateTime<Utc>> {
        let raw = self.tags.get("time")?;
        DateTime::parse_from_rfc3339(raw)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    }

    /// The `msgid` tag, if present.
    pub fn msgid(&self) -> Option<&str> {
        self.tags.get("msgid")
    }

    /// The `account` tag, if present.
    pub fn account(&self) -> Option<&str> {
        self.tags.get("account")
    }

    /// The `batch` tag (the ref this line belongs to), if present.
    pub fn batch_ref(&self) -> Option<&str> {
        self.tags.get("batch")
    }

    /// The `label` tag (labeled-response id), if present.
    pub fn label(&self) -> Option<&str> {
        self.tags.get("label")
    }
}

impl Source {
    /// Render the source prefix (without the leading `:`).
    fn emit(&self) -> String {
        match self {
            Source::Server(name) => name.clone(),
            Source::User { nick, user, host } => {
                let mut out = nick.clone();
                if let Some(user) = user {
                    out.push('!');
                    out.push_str(user);
                }
                if let Some(host) = host {
                    out.push('@');
                    out.push_str(host);
                }
                out
            }
        }
    }
}

impl Command {
    /// Render the command as it appears on the wire (numerics zero-padded to
    /// three digits).
    fn emit(&self) -> String {
        match self {
            Command::Numeric(n) => format!("{n:03}"),
            Command::Named(name) => name.clone(),
        }
    }
}

/// Split at the first space: `(token, remainder_with_leading_spaces_trimmed)`.
fn split_token(s: &str) -> (&str, &str) {
    match s.find(' ') {
        Some(i) => (&s[..i], s[i + 1..].trim_start_matches(' ')),
        None => (s, ""),
    }
}

/// A source prefix carries a user when it contains `!` or `@`; otherwise it is
/// a bare server name.
fn parse_source(token: &str) -> Source {
    if let Some((nick, rest)) = token.split_once('!') {
        let (user, host) = match rest.split_once('@') {
            Some((u, h)) => (Some(u.to_string()), Some(h.to_string())),
            None => (Some(rest.to_string()), None),
        };
        Source::User {
            nick: nick.to_string(),
            user,
            host,
        }
    } else if let Some((nick, host)) = token.split_once('@') {
        Source::User {
            nick: nick.to_string(),
            user: None,
            host: Some(host.to_string()),
        }
    } else {
        Source::Server(token.to_string())
    }
}

/// A token of exactly three ASCII digits is a numeric; anything else is named.
fn parse_command(token: &str) -> Command {
    if token.len() == 3 && token.bytes().all(|b| b.is_ascii_digit()) {
        // Three ASCII digits always fit in u16.
        Command::Numeric(token.parse().expect("three ascii digits fit in u16"))
    } else {
        Command::Named(token.to_string())
    }
}

/// Parse the param list. A param beginning with `:` is the trailing param and
/// consumes the rest of the line verbatim, spaces included.
fn parse_params(mut s: &str) -> Vec<String> {
    let mut params = Vec::new();
    loop {
        s = s.trim_start_matches(' ');
        if s.is_empty() {
            break;
        }
        if let Some(trailing) = s.strip_prefix(':') {
            params.push(trailing.to_string());
            break;
        }
        let (token, remainder) = split_token(s);
        params.push(token.to_string());
        s = remainder;
    }
    params
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_privmsg_with_tags_source_and_trailing() {
        let m = Message::parse(
            "@time=2026-07-17T12:34:56.789Z;msgid=abc :nick!u@h PRIVMSG #rust :hi there",
        )
        .unwrap();

        assert_eq!(m.tags.len(), 2);
        assert_eq!(m.tags.get("time"), Some("2026-07-17T12:34:56.789Z"));
        assert_eq!(m.tags.get("msgid"), Some("abc"));
        assert_eq!(
            m.source,
            Some(Source::User {
                nick: "nick".to_string(),
                user: Some("u".to_string()),
                host: Some("h".to_string()),
            })
        );
        assert_eq!(m.command, Command::Named("PRIVMSG".to_string()));
        // Trailing param keeps its internal spaces.
        assert_eq!(m.params, vec!["#rust".to_string(), "hi there".to_string()]);
    }

    #[test]
    fn parse_bare_tag_key_has_empty_value() {
        let m = Message::parse("@draft/foo :server NOTICE * :x").unwrap();
        assert_eq!(m.tags.get("draft/foo"), Some(""));
        assert_eq!(m.source, Some(Source::Server("server".to_string())));
        assert_eq!(m.command, Command::Named("NOTICE".to_string()));
        assert_eq!(m.params, vec!["*".to_string(), "x".to_string()]);
    }

    #[test]
    fn parse_no_tags_no_source() {
        let m = Message::parse("PING :token").unwrap();
        assert!(m.tags.is_empty());
        assert!(m.source.is_none());
        assert_eq!(m.command, Command::Named("PING".to_string()));
        assert_eq!(m.params, vec!["token".to_string()]);
    }

    #[test]
    fn semicolon_inside_tag_value_survives_framing() {
        // Proves 1.1 unescaping is applied during parse, not after: the `\:`
        // becomes `;` and does NOT split the tag.
        let m = Message::parse("@k=a\\:b :s PRIVMSG #c :y").unwrap();
        assert_eq!(m.tags.len(), 1);
        assert_eq!(m.tags.get("k"), Some("a;b"));
    }

    #[test]
    fn emit_roundtrips_structurally() {
        let corpus = [
            "@time=2026-07-17T12:34:56.789Z;msgid=abc :nick!u@h PRIVMSG #rust :hi there",
            "@draft/foo :server NOTICE * :x",
            "PING :token",
            "@k=a\\:b :s PRIVMSG #c :y",
            ":irc.example.net 001 adao :Welcome to the network",
            "@batch=hist :nick!u@h PRIVMSG #rust :older line",
            "CAP * LS :multi-prefix sasl server-time",
            ":nick@host QUIT :bye",
        ];
        for line in corpus {
            let m = Message::parse(line).unwrap();
            let reparsed = Message::parse(&m.emit()).unwrap();
            assert_eq!(m, reparsed, "roundtrip failed for: {line}");
        }
    }

    #[test]
    fn large_tag_section_parses_without_truncation() {
        // Tag section well over the 512-byte core limit but under 8191 (rule 2):
        // the parser must not assume a 512-byte total.
        let big = "x".repeat(600);
        let line = format!("@bigtag={big} :s PRIVMSG #c :hi");
        let m = Message::parse(&line).unwrap();
        assert_eq!(m.tags.get("bigtag"), Some(big.as_str()));
        assert_eq!(m.params, vec!["#c".to_string(), "hi".to_string()]);
    }

    #[test]
    fn numeric_command_roundtrips_zero_padded() {
        let m = Message::parse(":s 001 adao :Welcome").unwrap();
        assert_eq!(m.command, Command::Numeric(1));
        assert!(m.emit().contains(" 001 "));
    }

    #[test]
    fn server_time_parses_rfc3339() {
        let m = Message::parse("@time=2026-07-17T12:34:56.789Z :s PRIVMSG #c :hi").unwrap();
        let t = m.server_time().unwrap();
        assert_eq!(
            t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "2026-07-17T12:34:56.789Z"
        );
    }

    #[test]
    fn missing_command_is_an_error() {
        assert_eq!(
            Message::parse("@only=tags"),
            Err(ParseError::MissingCommand)
        );
    }
}
