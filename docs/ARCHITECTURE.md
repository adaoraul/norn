---
title: "IRCv3 Engine: Architecture"
summary: "This document specifies the protocol engine only, not the UI. The engine owns everything from raw socket bytes up to semantic events. The UI layer (TUI) consum…"
verified: "2026-10-08"
---
# IRCv3 Engine: Architecture

## Scope

This document specifies the **protocol engine** only, not the UI. The engine
owns everything from raw socket bytes up to semantic events. The UI layer (TUI)
consumes semantic events and never sees a raw IRC line or a numeric like `352`.

The guiding thesis for the client is that a modern bouncer plus IRCv3 is the
baseline, not the legacy path: assume `server-time`, `SASL`, `CHATHISTORY`, and
`labeled-response` are present and design around persistent, multi-device,
history-rich sessions.

## Design principles

1. **`Message` is the universal currency.** Every layer above framing produces
   or consumes a parsed `Message`. Tags are a parsed map on that struct, so
   `server-time`, `account`, `msgid`, and client-only tags are field lookups,
   never string scans.
2. **No raw lines leak to the UI.** The engine emits *semantic* events
   (`MessageReceived`, `HistoryLoaded`, `MemberJoined`, `CapabilitiesChanged`,
   `AuthResult`, `StandardReply`). The wire protocol stays sealed inside the
   engine.
3. **Bring-up is an explicit state machine.** Illegal transitions should be
   unrepresentable, enforced by the Rust type system where practical.
4. **Escaping/unescaping lives in exactly one place** (the tag codec) and is
   tested exhaustively. It is the single most bug-prone surface in IRCv3.
5. **Layers peel from the outside in.** Each stage has one job and hands a
   cleaner representation to the next.

## The pipeline

```
raw bytes
  -> line framing        (split on CRLF; respect the 8191-byte tag budget)
  -> message codec       (parse into Message { tags, source, command, params };
                          tag unescaping happens HERE, once)
  -> cap + SASL machine  (connection bring-up only; inert after Registered)
  -> batch collector     (buffers @batch groups, emits them as one unit)
  -> label router        (routes @label replies to awaiting futures)
  -> semantic emitter    (Message -> typed events) -> UI
```

## Core types

```rust
/// The universal currency. Everything above framing speaks Message.
pub struct Message {
    pub tags: Tags,             // parsed, unescaped
    pub source: Option<Source>, // servername or nick!user@host
    pub command: Command,       // PRIVMSG, JOIN, numeric, CAP, BATCH, ...
    pub params: Vec<String>,    // trailing param already merged in
}

pub struct Tags(HashMap<TagKey, String>);

pub struct TagKey {
    pub client_only: bool,      // leading '+'
    pub vendor: Option<String>, // "example.com" from "example.com/key"
    pub name: String,
}

pub enum Source {
    Server(String),
    User { nick: String, user: Option<String>, host: Option<String> },
}
```

Convenience accessors matter for keeping the higher layers clean:

```rust
impl Message {
    pub fn server_time(&self) -> Option<DateTime<Utc>>; // reads "time" tag
    pub fn msgid(&self) -> Option<&str>;                 // reads "msgid" tag
    pub fn account(&self) -> Option<&str>;               // reads "account" tag
    pub fn batch_ref(&self) -> Option<&str>;             // reads "batch" tag
    pub fn label(&self) -> Option<&str>;                 // reads "label" tag
}
```

### Semantic events (engine output)

```rust
pub enum Event {
    Registered { nick: String },
    CapabilitiesChanged { available: CapSet, enabled: CapSet },
    AuthResult(Result<AccountName, SaslError>),
    MessageReceived(ChatMessage),      // carries original server-time + msgid
    HistoryLoaded { target: String, messages: Vec<ChatMessage>, complete: bool },
    MemberJoined { target: String, who: User, account: Option<String> },
    MemberLeft { target: String, who: User, reason: LeaveReason },
    BatchCollapsed(CollapsedBatch),    // netsplit/netjoin folded to one event
    StandardReply(StandardReply),      // FAIL / WARN / NOTE, machine-readable
    Disconnected(DisconnectReason),
}
```

## Module boundaries (crate layout)

```
crates/
  irc-proto/          # pure, no I/O, no async. The trustable core.
    tags.rs           #   escape/unescape codec + Tags parse/emit
    message.rs        #   Message parse/emit, Source, Command
    caps.rs           #   capability names, CapSet, value parsing
    sasl/
      mod.rs          #   mechanism trait + AUTHENTICATE chunking
      plain.rs
      external.rs
      scram.rs        #   SCRAM-SHA-256 proof math
  irc-engine/         # async; owns the connection and the machines
    connection.rs     #   tokio socket + TLS + line framing
    bringup.rs        #   cap negotiation + SASL state machine
    batch.rs          #   batch collector (stack keyed by ref)
    labels.rs         #   label router (HashMap<Label, oneshot::Sender>)
    history.rs        #   CHATHISTORY request builder + pagination cursors
    engine.rs         #   wires the pipeline, emits Event
```

`irc-proto` is pure and synchronous so it can be fuzzed and property-tested in
isolation. All the deterministic, high-risk logic (codec, SASL math, cap value
parsing) lives there with zero async in the way.

## Connection bring-up state machine

```
Idle
  -> (send CAP LS 302, NICK, USER)      -> LsRequested
LsRequested
  -> (collect multiline LS, then REQ)   -> Negotiating
Negotiating
  -> if sasl ACKed: enter SaslInProgress (nested sub-machine)
  -> on all ACK/NAK resolved & no sasl: -> Ended
SaslInProgress
  -> 903 success                        -> Ended
  -> 904/905/906 failure                -> Ended (policy: abort or continue)
Ended
  -> (send CAP END)                     -> awaiting 001
awaiting 001
  -> 001 Welcome                        -> Registered
Registered
  -> cap-notify (CAP NEW/DEL), label router, batch collector active
```

Rules the machine enforces:

- Server pauses registration after `CAP LS`; **`001` will not arrive until
  `CAP END` is sent**. Forgetting `CAP END` hangs the connection. This is the
  classic first bug.
- `LS` (and `NEW`) replies can be multiline. A non-final line has a `*` before
  the trailing param. Buffer until a line without `*`.
- `CAP REQ` is atomic: one `ACK` or one `NAK` for the whole requested set, never
  partial. Prefer small logical groups, or one cap per REQ line, so a single
  rejection does not sink unrelated caps.
- If `sasl` was REQ'd, `CAP END` is sent **only after** SASL resolves.

## SASL sub-machine

Runs over repeated `AUTHENTICATE` inside `Negotiating`. Mechanism-agnostic
framing, mechanism-specific payload.

- Client sends `AUTHENTICATE <MECH>`, server replies `AUTHENTICATE +`, then a
  mechanism exchange of base64 blobs.
- **400-byte chunking:** each base64 payload is sent in chunks of at most 400
  bytes. The message is complete when a chunk **shorter than 400** is seen. An
  empty payload is `AUTHENTICATE +`. If the payload length is an exact multiple
  of 400, send all full chunks **then an extra `AUTHENTICATE +`** or the peer
  waits forever.
- Abort at any time with `AUTHENTICATE *`.

Mechanisms, in ship order:

1. **PLAIN**: base64 of `authzid\0authcid\0password`; authzid usually empty, so
   `\0account\0password`. TLS only.
2. **EXTERNAL**: no secret on the wire; identity proven by TLS client cert
   (CertFP). Reply is base64 of authzid, usually empty (`AUTHENTICATE +`).
3. **SCRAM-SHA-256** (add after engine is stable): four-message salted
   challenge-response, password never sent, server is verified too. See
   `sasl/scram.rs` and the proof math in TESTS.md.

Numerics: `900` logged in, `903` success, `904` fail, `905` too long, `906`
aborted, `907` already authenticated, `908` supported-mechanisms list (retry
with a listed mechanism). End on `903` (proceed to `CAP END`) or a failure
(policy: abort, or continue unauthenticated).

## Runtime components (active once Registered)

### Batch collector

`BATCH +<ref> <type> [params]` opens, lines carry `@batch=<ref>`, `BATCH -<ref>`
closes. Batches nest, so track a **stack keyed by ref**. Buffer an open batch
and emit it as one event. Types: `chathistory`, `netsplit`/`netjoin` (collapse
the quit/join flood into one `BatchCollapsed` event), `labeled-response`.

### Label router

Attach `@label=<id>` to a command; the server echoes it on the response. Three
outcomes, all handled:

- a single tagged reply,
- multiple replies wrapped in a `labeled-response` batch,
- **no output**, in which case the server sends a bare `@label=<id> ACK`.

Implementation: `HashMap<Label, oneshot::Sender<Response>>`. Each command gets a
label; a labeled line, labeled batch close, or bare ACK completes the matching
future. This turns "send and guess" into a real `async fn` returning the result.

### cap-notify

Because `CAP LS 302` was sent, the server may change the menu at runtime:
`CAP NEW :<caps>` (now available, may REQ live) and `CAP DEL :<caps>` (gone,
treat as removed, no REQ needed). This lights up features mid-session, e.g. caps
that unlock only after you authenticate.

## CHATHISTORY flow (the integration proof)

This is where `labeled-response`, `batch`, `server-time`, and `msgid` all meet.
If the engine is designed right, CHATHISTORY needs no special-casing: it is a
labeled command whose response is a `chathistory` batch of ordinary messages,
each carrying its original `time` and `msgid` tags.

### Command shape

```
CHATHISTORY <subcommand> <target> <selector...> <limit>
```

Subcommands and their use:

- `LATEST <target> * <limit>` : newest N messages (initial load on join).
- `LATEST <target> <selector> <limit>` : newest N since a point.
- `BEFORE <target> <selector> <limit>` : page **older** (scrollback up).
- `AFTER  <target> <selector> <limit>` : fill the gap **after** reconnect.
- `AROUND <target> <selector> <limit>` : context around a point (search hits).
- `BETWEEN <target> <selectorA> <selectorB> <limit>` : bounded range.
- `TARGETS <selectorA> <selectorB> <limit>` : which conversations have history.

A `<selector>` is `timestamp=YYYY-MM-DDThh:mm:ss.sssZ` or `msgid=<id>`.

### Wire trace

```
C: @label=5 CHATHISTORY LATEST #rust * 50
S: @label=5 :server BATCH +hist chathistory #rust
S: @batch=hist;time=2026-07-17T09:00:01.000Z;msgid=aaa :nick!u@h PRIVMSG #rust :older line
S: @batch=hist;time=2026-07-17T09:00:02.000Z;msgid=bbb :nick!u@h PRIVMSG #rust :newer line
S: :server BATCH -hist
```

Engine handling:

1. Label router sees `@label=5` on the `BATCH +hist` open and binds the batch's
   completion to command 5's future.
2. Batch collector buffers every `@batch=hist` line until `BATCH -hist`.
3. On close, the collected messages (each with its real `server-time` and
   `msgid`) are handed back as the labeled response.
4. Engine emits `HistoryLoaded { target, messages, complete }`, where
   `complete` is false if the server returned exactly `limit` messages (there
   may be more).

### Pagination cursors (`history.rs`)

- **Scroll up (older):** take the oldest message held, request
  `CHATHISTORY BEFORE #rust msgid=<oldest> 50`.
- **Reconnect gap-fill (newer):** take the last-seen message before the drop,
  request `CHATHISTORY AFTER #rust timestamp=<lastseen> 50`.
- `msgid` cursors are preferred over timestamps when available; they are exact
  and avoid millisecond-boundary ambiguity. This is why the `msgid` cap is a
  dependency of good history, not an optional extra.

Errors arrive via `standard-replies` as `FAIL CHATHISTORY <code> ...` and route
into the single `StandardReply` event, not a bespoke error path.

## Capability surface

Everything past the pipeline is either a cap negotiation or a tag, so nothing
below changes the architecture; each item is just a handler.

**Phase 1 (bring-up + core):** `sasl`, `server-time`, `message-tags`, `batch`,
`labeled-response`, `echo-message`, `cap-notify` (implicit with 302).

**Phase 2 (identity + history):** `account-tag`, `account-notify`,
`extended-join`, `chghost`, `away-notify`, `setname`, `multi-prefix`, `msgid`,
`CHATHISTORY`.

**Phase 3 (polish):** `UTF8ONLY`, `standard-replies` routing, client-only tags
for `+typing`, `+draft/reply`, `+draft/react`.

Note the `draft/` prefix marks unratified caps; a name may lose the prefix once
standardized (`draft/chathistory` -> `chathistory`). Accept both forms.
