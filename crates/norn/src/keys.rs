//! The key binding knowledge base: every key norn reacts to, in one table.
//!
//! Like the command KB (`crate::commands`) and the settings KB
//! (`crate::settings`), this is the single source of truth. The `/keys` help page
//! and the console's welcome text are built from it, and a test checks that
//! `docs/USER_GUIDE.md` documents every entry, so the guide cannot drift from the
//! keys the program actually has.

/// One key binding (or a small family of them, like `Alt+1..9`).
#[derive(Debug, Clone, Copy)]
pub struct KeyDoc {
    /// The key(s), as written in the guide. Alternatives are separated by ` / `.
    pub keys: &'static str,
    /// Where it applies: `chat`, or the name of a panel.
    pub context: &'static str,
    /// What it does.
    pub action: &'static str,
    /// A short label for the console welcome line; empty to leave it out.
    pub hint: &'static str,
}

const fn key(
    keys: &'static str,
    context: &'static str,
    action: &'static str,
    hint: &'static str,
) -> KeyDoc {
    KeyDoc {
        keys,
        context,
        action,
        hint,
    }
}

/// Every binding, grouped by context (contiguous per context).
pub const KEYS: &[KeyDoc] = &[
    key(
        "Ctrl+C",
        "chat",
        "Quit (press twice within 2 seconds; any other key cancels)",
        "",
    ),
    key(
        "Ctrl+K",
        "chat",
        "Open the buffer switcher (fuzzy: type a few letters of the network or channel)",
        "switch buffers",
    ),
    key("F1", "chat", "Show this list of keys", "keys"),
    key("F2", "chat", "Open the settings screen", "settings"),
    key("F3", "chat", "Open the networks manager", "networks"),
    key("F4", "chat", "Open the plugins manager", "plugins"),
    key("F9", "chat", "Toggle the nicklist", ""),
    key(
        "Ctrl+F",
        "chat",
        "Search this buffer's scrollback (also /search <text>)",
        "search",
    ),
    key(
        "Ctrl+End",
        "chat",
        "Jump to the newest line and follow live again",
        "",
    ),
    key(
        "Alt+U",
        "chat",
        "Jump to the first unread line (where you stopped reading)",
        "",
    ),
    key("Alt+Left / Alt+Right", "chat", "Previous / next buffer", ""),
    key(
        "Alt+1..9",
        "chat",
        "Jump to the buffer with that number (shown in the sidebar)",
        "",
    ),
    key("Alt+0", "chat", "Jump to the console", ""),
    key(
        "Tab",
        "chat",
        "Complete nicks, commands and arguments (repeat to cycle)",
        "",
    ),
    key("Esc", "chat", "Close the completion menu", ""),
    key("Enter", "chat", "Send the line", ""),
    key("Left / Right", "chat", "Move the cursor", ""),
    key(
        "Ctrl+Left / Ctrl+Right",
        "chat",
        "Move the cursor by a word",
        "",
    ),
    key(
        "Home / End",
        "chat",
        "Jump to the start / end of the line",
        "",
    ),
    key(
        "Ctrl+A / Ctrl+E",
        "chat",
        "Jump to the start / end of the line",
        "",
    ),
    key(
        "Delete",
        "chat",
        "Delete the character under the cursor",
        "",
    ),
    key("Ctrl+U", "chat", "Delete everything before the cursor", ""),
    key(
        "Ctrl+W / Alt+Backspace",
        "chat",
        "Delete the word before the cursor",
        "",
    ),
    key("Up / Down", "chat", "Recall earlier input lines", ""),
    key(
        "PageUp / PageDown",
        "chat",
        "Scroll the buffer (reaching the top loads older messages)",
        "",
    ),
    key(
        "Enter",
        "paste",
        "Send a multi-line paste, one message per line",
        "",
    ),
    key("Esc", "paste", "Discard a multi-line paste", ""),
    key(
        "Enter",
        "search",
        "Go to the next-older match (Up does too)",
        "",
    ),
    key("Down", "search", "Go to the next-newer match", ""),
    key("Esc", "search", "Leave the search, keeping your place", ""),
    key("Up / Down", "switcher", "Move the selection", ""),
    key("Enter", "switcher", "Switch to the selected buffer", ""),
    key("Esc", "switcher", "Close without switching", ""),
    key(
        "Enter",
        "settings",
        "Toggle, cycle or edit the selected setting",
        "",
    ),
    key(
        "Delete",
        "settings",
        "Remove the selected alias (press twice)",
        "",
    ),
    key("Esc", "settings", "Close the settings screen", ""),
    key("c", "networks", "Connect the selected network", ""),
    key("d", "networks", "Disconnect the selected network", ""),
    key(
        "x",
        "networks",
        "Remove the selected network (press twice)",
        "",
    ),
    key("Esc", "networks", "Go back, or close the manager", ""),
    key(
        "Space",
        "plugins",
        "Enable or disable the selected plugin",
        "",
    ),
    key("c", "plugins", "Open the selected plugin's settings", ""),
    key("Esc", "plugins", "Close the plugins manager", ""),
    key(
        "Right / Left",
        "help",
        "Move between the list and the details",
        "",
    ),
    key(
        "Enter",
        "help",
        "Put the selected command in the input line",
        "",
    ),
    key("Esc", "help", "Close help", ""),
];

/// The console welcome line built from the bindings that carry a hint, e.g.
/// `Ctrl+K switch buffers · F1 keys · F2 settings`.
pub fn welcome_line() -> String {
    KEYS.iter()
        .filter(|k| !k.hint.is_empty())
        .map(|k| format!("{} {}", k.keys, k.hint))
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUIDE: &str = include_str!("../../../docs/USER_GUIDE.md");

    #[test]
    fn every_binding_is_complete() {
        for k in KEYS {
            assert!(!k.keys.is_empty() && !k.context.is_empty() && !k.action.is_empty());
        }
    }

    #[test]
    fn the_user_guide_documents_every_key() {
        for k in KEYS {
            for alternative in k.keys.split(" / ") {
                assert!(
                    GUIDE.contains(alternative),
                    "docs/USER_GUIDE.md never mentions the key `{alternative}` ({} in {})",
                    k.action,
                    k.context
                );
            }
        }
    }

    #[test]
    fn the_user_guide_documents_every_setting() {
        for s in crate::settings::SETTINGS {
            assert!(
                GUIDE.contains(&format!("`{}`", s.key)),
                "docs/USER_GUIDE.md never mentions the setting `{}`",
                s.key
            );
        }
    }

    #[test]
    fn welcome_line_lists_the_main_keys() {
        let line = welcome_line();
        for expected in ["Ctrl+K", "F1", "F2", "F3", "F4"] {
            assert!(line.contains(expected), "{line}");
        }
    }
}
