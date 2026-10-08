---
id: "norn-6mxq27"
title: "Layout robustness: widths, scrolling popups, graphemes"
status: "done"
priority: "p3"
tags: []
parent: "norn-a4k4me"
blocked_by: ["norn-kg807h"]
refs: ["crates/norn/src/tui/view.rs"]
created: "2026-10-08T08:47:04Z"
updated: "2026-10-08T14:18:08Z"
closed: "2026-10-08T14:18:08Z"
started_by: ~
---

See the approved plan, phase 10. Run fmt, clippy and tests before done.

plan: i-would-like-us-virtual-steele-a0056f#step-10

## Acceptance

## Log
- 2026-10-08T08:47Z claude: created
- 2026-10-08T08:47Z claude: blocked by norn-kg807h
- 2026-10-08T14:09Z claude: started
- 2026-10-08T14:18Z claude: Implemented: view::layout/Panes shared by draw and mouse hit-testing (sidebar_width and nicklist_width settings, nicklist hidden under 80 cols, sidebar yields to a 20-col chat), terminal-too-small notice below 60x10 (no panics at any size, tested across overlays), sidebar window that keeps the active buffer in view with arrows and wheel scrolling, scroll_window shared by switcher/completion/help/plugins and completion shows k/N, input line scrolls sideways with a leading ellipsis, truncation ends in an ellipsis (nicks, nicklist, sidebar, header title, prompt), grapheme-aware text module (width, truncate, wrap) using unicode-segmentation (already in the tree via ratatui). fmt, clippy, tests pass.
- 2026-10-08T14:18Z claude: done
