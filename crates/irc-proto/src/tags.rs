//! IRCv3 message-tag codec: the single home for tag-value escaping.
//!
//! The escape scheme (CLAUDE.md rule 1) is the most bug-prone surface in
//! IRCv3, so it lives here and nowhere else. Unescape on parse, escape on
//! emit. Both directions are total: they never fail. See `TESTS.md` section 1
//! for the cases that pin this down.
//!
//! Escape table (in-memory value <-> value on the wire):
//!
//! | in-memory | wire |
//! |-----------|------|
//! | `;`       | `\:` |
//! | space     | `\s` |
//! | `\`       | `\\` |
//! | CR        | `\r` |
//! | LF        | `\n` |
//!
//! On unescape, a backslash before any other character drops the backslash and
//! keeps the character (`\q` -> `q`), and a lone trailing backslash is dropped.

use std::collections::HashMap;

/// Decode a tag value from its on-the-wire form to the in-memory string.
///
/// Lenient by design: an unrecognized escape (`\q`) yields the bare character,
/// and a trailing lone backslash is dropped. Never fails.
pub fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        // Saw a backslash: decode the escape pair, if any.
        match chars.next() {
            Some(':') => out.push(';'),
            Some('s') => out.push(' '),
            Some('\\') => out.push('\\'),
            Some('r') => out.push('\r'),
            Some('n') => out.push('\n'),
            // `\<other>`: drop the backslash, keep the character.
            Some(other) => out.push(other),
            // Trailing lone backslash: dropped.
            None => {}
        }
    }
    out
}

/// Encode an in-memory tag value into its on-the-wire form.
///
/// The exact inverse of [`unescape`] on the five defined escapes; every other
/// character passes through unchanged. This guarantees the round-trip property
/// `unescape(escape(s)) == s` for arbitrary strings.
pub fn escape(value: &str) -> String {
    // Worst case every char becomes a two-char escape.
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            ';' => out.push_str("\\:"),
            ' ' => out.push_str("\\s"),
            '\\' => out.push_str("\\\\"),
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}

/// A parsed tag key: `[+]<[vendor/]name>`.
///
/// - `client_only` is set by a leading `+` (e.g. `+typing`).
/// - `vendor` is the segment before the FIRST `/` (e.g. `example.com`).
/// - `name` is the remainder and may itself contain `/`
///   (e.g. `draft/react`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TagKey {
    /// Leading `+` marks a client-only tag.
    pub client_only: bool,
    /// Vendor prefix, the text before the first `/`.
    pub vendor: Option<String>,
    /// The key name; retains any further `/` segments.
    pub name: String,
}

impl TagKey {
    /// Parse a raw tag key such as `+example.com/draft/react`.
    ///
    /// Strips a single leading `+`, then splits on the first `/` into
    /// vendor/name. Total: any input yields a `TagKey`.
    pub fn parse(raw: &str) -> Self {
        let (client_only, rest) = match raw.strip_prefix('+') {
            Some(stripped) => (true, stripped),
            None => (false, raw),
        };
        match rest.split_once('/') {
            Some((vendor, name)) => TagKey {
                client_only,
                vendor: Some(vendor.to_string()),
                name: name.to_string(),
            },
            None => TagKey {
                client_only,
                vendor: None,
                name: rest.to_string(),
            },
        }
    }
}

/// A parsed, unescaped collection of message tags.
///
/// Keys are [`TagKey`] (so `client_only`, `vendor`, and `name` are structured
/// rather than string-scanned) and values are already unescaped. This is the
/// `tags` field on `Message`; higher layers do field lookups, never string
/// scans.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tags(HashMap<TagKey, String>);

impl Tags {
    /// An empty tag map.
    pub fn new() -> Self {
        Tags(HashMap::new())
    }

    /// Parse the tag section of a line (the text after `@`, before the first
    /// space; the `@` must already be stripped). Values are unescaped here,
    /// once. A segment without `=` is a bare key with an empty value.
    pub fn parse(section: &str) -> Self {
        let mut map = HashMap::new();
        for segment in section.split(';') {
            if segment.is_empty() {
                continue;
            }
            let (key, value) = match segment.split_once('=') {
                Some((k, v)) => (k, unescape(v)),
                None => (segment, String::new()),
            };
            map.insert(TagKey::parse(key), value);
        }
        Tags(map)
    }

    /// Emit the tag section (without the leading `@`). Values are escaped here,
    /// once. Bare keys (empty value) emit without a trailing `=`.
    ///
    /// Ordering follows the underlying map and is therefore unspecified; the
    /// parse of the emitted section is what round-trips, not the byte string.
    pub fn emit(&self) -> String {
        let mut parts = Vec::with_capacity(self.0.len());
        for (key, value) in &self.0 {
            let mut rendered = String::new();
            if key.client_only {
                rendered.push('+');
            }
            if let Some(vendor) = &key.vendor {
                rendered.push_str(vendor);
                rendered.push('/');
            }
            rendered.push_str(&key.name);
            if !value.is_empty() {
                rendered.push('=');
                rendered.push_str(&escape(value));
            }
            parts.push(rendered);
        }
        parts.join(";")
    }

    /// Look up a tag value by its raw key text (e.g. `"msgid"` or
    /// `"example.com/foo"`). The name is parsed into a [`TagKey`] for the
    /// lookup, so vendor and client-only prefixes match.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(&TagKey::parse(name)).map(String::as_str)
    }

    /// Insert a tag, returning any previous value for the key.
    pub fn insert(&mut self, key: TagKey, value: String) -> Option<String> {
        self.0.insert(key, value)
    }

    /// Number of tags present.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no tags.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterate over `(key, value)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&TagKey, &String)> {
        self.0.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // --- 1.1 Unescape -----------------------------------------------------

    #[test]
    fn unescape_space() {
        assert_eq!(unescape("hello\\sworld"), "hello world");
    }

    #[test]
    fn unescape_semicolon() {
        assert_eq!(unescape("a\\:b"), "a;b");
    }

    #[test]
    fn unescape_backslash() {
        assert_eq!(unescape("back\\\\slash"), "back\\slash");
    }

    #[test]
    fn unescape_cr_lf() {
        assert_eq!(unescape("line\\r\\nbreak"), "line\r\nbreak");
    }

    #[test]
    fn unescape_unknown_drops_backslash() {
        assert_eq!(unescape("\\q"), "q");
    }

    #[test]
    fn unescape_trailing_lone_backslash_dropped() {
        assert_eq!(unescape("trailing\\"), "trailing");
    }

    #[test]
    fn unescape_empty() {
        assert_eq!(unescape(""), "");
    }

    // --- 1.2 Escape (inverse) ---------------------------------------------

    #[test]
    fn escape_space() {
        assert_eq!(escape(" "), "\\s");
    }

    #[test]
    fn escape_semicolon() {
        assert_eq!(escape(";"), "\\:");
    }

    #[test]
    fn escape_backslash() {
        assert_eq!(escape("\\"), "\\\\");
    }

    #[test]
    fn escape_cr_lf() {
        assert_eq!(escape("\r"), "\\r");
        assert_eq!(escape("\n"), "\\n");
    }

    // --- 1.2 Round-trip property ------------------------------------------

    proptest! {
        // Arbitrary strings, biased toward the tricky bytes so the escape
        // boundary is exercised heavily.
        #[test]
        fn roundtrip_unescape_of_escape(s in tricky_string()) {
            prop_assert_eq!(unescape(&escape(&s)), s);
        }
    }

    /// A string strategy that samples freely from `;`, space, `\`, CR, LF, and
    /// ordinary characters.
    fn tricky_string() -> impl Strategy<Value = String> {
        let ch = prop_oneof![
            Just(';'),
            Just(' '),
            Just('\\'),
            Just('\r'),
            Just('\n'),
            any::<char>(),
        ];
        proptest::collection::vec(ch, 0..64).prop_map(|v| v.into_iter().collect())
    }

    // --- 1.3 Key parsing --------------------------------------------------

    #[test]
    fn key_client_only_no_vendor() {
        assert_eq!(
            TagKey::parse("+typing"),
            TagKey {
                client_only: true,
                vendor: None,
                name: "typing".to_string(),
            }
        );
    }

    #[test]
    fn key_vendor_and_name() {
        assert_eq!(
            TagKey::parse("example.com/foo"),
            TagKey {
                client_only: false,
                vendor: Some("example.com".to_string()),
                name: "foo".to_string(),
            }
        );
    }

    #[test]
    fn key_client_only_vendor_and_slashed_name() {
        assert_eq!(
            TagKey::parse("+example.com/draft/react"),
            TagKey {
                client_only: true,
                vendor: Some("example.com".to_string()),
                name: "draft/react".to_string(),
            }
        );
    }
}
