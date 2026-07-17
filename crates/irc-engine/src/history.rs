//! CHATHISTORY request builder + pagination cursors.
//!
//! CHATHISTORY is `<subcommand> <target> <selector...> <limit>`. A selector is
//! `timestamp=<iso8601>` or `msgid=<id>`, or the bare `*` wildcard for the
//! newest page. This module only builds the request lines (pure); turning the
//! resulting `chathistory` batch into a `HistoryLoaded` event is the engine's
//! job. Errors come back as `FAIL CHATHISTORY ...` standard-replies (rule 15),
//! not a bespoke error path.
//!
//! Pagination:
//! - scroll up (older): `BEFORE <target> <oldest-cursor> <limit>`
//! - reconnect gap-fill (newer): `AFTER <target> <last-seen-cursor> <limit>`
//!
//! `msgid` cursors are preferred over timestamps when available: they are exact
//! and avoid millisecond-boundary ambiguity.

/// A CHATHISTORY selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    /// The `*` wildcard: the newest messages.
    Latest,
    /// `timestamp=<iso8601>`.
    Timestamp(String),
    /// `msgid=<id>`.
    MsgId(String),
}

impl Selector {
    /// A timestamp selector from an ISO 8601 string.
    pub fn timestamp(value: impl Into<String>) -> Self {
        Selector::Timestamp(value.into())
    }

    /// A msgid selector.
    pub fn msgid(value: impl Into<String>) -> Self {
        Selector::MsgId(value.into())
    }

    /// The wire token for this selector.
    pub fn token(&self) -> String {
        match self {
            Selector::Latest => "*".to_string(),
            Selector::Timestamp(t) => format!("timestamp={t}"),
            Selector::MsgId(id) => format!("msgid={id}"),
        }
    }
}

/// A CHATHISTORY request. Build with the constructors, then [`command`] for the
/// wire line (the engine prepends the `@label`).
///
/// [`command`]: ChatHistoryRequest::command
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatHistoryRequest {
    /// Newest messages, optionally since a point.
    Latest {
        /// Conversation target.
        target: String,
        /// `*` for the newest, or a point to start from.
        selector: Selector,
        /// Max messages.
        limit: usize,
    },
    /// Page older (scrollback up).
    Before {
        /// Conversation target.
        target: String,
        /// The point to page back from.
        selector: Selector,
        /// Max messages.
        limit: usize,
    },
    /// Fill the gap after a reconnect (newer).
    After {
        /// Conversation target.
        target: String,
        /// The point to page forward from.
        selector: Selector,
        /// Max messages.
        limit: usize,
    },
    /// Context around a point (e.g. a search hit).
    Around {
        /// Conversation target.
        target: String,
        /// The point to center on.
        selector: Selector,
        /// Max messages.
        limit: usize,
    },
    /// A bounded range between two points.
    Between {
        /// Conversation target.
        target: String,
        /// Range start.
        start: Selector,
        /// Range end.
        end: Selector,
        /// Max messages.
        limit: usize,
    },
    /// Which conversations have history in a range.
    Targets {
        /// Range start.
        start: Selector,
        /// Range end.
        end: Selector,
        /// Max targets.
        limit: usize,
    },
}

impl ChatHistoryRequest {
    /// Newest `limit` messages (initial load on join).
    pub fn latest(target: impl Into<String>, limit: usize) -> Self {
        ChatHistoryRequest::Latest {
            target: target.into(),
            selector: Selector::Latest,
            limit,
        }
    }

    /// Newest `limit` messages since a point.
    pub fn latest_since(target: impl Into<String>, selector: Selector, limit: usize) -> Self {
        ChatHistoryRequest::Latest {
            target: target.into(),
            selector,
            limit,
        }
    }

    /// Page older from a cursor (scrollback up).
    pub fn before(target: impl Into<String>, selector: Selector, limit: usize) -> Self {
        ChatHistoryRequest::Before {
            target: target.into(),
            selector,
            limit,
        }
    }

    /// Page newer from a cursor (reconnect gap-fill).
    pub fn after(target: impl Into<String>, selector: Selector, limit: usize) -> Self {
        ChatHistoryRequest::After {
            target: target.into(),
            selector,
            limit,
        }
    }

    /// Context around a cursor.
    pub fn around(target: impl Into<String>, selector: Selector, limit: usize) -> Self {
        ChatHistoryRequest::Around {
            target: target.into(),
            selector,
            limit,
        }
    }

    /// A bounded range.
    pub fn between(
        target: impl Into<String>,
        start: Selector,
        end: Selector,
        limit: usize,
    ) -> Self {
        ChatHistoryRequest::Between {
            target: target.into(),
            start,
            end,
            limit,
        }
    }

    /// Which conversations have history in a range.
    pub fn targets(start: Selector, end: Selector, limit: usize) -> Self {
        ChatHistoryRequest::Targets { start, end, limit }
    }

    /// The conversation target, if the subcommand has one (`TARGETS` does not).
    pub fn target(&self) -> Option<&str> {
        match self {
            ChatHistoryRequest::Latest { target, .. }
            | ChatHistoryRequest::Before { target, .. }
            | ChatHistoryRequest::After { target, .. }
            | ChatHistoryRequest::Around { target, .. }
            | ChatHistoryRequest::Between { target, .. } => Some(target),
            ChatHistoryRequest::Targets { .. } => None,
        }
    }

    /// The requested limit; used to decide the `complete` flag on the response.
    pub fn limit(&self) -> usize {
        match self {
            ChatHistoryRequest::Latest { limit, .. }
            | ChatHistoryRequest::Before { limit, .. }
            | ChatHistoryRequest::After { limit, .. }
            | ChatHistoryRequest::Around { limit, .. }
            | ChatHistoryRequest::Between { limit, .. }
            | ChatHistoryRequest::Targets { limit, .. } => *limit,
        }
    }

    /// The wire command (without the leading `@label`).
    pub fn command(&self) -> String {
        match self {
            ChatHistoryRequest::Latest {
                target,
                selector,
                limit,
            } => format!("CHATHISTORY LATEST {target} {} {limit}", selector.token()),
            ChatHistoryRequest::Before {
                target,
                selector,
                limit,
            } => format!("CHATHISTORY BEFORE {target} {} {limit}", selector.token()),
            ChatHistoryRequest::After {
                target,
                selector,
                limit,
            } => format!("CHATHISTORY AFTER {target} {} {limit}", selector.token()),
            ChatHistoryRequest::Around {
                target,
                selector,
                limit,
            } => format!("CHATHISTORY AROUND {target} {} {limit}", selector.token()),
            ChatHistoryRequest::Between {
                target,
                start,
                end,
                limit,
            } => format!(
                "CHATHISTORY BETWEEN {target} {} {} {limit}",
                start.token(),
                end.token()
            ),
            ChatHistoryRequest::Targets { start, end, limit } => format!(
                "CHATHISTORY TARGETS {} {} {limit}",
                start.token(),
                end.token()
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_uses_the_wildcard_selector() {
        assert_eq!(
            ChatHistoryRequest::latest("#rust", 50).command(),
            "CHATHISTORY LATEST #rust * 50"
        );
    }

    #[test]
    fn scroll_up_pages_before_a_msgid() {
        assert_eq!(
            ChatHistoryRequest::before("#rust", Selector::msgid("bbb"), 50).command(),
            "CHATHISTORY BEFORE #rust msgid=bbb 50"
        );
    }

    #[test]
    fn gap_fill_pages_after_a_timestamp() {
        assert_eq!(
            ChatHistoryRequest::after("#rust", Selector::timestamp("2026-07-17T09:00:00.000Z"), 50)
                .command(),
            "CHATHISTORY AFTER #rust timestamp=2026-07-17T09:00:00.000Z 50"
        );
    }

    #[test]
    fn between_and_targets_shapes() {
        assert_eq!(
            ChatHistoryRequest::between(
                "#rust",
                Selector::msgid("aaa"),
                Selector::msgid("zzz"),
                100
            )
            .command(),
            "CHATHISTORY BETWEEN #rust msgid=aaa msgid=zzz 100"
        );
        assert_eq!(
            ChatHistoryRequest::targets(
                Selector::timestamp("2026-07-01T00:00:00.000Z"),
                Selector::timestamp("2026-07-31T00:00:00.000Z"),
                200
            )
            .command(),
            "CHATHISTORY TARGETS timestamp=2026-07-01T00:00:00.000Z timestamp=2026-07-31T00:00:00.000Z 200"
        );
    }

    #[test]
    fn target_and_limit_accessors() {
        let r = ChatHistoryRequest::latest("#rust", 42);
        assert_eq!(r.target(), Some("#rust"));
        assert_eq!(r.limit(), 42);
        assert_eq!(
            ChatHistoryRequest::targets(Selector::Latest, Selector::Latest, 5).target(),
            None
        );
    }
}
