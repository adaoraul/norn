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
    /// Whether the user asked to quit.
    pub quit: bool,
}

impl Translated {
    fn lines(lines: Vec<String>) -> Self {
        Translated {
            lines,
            ..Default::default()
        }
    }
    fn feedback(msg: impl Into<String>) -> Self {
        Translated {
            feedback: Some(msg.into()),
            ..Default::default()
        }
    }
}

/// Short help listing the available commands.
pub const HELP: &str = "commands: /join #chan · /part [#chan] · /msg <t> <text> · \
/query <nick> · /nick <n> · /names · /me <action> · /raw <line> · /help · /quit · \
/set [key val] · /network list|add|remove · /connect <name> · /disconnect [reason]";

/// Translate one input line for the given current target.
pub fn translate(input: &str, current: &mut Option<String>) -> Translated {
    let input = input.trim();
    if input.is_empty() {
        return Translated::default();
    }

    let Some(rest) = input.strip_prefix('/') else {
        return match current {
            Some(target) => Translated::lines(vec![format!("PRIVMSG {target} :{input}")]),
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
                Translated::lines(vec![format!("PRIVMSG {target} :{text}")])
            }
        }
        "me" => match current {
            Some(target) if !arg.is_empty() => {
                Translated::lines(vec![format!("PRIVMSG {target} :\u{1}ACTION {arg}\u{1}")])
            }
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
        "help" | "h" => Translated::feedback(HELP),
        "quit" => {
            let reason = if arg.is_empty() { "norn" } else { arg };
            Translated {
                lines: vec![format!("QUIT :{reason}")],
                quit: true,
                ..Default::default()
            }
        }
        other => Translated::feedback(format!("unknown command: /{other} (try /help)")),
    }
}

#[cfg(test)]
mod tests {
    use super::translate;

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
}
