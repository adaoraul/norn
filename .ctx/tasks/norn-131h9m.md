---
id: "norn-131h9m"
title: "Engine: server errors, MOTD, netsplit, modes"
status: "done"
priority: "p1"
tags: []
parent: "norn-a4k4me"
blocked_by: ["norn-3sveew"]
refs: ["crates/irc-engine/src/engine.rs"]
created: "2026-10-08T08:47:04Z"
updated: "2026-10-08T10:29:38Z"
closed: "2026-10-08T10:29:38Z"
started_by: ~
---

See the approved plan, phase 4. Run fmt, clippy and tests before done.

plan: i-would-like-us-virtual-steele-a0056f#step-4

## Acceptance

## Log
- 2026-10-08T08:47Z claude: created
- 2026-10-08T08:47Z claude: blocked by norn-3sveew
- 2026-10-08T10:25Z claude: started
- 2026-10-08T10:29Z claude: Implemented: CommandError/ServerError, Motd, ServerInfo, ModeChanged (+prefix_changes, roster set_prefix), ChannelModes, Netsplit/Netjoin replacing BatchCollapsed with roster updates; TUI routing, plain renderer, ARCHITECTURE/USER_GUIDE updated. 005 PREFIX is not parsed (bring-up consumes ISUPPORT): the common (qaohv) set is used. fmt, clippy, tests pass.
- 2026-10-08T10:29Z claude: done
