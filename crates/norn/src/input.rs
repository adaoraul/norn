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
pub const HELP: &str = "commands: /join /part /msg /query /nick /names /me /topic \
/whois /away /kick /mode /notice /invite /raw · client: /set /network ls|add|rm|show \
/connect /disconnect /reconnect /clear /alias /unalias /help /quit";

/// One command's metadata: the single registry powering the `/help` panel and
/// Tab-completion. Short aliases (`j`, `m`, `q`, ...) are omitted so completion
/// and help present the full names. Entries are grouped by `category` and kept
/// contiguous per category so [`help_rows`] can insert one header each.
pub struct CommandInfo {
    /// The command name (no leading slash).
    pub name: &'static str,
    /// A usage sketch, e.g. `/msg <target> <text>`.
    pub usage: &'static str,
    /// A one-line description.
    pub help: &'static str,
    /// The group heading it appears under in `/help`.
    pub category: &'static str,
}

/// Every command, grouped by category (contiguous per category).
#[rustfmt::skip]
pub const COMMAND_INFO: &[CommandInfo] = &[
    CommandInfo { name: "msg", usage: "/msg <target> <text>", help: "Send a private message", category: "Chat" },
    CommandInfo { name: "me", usage: "/me <action>", help: "Send an action (/me waves)", category: "Chat" },
    CommandInfo { name: "query", usage: "/query <nick>", help: "Open a private chat buffer", category: "Chat" },
    CommandInfo { name: "notice", usage: "/notice <target> <text>", help: "Send a notice", category: "Chat" },
    CommandInfo { name: "join", usage: "/join #channel", help: "Join a channel", category: "Channel" },
    CommandInfo { name: "part", usage: "/part [#channel]", help: "Leave a channel", category: "Channel" },
    CommandInfo { name: "names", usage: "/names [#channel]", help: "List the members of a channel", category: "Channel" },
    CommandInfo { name: "topic", usage: "/topic [text]", help: "View or set the channel topic", category: "Channel" },
    CommandInfo { name: "kick", usage: "/kick <nick> [reason]", help: "Kick a user from the channel", category: "Channel" },
    CommandInfo { name: "mode", usage: "/mode <args>", help: "Set channel or user modes", category: "Channel" },
    CommandInfo { name: "invite", usage: "/invite <nick> [#chan]", help: "Invite a user to a channel", category: "Channel" },
    CommandInfo { name: "close", usage: "/close", help: "Close the active buffer (part if a channel)", category: "Channel" },
    CommandInfo { name: "nick", usage: "/nick <newnick>", help: "Change your nickname", category: "You" },
    CommandInfo { name: "away", usage: "/away [message]", help: "Set or clear your away status", category: "You" },
    CommandInfo { name: "whois", usage: "/whois <nick>", help: "Look up information about a user", category: "You" },
    CommandInfo { name: "network", usage: "/network ls|add|rm|show", help: "Manage network definitions", category: "Networks" },
    CommandInfo { name: "connect", usage: "/connect <name>", help: "Connect a defined network", category: "Networks" },
    CommandInfo { name: "disconnect", usage: "/disconnect [reason]", help: "Disconnect the active network", category: "Networks" },
    CommandInfo { name: "reconnect", usage: "/reconnect", help: "Reconnect the active network", category: "Networks" },
    CommandInfo { name: "set", usage: "/set [key value]", help: "View or change settings (timestamps, theme, nicklist)", category: "Client" },
    CommandInfo { name: "alias", usage: "/alias [name expansion]", help: "List or define command aliases", category: "Client" },
    CommandInfo { name: "unalias", usage: "/unalias <name>", help: "Remove a command alias", category: "Client" },
    CommandInfo { name: "clear", usage: "/clear", help: "Clear the active buffer's scrollback", category: "Client" },
    CommandInfo { name: "raw", usage: "/raw <line>", help: "Send a raw IRC line", category: "Client" },
    CommandInfo { name: "help", usage: "/help [command]", help: "Open this help panel", category: "Client" },
    CommandInfo { name: "quit", usage: "/quit [reason]", help: "Quit norn", category: "Client" },
];

/// One rendered row of the `/help` panel: a category header or a command.
pub enum HelpRow {
    /// A group heading.
    Header(&'static str),
    /// A command entry.
    Command(&'static CommandInfo),
}

/// The help rows matching `query` (case-insensitive over name/usage/help), with
/// a header before each category that has any match. Empty query lists all.
pub fn help_rows(query: &str) -> Vec<HelpRow> {
    let q = query.trim().to_lowercase();
    let hit = |c: &CommandInfo| {
        q.is_empty()
            || c.name.contains(&q)
            || c.usage.to_lowercase().contains(&q)
            || c.help.to_lowercase().contains(&q)
    };
    let mut rows = Vec::new();
    let mut current = "";
    for info in COMMAND_INFO {
        if !hit(info) {
            continue;
        }
        if info.category != current {
            rows.push(HelpRow::Header(info.category));
            current = info.category;
        }
        rows.push(HelpRow::Command(info));
    }
    rows
}

/// The commands matching `query`, flat (no headers), in display order. This is
/// the selectable list for the `/help` picker.
pub fn help_commands(query: &str) -> Vec<&'static CommandInfo> {
    help_rows(query)
        .into_iter()
        .filter_map(|row| match row {
            HelpRow::Command(info) => Some(info),
            HelpRow::Header(_) => None,
        })
        .collect()
}

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
                Translated::lines(vec![format!("NOTICE {target} :{text}")])
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
    use super::{help_rows, translate, HelpRow};

    #[test]
    fn help_rows_list_all_then_filter() {
        // Empty query lists every command plus a header per category.
        let all = help_rows("");
        let commands = all
            .iter()
            .filter(|r| matches!(r, HelpRow::Command(_)))
            .count();
        assert_eq!(commands, super::COMMAND_INFO.len());
        assert!(all.iter().any(|r| matches!(r, HelpRow::Header(_))));

        // A filter narrows to matching commands (name/usage/help) and keeps only
        // the headers for categories that still have a match.
        let net = help_rows("network");
        assert!(net
            .iter()
            .any(|r| matches!(r, HelpRow::Command(c) if c.name == "network")));
        assert!(net
            .iter()
            .all(|r| !matches!(r, HelpRow::Command(c) if c.name == "join")));
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
