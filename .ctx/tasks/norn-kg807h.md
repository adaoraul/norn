---
id: "norn-kg807h"
title: "Scrollback model, search, timestamps"
status: "done"
priority: "p3"
tags: []
parent: "norn-a4k4me"
blocked_by: ["norn-qtw8yd"]
refs: ["crates/norn/src/tui/state.rs"]
created: "2026-10-08T08:47:04Z"
updated: "2026-10-08T14:06:04Z"
closed: "2026-10-08T14:06:04Z"
started_by: ~
---

See the approved plan, phase 9. Run fmt, clippy and tests before done.

plan: i-would-like-us-virtual-steele-a0056f#step-9

## Acceptance

## Log
- 2026-10-08T08:47Z claude: created
- 2026-10-08T08:47Z claude: blocked by norn-qtw8yd
- 2026-10-08T13:54Z claude: started
- 2026-10-08T14:06Z claude: Implemented: stable line numbers (Buffer.base_seq, Scroll::{Live,Anchor(seq)}, unread_marker as a sequence number, prepend/trim keep both put), view renders upward from the anchor and fills down when the top is reached, App.visible_lines (Cell, written by the draw) makes a page what was on screen so wrapped lines are never skipped and line 0 is reachable; Ctrl+End, Alt+U, Ctrl+F and /search with highlighted matches, a current-hit arrow and a n/m counter; timestamp_format setting (validated strftime, invalid falls back), column width from the widest stamp, day-change separators. Deviations: scroll moves by whole lines rather than rows (the view reports its page size back); separators only show while timestamps are on. fmt, clippy, tests pass.
- 2026-10-08T14:06Z claude: done
