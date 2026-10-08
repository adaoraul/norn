//! Standard replies: `FAIL` / `WARN` / `NOTE` into one structured event.
//!
//! Rule 15: route all three into a single machine-readable shape rather than
//! scattering bespoke error handling. The wire form is
//! `<FAIL|WARN|NOTE> <command> <code> [context...] :<description>`.

use irc_proto::{Command, Message};

/// Which standard-reply severity this is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyKind {
    /// `FAIL`: the command failed.
    Fail,
    /// `WARN`: advisory, the command may still have succeeded.
    Warn,
    /// `NOTE`: informational.
    Note,
}

impl ReplyKind {
    /// The wire keyword, for display.
    pub fn label(&self) -> &'static str {
        match self {
            ReplyKind::Fail => "FAIL",
            ReplyKind::Warn => "WARN",
            ReplyKind::Note => "NOTE",
        }
    }
}

/// A parsed standard reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardReply {
    /// Severity.
    pub kind: ReplyKind,
    /// The command it concerns (e.g. `CHATHISTORY`), or `*` if general.
    pub command: String,
    /// The machine-readable code (e.g. `INVALID_PARAMS`).
    pub code: String,
    /// Any context params between the code and the description.
    pub context: Vec<String>,
    /// Human-readable description (the trailing param).
    pub description: String,
}

impl StandardReply {
    /// Parse a `FAIL`/`WARN`/`NOTE` message, or `None` for anything else.
    pub fn from_message(msg: &Message) -> Option<StandardReply> {
        let kind = match &msg.command {
            Command::Named(name) => match name.as_str() {
                "FAIL" => ReplyKind::Fail,
                "WARN" => ReplyKind::Warn,
                "NOTE" => ReplyKind::Note,
                _ => return None,
            },
            _ => return None,
        };

        let mut params = msg.params.iter();
        let command = params.next()?.clone();
        let code = params.next()?.clone();
        let rest: Vec<String> = params.cloned().collect();
        let (description, context) = match rest.split_last() {
            Some((last, ctx)) => (last.clone(), ctx.to_vec()),
            None => (String::new(), Vec::new()),
        };

        Some(StandardReply {
            kind,
            command,
            code,
            context,
            description,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fail_chathistory() {
        let msg =
            Message::parse("FAIL CHATHISTORY INVALID_PARAMS :Messages could not be retrieved")
                .unwrap();
        let sr = StandardReply::from_message(&msg).unwrap();
        assert_eq!(sr.kind, ReplyKind::Fail);
        assert_eq!(sr.command, "CHATHISTORY");
        assert_eq!(sr.code, "INVALID_PARAMS");
        assert!(sr.context.is_empty());
        assert_eq!(sr.description, "Messages could not be retrieved");
    }

    #[test]
    fn parses_context_params() {
        let msg = Message::parse("FAIL JOIN NEED_KEY #secret :Cannot join without a key").unwrap();
        let sr = StandardReply::from_message(&msg).unwrap();
        assert_eq!(sr.command, "JOIN");
        assert_eq!(sr.code, "NEED_KEY");
        assert_eq!(sr.context, vec!["#secret".to_string()]);
        assert_eq!(sr.description, "Cannot join without a key");
    }

    #[test]
    fn warn_and_note_kinds() {
        assert_eq!(
            StandardReply::from_message(&Message::parse("WARN * ACCOUNT_REQUIRED :soon").unwrap())
                .unwrap()
                .kind,
            ReplyKind::Warn
        );
        assert_eq!(
            StandardReply::from_message(&Message::parse("NOTE * FOO :bar").unwrap())
                .unwrap()
                .kind,
            ReplyKind::Note
        );
    }

    #[test]
    fn non_reply_is_ignored() {
        assert!(StandardReply::from_message(&Message::parse("PING :x").unwrap()).is_none());
    }
}
