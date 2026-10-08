//! Batch collector: buffers `@batch` groups and emits each as one unit.
//!
//! `BATCH +<ref> <type> [params]` opens a batch, member lines carry
//! `@batch=<ref>`, and `BATCH -<ref>` closes it. Batches nest, so this tracks a
//! stack keyed by ref (rule 11); a nested batch is folded into its parent as a
//! single item, and only a completed top-level batch is handed out. Individual
//! `@batch=` lines never leak.
//!
//! This is sans-I/O and synchronous. It produces structural [`CompletedBatch`]
//! values; mapping a batch to a semantic event (`HistoryLoaded` for
//! `chathistory`, `Netsplit`/`Netjoin` for `netsplit`/`netjoin`) is the emitter's
//! job in a later step.

use irc_proto::{Command, Message, Tags};

/// One element of a batch, in receive order: either a plain message or a nested
/// batch folded in as a unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchItem {
    /// A member message (retains all its tags, e.g. `time`/`msgid`).
    Message(Message),
    /// A nested batch, already completed.
    Batch(CompletedBatch),
}

/// A closed batch and everything it contained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedBatch {
    /// The batch reference from `+<ref>`.
    pub reference: String,
    /// The batch type (e.g. `chathistory`, `netsplit`, `labeled-response`).
    pub batch_type: String,
    /// Any params after the type on the `BATCH +` line (e.g. the target).
    pub params: Vec<String>,
    /// Tags on the `BATCH +` open line (e.g. `label`).
    pub tags: Tags,
    /// Contents in receive order.
    pub items: Vec<BatchItem>,
}

impl CompletedBatch {
    /// The `label` tag on the open line, if this batch answers a labeled
    /// command.
    pub fn label(&self) -> Option<&str> {
        self.tags.get("label")
    }

    /// All member messages flattened depth-first, nested batches inlined.
    pub fn messages(&self) -> Vec<&Message> {
        let mut out = Vec::new();
        for item in &self.items {
            match item {
                BatchItem::Message(m) => out.push(m),
                BatchItem::Batch(b) => out.extend(b.messages()),
            }
        }
        out
    }

    /// Number of top-level items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the batch has no items.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// An in-progress batch on the collector's stack.
#[derive(Debug)]
struct OpenBatch {
    reference: String,
    batch_type: String,
    params: Vec<String>,
    tags: Tags,
    /// Parent batch ref (from `@batch` on the open line), if nested.
    parent: Option<String>,
    items: Vec<BatchItem>,
}

/// What the collector produced for a single input message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectorOutput {
    /// A normal message not belonging to any batch.
    Passthrough(Message),
    /// A completed top-level batch.
    Batch(CompletedBatch),
}

/// Collects nested `@batch` groups.
#[derive(Debug, Default)]
pub struct BatchCollector {
    stack: Vec<OpenBatch>,
}

impl BatchCollector {
    /// A fresh collector.
    pub fn new() -> Self {
        BatchCollector::default()
    }

    /// Whether any batch is currently open.
    pub fn is_active(&self) -> bool {
        !self.stack.is_empty()
    }

    /// Feed one message. Returns:
    /// - `None` while the message is buffered inside an open batch,
    /// - `Some(Passthrough)` for a non-batched message,
    /// - `Some(Batch)` when a top-level batch closes.
    pub fn handle(&mut self, msg: Message) -> Option<CollectorOutput> {
        if let Command::Named(name) = &msg.command {
            if name == "BATCH" {
                return self.handle_control(msg);
            }
        }

        // A member of an open batch is buffered; anything else passes through
        // even while a batch is open (concurrent non-batched traffic).
        if let Some(reference) = msg.batch_ref().map(String::from) {
            if self.has_open(&reference) {
                self.append_to(&reference, BatchItem::Message(msg));
                return None;
            }
        }
        Some(CollectorOutput::Passthrough(msg))
    }

    fn handle_control(&mut self, msg: Message) -> Option<CollectorOutput> {
        let first = msg.params.first().cloned().unwrap_or_default();

        if let Some(reference) = first.strip_prefix('+') {
            let batch_type = msg.params.get(1).cloned().unwrap_or_default();
            let params = msg.params.iter().skip(2).cloned().collect();
            let parent = msg.batch_ref().map(String::from);
            self.stack.push(OpenBatch {
                reference: reference.to_string(),
                batch_type,
                params,
                tags: msg.tags.clone(),
                parent,
                items: Vec::new(),
            });
            None
        } else if let Some(reference) = first.strip_prefix('-') {
            self.close(reference)
        } else {
            Some(CollectorOutput::Passthrough(msg))
        }
    }

    fn close(&mut self, reference: &str) -> Option<CollectorOutput> {
        let pos = self.stack.iter().rposition(|b| b.reference == reference)?;
        let open = self.stack.remove(pos);
        let parent = open.parent.clone();
        let completed = CompletedBatch {
            reference: open.reference,
            batch_type: open.batch_type,
            params: open.params,
            tags: open.tags,
            items: open.items,
        };
        match parent {
            // Nested: fold into the parent as one item; nothing emitted yet.
            Some(parent_ref) => {
                self.append_to(&parent_ref, BatchItem::Batch(completed));
                None
            }
            None => Some(CollectorOutput::Batch(completed)),
        }
    }

    fn has_open(&self, reference: &str) -> bool {
        self.stack.iter().any(|b| b.reference == reference)
    }

    fn append_to(&mut self, reference: &str, item: BatchItem) {
        if let Some(b) = self
            .stack
            .iter_mut()
            .rev()
            .find(|b| b.reference == reference)
        {
            b.items.push(item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Message {
        Message::parse(line).unwrap()
    }

    #[test]
    fn simple_batch_emits_one_unit_with_no_leaks() {
        let mut c = BatchCollector::new();
        assert_eq!(c.handle(parse("BATCH +x netjoin")), None);
        // The inner line is buffered, not leaked.
        assert_eq!(
            c.handle(parse("@batch=x :n!u@h JOIN #c")),
            None,
            "inner line must not pass through"
        );
        let out = c.handle(parse("BATCH -x")).expect("batch should close");
        match out {
            CollectorOutput::Batch(b) => {
                assert_eq!(b.batch_type, "netjoin");
                assert_eq!(b.len(), 1);
            }
            other => panic!("expected Batch, got {other:?}"),
        }
        assert!(!c.is_active());
    }

    #[test]
    fn nested_batches_reconstruct_via_the_ref_stack() {
        let mut c = BatchCollector::new();
        assert_eq!(c.handle(parse("BATCH +outer draft/x")), None);
        assert_eq!(c.handle(parse("@batch=outer BATCH +inner draft/x")), None);
        assert_eq!(
            c.handle(parse("@batch=inner :n!u@h PRIVMSG #c :inner-line")),
            None
        );
        assert_eq!(c.handle(parse("@batch=outer BATCH -inner")), None);
        assert_eq!(
            c.handle(parse("@batch=outer :n!u@h PRIVMSG #c :outer-line")),
            None
        );
        let out = c.handle(parse("BATCH -outer")).expect("outer closes");

        let CollectorOutput::Batch(outer) = out else {
            panic!("expected Batch");
        };
        // Outer holds: [nested inner batch, outer-line message].
        assert_eq!(outer.len(), 2);
        assert!(matches!(outer.items[0], BatchItem::Batch(_)));
        assert!(matches!(outer.items[1], BatchItem::Message(_)));

        // Flattened, messages come out in receive order.
        let flat: Vec<&str> = outer
            .messages()
            .iter()
            .map(|m| m.params.last().unwrap().as_str())
            .collect();
        assert_eq!(flat, vec!["inner-line", "outer-line"]);
    }

    #[test]
    fn netsplit_collapses_many_quits_into_one_batch() {
        let mut c = BatchCollector::new();
        assert_eq!(c.handle(parse("BATCH +ns netsplit irc.a irc.b")), None);
        for nick in ["a", "b", "c", "d"] {
            let line = format!("@batch=ns :{nick}!u@h QUIT :*.net *.split");
            assert_eq!(c.handle(parse(&line)), None);
        }
        let out = c.handle(parse("BATCH -ns")).expect("closes");
        let CollectorOutput::Batch(b) = out else {
            panic!("expected Batch");
        };
        // One grouped unit carrying all four quits, none leaked.
        assert_eq!(b.batch_type, "netsplit");
        assert_eq!(b.params, vec!["irc.a".to_string(), "irc.b".to_string()]);
        assert_eq!(b.messages().len(), 4);
    }

    #[test]
    fn non_batched_message_passes_through_even_while_a_batch_is_open() {
        let mut c = BatchCollector::new();
        assert_eq!(c.handle(parse("BATCH +x chathistory #c")), None);
        // A concurrent, non-batched message is not swallowed.
        let out = c.handle(parse(":s!u@h PRIVMSG #other :live"));
        assert!(matches!(out, Some(CollectorOutput::Passthrough(_))));
    }

    #[test]
    fn open_line_label_is_retained() {
        let mut c = BatchCollector::new();
        c.handle(parse("@label=5 :server BATCH +hist chathistory #rust"));
        let out = c.handle(parse("BATCH -hist")).unwrap();
        let CollectorOutput::Batch(b) = out else {
            panic!("expected Batch");
        };
        assert_eq!(b.label(), Some("5"));
        assert_eq!(b.batch_type, "chathistory");
        assert_eq!(b.params, vec!["#rust".to_string()]);
    }
}
