//! Translate user input lines into outgoing IRC lines.
//!
//! Plain text is sent as a message to the current target. Lines starting with
//! `/` are commands. Errors and command feedback are returned (never printed),
//! so callers can show them in the active buffer rather than corrupting a raw
//! terminal.

/// The result of translating one input line.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Translated {
    /// Raw IRC lines to send.
    pub lines: Vec<String>,
    /// A local message to show the user (usage/error/help), not sent.
    pub feedback: Option<String>,
    /// Whether `feedback` reports a mistake (usage, unknown command) rather than
    /// plain information such as the help text.
    pub feedback_is_error: bool,
    /// Whether the user asked to quit.
    pub quit: bool,
    /// The reason given to `/quit`, if any.
    pub quit_reason: Option<String>,
    /// Chat the user is sending, so a client can echo it locally when the
    /// server does not (no `echo-message`).
    pub outgoing: Vec<Outgoing>,
}

/// One outgoing chat message, as the user typed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    /// The channel or nick it is sent to.
    pub target: String,
    /// The message text (for an action, without the CTCP wrapper).
    pub text: String,
    /// Whether it is a `/me` action.
    pub action: bool,
    /// Whether it is a NOTICE.
    pub notice: bool,
}

impl Translated {
    fn lines(lines: Vec<String>) -> Self {
        Translated {
            lines,
            ..Default::default()
        }
    }
    fn chat(line: String, target: &str, text: &str, action: bool, notice: bool) -> Self {
        Translated {
            lines: vec![line],
            outgoing: vec![Outgoing {
                target: target.to_string(),
                text: text.to_string(),
                action,
                notice,
            }],
            ..Default::default()
        }
    }
    fn feedback(msg: impl Into<String>) -> Self {
        Translated {
            feedback: Some(msg.into()),
            feedback_is_error: true,
            ..Default::default()
        }
    }
}

/// The commands [`translate`] understands. Plain mode has no client commands
/// (`/set`, `/network`, ...): those belong to the TUI.
const TRANSLATED: &[&str] = &[
    "join", "part", "msg", "me", "nick", "names", "raw", "whois", "topic", "away", "kick", "mode",
    "notice", "invite", "help", "quit",
];

/// Short help listing the commands available here, taken from the command
/// knowledge base so it cannot go stale.
pub fn help_text() -> String {
    let names: Vec<String> = crate::commands::COMMANDS
        .iter()
        .filter(|c| TRANSLATED.contains(&c.name))
        .map(|c| format!("/{}", c.name))
        .collect();
    format!("commands: {}", names.join(" "))
}

/// Translate one input line for the given current target.
pub fn translate(input: &str, current: &mut Option<String>) -> Translated {
    let input = input.trim();
    if input.is_empty() {
        return Translated::default();
    }

    let Some(rest) = input.strip_prefix('/') else {
        return match current {
            Some(target) => Translated::chat(
                format!("PRIVMSG {target} :{input}"),
                target,
                input,
                false,
                false,
            ),
            None => Translated::feedback("no target; use /join #channel or /msg <target> <text>"),
        };
    };

    let mut parts = rest.splitn(2, ' ');
    let command = parts.next().unwrap_or("").to_ascii_lowercase();
    let arg = parts.next().unwrap_or("").trim();

    match command.as_str() {
        "join" | "j" => match arg.split_whitespace().next() {
            Some(channel) => {
                *current = Some(channel.to_string());
                Translated::lines(vec![format!("JOIN {channel}")])
            }
            None => Translated::feedback("usage: /join #channel"),
        },
        "part" => {
            let channel = if arg.is_empty() {
                current.clone().unwrap_or_default()
            } else {
                arg.split_whitespace().next().unwrap_or("").to_string()
            };
            if channel.is_empty() {
                Translated::feedback("usage: /part [#channel]")
            } else {
                Translated::lines(vec![format!("PART {channel}")])
            }
        }
        "msg" | "m" => {
            let mut split = arg.splitn(2, ' ');
            let target = split.next().unwrap_or("");
            let text = split.next().unwrap_or("").trim();
            if target.is_empty() || text.is_empty() {
                Translated::feedback("usage: /msg <target> <text>")
            } else {
                *current = Some(target.to_string());
                Translated::chat(
                    format!("PRIVMSG {target} :{text}"),
                    target,
                    text,
                    false,
                    false,
                )
            }
        }
        "me" => match current {
            Some(target) if !arg.is_empty() => Translated::chat(
                format!("PRIVMSG {target} :\u{1}ACTION {arg}\u{1}"),
                target,
                arg,
                true,
                false,
            ),
            Some(_) => Translated::feedback("usage: /me <action>"),
            None => Translated::feedback("no target for /me"),
        },
        "nick" => {
            if arg.is_empty() {
                Translated::feedback("usage: /nick <newnick>")
            } else {
                Translated::lines(vec![format!("NICK {arg}")])
            }
        }
        "names" => {
            let channel = if arg.is_empty() {
                current.clone().unwrap_or_default()
            } else {
                arg.to_string()
            };
            Translated::lines(vec![format!("NAMES {channel}").trim_end().to_string()])
        }
        "raw" | "quote" => {
            if arg.is_empty() {
                Translated::feedback("usage: /raw <line>")
            } else {
                Translated::lines(vec![arg.to_string()])
            }
        }
        "whois" => match arg.split_whitespace().next() {
            Some(nick) => Translated::lines(vec![format!("WHOIS {nick}")]),
            None => Translated::feedback("usage: /whois <nick>"),
        },
        "topic" => match current {
            Some(channel) if arg.is_empty() => Translated::lines(vec![format!("TOPIC {channel}")]),
            Some(channel) => Translated::lines(vec![format!("TOPIC {channel} :{arg}")]),
            None => Translated::feedback("no channel here for /topic"),
        },
        "away" => {
            if arg.is_empty() {
                Translated::lines(vec!["AWAY".to_string()])
            } else {
                Translated::lines(vec![format!("AWAY :{arg}")])
            }
        }
        "kick" => {
            let Some(channel) = current.clone() else {
                return Translated::feedback("no channel here for /kick");
            };
            let mut split = arg.splitn(2, ' ');
            let nick = split.next().unwrap_or("");
            let reason = split.next().unwrap_or("").trim();
            if nick.is_empty() {
                Translated::feedback("usage: /kick <nick> [reason]")
            } else if reason.is_empty() {
                Translated::lines(vec![format!("KICK {channel} {nick}")])
            } else {
                Translated::lines(vec![format!("KICK {channel} {nick} :{reason}")])
            }
        }
        "mode" => match current {
            Some(target) if arg.is_empty() => Translated::lines(vec![format!("MODE {target}")]),
            Some(target) => Translated::lines(vec![format!("MODE {target} {arg}")]),
            None => Translated::feedback("no target here for /mode"),
        },
        "notice" => {
            let mut split = arg.splitn(2, ' ');
            let target = split.next().unwrap_or("");
            let text = split.next().unwrap_or("").trim();
            if target.is_empty() || text.is_empty() {
                Translated::feedback("usage: /notice <target> <text>")
            } else {
                Translated::chat(
                    format!("NOTICE {target} :{text}"),
                    target,
                    text,
                    false,
                    true,
                )
            }
        }
        "invite" => {
            let mut split = arg.split_whitespace();
            let nick = split.next().unwrap_or("");
            let channel = split.next().map(str::to_string).or_else(|| current.clone());
            match (nick.is_empty(), channel) {
                (false, Some(channel)) => {
                    Translated::lines(vec![format!("INVITE {nick} {channel}")])
                }
                _ => Translated::feedback("usage: /invite <nick> [#channel]"),
            }
        }
        "help" | "h" => Translated {
            feedback: Some(help_text()),
            ..Default::default()
        },
        "quit" => {
            let reason = if arg.is_empty() { "norn" } else { arg };
            Translated {
                lines: vec![format!("QUIT :{reason}")],
                quit: true,
                quit_reason: (!arg.is_empty()).then(|| arg.to_string()),
                ..Default::default()
            }
        }
        other => Translated::feedback(format!("unknown command: /{other} (try /help)")),
    }
}

#[cfg(test)]
mod tests {
    use super::{help_text, translate};

    #[test]
    fn help_text_lists_exactly_the_commands_translate_handles() {
        let help = help_text();
        for name in super::TRANSLATED {
            assert!(
                crate::commands::find(name).is_some(),
                "/{name} is not in the command knowledge base"
            );
            assert!(help.contains(&format!("/{name}")), "{help}");
            // And translate really does handle it (never "unknown command").
            let mut current = Some("#c".to_string());
            let t = translate(&format!("/{name}"), &mut current);
            assert!(
                !t.feedback
                    .as_deref()
                    .is_some_and(|f| f.contains("unknown command")),
                "/{name} is listed but unknown to translate"
            );
        }
        // Client commands are not offered in plain mode.
        assert!(!help.contains("/set") && !help.contains("/network"));
    }

    #[test]
    fn plain_text_needs_a_target() {
        let mut current = None;
        let t = translate("hello", &mut current);
        assert!(t.lines.is_empty());
        assert!(t.feedback.is_some());
    }

    #[test]
    fn join_sets_current_target_and_plain_text_messages_it() {
        let mut current = None;
        assert_eq!(
            translate("/join #rust", &mut current).lines,
            vec!["JOIN #rust"]
        );
        assert_eq!(current.as_deref(), Some("#rust"));
        assert_eq!(
            translate("hi all", &mut current).lines,
            vec!["PRIVMSG #rust :hi all"]
        );
    }

    #[test]
    fn msg_and_nick_and_raw() {
        let mut current = None;
        assert_eq!(
            translate("/msg bob hey there", &mut current).lines,
            vec!["PRIVMSG bob :hey there"]
        );
        assert_eq!(
            translate("/nick newnick", &mut current).lines,
            vec!["NICK newnick"]
        );
        assert_eq!(
            translate("/raw WHOIS bob", &mut current).lines,
            vec!["WHOIS bob"]
        );
    }

    #[test]
    fn unknown_command_returns_feedback_not_a_line() {
        let mut current = Some("#c".to_string());
        let t = translate("/nope", &mut current);
        assert!(t.lines.is_empty());
        assert!(t.feedback.unwrap().contains("unknown command"));
    }

    #[test]
    fn me_produces_ctcp_action() {
        let mut current = Some("#c".to_string());
        assert_eq!(
            translate("/me waves", &mut current).lines,
            vec!["PRIVMSG #c :\u{1}ACTION waves\u{1}"]
        );
    }

    #[test]
    fn quit_flags_shutdown() {
        let mut current = None;
        let t = translate("/quit bye", &mut current);
        assert_eq!(t.lines, vec!["QUIT :bye"]);
        assert!(t.quit);
    }

    #[test]
    fn whois_and_away() {
        let mut current = None;
        assert_eq!(
            translate("/whois bob", &mut current).lines,
            vec!["WHOIS bob"]
        );
        assert!(translate("/whois", &mut current).feedback.is_some());
        assert_eq!(
            translate("/away lunch", &mut current).lines,
            vec!["AWAY :lunch"]
        );
        assert_eq!(translate("/away", &mut current).lines, vec!["AWAY"]);
    }

    #[test]
    fn topic_views_and_sets_current_channel() {
        let mut current = Some("#rust".to_string());
        assert_eq!(translate("/topic", &mut current).lines, vec!["TOPIC #rust"]);
        assert_eq!(
            translate("/topic hello world", &mut current).lines,
            vec!["TOPIC #rust :hello world"]
        );
        // No channel -> feedback, not a line.
        let mut none = None;
        assert!(translate("/topic x", &mut none).feedback.is_some());
    }

    #[test]
    fn kick_mode_notice_invite() {
        let mut current = Some("#rust".to_string());
        assert_eq!(
            translate("/kick spammer being rude", &mut current).lines,
            vec!["KICK #rust spammer :being rude"]
        );
        assert_eq!(
            translate("/kick spammer", &mut current).lines,
            vec!["KICK #rust spammer"]
        );
        assert_eq!(
            translate("/mode +o bob", &mut current).lines,
            vec!["MODE #rust +o bob"]
        );
        assert_eq!(
            translate("/notice bob hey there", &mut current).lines,
            vec!["NOTICE bob :hey there"]
        );
        // invite defaults the channel to the current target.
        assert_eq!(
            translate("/invite bob", &mut current).lines,
            vec!["INVITE bob #rust"]
        );
        assert_eq!(
            translate("/invite bob #other", &mut current).lines,
            vec!["INVITE bob #other"]
        );
    }
}
