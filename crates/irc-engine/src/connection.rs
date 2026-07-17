//! Async connection driver: the only I/O in the bring-up path.
//!
//! It reads bytes from a stream, frames them into lines, parses each into a
//! [`Message`], and feeds the [`BringupMachine`], writing the machine's
//! outgoing lines back and collecting its events. It is generic over any
//! `AsyncRead + AsyncWrite` so it can be driven by an in-memory pipe in tests;
//! real TLS setup (a rustls stream) plugs in as the `S` type later.

use irc_proto::Message;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::bringup::{Action, BringupMachine};
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
