---
id: "norn-a4k4me"
title: "norn TUI review fixes"
kind: "epic"
status: "done"
priority: "p2"
tags: []
blocked_by: []
refs: ["docs/DESIGN.md", "crates/norn/src/tui/"]
created: "2026-10-08T08:46:58Z"
updated: "2026-10-08T15:42:22Z"
closed: "2026-10-08T15:42:22Z"
started_by: ~
---

Fix the findings of the TUI review (Nielsen heuristics, WCAG 2.1 AA, cognitive load): bugs that hide information (quits, nick changes, netsplits, MODE, server errors, away), silent command failures, no status/lag indicator, contrast failures (DIM2 3.05:1, FAINT 1.84:1), undiscoverable keys, unsafe Ctrl+C and deletes, fixed layout, input gaps, no NO_COLOR/16-colour, raw mIRC codes. One commit per phase; each updates docs/USER_GUIDE.md.

Verification:
- quit and nick lines appear
- /join from the console shows an error
- the state glyph and lag appear in the sidebar
- Ctrl+C needs two presses
- an 80x24 and a 50x10 terminal both render correctly
- a multi-line paste asks for confirmation

plan: i-would-like-us-virtual-steele-a0056f

## Acceptance

## Log
- 2026-10-08T08:46Z claude: created
- 2026-10-08T15:42Z adaoraul: done
