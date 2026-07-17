//! The semantic emitter: wires the runtime pipeline and produces `Event`s.
//!
//! Post-registration messages flow through the batch collector; the resulting
//! passthroughs and completed batches are mapped to semantic events. This is
//! where CHATHISTORY becomes the integration proof: a labeled command whose
//! response is a `chathistory` batch of ordinary messages, each carrying its
//! original `time`/`msgid`, is turned into one `HistoryLoaded` with no special
//! casing beyond correlating the label to the requested limit.

use std::collections::HashMap;

use irc_proto::Message;

use crate::batch::{BatchCollector, CollectorOutput, CompletedBatch};
use crate::chat::ChatMessage;
use crate::event::Event;
use crate::history::ChatHistoryRequest;
use crate::identity::identity_event;
use crate::stdreply::StandardReply;

/// Wires batch collection and request correlation into semantic events.
#[derive(Debug, Default)]
pub struct Engine {
    batches: BatchCollector,
    /// Outstanding CHATHISTORY requests, keyed by the label we attached, so a
    /// closing batch knows its requested limit (for the `complete` flag).
    history: HashMap<String, ChatHistoryRequest>,
    next_label: u64,
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
        self.next_label += 1;
        let label = self.next_label.to_string();
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
        // Standard replies (rule 15) take precedence over chat interpretation.
        if let Some(reply) = StandardReply::from_message(msg) {
            events.push(Event::StandardReply(reply));
            return;
        }
        // Membership/identity commands (JOIN/PART/QUIT/NICK/ACCOUNT/...).
        if let Some(event) = identity_event(msg) {
            events.push(event);
            return;
        }
        if let Some(chat) = ChatMessage::from_message(msg) {
            events.push(Event::MessageReceived(chat));
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
