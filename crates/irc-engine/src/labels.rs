//! Label router: turns a labeled command into an awaitable result.
//!
//! A command is tagged `@label=<id>`; the server echoes the label on its
//! response. There are three response shapes and all must complete the waiter
//! (rule 12), or a command with empty results hangs forever:
//!
//! - a single tagged reply,
//! - a `labeled-response` batch,
//! - a bare `@label=<id> ACK` when the command produced no output.
//!
//! Each registered label owns a [`oneshot`] sender; a matching reply, batch
//! close, or bare ACK sends the result and removes the entry. An unknown or
//! stale label is dropped without panicking.

use std::collections::HashMap;

use irc_proto::Message;
use tokio::sync::oneshot;

use crate::batch::CompletedBatch;

/// The result delivered to a labeled command's waiter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabeledResponse {
    /// A single tagged reply.
    Single(Message),
    /// A `labeled-response` batch.
    Batch(CompletedBatch),
    /// A bare ACK: the command produced no output.
    Empty,
}

/// Routes labeled responses back to awaiting callers.
#[derive(Debug, Default)]
pub struct LabelRouter {
    pending: HashMap<String, oneshot::Sender<LabeledResponse>>,
    counter: u64,
}

impl LabelRouter {
    /// A fresh router.
    pub fn new() -> Self {
        LabelRouter::default()
    }

    /// Mint a fresh label with no waiter attached. Use this for commands whose
    /// response is correlated out-of-band (e.g. turned into an emitted event,
    /// as CHATHISTORY is) rather than awaited through a oneshot. Sharing this
    /// allocator keeps every `@label` on a connection drawn from one sequence,
    /// so a router-awaited command and an event-correlated one never collide.
    pub fn allocate(&mut self) -> String {
        self.counter += 1;
        self.counter.to_string()
    }

    /// Allocate a unique label and return it together with the receiver that
    /// resolves when the response arrives. Attach the label to the outgoing
    /// command as `@label=<id>` and `await` the receiver for the result.
    pub fn register(&mut self) -> (String, oneshot::Receiver<LabeledResponse>) {
        let label = self.allocate();
        let (tx, rx) = oneshot::channel();
        self.pending.insert(label.clone(), tx);
        (label, rx)
    }

    /// Complete the waiter for `label`. Returns `false` if the label was
    /// unknown/stale or the receiver was already dropped (no panic either way).
    pub fn resolve(&mut self, label: &str, response: LabeledResponse) -> bool {
        match self.pending.remove(label) {
            Some(tx) => tx.send(response).is_ok(),
            None => false,
        }
    }

    /// Complete with a single tagged reply.
    pub fn resolve_single(&mut self, label: &str, msg: Message) -> bool {
        self.resolve(label, LabeledResponse::Single(msg))
    }

    /// Complete with a `labeled-response` batch.
    pub fn resolve_batch(&mut self, label: &str, batch: CompletedBatch) -> bool {
        self.resolve(label, LabeledResponse::Batch(batch))
    }

    /// Complete with an empty result (a bare `@label=<id> ACK`).
    pub fn resolve_empty(&mut self, label: &str) -> bool {
        self.resolve(label, LabeledResponse::Empty)
    }

    /// Whether a label is still awaiting a response.
    pub fn is_pending(&self, label: &str) -> bool {
        self.pending.contains_key(label)
    }

    /// How many labels are outstanding.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::{BatchCollector, CollectorOutput};
    use std::time::Duration;

    #[tokio::test]
    async fn single_reply_completes_the_future() {
        let mut router = LabelRouter::new();
        let (label, rx) = router.register();

        let reply = Message::parse(&format!("@label={label} :s NOTICE * :done")).unwrap();
        assert!(router.resolve_single(&label, reply.clone()));
        assert!(!router.is_pending(&label));

        assert_eq!(rx.await.unwrap(), LabeledResponse::Single(reply));
    }

    #[tokio::test]
    async fn labeled_batch_completes_the_future() {
        // Build a real labeled-response batch through the collector.
        let mut c = BatchCollector::new();
        let mut router = LabelRouter::new();
        let (label, rx) = router.register();

        c.handle(Message::parse(&format!("@label={label} :s BATCH +lr labeled-response")).unwrap());
        c.handle(Message::parse("@batch=lr :s NOTICE #c :one").unwrap());
        let out = c.handle(Message::parse("BATCH -lr").unwrap()).unwrap();
        let CollectorOutput::Batch(batch) = out else {
            panic!("expected batch");
        };
        assert_eq!(batch.label(), Some(label.as_str()));

        assert!(router.resolve_batch(&label, batch.clone()));
        assert_eq!(rx.await.unwrap(), LabeledResponse::Batch(batch));
    }

    #[tokio::test]
    async fn bare_ack_completes_without_hanging() {
        // Rule 12: an empty result must complete the future, not hang.
        let mut router = LabelRouter::new();
        let (label, rx) = router.register();
        assert!(router.resolve_empty(&label));

        let resp = tokio::time::timeout(Duration::from_secs(1), rx)
            .await
            .expect("future must not hang")
            .expect("sender delivered");
        assert_eq!(resp, LabeledResponse::Empty);
    }

    #[tokio::test]
    async fn unknown_label_is_dropped_without_panic() {
        let mut router = LabelRouter::new();
        assert!(!router.resolve_empty("does-not-exist"));
        assert_eq!(router.pending_count(), 0);
    }

    #[test]
    fn allocate_and_register_draw_from_one_sequence() {
        // A bare allocation and a registered (awaited) label must never repeat:
        // one shared counter backs both so labels on a connection stay unique.
        let mut router = LabelRouter::new();
        let a = router.allocate();
        let (b, _rx) = router.register();
        let c = router.allocate();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
        // A bare allocation registers no waiter.
        assert!(!router.is_pending(&a));
        assert!(router.is_pending(&b));
    }

    #[tokio::test]
    async fn dropped_receiver_resolves_to_false() {
        let mut router = LabelRouter::new();
        let (label, rx) = router.register();
        drop(rx); // caller gave up waiting
        assert!(!router.resolve_empty(&label));
    }
}
