//! Render engine events as plain lines on stdout.
//!
//! A deliberately simple text view for now: enough to watch a live session. The
//! ratatui UI will later consume the same `Event`s instead of printing.

use irc_engine::{ChatMessage, Event, LeaveReason, MessageKind};
use irc_proto::Source;

/// Print one event.
pub fn render(event: &Event) {
    match event {
        Event::Registered { nick } => println!("-- registered as {nick}"),
        Event::CapabilitiesChanged { enabled, .. } => {
            println!("-- capabilities: {} enabled", enabled.len());
        }
        Event::AuthResult(Ok(account)) => println!("-- authenticated as {account}"),
        Event::AuthResult(Err(err)) => println!("-- authentication failed: {err}"),
        Event::MessageReceived(msg) => print_chat(msg),
        Event::HistoryLoaded {
            target,
            messages,
            complete,
        } => {
            println!(
                "-- history for {target}: {} message(s){}",
                messages.len(),
                if *complete { "" } else { " (more available)" }
            );
            for msg in messages {
                print_chat(msg);
            }
        }
        Event::BatchCollapsed(batch) => {
            println!("-- {} batch ({} items)", batch.batch_type, batch.len());
        }
        Event::MemberJoined {
            target,
            who,
            account,
        } => match account {
            Some(account) => println!("-- {} ({account}) joined {target}", who.nick),
            None => println!("-- {} joined {target}", who.nick),
        },
        Event::MemberLeft {
            target,
            who,
            reason,
        } => match reason {
            LeaveReason::Part(reason) => println!("-- {} left {target} ({reason})", who.nick),
            LeaveReason::Quit(reason) => println!("-- {} quit ({reason})", who.nick),
            LeaveReason::Kicked { by, reason } => {
                println!(
                    "-- {} was kicked from {target} by {by} ({reason})",
                    who.nick
                )
            }
        },
        Event::NickChanged { old, new } => println!("-- {old} is now known as {new}"),
        Event::AccountChanged { nick, account } => match account {
            Some(account) => println!("-- {nick} logged in as {account}"),
            None => println!("-- {nick} logged out"),
        },
        Event::HostChanged { nick, user, host } => {
            println!("-- {nick} changed host to {user}@{host}")
        }
        Event::AwayChanged { nick, message } => match message {
            Some(message) => println!("-- {nick} is away ({message})"),
            None => println!("-- {nick} is back"),
        },
        Event::RealnameChanged { nick, realname } => {
            println!("-- {nick} set realname to {realname}")
        }
        Event::StandardReply(reply) => println!(
            "-- {:?} {} {}: {}",
            reply.kind, reply.command, reply.code, reply.description
        ),
        Event::Disconnected(reason) => println!("-- disconnected: {reason:?}"),
    }
}

fn print_chat(msg: &ChatMessage) {
    let who = msg.sender.as_ref().map(source_nick).unwrap_or("?");
    match msg.kind {
        MessageKind::Privmsg => println!("[{}] <{}> {}", msg.target, who, msg.text),
        MessageKind::Notice => println!("[{}] -{}- {}", msg.target, who, msg.text),
    }
}

fn source_nick(source: &Source) -> &str {
    match source {
        Source::User { nick, .. } => nick,
        Source::Server(name) => name,
    }
}
