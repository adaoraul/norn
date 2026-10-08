//! The settings knowledge base: a structured description of every client
//! preference in [`crate::config::ClientConfig`].
//!
//! Like the command KB (`crate::commands`), this is the single source of truth.
//! The `/settings` screen renders it (categorized, typed, filterable, with a
//! description line) and `/set` validates against it; the actual get/set of
//! values lives on `App` (`setting_value`/`set_setting`) since some changes have
//! live side effects.

/// What kind of value a setting holds. Drives the type label and how the
/// `/settings` screen edits it (toggle, cycle, or inline text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    /// `on`/`off`.
    Bool,
    /// A whole number, bounded to `min..=max` inclusive.
    Int { min: usize, max: usize },
    /// One of a fixed set of values.
    Enum(&'static [&'static str]),
    /// Freeform short text.
    Str,
}

impl SettingKind {
    /// The short type label shown on the right of a settings row.
    pub fn label(&self) -> &'static str {
        match self {
            SettingKind::Bool => "bool",
            SettingKind::Int { .. } => "int",
            SettingKind::Enum(_) => "enum",
            SettingKind::Str => "str",
        }
    }
}

/// One client preference.
#[derive(Debug, Clone, Copy)]
pub struct SettingDoc {
    /// The key, e.g. `theme` (matches the `ClientConfig` field).
    pub key: &'static str,
    /// The group heading it appears under.
    pub category: &'static str,
    /// Its value kind.
    pub kind: SettingKind,
    /// What it does (shown on the detail line).
    pub desc: &'static str,
    /// The default value, rendered for the detail line.
    pub default: &'static str,
}

/// Every setting, grouped by category (contiguous per category).
pub const SETTINGS: &[SettingDoc] = &[
    SettingDoc {
        key: "timestamps",
        category: "look & feel",
        kind: SettingKind::Bool,
        desc: "show a timestamp in front of each message",
        default: "on",
    },
    SettingDoc {
        key: "timestamp_format",
        category: "look & feel",
        kind: SettingKind::Str,
        desc: "how timestamps are written, as a strftime pattern (e.g. %H:%M:%S for seconds)",
        default: "%H:%M",
    },
    SettingDoc {
        key: "color_mode",
        category: "look & feel",
        kind: SettingKind::Enum(crate::tui::color::COLOR_MODE_NAMES),
        desc: "how many colours to use: auto follows NO_COLOR / COLORTERM / TERM",
        default: "auto",
    },
    SettingDoc {
        key: "paint_background",
        category: "look & feel",
        kind: SettingKind::Bool,
        desc: "paint the window background (off lets your terminal's show through)",
        default: "on",
    },
    SettingDoc {
        key: "mirc_formatting",
        category: "look & feel",
        kind: SettingKind::Enum(&["render", "strip"]),
        desc: "show mIRC bold/colour/underline in messages, or strip the codes",
        default: "render",
    },
    SettingDoc {
        key: "nick_colors",
        category: "look & feel",
        kind: SettingKind::Bool,
        desc: "color nicks by a per-nick hue (off = one muted color)",
        default: "on",
    },
    SettingDoc {
        key: "theme",
        category: "look & feel",
        kind: SettingKind::Enum(crate::tui::theme::THEME_NAMES),
        desc: "accent color theme",
        default: "teal",
    },
    SettingDoc {
        key: "nicklist",
        category: "look & feel",
        kind: SettingKind::Bool,
        desc: "show the channel nicklist by default",
        default: "on",
    },
    SettingDoc {
        key: "sidebar_width",
        category: "look & feel",
        kind: SettingKind::Int { min: 12, max: 48 },
        desc: "width of the buffer sidebar in columns (narrow terminals shrink it)",
        default: "24",
    },
    SettingDoc {
        key: "nicklist_width",
        category: "look & feel",
        kind: SettingKind::Int { min: 10, max: 32 },
        desc: "width of the channel nicklist in columns (hidden below 80 columns)",
        default: "18",
    },
    SettingDoc {
        key: "completion_char",
        category: "behavior",
        kind: SettingKind::Str,
        desc: "character after a nick completed at line start (then a space)",
        default: ":",
    },
    SettingDoc {
        key: "beep_on_highlight",
        category: "behavior",
        kind: SettingKind::Bool,
        desc: "ring the terminal bell when a message highlights your nick",
        default: "off",
    },
    SettingDoc {
        key: "mouse",
        category: "behavior",
        kind: SettingKind::Bool,
        desc: "capture the mouse (click buffers, wheel scroll); off lets the terminal select text",
        default: "on",
    },
    SettingDoc {
        key: "scrollback_lines",
        category: "history",
        kind: SettingKind::Int {
            min: 100,
            max: 1_000_000,
        },
        desc: "maximum number of lines kept per buffer",
        default: "5000",
    },
    SettingDoc {
        key: "idle_secs",
        category: "behavior",
        kind: SettingKind::Int {
            min: 0,
            max: 86_400,
        },
        desc: "seconds of inactivity before plugins get on_idle (0 disables)",
        default: "300",
    },
];

/// Look up a setting by key.
pub fn find(key: &str) -> Option<&'static SettingDoc> {
    let k = key.to_ascii_lowercase();
    SETTINGS.iter().find(|s| s.key == k)
}

/// Whether a setting matches `query` (case-insensitive over key/category/desc).
pub fn matches(doc: &SettingDoc, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty()
        || doc.key.contains(&q)
        || doc.category.contains(&q)
        || doc.desc.to_lowercase().contains(&q)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_has_docs_and_a_default() {
        for s in SETTINGS {
            assert!(!s.desc.is_empty(), "{} has no description", s.key);
            assert!(!s.default.is_empty(), "{} has no default", s.key);
            // Enum defaults must be a listed value.
            if let SettingKind::Enum(values) = s.kind {
                assert!(values.contains(&s.default), "{} default not in enum", s.key);
            }
        }
    }

    #[test]
    fn find_resolves_keys() {
        assert_eq!(find("theme").map(|s| s.key), Some("theme"));
        assert_eq!(find("THEME").map(|s| s.key), Some("theme"));
        assert!(find("nope").is_none());
    }
}
