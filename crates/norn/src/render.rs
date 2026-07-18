//! Format engine events as plain text lines.
//!
//! Used by the `--plain` renderer. Each event maps to zero or more lines; the
//! caller prefixes them with the network name and prints them. The TUI consumes
//! the same `Event`s directly instead.

use irc_engine::{ChatMessage, Event, LeaveReason, MessageKind, TopicChange};
use irc_proto::Source;

/// Format one event as zero or more display lines.
pub fn render(event: &Event) -> Vec<String> {
    match event {
        Event::Registered { nick } => vec![format!("-- registered as {nick}")],
        Event::CapabilitiesChanged { enabled, .. } => {
            vec![format!("-- capabilities: {} enabled", enabled.len())]
        }
        Event::AuthResult(Ok(account)) => vec![format!("-- authenticated as {account}")],
        Event::AuthResult(Err(err)) => vec![format!("-- authentication failed: {err}")],
        Event::MessageReceived(msg) => vec![chat_line(msg)],
        Event::HistoryLoaded {
            target,
            messages,
            complete,
        } => {
            let mut lines = vec![format!(
                "-- history for {target}: {} message(s){}",
                messages.len(),
                if *complete { "" } else { " (more available)" }
            )];
            lines.extend(messages.iter().map(chat_line));
            lines
        }
        Event::BatchCollapsed(batch) => {
            vec![format!(
                "-- {} batch ({} items)",
                batch.batch_type,
                batch.len()
            )]
        }
        Event::NamesLoaded { target, members } => {
            vec![format!("-- {} has {} member(s)", target, members.len())]
        }
        Event::TopicChanged {
            target,
            change,
            set_by,
            ..
        } => match (change, set_by) {
            (TopicChange::Set(topic), _) => vec![format!("-- topic for {target}: {topic}")],
            (TopicChange::Unchanged, Some(by)) => {
                vec![format!("-- topic for {target} set by {by}")]
            }
            (TopicChange::Unchanged, None) => vec![],
            (TopicChange::Cleared, _) => vec![format!("-- {target} has no topic")],
        },
        Event::MemberJoined {
            target,
            who,
            account,
        } => match account {
            Some(account) => vec![format!("-- {} ({account}) joined {target}", who.nick)],
            None => vec![format!("-- {} joined {target}", who.nick)],
        },
        Event::MemberLeft {
            target,
            who,
            reason,
        } => match reason {
            LeaveReason::Part(reason) => vec![format!("-- {} left {target} ({reason})", who.nick)],
            LeaveReason::Quit(reason) => vec![format!("-- {} quit ({reason})", who.nick)],
            LeaveReason::Kicked { by, reason } => {
                vec![format!(
                    "-- {} was kicked from {target} by {by} ({reason})",
                    who.nick
                )]
            }
        },
        Event::NickChanged { old, new } => vec![format!("-- {old} is now known as {new}")],
        Event::AccountChanged { nick, account } => match account {
            Some(account) => vec![format!("-- {nick} logged in as {account}")],
            None => vec![format!("-- {nick} logged out")],
        },
        Event::HostChanged { nick, user, host } => {
            vec![format!("-- {nick} changed host to {user}@{host}")]
        }
        Event::AwayChanged { nick, message } => match message {
            Some(message) => vec![format!("-- {nick} is away ({message})")],
            None => vec![format!("-- {nick} is back")],
        },
        Event::RealnameChanged { nick, realname } => {
            vec![format!("-- {nick} set realname to {realname}")]
        }
        Event::WhoisReceived(info) if info.not_found => {
            vec![format!("-- no such nick: {}", info.nick)]
        }
        Event::WhoisReceived(info) => {
            let mut lines = match (&info.user, &info.host) {
                (Some(user), Some(host)) => vec![format!("-- {} is {user}@{host}", info.nick)],
                _ => vec![format!("-- whois {}", info.nick)],
            };
            if let Some(realname) = &info.realname {
                lines.push(format!("--   realname: {realname}"));
            }
            if let Some(account) = &info.account {
                lines.push(format!("--   account: {account}"));
            }
            if let Some(server) = &info.server {
                lines.push(format!("--   server: {server}"));
            }
            if let Some(channels) = &info.channels {
                lines.push(format!("--   channels: {channels}"));
            }
            if info.is_operator {
                lines.push("--   is an IRC operator".to_string());
            }
            if info.secure {
                lines.push("--   using a secure connection".to_string());
            }
            if let Some(away) = &info.away {
                lines.push(format!("--   away: {away}"));
            }
            lines
        }
        Event::StandardReply(reply) => vec![format!(
            "-- {:?} {} {}: {}",
            reply.kind, reply.command, reply.code, reply.description
        )],
        Event::Disconnected(reason) => vec![format!("-- disconnected: {reason:?}")],
    }
}

fn chat_line(msg: &ChatMessage) -> String {
    let who = msg.sender.as_ref().map(source_nick).unwrap_or("?");
    match msg.kind {
        MessageKind::Privmsg => format!("[{}] <{}> {}", msg.target, who, msg.text),
        MessageKind::Notice => format!("[{}] -{}- {}", msg.target, who, msg.text),
    }
}

fn source_nick(source: &Source) -> &str {
    match source {
        Source::User { nick, .. } => nick,
        Source::Server(name) => name,
    }
}
