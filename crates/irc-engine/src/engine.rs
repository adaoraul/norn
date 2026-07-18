//! The semantic emitter: wires the runtime pipeline and produces `Event`s.
//!
//! Post-registration messages flow through the batch collector; the resulting
//! passthroughs and completed batches are mapped to semantic events. This is
//! where CHATHISTORY becomes the integration proof: a labeled command whose
//! response is a `chathistory` batch of ordinary messages, each carrying its
//! original `time`/`msgid`, is turned into one `HistoryLoaded` with no special
//! casing beyond correlating the label to the requested limit.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use irc_proto::{Command, Message};

use crate::batch::{BatchCollector, CollectorOutput, CompletedBatch};
use crate::chat::ChatMessage;
use crate::event::{Event, LeaveReason, TopicChange, WhoisInfo};
use crate::history::ChatHistoryRequest;
use crate::identity::identity_event;
use crate::labels::LabelRouter;
use crate::roster::Roster;
use crate::stdreply::StandardReply;

/// Normalize a channel name for roster keying (ASCII case-insensitive).
fn norm(channel: &str) -> String {
    channel.to_ascii_lowercase()
}

/// Wires batch collection and request correlation into semantic events.
#[derive(Debug, Default)]
pub struct Engine {
    batches: BatchCollector,
    /// Outstanding CHATHISTORY requests, keyed by the label we attached, so a
    /// closing batch knows its requested limit (for the `complete` flag).
    history: HashMap<String, ChatHistoryRequest>,
    /// Per-channel membership, accumulated from NAMES and membership events.
    rosters: HashMap<String, Roster>,
    /// The single `@label` authority for this connection. CHATHISTORY mints
    /// bare labels here (correlated via `history` on batch close); awaited
    /// labeled commands would `register` for a oneshot on the same sequence.
    labels: LabelRouter,
    /// In-progress WHOIS replies, keyed by normalized nick. Populated by the
    /// whois numerics and drained into one `WhoisReceived` at end-of-whois (318).
    whois: HashMap<String, WhoisInfo>,
}

impl Engine {
    /// A fresh engine.
    pub fn new() -> Self {
        Engine::default()
    }

    /// Build a CHATHISTORY request line (with a fresh `@label`) and record its
    /// context so the response can be turned into `HistoryLoaded`. Send the
    /// returned line to the server.
    pub fn request_history(&mut self, request: ChatHistoryRequest) -> String {
        let label = self.labels.allocate();
        let line = format!("@label={label} {}", request.command());
        self.history.insert(label, request);
        line
    }

    /// Feed one post-registration message; return the semantic events it
    /// produced (usually zero or one).
    pub fn handle(&mut self, msg: Message) -> Vec<Event> {
        let mut events = Vec::new();
        match self.batches.handle(msg) {
            None => {}
            Some(CollectorOutput::Passthrough(m)) => self.on_passthrough(&m, &mut events),
            Some(CollectorOutput::Batch(b)) => self.on_batch(b, &mut events),
        }
        events
    }

    fn on_passthrough(&mut self, msg: &Message, events: &mut Vec<Event>) {
        // Channel state (NAMES/TOPIC): numerics and TOPIC that are neither a
        // standard reply, an identity command, nor a chat message.
        if self.handle_channel_state(msg, events) {
            return;
        }
        // WHOIS reply numerics accumulate into one WhoisReceived at 318.
        if self.handle_whois(msg, events) {
            return;
        }
        // Standard replies (rule 15) take precedence over chat interpretation.
        if let Some(reply) = StandardReply::from_message(msg) {
            events.push(Event::StandardReply(reply));
            return;
        }
        // Membership/identity commands (JOIN/PART/QUIT/NICK/ACCOUNT/...).
        if let Some(event) = identity_event(msg) {
            self.update_roster(&event);
            events.push(event);
            return;
        }
        if let Some(chat) = ChatMessage::from_message(msg) {
            events.push(Event::MessageReceived(chat));
        }
    }

    /// Handle NAMES (353/366) and TOPIC (331/332/333 + the `TOPIC` command),
    /// updating the roster and emitting events. Returns whether it applied.
    fn handle_channel_state(&mut self, msg: &Message, events: &mut Vec<Event>) -> bool {
        match &msg.command {
            // 353 RPL_NAMREPLY: <me> <symbol> <channel> :<prefixed nicks>
            Command::Numeric(353) => {
                let (Some(channel), Some(names)) = (msg.params.get(2), msg.params.get(3)) else {
                    return true;
                };
                self.rosters
                    .entry(norm(channel))
                    .or_default()
                    .apply_names_reply(names);
                true
            }
            // 366 RPL_ENDOFNAMES: <me> <channel> :End of /NAMES
            Command::Numeric(366) => {
                if let Some(channel) = msg.params.get(1) {
                    let members = self
                        .rosters
                        .get(&norm(channel))
                        .map(Roster::snapshot)
                        .unwrap_or_default();
                    events.push(Event::NamesLoaded {
                        target: channel.clone(),
                        members,
                    });
                }
                true
            }
            // 332 RPL_TOPIC: <me> <channel> :<topic>
            Command::Numeric(332) => {
                if let Some(channel) = msg.params.get(1) {
                    events.push(Event::TopicChanged {
                        target: channel.clone(),
                        change: TopicChange::Set(msg.params.get(2).cloned().unwrap_or_default()),
                        set_by: None,
                        set_at: None,
                    });
                }
                true
            }
            // 331 RPL_NOTOPIC: <me> <channel> :No topic is set
            Command::Numeric(331) => {
                if let Some(channel) = msg.params.get(1) {
                    events.push(Event::TopicChanged {
                        target: channel.clone(),
                        change: TopicChange::Cleared,
                        set_by: None,
                        set_at: None,
                    });
                }
                true
            }
            // 333 RPL_TOPICWHOTIME: <me> <channel> <setter> <unixtime>
            Command::Numeric(333) => {
                if let Some(channel) = msg.params.get(1) {
                    let set_at = msg
                        .params
                        .get(3)
                        .and_then(|s| s.parse::<i64>().ok())
                        .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0));
                    events.push(Event::TopicChanged {
                        target: channel.clone(),
                        // Metadata only: never touch the stored topic text.
                        change: TopicChange::Unchanged,
                        set_by: msg.params.get(2).cloned(),
                        set_at,
                    });
                }
                true
            }
            // Live topic change: :nick!u@h TOPIC <channel> :<new topic>
            Command::Named(name) if name == "TOPIC" => {
                if let Some(channel) = msg.params.first() {
                    // A present-but-empty trailing param is a genuine clear.
                    let change = match msg.params.get(1) {
                        Some(t) if !t.is_empty() => TopicChange::Set(t.clone()),
                        _ => TopicChange::Cleared,
                    };
                    let set_by = msg.source.as_ref().and_then(|s| match s {
                        irc_proto::Source::User { nick, .. } => Some(nick.clone()),
                        irc_proto::Source::Server(_) => None,
                    });
                    events.push(Event::TopicChanged {
                        target: channel.clone(),
                        change,
                        set_by,
                        set_at: msg.server_time(),
                    });
                }
                true
            }
            // 352 RPL_WHOREPLY: <me> <chan> <user> <host> <server> <nick> <flags> :<hop> <real>
            // The first flag char is H (here) or G (gone/away). `away-notify`
            // only reports live transitions, so a WHO poll on join seeds the
            // initial away state; emit AwayChanged for members flagged away.
            Command::Numeric(352) => {
                if let (Some(nick), Some(flags)) = (msg.params.get(5), msg.params.get(6)) {
                    if flags.starts_with('G') {
                        for roster in self.rosters.values_mut() {
                            roster.set_away(nick, true);
                        }
                        events.push(Event::AwayChanged {
                            nick: nick.clone(),
                            // WHO does not carry the away text, only the flag.
                            message: Some(String::new()),
                        });
                    }
                }
                true
            }
            // 315 RPL_ENDOFWHO: consume it (the roster is already seeded).
            Command::Numeric(315) => true,
            _ => false,
        }
    }

    /// Accumulate WHOIS reply numerics into a per-nick [`WhoisInfo`] and emit one
    /// `WhoisReceived` at end-of-whois (318). For every whois numeric the target
    /// nick is `params[1]`. Returns whether the message was a whois numeric (and
    /// thus consumed). A standalone RPL_AWAY (301) that is not part of an
    /// in-progress whois is left for other handlers.
    fn handle_whois(&mut self, msg: &Message, events: &mut Vec<Event>) -> bool {
        let Command::Numeric(n) = &msg.command else {
            return false;
        };
        let n = *n;
        if !matches!(n, 301 | 311 | 312 | 313 | 317 | 318 | 319 | 330 | 671) {
            return false;
        }
        let Some(nick) = msg.params.get(1).cloned() else {
            return true; // malformed whois numeric: consume it silently
        };
        let key = norm(&nick);
        match n {
            // 311 RPL_WHOISUSER: <me> <nick> <user> <host> * :<realname>
            311 => {
                let e = self.whois.entry(key).or_default();
                e.nick = nick;
                e.user = msg.params.get(2).cloned();
                e.host = msg.params.get(3).cloned();
                e.realname = msg.params.get(5).cloned();
            }
            // 312 RPL_WHOISSERVER: <me> <nick> <server> :<server info>
            312 => {
                let e = self.whois.entry(key).or_default();
                e.nick = nick;
                e.server = msg.params.get(2).cloned();
            }
            // 313 RPL_WHOISOPERATOR
            313 => {
                let e = self.whois.entry(key).or_default();
                e.nick = nick;
                e.is_operator = true;
            }
            // 317 RPL_WHOISIDLE: <me> <nick> <seconds> [<signon>] :...
            317 => {
                let e = self.whois.entry(key).or_default();
                e.nick = nick;
                e.idle_secs = msg.params.get(2).and_then(|s| s.parse().ok());
                e.signon = msg
                    .params
                    .get(3)
                    .and_then(|s| s.parse::<i64>().ok())
                    .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0));
            }
            // 319 RPL_WHOISCHANNELS: <me> <nick> :<prefixed channel list>
            319 => {
                let e = self.whois.entry(key).or_default();
                e.nick = nick;
                e.channels = msg.params.get(2).cloned().filter(|c| !c.is_empty());
            }
            // 330 RPL_WHOISACCOUNT: <me> <nick> <account> :is logged in as
            330 => {
                let e = self.whois.entry(key).or_default();
                e.nick = nick;
                e.account = msg.params.get(2).cloned();
            }
            // 671 RPL_WHOISSECURE
            671 => {
                let e = self.whois.entry(key).or_default();
                e.nick = nick;
                e.secure = true;
            }
            // 301 RPL_AWAY: only fold into an in-progress whois; a bare away
            // notice (when messaging an away user) is left for other handlers.
            301 => match self.whois.get_mut(&key) {
                Some(e) => e.away = msg.params.get(2).cloned(),
                None => return false,
            },
            // 318 RPL_ENDOFWHOIS: emit what we gathered (or a bare nick).
            318 => {
                let info = self.whois.remove(&key).unwrap_or(WhoisInfo {
                    nick,
                    ..Default::default()
                });
                events.push(Event::WhoisReceived(info));
            }
            _ => unreachable!("guarded by the matches! above"),
        }
        true
    }

    /// Mirror a membership event into the per-channel rosters so the engine's
    /// roster stays authoritative alongside the emitted deltas.
    fn update_roster(&mut self, event: &Event) {
        match event {
            Event::MemberJoined { target, who, .. } => {
                self.rosters
                    .entry(norm(target))
                    .or_default()
                    .insert(&who.nick);
            }
            Event::MemberLeft {
                target,
                who,
                reason,
            } => match reason {
                LeaveReason::Quit(_) => {
                    for roster in self.rosters.values_mut() {
                        roster.remove(&who.nick);
                    }
                }
                _ => {
                    if let Some(roster) = self.rosters.get_mut(&norm(target)) {
                        roster.remove(&who.nick);
                    }
                }
            },
            Event::NickChanged { old, new } => {
                for roster in self.rosters.values_mut() {
                    roster.rename(old, new);
                }
            }
            // `away-notify` is network-wide: mirror it into every channel the
            // engine tracks so NAMES snapshots stay accurate.
            Event::AwayChanged { nick, message } => {
                let away = message.is_some();
                for roster in self.rosters.values_mut() {
                    roster.set_away(nick, away);
                }
            }
            _ => {}
        }
    }

    fn on_batch(&mut self, batch: CompletedBatch, events: &mut Vec<Event>) {
        match batch.batch_type.as_str() {
            "chathistory" | "draft/chathistory" => self.on_history_batch(batch, events),
            "netsplit" | "netjoin" => events.push(Event::BatchCollapsed(batch)),
            _ => {
                // Other batches (e.g. labeled-response with no dedicated
                // handler yet): surface any chat members individually.
                for member in batch.messages() {
                    if let Some(chat) = ChatMessage::from_message(member) {
                        events.push(Event::MessageReceived(chat));
                    }
                }
            }
        }
    }

    fn on_history_batch(&mut self, batch: CompletedBatch, events: &mut Vec<Event>) {
        // The limit came from our request; correlate by the echoed label.
        let request = batch.label().and_then(|label| self.history.remove(label));

        let target = request
            .as_ref()
            .and_then(|r| r.target().map(String::from))
            .or_else(|| batch.params.first().cloned())
            .unwrap_or_default();

        let returned = batch.messages().len();
        // Exactly the limit returned means there may be more (complete = false);
        // fewer means we have it all. Unknown limit defaults to complete.
        let complete = match request.as_ref().map(|r| r.limit()) {
            Some(limit) => returned < limit,
            None => true,
        };

        let messages: Vec<ChatMessage> = batch
            .messages()
            .into_iter()
            .filter_map(ChatMessage::from_message)
            .collect();

        events.push(Event::HistoryLoaded {
            target,
            messages,
            complete,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::MessageKind;
    use crate::history::ChatHistoryRequest;
    use chrono::SecondsFormat;

    fn label_of(line: &str) -> String {
        Message::parse(line).unwrap().label().unwrap().to_string()
    }

    fn feed(e: &mut Engine, line: &str) -> Vec<Event> {
        e.handle(Message::parse(line).unwrap())
    }

    #[test]
    fn names_reply_accumulates_then_emits_at_end() {
        let mut e = Engine::new();
        assert!(feed(&mut e, ":s 353 me = #rust :@alice +bob").is_empty());
        assert!(feed(&mut e, ":s 353 me = #rust :carol").is_empty());
        let events = feed(&mut e, ":s 366 me #rust :End of /NAMES list");
        assert_eq!(events.len(), 1);
        let Event::NamesLoaded { target, members } = &events[0] else {
            panic!("expected NamesLoaded, got {:?}", events[0]);
        };
        assert_eq!(target, "#rust");
        assert_eq!(members.len(), 3);
        let alice = members.iter().find(|m| m.nick == "alice").unwrap();
        assert_eq!(alice.highest(), Some(crate::roster::MemberPrefix::Op));
    }

    #[test]
    fn topic_numeric_and_whotime() {
        let mut e = Engine::new();
        let events = feed(&mut e, ":s 332 me #rust :Rust programming");
        assert!(matches!(
            &events[0],
            Event::TopicChanged { target, change: TopicChange::Set(t), .. }
                if target == "#rust" && t == "Rust programming"
        ));
        let events = feed(&mut e, ":s 333 me #rust setter 1600000000");
        let Event::TopicChanged {
            change,
            set_by,
            set_at,
            ..
        } = &events[0]
        else {
            panic!("expected TopicChanged");
        };
        // Metadata only: unchanged, so a consumer keeps the stored text.
        assert_eq!(*change, TopicChange::Unchanged);
        assert_eq!(set_by.as_deref(), Some("setter"));
        assert!(set_at.is_some());
    }

    #[test]
    fn who_reply_seeds_away_state() {
        let mut e = Engine::new();
        // An away member (G flag) emits AwayChanged; a present one (H) does not.
        let away = feed(&mut e, ":s 352 me #rust ~u host serv alice G@ :0 Alice");
        assert!(matches!(
            &away[0],
            Event::AwayChanged { nick, message: Some(m) } if nick == "alice" && m.is_empty()
        ));
        assert!(feed(&mut e, ":s 352 me #rust ~u host serv bob H+ :0 Bob").is_empty());
        // End-of-WHO is consumed, not surfaced.
        assert!(feed(&mut e, ":s 315 me #rust :End of /WHO list").is_empty());
    }

    #[test]
    fn notopic_and_live_topic() {
        let mut e = Engine::new();
        assert!(matches!(
            &feed(&mut e, ":s 331 me #rust :No topic is set")[0],
            Event::TopicChanged {
                change: TopicChange::Cleared,
                ..
            }
        ));
        assert!(matches!(
            &feed(&mut e, ":op!u@h TOPIC #rust :new topic")[0],
            Event::TopicChanged { change: TopicChange::Set(t), set_by: Some(by), .. }
                if t == "new topic" && by == "op"
        ));
        // A live TOPIC with an empty trailing is a genuine clear (distinct from
        // the metadata-only 333, which would be Unchanged).
        assert!(matches!(
            &feed(&mut e, ":op!u@h TOPIC #rust :")[0],
            Event::TopicChanged {
                change: TopicChange::Cleared,
                set_by: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn roster_tracks_join_part_nick_across_events() {
        let mut e = Engine::new();
        feed(&mut e, ":s 353 me = #rust :@alice");
        feed(&mut e, ":s 366 me #rust :End");
        feed(&mut e, ":bob!u@h JOIN #rust");
        // The engine's roster now has alice + bob; re-emit via a fresh 366 path
        // is not exposed, so assert indirectly through a NAMES snapshot request:
        feed(&mut e, ":carol!u@h NICK caroline");
        feed(&mut e, ":bob!u@h PART #rust :bye");
        // Drive another end-of-names on the same channel to snapshot the roster.
        feed(&mut e, ":s 353 me = #rust :@alice caroline");
        let events = feed(&mut e, ":s 366 me #rust :End");
        let Event::NamesLoaded { members, .. } = &events[0] else {
            panic!("expected NamesLoaded");
        };
        let nicks: Vec<&str> = members.iter().map(|m| m.nick.as_str()).collect();
        assert!(nicks.contains(&"alice"));
        assert!(!nicks.contains(&"bob")); // parted
    }

    #[test]
    fn unrelated_numeric_yields_no_event() {
        let mut e = Engine::new();
        assert!(feed(&mut e, ":s 375 me :- Message of the Day -").is_empty());
    }

    #[test]
    fn whois_accumulates_then_emits_at_end() {
        let mut e = Engine::new();
        // The reply numerics accumulate; only 318 emits one WhoisReceived.
        assert!(feed(&mut e, ":s 311 me alice ~u host.example * :Alice A").is_empty());
        assert!(feed(&mut e, ":s 319 me alice :@#rust +#ratatui").is_empty());
        assert!(feed(&mut e, ":s 330 me alice aliceacct :is logged in as").is_empty());
        assert!(feed(&mut e, ":s 317 me alice 42 1700000000 :seconds idle").is_empty());
        assert!(feed(&mut e, ":s 671 me alice :is using a secure connection").is_empty());
        let events = feed(&mut e, ":s 318 me alice :End of /WHOIS list");
        assert_eq!(events.len(), 1);
        let Event::WhoisReceived(info) = &events[0] else {
            panic!("expected WhoisReceived, got {:?}", events[0]);
        };
        assert_eq!(info.nick, "alice");
        assert_eq!(info.user.as_deref(), Some("~u"));
        assert_eq!(info.host.as_deref(), Some("host.example"));
        assert_eq!(info.realname.as_deref(), Some("Alice A"));
        assert_eq!(info.channels.as_deref(), Some("@#rust +#ratatui"));
        assert_eq!(info.account.as_deref(), Some("aliceacct"));
        assert_eq!(info.idle_secs, Some(42));
        assert!(info.secure);
        // The accumulator is drained after the end.
        let again = feed(&mut e, ":s 318 me alice :End of /WHOIS list");
        assert!(matches!(&again[0], Event::WhoisReceived(i) if i.user.is_none()));
    }

    #[test]
    fn standalone_away_numeric_is_not_a_whois() {
        // RPL_AWAY outside a whois (messaging an away user) must not fabricate a
        // whois block; it is simply consumed with no event.
        let mut e = Engine::new();
        assert!(feed(&mut e, ":s 301 me bob :gone fishing").is_empty());
    }

    // The integration proof: label routing + batch collection + server-time all
    // meet, yielding exactly one correctly ordered, correctly timestamped
    // HistoryLoaded (TESTS.md section 8).
    #[test]
    fn chathistory_latest_yields_one_history_loaded() {
        let mut e = Engine::new();
        let line = e.request_history(ChatHistoryRequest::latest("#rust", 2));
        let label = label_of(&line);
        assert_eq!(line, format!("@label={label} CHATHISTORY LATEST #rust * 2"));

        let mut events = Vec::new();
        events.extend(
            e.handle(
                Message::parse(&format!(
                    "@label={label} :server BATCH +hist chathistory #rust"
                ))
                .unwrap(),
            ),
        );
        events.extend(e.handle(
            Message::parse(
                "@batch=hist;time=2026-07-17T09:00:01.000Z;msgid=aaa :nick!u@h PRIVMSG #rust :older line",
            )
            .unwrap(),
        ));
        events.extend(e.handle(
            Message::parse(
                "@batch=hist;time=2026-07-17T09:00:02.000Z;msgid=bbb :nick!u@h PRIVMSG #rust :newer line",
            )
            .unwrap(),
        ));
        events.extend(e.handle(Message::parse(":server BATCH -hist").unwrap()));

        assert_eq!(events.len(), 1, "exactly one HistoryLoaded");
        let Event::HistoryLoaded {
            target,
            messages,
            complete,
        } = &events[0]
        else {
            panic!("expected HistoryLoaded, got {:?}", events[0]);
        };

        assert_eq!(target, "#rust");
        // Returned exactly the limit (2), so there may be more.
        assert!(!complete);
        assert_eq!(messages.len(), 2);

        // Order preserved, original time/msgid retained (rule 13).
        assert_eq!(messages[0].kind, MessageKind::Privmsg);
        assert_eq!(messages[0].msgid.as_deref(), Some("aaa"));
        assert_eq!(messages[0].text, "older line");
        assert_eq!(
            messages[0]
                .time
                .unwrap()
                .to_rfc3339_opts(SecondsFormat::Millis, true),
            "2026-07-17T09:00:01.000Z"
        );
        assert_eq!(messages[1].msgid.as_deref(), Some("bbb"));
        assert_eq!(messages[1].text, "newer line");
    }

    #[test]
    fn fewer_than_limit_marks_complete() {
        let mut e = Engine::new();
        let line = e.request_history(ChatHistoryRequest::latest("#rust", 50));
        let label = label_of(&line);

        e.handle(Message::parse(&format!("@label={label} :s BATCH +h chathistory #rust")).unwrap());
        e.handle(Message::parse("@batch=h;msgid=x :n!u@h PRIVMSG #rust :only one").unwrap());
        let events = e.handle(Message::parse(":s BATCH -h").unwrap());

        let Event::HistoryLoaded {
            complete, messages, ..
        } = &events[0]
        else {
            panic!("expected HistoryLoaded");
        };
        assert!(complete); // 1 < 50
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn fail_chathistory_becomes_one_standard_reply() {
        let mut e = Engine::new();
        let events = e.handle(
            Message::parse("FAIL CHATHISTORY INVALID_PARAMS :Messages could not be retrieved")
                .unwrap(),
        );
        assert_eq!(events.len(), 1);
        let Event::StandardReply(sr) = &events[0] else {
            panic!("expected StandardReply, got {:?}", events[0]);
        };
        assert_eq!(sr.command, "CHATHISTORY");
        assert_eq!(sr.code, "INVALID_PARAMS");
    }

    #[test]
    fn plain_privmsg_surfaces_as_message_received() {
        let mut e = Engine::new();
        let events =
            e.handle(Message::parse("@msgid=z :bob!u@h PRIVMSG #rust :live message").unwrap());
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], Event::MessageReceived(_)));
    }

    #[test]
    fn join_surfaces_as_member_joined() {
        let mut e = Engine::new();
        let events = e.handle(Message::parse(":nick!u@h JOIN #rust adao :Adao").unwrap());
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            Event::MemberJoined { target, account: Some(a), .. }
                if target == "#rust" && a == "adao"
        ));
    }

    #[test]
    fn netsplit_batch_collapses_to_one_event() {
        let mut e = Engine::new();
        e.handle(Message::parse("BATCH +ns netsplit irc.a irc.b").unwrap());
        e.handle(Message::parse("@batch=ns :a!u@h QUIT :*.net *.split").unwrap());
        e.handle(Message::parse("@batch=ns :b!u@h QUIT :*.net *.split").unwrap());
        let events = e.handle(Message::parse("BATCH -ns").unwrap());

        assert_eq!(events.len(), 1);
        let Event::BatchCollapsed(b) = &events[0] else {
            panic!("expected BatchCollapsed");
        };
        assert_eq!(b.batch_type, "netsplit");
        assert_eq!(b.messages().len(), 2);
    }
}
