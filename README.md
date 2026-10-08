---
title: "norn"
summary: "A terminal IRCv3 client, built on a from-scratch protocol engine."
verified: "2026-10-08"
---
# norn

A terminal IRCv3 client, built on a from-scratch protocol engine.

norn's design bet is that a modern bouncer plus IRCv3 (`server-time`, `SASL`,
`CHATHISTORY`, `labeled-response`) is the *baseline*, not a legacy add-on. The
protocol core is a pure, synchronous, exhaustively tested library; the async
engine and the TUI are built on top of it.

> **Status:** early (0.1.0), single maintainer, no published release yet. The
> engine runs against real networks (Libera.Chat), but interfaces may still shift.

## Features

- **IRCv3 throughout** - `server-time`, multiline `CAP LS 302`, atomic `CAP REQ`,
  batches (nested), `labeled-response`, `CHATHISTORY`, and standard-replies
  (`FAIL`/`WARN`/`NOTE`) routed into structured events.
- **SASL** - `PLAIN` (TLS only) and `SCRAM-SHA-256` (with server-signature
  verification). NickServ auto-identify as a fallback when SASL is unavailable.
- **Passwords never touch the config file** - each network names a
  `password_command` (its stdout is the password) or falls back to the
  `NORN_PASSWORD` environment variable.
- **Multi-network** - connect to several networks at once, each with its own
  buffers; `auto_connect` / `auto_join` per network.
- **TUI** (built on [ratatui](https://ratatui.rs)) - buffer switcher, nicklist,
  nick + command completion, scrollback with on-demand `CHATHISTORY` paging,
  themes, and modal manager screens for settings, networks, and plugins.
- **Automation** - declarative `[[trigger]]` rules, plus an embedded, sandboxed
  **Rhai** plugin system with event hooks, a persistent KV store, presence
  accessors, and desktop notifications.

## Build

norn is a Cargo workspace; build it with a recent stable Rust toolchain
(edition 2021):

```sh
cargo build --release
```

The client binary lands at `target/release/norn`.

## Quick start

Two ways to connect.

**Ad-hoc, from the command line** - one network, nothing persisted:

```sh
norn --server irc.libera.chat --nick yournick --join '#rust,#ratatui'
```

**From a config file** - the durable way. norn reads
`~/.config/norn/config.toml` by default (override with `--config <path>`). A
minimal file:

```toml
[client]
theme = "teal"

[[network]]
name = "libera"
host = "irc.libera.chat"
port = 6697
tls  = true
nick = "yournick"
auto_join = ["#rust", "#ratatui"]
```

To authenticate with SASL, add an account and a password source (see below):

```toml
[[network]]
name = "libera"
host = "irc.libera.chat"
nick = "yournick"
sasl_account = "yournick"
sasl_mech = "scram"                # plain | scram
password_command = "pass irc/libera"
auto_join = ["#rust"]
```

You can also define and manage networks from inside norn with `/network add ...`
and the `/networks` manager (F3) - both persist to the same file.

## Passwords

norn never stores a password inline. When a network needs one (for SASL or
NickServ), it is resolved at connect time from, in order:

1. `password_command` - a shell command whose standard output is the password
   (e.g. a password manager: `pass irc/libera`, `secret-tool lookup ...`).
2. The `NORN_PASSWORD` environment variable (fallback).

## Workspace layout

| Crate                       | What it is                                                                 |
| --------------------------- | -------------------------------------------------------------------------- |
| [`irc-proto`](crates/irc-proto) | Pure, synchronous protocol core: codec, cap parsing, SASL math. No I/O, no async. |
| [`irc-engine`](crates/irc-engine) | Async (tokio) engine: connection, TLS, bring-up state machines, batch/label routing, `CHATHISTORY`, event emission. |
| [`norn`](crates/norn)       | The terminal client: TUI, config, commands, triggers, and the Rhai plugin host. |

## Documentation

- **[User guide](docs/USER_GUIDE.md)** - running norn, the config file, networks
  and SASL, settings, the full command reference, keybindings, and features.
- **[Plugin developer guide](docs/PLUGINS.md)** - writing Rhai plugins: hooks,
  the host API, per-plugin config, the sandbox, and worked examples.
- **[IRCv3 Engine: Architecture](docs/ARCHITECTURE.md)** - the engine design and wire-level
  traces.

## License

Dual-licensed under either of [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE), at your option.
