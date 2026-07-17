//! Async connection driver: the only I/O in the engine.
//!
//! It reads bytes from a stream, frames them into lines, parses each into a
//! [`Message`], and feeds the [`BringupMachine`] (during bring-up) or the
//! [`Engine`] (once registered), writing outgoing lines back. It is generic
//! over any `AsyncRead + AsyncWrite` so it can be driven by an in-memory pipe in
//! tests; a real TLS stream plugs in as the `S` type.
//!
//! [`run`] is the unified driver: it hands the bring-up machine each message
//! until `Registered`, then switches to the engine in the same read loop, so no
//! already-framed line is dropped at the handoff. It also answers server `PING`
//! keepalives in both phases.
//!
//! [`run`]: Connection::run

use irc_proto::{Command, Message};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::bringup::{Action, BringupMachine};
use crate::engine::Engine;
use crate::event::Event;
use crate::framing::LineFramer;

/// A connection wrapping a byte stream and its line framer.
pub struct Connection<S> {
    stream: S,
    framer: LineFramer,
}

impl<S> Connection<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    /// Wrap an established stream.
    pub fn new(stream: S) -> Self {
        Connection {
            stream,
            framer: LineFramer::new(),
        }
    }

    /// Run the bring-up handshake to completion, returning the events emitted
    /// (in order). Completes when the machine reaches `Registered`, or when the
    /// stream ends first.
    pub async fn run_bringup(
        &mut self,
        machine: &mut BringupMachine,
    ) -> std::io::Result<Vec<Event>> {
        let mut events = Vec::new();

        // Opening handshake.
        self.apply(machine.start(), &mut events).await?;
        if machine.is_registered() {
            return Ok(events);
        }

        let mut buf = [0u8; 4096];
        loop {
            let n = self.stream.read(&mut buf).await?;
            if n == 0 {
                break; // EOF before registration
            }
            for line in self.framer.push(&buf[..n]) {
                if let Ok(msg) = Message::parse(&line) {
                    let actions = machine.handle(&msg);
                    self.apply(actions, &mut events).await?;
                    if machine.is_registered() {
                        return Ok(events);
                    }
                }
            }
        }

        Ok(events)
    }

    /// Drive the whole session: bring-up, then the post-registration runtime.
    ///
    /// Sends the opening handshake, then loops selecting between inbound bytes
    /// and outbound user lines from `outgoing`. Inbound messages go to the
    /// bring-up machine until `Registered`, then to the `Engine`; server `PING`s
    /// are answered in both phases. Outbound lines are only sent once registered
    /// (earlier ones stay queued in the channel), so user input typed during
    /// bring-up is not sent prematurely. Every event is passed to `on_event`.
    /// Returns when the stream ends.
    pub async fn run<F>(
        &mut self,
        machine: &mut BringupMachine,
        engine: &mut Engine,
        outgoing: &mut mpsc::UnboundedReceiver<String>,
        mut on_event: F,
    ) -> std::io::Result<()>
    where
        F: FnMut(&Event),
    {
        let opening = machine.start();
        self.dispatch(opening, &mut on_event).await?;

        let mut buf = [0u8; 4096];
        let mut input_open = true;
        loop {
            tokio::select! {
                result = self.stream.read(&mut buf) => {
                    let n = result?;
                    if n == 0 {
                        break; // stream closed
                    }
                    for line in self.framer.push(&buf[..n]) {
                        let Ok(msg) = Message::parse(&line) else {
                            continue;
                        };
                        if machine.is_registered() {
                            self.answer_ping(&msg).await?;
                            for event in engine.handle(msg) {
                                on_event(&event);
                            }
                        } else {
                            let actions = machine.handle(&msg);
                            self.dispatch(actions, &mut on_event).await?;
                        }
                    }
                }
                // Only accept user input once registered; before that it stays
                // queued. Disabled entirely once the input channel closes.
                maybe_line = outgoing.recv(), if input_open && machine.is_registered() => {
                    match maybe_line {
                        Some(line) => self.send(&line).await?,
                        None => input_open = false,
                    }
                }
            }
        }
        Ok(())
    }

    /// Send a raw line (a trailing CRLF is appended) and flush.
    pub async fn send(&mut self, line: &str) -> std::io::Result<()> {
        self.stream.write_all(line.as_bytes()).await?;
        self.stream.write_all(b"\r\n").await?;
        self.stream.flush().await?;
        Ok(())
    }

    /// Reply to a server `PING` with a matching `PONG`.
    async fn answer_ping(&mut self, msg: &Message) -> std::io::Result<()> {
        if matches!(&msg.command, Command::Named(name) if name == "PING") {
            let token = msg.params.first().cloned().unwrap_or_default();
            self.send(&format!("PONG :{token}")).await?;
        }
        Ok(())
    }

    /// Write outgoing lines (appending CRLF) and route emitted events to a sink.
    async fn dispatch<F>(&mut self, actions: Vec<Action>, on_event: &mut F) -> std::io::Result<()>
    where
        F: FnMut(&Event),
    {
        for action in actions {
            match action {
                Action::Send(line) => {
                    self.stream.write_all(line.as_bytes()).await?;
                    self.stream.write_all(b"\r\n").await?;
                }
                Action::Emit(event) => on_event(&event),
            }
        }
        self.stream.flush().await?;
        Ok(())
    }

    /// Write outgoing lines (appending CRLF) and collect emitted events.
    async fn apply(
        &mut self,
        actions: Vec<Action>,
        events: &mut Vec<Event>,
    ) -> std::io::Result<()> {
        for action in actions {
            match action {
                Action::Send(line) => {
                    self.stream.write_all(line.as_bytes()).await?;
                    self.stream.write_all(b"\r\n").await?;
                }
                Action::Emit(event) => events.push(event),
            }
        }
        self.stream.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bringup::{BringupConfig, SaslFailPolicy};
    use irc_proto::sasl::Plain;
    use irc_proto::Mechanism;
    use tokio::io::AsyncReadExt;

    fn config() -> BringupConfig {
        BringupConfig {
            nick: "adao".into(),
            user: "adao".into(),
            realname: "Adao".into(),
            cap_groups: vec![vec!["server-time".into()]],
            sasl: vec![Box::new(Plain::new("adao", "hunter2")) as Box<dyn Mechanism>],
            tls: true,
            sasl_fail_policy: SaslFailPolicy::Continue,
        }
    }

    // Drives the full pipeline (framing + parse + machine + I/O) over an
    // in-memory duplex acting as a scripted server.
    #[tokio::test]
    async fn run_bringup_over_a_scripted_pipe() {
        let (client, mut server) = tokio::io::duplex(64 * 1024);

        // The scripted server transcript. Preloading it is fine: the machine is
        // driven by inbound lines, not by request/response gating on the wire.
        let transcript = "\
CAP * LS :sasl server-time\r\n\
CAP * ACK :server-time\r\n\
CAP * ACK :sasl\r\n\
AUTHENTICATE +\r\n\
900 adao adao!u@h adao :logged in\r\n\
903 adao :SASL authentication successful\r\n\
001 adao :Welcome to the network\r\n";

        let server_task = tokio::spawn(async move {
            server.write_all(transcript.as_bytes()).await.unwrap();
            server.flush().await.unwrap();
            // Drain whatever the client sends until it stops (returns at
            // Registered and drops its half).
            let mut sink = Vec::new();
            let _ = server.read_to_end(&mut sink).await;
            sink
        });

        let mut conn = Connection::new(client);
        let mut machine = BringupMachine::new(config());
        let events = conn.run_bringup(&mut machine).await.unwrap();

        assert!(machine.is_registered());
        assert!(events.contains(&Event::Registered {
            nick: "adao".to_string()
        }));
        assert!(events.contains(&Event::AuthResult(Ok("adao".to_string()))));

        // Drop the client so the server task's read_to_end completes, then
        // confirm the client actually sent the handshake and CAP END.
        drop(conn);
        let sent = String::from_utf8(server_task.await.unwrap()).unwrap();
        assert!(sent.contains("CAP LS 302\r\n"));
        assert!(sent.contains("AUTHENTICATE PLAIN\r\n"));
        assert!(sent.contains("CAP END\r\n"));
    }
}
