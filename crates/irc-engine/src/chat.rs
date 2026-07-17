//! `ChatMessage`: a PRIVMSG/NOTICE lifted into a semantic form.
//!
//! It carries the original `server-time` and `msgid` (rule 13), so replayed
//! history renders with its original timestamp, not the reconnect time. This is
//! the payload of `MessageReceived` and of each entry in `HistoryLoaded`.

use chrono::{DateTime, SecondsFormat, Utc};
use irc_proto::{Command, Message, Source};

use crate::history::Selector;

/// Whether a chat line is a message or a notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageKind {
    /// `PRIVMSG`.
    Privmsg,
    /// `NOTICE`.
    Notice,
}

/// A chat message with its original metadata preserved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    /// The `server-time` tag, parsed (rule 13). `None` if absent.
    pub time: Option<DateTime<Utc>>,
    /// The `msgid` tag, if present.
    pub msgid: Option<String>,
    /// The `account` tag, if present.
    pub account: Option<String>,
    /// Who sent it.
    pub sender: Option<Source>,
    /// The channel or nick it was addressed to.
    pub target: String,
    /// The message text.
    pub text: String,
    /// PRIVMSG vs NOTICE.
    pub kind: MessageKind,
}

impl ChatMessage {
    /// Lift a PRIVMSG/NOTICE message into a `ChatMessage`. Returns `None` for
    /// any other command.
    pub fn from_message(msg: &Message) -> Option<ChatMessage> {
        let kind = match &msg.command {
            Command::Named(name) if name == "PRIVMSG" => MessageKind::Privmsg,
            Command::Named(name) if name == "NOTICE" => MessageKind::Notice,
            _ => return None,
        };
        let target = msg.params.first()?.clone();
        let text = msg.params.get(1).cloned().unwrap_or_default();
        Some(ChatMessage {
            time: msg.server_time(),
            msgid: msg.msgid().map(String::from),
            account: msg.account().map(String::from),
            sender: msg.source.clone(),
            target,
            text,
            kind,
        })
    }

    /// A pagination cursor for this message, preferring `msgid` (exact) over
    /// `timestamp` (millisecond-boundary ambiguous). `None` if neither is
    /// present.
    pub fn cursor(&self) -> Option<Selector> {
        if let Some(id) = &self.msgid {
            Some(Selector::msgid(id))
        } else {
            self.time
                .map(|t| Selector::timestamp(t.to_rfc3339_opts(SecondsFormat::Millis, true)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifts_privmsg_with_time_and_msgid() {
        let msg = Message::parse(
            "@time=2026-07-17T09:00:01.000Z;msgid=aaa :nick!u@h PRIVMSG #rust :hi there",
        )
        .unwrap();
        let cm = ChatMessage::from_message(&msg).unwrap();
        assert_eq!(cm.kind, MessageKind::Privmsg);
        assert_eq!(cm.target, "#rust");
        assert_eq!(cm.text, "hi there");
        assert_eq!(cm.msgid.as_deref(), Some("aaa"));
        assert_eq!(
            cm.time
                .unwrap()
                .to_rfc3339_opts(SecondsFormat::Millis, true),
            "2026-07-17T09:00:01.000Z"
        );
    }

    #[test]
    fn non_chat_command_is_not_lifted() {
        let msg = Message::parse(":s 001 adao :Welcome").unwrap();
        assert!(ChatMessage::from_message(&msg).is_none());
    }

    #[test]
    fn cursor_prefers_msgid_over_timestamp() {
        let with_id = ChatMessage::from_message(
            &Message::parse("@time=2026-07-17T09:00:01.000Z;msgid=aaa :n!u@h PRIVMSG #c :x")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(with_id.cursor(), Some(Selector::msgid("aaa")));

        let no_id = ChatMessage::from_message(
            &Message::parse("@time=2026-07-17T09:00:01.000Z :n!u@h PRIVMSG #c :x").unwrap(),
        )
        .unwrap();
        assert_eq!(
            no_id.cursor(),
            Some(Selector::timestamp("2026-07-17T09:00:01.000Z"))
        );
    }
}
