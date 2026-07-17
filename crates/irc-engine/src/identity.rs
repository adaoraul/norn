//! Identity and membership messages to semantic events.
//!
//! Maps the phase-2 identity caps' messages into events: JOIN (with
//! `extended-join` account), PART/QUIT/KICK, and the notifications `NICK`,
//! `ACCOUNT` (`account-notify`), `CHGHOST` (`chghost`), `AWAY` (`away-notify`),
//! and `SETNAME` (`setname`). The `account-tag` and `multi-prefix` caps enrich
//! other data rather than producing events of their own (account-tag lands on
//! `ChatMessage`; multi-prefix affects NAMES/WHO prefixes).

use irc_proto::{Command, Message, Source};

use crate::event::{Event, LeaveReason, User};

/// Map an identity/membership command to an event, or `None` if it is not one.
pub fn identity_event(msg: &Message) -> Option<Event> {
    let Command::Named(command) = &msg.command else {
        return None;
    };
    let source = msg.source.as_ref();

    match command.as_str() {
        "JOIN" => {
            let who = User::from_source(source?)?;
            let target = msg.params.first()?.clone();
            // extended-join: params are `<channel> <account> :<realname>`.
            let account = match msg.params.get(1) {
                Some(a) if a.as_str() != "*" => Some(a.clone()),
                _ => None,
            };
            Some(Event::MemberJoined {
                target,
                who,
                account,
            })
        }
        "PART" => {
            let who = User::from_source(source?)?;
            let target = msg.params.first()?.clone();
            let reason = msg.params.get(1).cloned().unwrap_or_default();
            Some(Event::MemberLeft {
                target,
                who,
                reason: LeaveReason::Part(reason),
            })
        }
        "QUIT" => {
            let who = User::from_source(source?)?;
            let reason = msg.params.first().cloned().unwrap_or_default();
            Some(Event::MemberLeft {
                target: String::new(),
                who,
                reason: LeaveReason::Quit(reason),
            })
        }
        "KICK" => {
            let by = source_nick(source?)?;
            let target = msg.params.first()?.clone();
            let kicked = msg.params.get(1)?.clone();
            let reason = msg.params.get(2).cloned().unwrap_or_default();
            Some(Event::MemberLeft {
                target,
                who: User::nick(kicked),
                reason: LeaveReason::Kicked { by, reason },
            })
        }
        "NICK" => {
            let old = source_nick(source?)?;
            let new = msg.params.first()?.clone();
            Some(Event::NickChanged { old, new })
        }
        "ACCOUNT" => {
            let nick = source_nick(source?)?;
            let account = match msg.params.first() {
                Some(a) if a.as_str() != "*" => Some(a.clone()),
                _ => None,
            };
            Some(Event::AccountChanged { nick, account })
        }
        "CHGHOST" => {
            let nick = source_nick(source?)?;
            let user = msg.params.first()?.clone();
            let host = msg.params.get(1)?.clone();
            Some(Event::HostChanged { nick, user, host })
        }
        "AWAY" => {
            let nick = source_nick(source?)?;
            // A trailing message means away; no param means back.
            let message = msg.params.first().cloned();
            Some(Event::AwayChanged { nick, message })
        }
        "SETNAME" => {
            let nick = source_nick(source?)?;
            let realname = msg.params.first()?.clone();
            Some(Event::RealnameChanged { nick, realname })
        }
        _ => None,
    }
}

fn source_nick(source: &Source) -> Option<String> {
    match source {
        Source::User { nick, .. } => Some(nick.clone()),
        Source::Server(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(line: &str) -> Event {
        identity_event(&Message::parse(line).unwrap()).expect("should map to an event")
    }

    #[test]
    fn extended_join_carries_the_account() {
        let e = ev(":nick!u@h JOIN #rust adao :Adao Raul");
        assert_eq!(
            e,
            Event::MemberJoined {
                target: "#rust".to_string(),
                who: User {
                    nick: "nick".to_string(),
                    user: Some("u".to_string()),
                    host: Some("h".to_string()),
                },
                account: Some("adao".to_string()),
            }
        );
    }

    #[test]
    fn plain_join_and_logged_out_join_have_no_account() {
        assert!(matches!(
            ev(":nick!u@h JOIN #rust"),
            Event::MemberJoined { account: None, .. }
        ));
        // extended-join uses `*` for an unauthenticated user.
        assert!(matches!(
            ev(":nick!u@h JOIN #rust * :Real Name"),
            Event::MemberJoined { account: None, .. }
        ));
    }

    #[test]
    fn part_and_quit_and_kick() {
        assert!(matches!(
            ev(":n!u@h PART #rust :bye"),
            Event::MemberLeft { reason: LeaveReason::Part(r), .. } if r == "bye"
        ));
        assert!(matches!(
            ev(":n!u@h QUIT :Ping timeout"),
            Event::MemberLeft { target, reason: LeaveReason::Quit(_), .. } if target.is_empty()
        ));
        let kick = ev(":op!u@h KICK #rust baduser :spam");
        assert!(matches!(
            kick,
            Event::MemberLeft {
                who,
                reason: LeaveReason::Kicked { by, reason },
                ..
            } if who.nick == "baduser" && by == "op" && reason == "spam"
        ));
    }

    #[test]
    fn nick_change() {
        assert_eq!(
            ev(":old!u@h NICK new"),
            Event::NickChanged {
                old: "old".to_string(),
                new: "new".to_string()
            }
        );
    }

    #[test]
    fn account_login_and_logout() {
        assert!(matches!(
            ev(":n!u@h ACCOUNT adao"),
            Event::AccountChanged { account: Some(a), .. } if a == "adao"
        ));
        assert!(matches!(
            ev(":n!u@h ACCOUNT *"),
            Event::AccountChanged { account: None, .. }
        ));
    }

    #[test]
    fn chghost_away_and_setname() {
        assert_eq!(
            ev(":n!u@h CHGHOST newuser new.host"),
            Event::HostChanged {
                nick: "n".to_string(),
                user: "newuser".to_string(),
                host: "new.host".to_string(),
            }
        );
        assert!(matches!(
            ev(":n!u@h AWAY :lunch"),
            Event::AwayChanged { message: Some(m), .. } if m == "lunch"
        ));
        assert!(matches!(
            ev(":n!u@h AWAY"),
            Event::AwayChanged { message: None, .. }
        ));
        assert!(matches!(
            ev(":n!u@h SETNAME :New Real Name"),
            Event::RealnameChanged { realname, .. } if realname == "New Real Name"
        ));
    }

    #[test]
    fn non_identity_command_is_none() {
        assert!(identity_event(&Message::parse("PING :x").unwrap()).is_none());
        assert!(identity_event(&Message::parse(":s 001 nick :hi").unwrap()).is_none());
    }
}
