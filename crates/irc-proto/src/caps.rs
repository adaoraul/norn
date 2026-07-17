//! Capability names, values, and sets.
//!
//! Three concerns live here, all pure:
//!
//! - [`CapName`]: a capability name whose logical identity ignores a `draft/`
//!   prefix, so `draft/chathistory` and `chathistory` are the same capability
//!   (rule 14). The original string is retained for `CAP REQ`, which must echo
//!   the name the server advertised.
//! - [`Cap`]: a name plus an optional value. `CAP LS 302` (rule 3) enables
//!   values like `sasl=PLAIN,EXTERNAL`, so values parse into a comma list.
//! - [`CapSet`] and [`LsAccumulator`]: a collection of caps, and the buffer
//!   that assembles a multiline `CAP LS`/`CAP NEW` listing until the line
//!   without the `*` continuation marker (rule 4).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// A capability name with `draft/`-insensitive identity.
///
/// Equality and hashing use the "core" name (the `draft/` prefix stripped), so
/// the ratified and unratified spellings collide as one logical capability.
/// [`CapName::as_str`] returns the exact advertised spelling for use in
/// `CAP REQ`.
#[derive(Debug, Clone)]
pub struct CapName(String);

impl CapName {
    /// Wrap a raw capability name as advertised.
    pub fn new(name: impl Into<String>) -> Self {
        CapName(name.into())
    }

    /// The name with any leading `draft/` removed: its logical identity.
    pub fn core(&self) -> &str {
        self.0.strip_prefix("draft/").unwrap_or(&self.0)
    }

    /// The name exactly as advertised. Use this when sending `CAP REQ`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl PartialEq for CapName {
    fn eq(&self, other: &Self) -> bool {
        self.core() == other.core()
    }
}

impl Eq for CapName {}

impl Hash for CapName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.core().hash(state);
    }
}

/// A single capability: a name and an optional value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cap {
    /// The capability name.
    pub name: CapName,
    /// The raw value after `=`, if the server advertised one.
    pub value: Option<String>,
}

impl Cap {
    /// Parse one `CAP LS` token: `name` or `name=value`.
    pub fn parse(token: &str) -> Cap {
        match token.split_once('=') {
            Some((name, value)) => Cap {
                name: CapName::new(name),
                value: Some(value.to_string()),
            },
            None => Cap {
                name: CapName::new(token),
                value: None,
            },
        }
    }

    /// The value split on `,` (e.g. sasl mechanisms). Empty when there is no
    /// value or the value is empty.
    pub fn values(&self) -> Vec<&str> {
        match &self.value {
            Some(v) if !v.is_empty() => v.split(',').collect(),
            _ => Vec::new(),
        }
    }
}

/// A set of capabilities keyed by logical name.
///
/// Because [`CapName`] identity ignores `draft/`, inserting both spellings
/// keeps a single entry. Lookups accept either spelling.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CapSet(HashMap<CapName, Option<String>>);

impl CapSet {
    /// An empty set.
    pub fn new() -> Self {
        CapSet(HashMap::new())
    }

    /// Insert a parsed capability.
    pub fn insert(&mut self, cap: Cap) {
        self.0.insert(cap.name, cap.value);
    }

    /// Parse and insert a single `name[=value]` token.
    pub fn insert_token(&mut self, token: &str) {
        self.insert(Cap::parse(token));
    }

    /// Parse and insert a space-separated list of tokens.
    pub fn parse_line(&mut self, list: &str) {
        for token in list.split(' ') {
            if !token.is_empty() {
                self.insert_token(token);
            }
        }
    }

    /// Whether a capability is present (either spelling).
    pub fn contains(&self, name: &str) -> bool {
        self.0.contains_key(&CapName::new(name))
    }

    /// The raw value of a capability, if present and non-empty.
    pub fn value(&self, name: &str) -> Option<&str> {
        self.0.get(&CapName::new(name)).and_then(|v| v.as_deref())
    }

    /// The value split on `,` (e.g. sasl mechanisms).
    pub fn values(&self, name: &str) -> Vec<&str> {
        match self.value(name) {
            Some(v) if !v.is_empty() => v.split(',').collect(),
            _ => Vec::new(),
        }
    }

    /// Remove a capability (used for `CAP DEL`). Returns whether it was present.
    pub fn remove(&mut self, name: &str) -> bool {
        self.0.remove(&CapName::new(name)).is_some()
    }

    /// Number of capabilities.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterate over capability names.
    pub fn names(&self) -> impl Iterator<Item = &CapName> {
        self.0.keys()
    }
}

/// Assembles a multiline `CAP LS` (or `CAP NEW`) listing.
///
/// A non-final line carries a `*` continuation marker before the trailing cap
/// list; the listing is complete only on a line WITHOUT it (rule 4). Feed each
/// chunk; the assembled [`CapSet`] is returned on the final chunk and the
/// buffer resets for reuse.
#[derive(Debug, Clone, Default)]
pub struct LsAccumulator {
    pending: CapSet,
}

impl LsAccumulator {
    /// A fresh, empty accumulator.
    pub fn new() -> Self {
        LsAccumulator::default()
    }

    /// Feed one chunk of the listing. `more` is true when the line had the `*`
    /// continuation marker. Returns the completed set on the final chunk
    /// (`more == false`), otherwise `None`.
    pub fn feed(&mut self, list: &str, more: bool) -> Option<CapSet> {
        self.pending.parse_line(list);
        if more {
            None
        } else {
            Some(std::mem::take(&mut self.pending))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_parse_yields_mechanisms() {
        let cap = Cap::parse("sasl=PLAIN,EXTERNAL,SCRAM-SHA-256");
        assert_eq!(cap.name.as_str(), "sasl");
        assert_eq!(cap.values(), vec!["PLAIN", "EXTERNAL", "SCRAM-SHA-256"]);
    }

    #[test]
    fn valueless_cap_is_present_with_no_value() {
        let cap = Cap::parse("server-time");
        assert_eq!(cap.name.as_str(), "server-time");
        assert_eq!(cap.value, None);
        assert!(cap.values().is_empty());
    }

    #[test]
    fn multiline_ls_assembles_only_after_final_line() {
        let mut acc = LsAccumulator::new();
        // Continuation line (had `*`): not complete yet.
        assert!(acc.feed("cap1 cap2", true).is_none());
        // Final line (no `*`): complete.
        let set = acc.feed("cap3", false).expect("listing should complete");
        assert!(set.contains("cap1"));
        assert!(set.contains("cap2"));
        assert!(set.contains("cap3"));
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn draft_and_ratified_names_resolve_equal() {
        let mut set = CapSet::new();
        set.insert(Cap::parse("draft/chathistory"));
        assert!(set.contains("chathistory"));
        assert!(set.contains("draft/chathistory"));
        assert_eq!(set.len(), 1);

        // Names are logically equal regardless of spelling.
        assert_eq!(
            CapName::new("draft/chathistory"),
            CapName::new("chathistory")
        );
    }

    #[test]
    fn sasl_mechanisms_via_capset() {
        let mut set = CapSet::new();
        set.parse_line("sasl=PLAIN,EXTERNAL server-time batch");
        assert_eq!(set.values("sasl"), vec!["PLAIN", "EXTERNAL"]);
        assert!(set.contains("server-time"));
        assert_eq!(set.value("server-time"), None);
    }

    #[test]
    fn remove_drops_a_cap() {
        let mut set = CapSet::new();
        set.parse_line("batch server-time");
        assert!(set.remove("batch"));
        assert!(!set.contains("batch"));
        assert!(!set.remove("batch"));
    }
}
