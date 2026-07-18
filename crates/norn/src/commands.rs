//! The command knowledge base: a structured description of every slash-command,
//! its subcommands, parameters, and examples.
//!
//! This is the single source of truth consumed by the `/help` panel (list and
//! detail views) and by Tab-completion. Defining a command's shape here means
//! help text and completion candidates never drift apart.

use crate::tui::theme;

/// What kind of value an argument accepts. Drives completion candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    /// One of a fixed set of values (e.g. `on`/`off`).
    Enum(&'static [&'static str]),
    /// A nick from the active channel.
    Nick,
    /// A channel (an open channel buffer).
    Channel,
    /// A defined or connected network name.
    Network,
    /// A command name (for `/help`).
    Command,
    /// A user alias name (for `/unalias`).
    Alias,
    /// A repeatable `key=value` option; the key completes with a trailing `=`.
    OptionKey,
    /// Freeform text (message bodies, reasons, modes) - no completion.
    Free,
}

/// One parameter of a command or subcommand.
#[derive(Debug, Clone, Copy)]
pub struct ParamDoc {
    /// The parameter name (no `<>`), e.g. `target`, `host`, `value`.
    pub name: &'static str,
    /// What it means.
    pub desc: &'static str,
    /// Whether it must be supplied.
    pub required: bool,
    /// What it accepts (for completion).
    pub kind: ArgKind,
}

/// A subcommand (e.g. `network add`, `set theme`).
#[derive(Debug, Clone, Copy)]
pub struct SubDoc {
    /// The subcommand name.
    pub name: &'static str,
    /// Its usage sketch.
    pub usage: &'static str,
    /// What it does.
    pub desc: &'static str,
    /// Its parameters (positional, or repeatable `OptionKey`s).
    pub params: &'static [ParamDoc],
    /// Example invocations.
    pub examples: &'static [&'static str],
}

/// A command's full documentation.
#[derive(Debug, Clone, Copy)]
pub struct CommandDoc {
    /// The command name (no leading slash).
    pub name: &'static str,
    /// The group heading it appears under in `/help`.
    pub category: &'static str,
    /// A usage sketch, e.g. `/msg <target> <text>`.
    pub usage: &'static str,
    /// A one-line description (list view).
    pub summary: &'static str,
    /// A fuller description (detail view).
    pub description: &'static str,
    /// Short aliases that also invoke it (e.g. `j` for `join`).
    pub aliases: &'static [&'static str],
    /// Positional parameters / options (for commands without subcommands).
    pub params: &'static [ParamDoc],
    /// Subcommands, if any.
    pub subcommands: &'static [SubDoc],
    /// Example invocations.
    pub examples: &'static [&'static str],
}

/// Shorthand for a required parameter.
const fn req(name: &'static str, kind: ArgKind, desc: &'static str) -> ParamDoc {
    ParamDoc {
        name,
        desc,
        required: true,
        kind,
    }
}
/// Shorthand for an optional parameter.
const fn opt(name: &'static str, kind: ArgKind, desc: &'static str) -> ParamDoc {
    ParamDoc {
        name,
        desc,
        required: false,
        kind,
    }
}

const ON_OFF: &[&str] = &["on", "off"];

/// The `/network add` options (repeatable `key=value`).
const NETWORK_ADD_OPTS: &[ParamDoc] = &[
    req("host", ArgKind::OptionKey, "server hostname"),
    req("nick", ArgKind::OptionKey, "nickname to register"),
    opt("port", ArgKind::OptionKey, "server port (default 6697)"),
    opt("tls", ArgKind::OptionKey, "on|off, use TLS (default on)"),
    opt("user", ArgKind::OptionKey, "username/ident (default: nick)"),
    opt("realname", ArgKind::OptionKey, "realname (default: nick)"),
    opt("sasl_account", ArgKind::OptionKey, "SASL account name"),
    opt(
        "sasl_mech",
        ArgKind::OptionKey,
        "plain|scram (default plain)",
    ),
    opt(
        "password_command",
        ArgKind::OptionKey,
        "shell command whose stdout is the password",
    ),
    opt(
        "join",
        ArgKind::OptionKey,
        "channels to auto-join, comma-separated",
    ),
];

const NETWORK_SUBS: &[SubDoc] = &[
    SubDoc {
        name: "ls",
        usage: "/network ls",
        desc: "List defined networks.",
        params: &[],
        examples: &["/network ls"],
    },
    SubDoc {
        name: "add",
        usage: "/network add <name> host=<h> nick=<n> [options]",
        desc: "Define a network. Does not connect - use /connect afterwards. \
Passwords are never stored inline; set password_command (or use NORN_PASSWORD).",
        params: NETWORK_ADD_OPTS,
        examples: &[
            "/network add libera host=irc.libera.chat nick=svan join=#rust,#ratatui",
            "/network add libera host=irc.libera.chat nick=svan sasl_account=svan \
sasl_mech=scram password_command=\"pass irc/libera\"",
        ],
    },
    SubDoc {
        name: "rm",
        usage: "/network rm <name>",
        desc: "Remove a network definition (aliases: remove, del).",
        params: &[req("name", ArgKind::Network, "the network to remove")],
        examples: &["/network rm libera"],
    },
    SubDoc {
        name: "show",
        usage: "/network show <name>",
        desc: "Show a network's settings and live state.",
        params: &[req("name", ArgKind::Network, "the network to show")],
        examples: &["/network show libera"],
    },
];

const SET_SUBS: &[SubDoc] = &[
    SubDoc {
        name: "timestamps",
        usage: "/set timestamps on|off",
        desc: "Show message timestamps.",
        params: &[req("value", ArgKind::Enum(ON_OFF), "on or off")],
        examples: &["/set timestamps off"],
    },
    SubDoc {
        name: "nicklist",
        usage: "/set nicklist on|off",
        desc: "Show the channel nicklist by default.",
        params: &[req("value", ArgKind::Enum(ON_OFF), "on or off")],
        examples: &["/set nicklist on"],
    },
    SubDoc {
        name: "theme",
        usage: "/set theme <name>",
        desc: "Accent color theme.",
        params: &[req(
            "value",
            ArgKind::Enum(theme::THEME_NAMES),
            "a theme name",
        )],
        examples: &["/set theme amber"],
    },
];

/// Every command, grouped by category (contiguous per category).
pub const COMMANDS: &[CommandDoc] = &[
    CommandDoc {
        name: "msg",
        category: "Chat",
        usage: "/msg <target> <text>",
        summary: "Send a private message",
        description: "Send a message to a user or channel without switching to \
its buffer. The target becomes the current target for following plain text.",
        aliases: &["m"],
        params: &[
            req("target", ArgKind::Nick, "nick or channel to message"),
            req("text", ArgKind::Free, "the message"),
        ],
        subcommands: &[],
        examples: &["/msg bob hey there", "/msg #rust hello"],
    },
    CommandDoc {
        name: "me",
        category: "Chat",
        usage: "/me <action>",
        summary: "Send an action (/me waves)",
        description: "Send a CTCP ACTION to the current target, rendered as \
`* you <action>`.",
        aliases: &[],
        params: &[req("action", ArgKind::Free, "what you are doing")],
        subcommands: &[],
        examples: &["/me waves at bob"],
    },
    CommandDoc {
        name: "query",
        category: "Chat",
        usage: "/query <nick>",
        summary: "Open a private chat buffer",
        description: "Open (or focus) a private message buffer with a user.",
        aliases: &["q"],
        params: &[req("nick", ArgKind::Nick, "the user to chat with")],
        subcommands: &[],
        examples: &["/query bob"],
    },
    CommandDoc {
        name: "notice",
        category: "Chat",
        usage: "/notice <target> <text>",
        summary: "Send a notice",
        description: "Send an IRC NOTICE - a message that clients should not \
auto-reply to.",
        aliases: &[],
        params: &[
            req("target", ArgKind::Nick, "nick or channel"),
            req("text", ArgKind::Free, "the notice"),
        ],
        subcommands: &[],
        examples: &["/notice bob heads up"],
    },
    CommandDoc {
        name: "join",
        category: "Channel",
        usage: "/join #channel",
        summary: "Join a channel",
        description: "Join a channel and make it the current target.",
        aliases: &["j"],
        params: &[req("channel", ArgKind::Channel, "the channel to join")],
        subcommands: &[],
        examples: &["/join #rust"],
    },
    CommandDoc {
        name: "part",
        category: "Channel",
        usage: "/part [#channel]",
        summary: "Leave a channel",
        description: "Leave a channel (the current one if omitted).",
        aliases: &[],
        params: &[opt("channel", ArgKind::Channel, "the channel to leave")],
        subcommands: &[],
        examples: &["/part", "/part #rust"],
    },
    CommandDoc {
        name: "names",
        category: "Channel",
        usage: "/names [#channel]",
        summary: "List the members of a channel",
        description: "Request the member list of a channel (the current one if \
omitted).",
        aliases: &[],
        params: &[opt("channel", ArgKind::Channel, "the channel")],
        subcommands: &[],
        examples: &["/names"],
    },
    CommandDoc {
        name: "topic",
        category: "Channel",
        usage: "/topic [text]",
        summary: "View or set the channel topic",
        description: "With no text, show the current topic; with text, set it.",
        aliases: &[],
        params: &[opt("text", ArgKind::Free, "the new topic")],
        subcommands: &[],
        examples: &["/topic", "/topic Rust programming"],
    },
    CommandDoc {
        name: "kick",
        category: "Channel",
        usage: "/kick <nick> [reason]",
        summary: "Kick a user from the channel",
        description: "Remove a user from the current channel (requires ops).",
        aliases: &[],
        params: &[
            req("nick", ArgKind::Nick, "the user to kick"),
            opt("reason", ArgKind::Free, "why"),
        ],
        subcommands: &[],
        examples: &["/kick spammer being rude"],
    },
    CommandDoc {
        name: "mode",
        category: "Channel",
        usage: "/mode <args>",
        summary: "Set channel or user modes",
        description: "Apply modes to the current channel (or yourself). The args \
are passed through to the server.",
        aliases: &[],
        params: &[req("args", ArgKind::Free, "mode string, e.g. +o bob")],
        subcommands: &[],
        examples: &["/mode +o bob", "/mode +m"],
    },
    CommandDoc {
        name: "invite",
        category: "Channel",
        usage: "/invite <nick> [#chan]",
        summary: "Invite a user to a channel",
        description: "Invite a user to a channel (the current one if omitted).",
        aliases: &[],
        params: &[
            req("nick", ArgKind::Nick, "the user to invite"),
            opt("channel", ArgKind::Channel, "the channel"),
        ],
        subcommands: &[],
        examples: &["/invite bob", "/invite bob #rust"],
    },
    CommandDoc {
        name: "close",
        category: "Channel",
        usage: "/close",
        summary: "Close the active buffer",
        description: "Close the active buffer; parts the channel if it is one \
(aliases: wc).",
        aliases: &["wc"],
        params: &[],
        subcommands: &[],
        examples: &["/close"],
    },
    CommandDoc {
        name: "nick",
        category: "You",
        usage: "/nick <newnick>",
        summary: "Change your nickname",
        description: "Ask the server to change your nickname.",
        aliases: &[],
        params: &[req("newnick", ArgKind::Free, "the new nickname")],
        subcommands: &[],
        examples: &["/nick svan_"],
    },
    CommandDoc {
        name: "away",
        category: "You",
        usage: "/away [message]",
        summary: "Set or clear your away status",
        description: "Mark yourself away with a message, or clear it with no \
argument.",
        aliases: &[],
        params: &[opt(
            "message",
            ArgKind::Free,
            "away reason (omit to return)",
        )],
        subcommands: &[],
        examples: &["/away lunch", "/away"],
    },
    CommandDoc {
        name: "whois",
        category: "You",
        usage: "/whois <nick>",
        summary: "Look up information about a user",
        description: "Request WHOIS information about a user.",
        aliases: &[],
        params: &[req("nick", ArgKind::Nick, "the user")],
        subcommands: &[],
        examples: &["/whois bob"],
    },
    CommandDoc {
        name: "network",
        category: "Networks",
        usage: "/network ls|add|rm|show",
        summary: "Manage network definitions",
        description: "Define, list, inspect, and remove networks. Definitions are \
saved to config and auto-connect on next launch; use /connect to dial one now.",
        aliases: &["net"],
        params: &[],
        subcommands: NETWORK_SUBS,
        examples: &[
            "/network ls",
            "/network add libera host=irc.libera.chat nick=svan",
        ],
    },
    CommandDoc {
        name: "connect",
        category: "Networks",
        usage: "/connect <name>",
        summary: "Connect a defined network",
        description: "Connect a network by name - revives an idle one or dials a \
newly defined one (alias: server).",
        aliases: &["server"],
        params: &[req("name", ArgKind::Network, "the network to connect")],
        subcommands: &[],
        examples: &["/connect libera"],
    },
    CommandDoc {
        name: "disconnect",
        category: "Networks",
        usage: "/disconnect [reason]",
        summary: "Disconnect the active network",
        description: "Disconnect the active network but keep it so /connect can \
revive it.",
        aliases: &[],
        params: &[opt("reason", ArgKind::Free, "quit reason")],
        subcommands: &[],
        examples: &["/disconnect", "/disconnect back later"],
    },
    CommandDoc {
        name: "reconnect",
        category: "Networks",
        usage: "/reconnect",
        summary: "Reconnect the active network",
        description: "Drop and redial the active network.",
        aliases: &[],
        params: &[],
        subcommands: &[],
        examples: &["/reconnect"],
    },
    CommandDoc {
        name: "set",
        category: "Client",
        usage: "/set [key value]",
        summary: "View or change settings",
        description: "With no arguments, list the current settings; with a key \
and value, change one (applied live and auto-saved).",
        aliases: &[],
        params: &[],
        subcommands: SET_SUBS,
        examples: &["/set", "/set theme amber", "/set timestamps off"],
    },
    CommandDoc {
        name: "alias",
        category: "Client",
        usage: "/alias [name expansion]",
        summary: "List or define command aliases",
        description: "With no arguments, list aliases; otherwise define one. The \
expansion may use $1..$9 and $* for arguments, chain commands with ;, and has \
its trailing args appended when it contains no placeholder.",
        aliases: &[],
        params: &[
            opt("name", ArgKind::Alias, "the alias name"),
            opt("expansion", ArgKind::Free, "what it expands to"),
        ],
        subcommands: &[],
        examples: &[
            "/alias exit quit",
            "/alias j join $1",
            "/alias hello msg $1 hi;msg $1 there",
        ],
    },
    CommandDoc {
        name: "unalias",
        category: "Client",
        usage: "/unalias <name>",
        summary: "Remove a command alias",
        description: "Delete a user-defined alias.",
        aliases: &[],
        params: &[req("name", ArgKind::Alias, "the alias to remove")],
        subcommands: &[],
        examples: &["/unalias j"],
    },
    CommandDoc {
        name: "clear",
        category: "Client",
        usage: "/clear",
        summary: "Clear the active buffer's scrollback",
        description: "Erase the lines in the active buffer.",
        aliases: &[],
        params: &[],
        subcommands: &[],
        examples: &["/clear"],
    },
    CommandDoc {
        name: "raw",
        category: "Client",
        usage: "/raw <line>",
        summary: "Send a raw IRC line",
        description: "Send a line verbatim to the server (alias: quote).",
        aliases: &["quote"],
        params: &[req("line", ArgKind::Free, "the raw IRC line")],
        subcommands: &[],
        examples: &["/raw WHOIS bob", "/raw PRIVMSG #rust :hi"],
    },
    CommandDoc {
        name: "help",
        category: "Client",
        usage: "/help [command]",
        summary: "Open the help panel",
        description: "Open this help panel. With a command name, open straight to \
its details.",
        aliases: &["h"],
        params: &[opt("command", ArgKind::Command, "a command to detail")],
        subcommands: &[],
        examples: &["/help", "/help network"],
    },
    CommandDoc {
        name: "quit",
        category: "Client",
        usage: "/quit [reason]",
        summary: "Quit norn",
        description: "Disconnect all networks and exit.",
        aliases: &[],
        params: &[opt("reason", ArgKind::Free, "quit reason")],
        subcommands: &[],
        examples: &["/quit", "/quit see you"],
    },
];

/// One rendered row of the `/help` list: a category header or a command.
pub enum HelpRow {
    /// A group heading.
    Header(&'static str),
    /// A command entry.
    Command(&'static CommandDoc),
}

/// Look up a command by name or short alias (a leading `/` is ignored).
pub fn find(name: &str) -> Option<&'static CommandDoc> {
    let n = name.trim_start_matches('/').to_ascii_lowercase();
    COMMANDS
        .iter()
        .find(|c| c.name == n || c.aliases.contains(&n.as_str()))
}

/// The help rows matching `query` (case-insensitive over name/usage/summary),
/// with a header before each category that has a match. Empty query lists all.
pub fn help_rows(query: &str) -> Vec<HelpRow> {
    let q = query.trim().to_lowercase();
    let hit = |c: &CommandDoc| {
        q.is_empty()
            || c.name.contains(&q)
            || c.usage.to_lowercase().contains(&q)
            || c.summary.to_lowercase().contains(&q)
    };
    let mut rows = Vec::new();
    let mut current = "";
    for doc in COMMANDS {
        if !hit(doc) {
            continue;
        }
        if doc.category != current {
            rows.push(HelpRow::Header(doc.category));
            current = doc.category;
        }
        rows.push(HelpRow::Command(doc));
    }
    rows
}

/// The commands matching `query`, flat (no headers), in display order - the
/// selectable list for the `/help` picker.
pub fn help_commands(query: &str) -> Vec<&'static CommandDoc> {
    help_rows(query)
        .into_iter()
        .filter_map(|row| match row {
            HelpRow::Command(doc) => Some(doc),
            HelpRow::Header(_) => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_is_documented() {
        for doc in COMMANDS {
            assert!(!doc.summary.is_empty(), "{} has no summary", doc.name);
            assert!(!doc.usage.is_empty(), "{} has no usage", doc.name);
            assert!(
                !doc.description.is_empty(),
                "{} has no description",
                doc.name
            );
        }
    }

    #[test]
    fn find_resolves_names_and_aliases() {
        assert_eq!(find("network").map(|c| c.name), Some("network"));
        assert_eq!(find("/join").map(|c| c.name), Some("join"));
        assert_eq!(find("j").map(|c| c.name), Some("join")); // alias
        assert_eq!(find("net").map(|c| c.name), Some("network")); // alias
        assert!(find("nope").is_none());
    }

    #[test]
    fn help_rows_list_all_then_filter() {
        let all = help_rows("");
        let commands = all
            .iter()
            .filter(|r| matches!(r, HelpRow::Command(_)))
            .count();
        assert_eq!(commands, COMMANDS.len());
        assert!(all.iter().any(|r| matches!(r, HelpRow::Header(_))));

        let net = help_rows("network");
        assert!(net
            .iter()
            .any(|r| matches!(r, HelpRow::Command(c) if c.name == "network")));
        assert!(net
            .iter()
            .all(|r| !matches!(r, HelpRow::Command(c) if c.name == "join")));
    }
}
