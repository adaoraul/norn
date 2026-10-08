---
id: "norn-qtw8yd"
title: "Input editing: LineEditor, paste, completion"
status: "done"
priority: "p2"
tags: []
parent: "norn-a4k4me"
blocked_by: ["norn-f5st8j"]
refs: ["crates/norn/src/tui/input.rs"]
created: "2026-10-08T08:47:04Z"
updated: "2026-10-08T13:43:43Z"
closed: "2026-10-08T13:43:43Z"
started_by: ~
---

See the approved plan, phase 8. Run fmt, clippy and tests before done.

plan: i-would-like-us-virtual-steele-a0056f#step-8

## Acceptance

## Log
- 2026-10-08T08:47Z claude: created
- 2026-10-08T08:47Z claude: blocked by norn-f5st8j
- 2026-10-08T13:36Z claude: started
- 2026-10-08T13:43Z claude: Implemented: tui/editor.rs (shared by the input line and the settings/networks/plugin inline edits, which now have real cursors), Delete/Ctrl+A/E/U/W/Ctrl+arrows/Alt+b,f/Alt+Backspace, completion keeps the tail and no doubled space, nick completion by recency (Buffer.last_spoke), history capped at 500 (memory only), bracketed paste with confirmation for multi-line (warns about / lines), PageUp/PageDown by pane height, wheel 3 lines. Deviations: word movement on Ctrl+arrows (Alt+arrows switch buffers); the editor works on (&mut String, &mut usize) instead of owning the text, to avoid refactoring every caller. Deferred as planned: multiline composition, persistent history. fmt, clippy, tests pass.
- 2026-10-08T13:43Z claude: done
