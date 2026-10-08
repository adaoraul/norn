---
id: "norn-f5st8j"
title: "Discoverability and help: keys registry, /set completion"
status: "done"
priority: "p2"
tags: []
parent: "norn-a4k4me"
blocked_by: ["norn-csggjs"]
refs: ["crates/norn/src/commands.rs"]
created: "2026-10-08T08:47:04Z"
updated: "2026-10-08T13:34:17Z"
closed: "2026-10-08T13:34:17Z"
started_by: ~
---

See the approved plan, phase 7. Run fmt, clippy and tests before done.

plan: i-would-like-us-virtual-steele-a0056f#step-7

## Acceptance

## Log
- 2026-10-08T08:47Z claude: created
- 2026-10-08T08:47Z claude: blocked by norn-csggjs
- 2026-10-08T13:26Z claude: started
- 2026-10-08T13:34Z claude: Implemented: keys.rs registry (KEYS, welcome_line) + /keys and F1 opening help on a generated key table, /set help lists SETTINGS from the registry, /set completion from the registry (keys and per-kind values; SET_SUBS and ArgKind::Enum removed), plain-mode help generated from COMMANDS (translate-only), welcome line, switcher footer and list scrolling, Networks form and Plugins hints, Alt+1..9 numbered in the sidebar (Alt+0 console), Ctrl+K fuzzy ranking. Tests guard USER_GUIDE against missing keys/settings. fmt, clippy, tests pass.
- 2026-10-08T13:34Z claude: done
