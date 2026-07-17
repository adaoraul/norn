//! Channel membership: prefixes, members, and a per-channel roster.
//!
//! The roster is built from `NAMES` replies (353/366) and mutated by the
//! granular membership events (join/part/quit/nick). Membership prefixes follow
//! the common PREFIX set; `multi-prefix` means a member can carry several at
//! once (e.g. `@+nick`), highest rank first.

use std::collections::BTreeMap;

/// A channel membership prefix, most privileged first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemberPrefix {
    /// `~` channel owner.
    Owner,
    /// `&` protected/admin.
    Admin,
    /// `@` operator.
    Op,
    /// `%` half-operator.
    Halfop,
    /// `+` voice.
    Voice,
}

impl MemberPrefix {
    /// Parse a single prefix symbol.
    pub fn from_symbol(c: char) -> Option<Self> {
        match c {
            '~' => Some(MemberPrefix::Owner),
            '&' => Some(MemberPrefix::Admin),
            '@' => Some(MemberPrefix::Op),
            '%' => Some(MemberPrefix::Halfop),
            '+' => Some(MemberPrefix::Voice),
            _ => None,
        }
    }

    /// The wire symbol for this prefix.
    pub fn symbol(self) -> char {
        match self {
            MemberPrefix::Owner => '~',
            MemberPrefix::Admin => '&',
            MemberPrefix::Op => '@',
            MemberPrefix::Halfop => '%',
            MemberPrefix::Voice => '+',
        }
    }

    /// Rank, lower is more privileged (owner = 0).
    pub fn rank(self) -> u8 {
        match self {
            MemberPrefix::Owner => 0,
            MemberPrefix::Admin => 1,
            MemberPrefix::Op => 2,
            MemberPrefix::Halfop => 3,
            MemberPrefix::Voice => 4,
        }
    }
}

/// A channel member: a nick plus any membership prefixes (highest rank first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The nickname, original case.
    pub nick: String,
    /// Prefixes held, sorted most-privileged first.
    pub prefixes: Vec<MemberPrefix>,
}

impl Member {
    /// The most privileged prefix, if any.
    pub fn highest(&self) -> Option<MemberPrefix> {
        self.prefixes.first().copied()
    }
}

/// Split a leading run of membership-prefix symbols from a NAMES token.
fn split_prefixes(token: &str) -> (Vec<MemberPrefix>, &str) {
    let mut prefixes = Vec::new();
    let mut rest = token;
    while let Some(c) = rest.chars().next() {
        match MemberPrefix::from_symbol(c) {
            Some(prefix) => {
                prefixes.push(prefix);
                rest = &rest[c.len_utf8()..];
            }
            None => break,
        }
    }
    prefixes.sort_by_key(|p| p.rank());
    (prefixes, rest)
}

/// The membership of one channel, keyed by lowercased nick.
#[derive(Debug, Clone, Default)]
pub struct Roster {
    members: BTreeMap<String, Member>,
}

impl Roster {
    /// An empty roster.
    pub fn new() -> Self {
        Roster::default()
    }

    /// Accumulate one `353 RPL_NAMREPLY` trailing param (space-separated,
    /// prefixed nicks). Call once per 353 line; several lines merge.
    pub fn apply_names_reply(&mut self, names: &str) {
        for token in names.split_whitespace() {
            let (prefixes, nick) = split_prefixes(token);
            if nick.is_empty() {
                continue;
            }
            self.members.insert(
                nick.to_ascii_lowercase(),
                Member {
                    nick: nick.to_string(),
                    prefixes,
                },
            );
        }
    }

    /// Add a member with no prefixes (a plain JOIN), if absent.
    pub fn insert(&mut self, nick: &str) {
        self.members
            .entry(nick.to_ascii_lowercase())
            .or_insert_with(|| Member {
                nick: nick.to_string(),
                prefixes: Vec::new(),
            });
    }

    /// Remove a member. Returns whether they were present.
    pub fn remove(&mut self, nick: &str) -> bool {
        self.members.remove(&nick.to_ascii_lowercase()).is_some()
    }

    /// Rename a member, preserving their prefixes.
    pub fn rename(&mut self, old: &str, new: &str) {
        if let Some(mut member) = self.members.remove(&old.to_ascii_lowercase()) {
            member.nick = new.to_string();
            self.members.insert(new.to_ascii_lowercase(), member);
        }
    }

    /// Whether a nick is a member.
    pub fn contains(&self, nick: &str) -> bool {
        self.members.contains_key(&nick.to_ascii_lowercase())
    }

    /// Iterate members (alphabetical by lowercased nick).
    pub fn members(&self) -> impl Iterator<Item = &Member> {
        self.members.values()
    }

    /// A snapshot of all members.
    pub fn snapshot(&self) -> Vec<Member> {
        self.members.values().cloned().collect()
    }

    /// Number of members.
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Whether the roster is empty.
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_reply_parses_prefixes() {
        let mut r = Roster::new();
        r.apply_names_reply("@alice +bob carol");
        assert_eq!(r.len(), 3);
        let alice = r.members().find(|m| m.nick == "alice").unwrap();
        assert_eq!(alice.highest(), Some(MemberPrefix::Op));
        let bob = r.members().find(|m| m.nick == "bob").unwrap();
        assert_eq!(bob.highest(), Some(MemberPrefix::Voice));
        let carol = r.members().find(|m| m.nick == "carol").unwrap();
        assert_eq!(carol.highest(), None);
    }

    #[test]
    fn multi_prefix_sorted_highest_first() {
        let mut r = Roster::new();
        r.apply_names_reply("+@dave");
        let dave = r.members().next().unwrap();
        assert_eq!(dave.prefixes, vec![MemberPrefix::Op, MemberPrefix::Voice]);
        assert_eq!(dave.highest(), Some(MemberPrefix::Op));
    }

    #[test]
    fn multiple_replies_merge() {
        let mut r = Roster::new();
        r.apply_names_reply("@alice bob");
        r.apply_names_reply("carol @dave");
        assert_eq!(r.len(), 4);
        assert!(r.contains("CAROL")); // case-insensitive
    }

    #[test]
    fn join_part_rename() {
        let mut r = Roster::new();
        r.apply_names_reply("@alice");
        r.insert("bob");
        assert!(r.contains("bob"));
        r.rename("bob", "bobby");
        assert!(!r.contains("bob"));
        assert!(r.contains("bobby"));
        assert!(r.remove("bobby"));
        assert!(!r.remove("bobby"));
    }
}
