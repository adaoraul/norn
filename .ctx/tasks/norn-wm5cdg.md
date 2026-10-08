---
id: "norn-wm5cdg"
title: "Status visibility: state, lag, away, modes"
status: "done"
priority: "p2"
tags: []
parent: "norn-a4k4me"
blocked_by: ["norn-131h9m"]
refs: ["crates/norn/src/tui/view.rs"]
created: "2026-10-08T08:47:04Z"
updated: "2026-10-08T11:55:59Z"
closed: "2026-10-08T11:55:59Z"
started_by: ~
---

See the approved plan, phase 5. Run fmt, clippy and tests before done.

plan: i-would-like-us-virtual-steele-a0056f#step-5

## Acceptance

## Log
- 2026-10-08T08:47Z claude: created
- 2026-10-08T08:47Z claude: blocked by norn-131h9m
- 2026-10-08T11:52Z claude: started
- 2026-10-08T11:55Z claude: Implemented: Event::Pong + LagProbe (30s PING, session owns the clock), UiEventKind::Lag, per-network state glyphs, lag in sidebar (>1s) and activity bar, !N mention badges, header network/modes/(not joined), [away], net-qualified activity entries, gold mention gutter bar, MODE #chan requested on own join. Activity names are only network-qualified when more than one network exists (less noise). fmt, clippy, tests pass.
- 2026-10-08T11:55Z claude: done
