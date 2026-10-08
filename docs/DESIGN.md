---
title: "norn design"
summary: "norn is a terminal IRCv3 client for people who connect to modern servers or bouncers. It treats server-time, SASL, CHATHISTORY and labeled-response as the base…"
status: active
covers: ["**"]
verified: "2026-10-08"
---

# norn design

## Overview

norn is a terminal IRCv3 client for people who connect to modern servers or bouncers. It treats `server-time`, SASL, `CHATHISTORY` and `labeled-response` as the baseline rather than a legacy path. It is a Cargo workspace of three crates: a pure protocol core, an async engine, and the ratatui client on top. The project is at 0.1.0 with a single maintainer, and is dual-licensed MIT OR Apache-2.0.

## Principles

1. **Pure protocol core.** `irc-proto` does no I/O and uses no async, so the codec and SASL math stay property-testable in isolation.
2. **No raw lines above the engine.** The UI sees semantic `Event`s, never IRC lines or numerics.
3. **One tag codec.** Tag escaping and unescaping lives only in `irc-proto/src/tags.rs`.
4. **Sans-I/O bring-up.** The cap and SASL machine is driven by transcripts; `Connection` only feeds it bytes.
5. **Networks share no mutable state.** Each runs as its own task and talks over channels only.
6. **Passwords never touch the config file.** They come from `password_command` or `NORN_PASSWORD`.
7. **Registries are the source of truth.** Commands and settings are described once, then rendered and validated from that description.

## Stack & checks

Rust 2021 Cargo workspace (tokio, rustls, ratatui, Rhai).

- Fast: `cargo fmt --check`
- Fast: `cargo clippy --workspace --all-targets -- -D warnings`
- Full: `cargo test --workspace`

## Architecture

Three crates, each depending only on the one below.

- `crates/irc-proto`: pure and synchronous, no I/O. Tag codec, `Message`, capability parsing, SASL (PLAIN, EXTERNAL, SCRAM-SHA-256).
- `crates/irc-engine`: tokio-based. `Connection` and framing, the sans-I/O `BringupMachine` (CAP/SASL), and `Engine` (batch collector, label router, rosters, history). Emits semantic `Event`s; no raw lines reach the UI.

- `crates/norn`: the client.
  - `session.rs` runs one independent task per network; `transport.rs` opens TCP or rustls.
  - `tui/` (or `plain.rs`) consumes tagged `UiEvent`s from one shared channel and sends `NetCommand`s down a per-network channel. Tasks share no mutable state.
  - `config.rs`, `commands.rs`, `settings.rs`: configuration and commands.
  - `addons/`: declarative triggers and Rhai plugins behind one `AddonHost` boundary.

## Data

- **Config:** TOML at `~/.config/norn/config.toml` (or `--config`). It holds `[client]` preferences, `[[network]]`, `[[trigger]]` and `[plugin_config.*]`. `config.rs` owns it and rewrites the whole file on auto-save, so hand-written comments are lost.
- **Plugin KV store:** `~/.config/norn/plugins/store.toml`, namespaced per plugin. `RhaiHost` owns it and flushes about every 5 s and at shutdown.
- **Passwords:** never stored. They come from a network's `password_command` or `NORN_PASSWORD`.
- **Everything else:** buffers, rosters and history live only in memory (`tui/state.rs`, `Engine`). Scrollback is capped by `scrollback_lines`.

## Flow

1. **Startup:** `main.rs` parses the CLI. `config::resolve_startup` merges the TOML file and flags, then spawns one `session::run_network` task per network and starts `tui::run` (or `plain::run`).
2. **Inbound:** `run_once` calls `transport::connect`, then `Connection::run_bringup` drives the `BringupMachine` through CAP LS 302, REQ, SASL, CAP END and 001. It sends the `auto_join` JOINs, then loops on `conn.recv()`. `on_message` answers PING and passes each message to `Engine::handle`. Events go out as `UiEvent`s to `App::apply` (`tui/state.rs`) and are drawn by `tui/view.rs`. When we join a channel it requests `CHATHISTORY LATEST 50` and `WHO`.

3. **Outbound:** keys go through `tui/input.rs`, `input.rs` and `commands.rs`, which produce a `NetCommand` (`Raw`, `RequestHistory`, `Connect`, `Disconnect`, `Quit`). `run_once` sends it, and history replies come back as `HistoryLoaded`. A dropped connection reconnects with backoff from 1 s up to 30 s.
