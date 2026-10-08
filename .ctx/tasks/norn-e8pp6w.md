---
id: "norn-e8pp6w"
title: "mIRC formatting and palette/color_mode"
status: "done"
priority: "p3"
tags: []
parent: "norn-a4k4me"
blocked_by: ["norn-6mxq27"]
refs: ["crates/norn/src/tui/theme.rs"]
created: "2026-10-08T08:47:04Z"
updated: "2026-10-08T14:44:09Z"
closed: "2026-10-08T14:44:09Z"
started_by: ~
---

See the approved plan, phase 11. Run fmt, clippy and tests before done.

plan: i-would-like-us-virtual-steele-a0056f#step-11

## Acceptance

## Log
- 2026-10-08T08:47Z claude: created
- 2026-10-08T08:47Z claude: blocked by norn-6mxq27
- 2026-10-08T14:34Z claude: started
- 2026-10-08T14:44Z claude: Implemented: format.rs (mIRC bold/italic/underline/strike/reverse, 99-colour palette, hex colours; strip drops every control character), wrapped rows rebuilt from styled runs, readability guard for colours, mirc_formatting setting; mentions/search/plugins/triggers/line mode see plain text; notices as -nick-; tui/color.rs (ColorMode, auto from NO_COLOR/COLORTERM/TERM, 256/16/none, paint_background) applied once to the finished frame; voice +, help examples, connecting glyph and query labels follow the accent / nick_colors. Deviations: colour modes are applied to the finished screen buffer instead of turning the theme constants into a Palette struct (covers every style, including mIRC colours, in one place); the light palette stretch goal is not done. fmt, clippy, tests pass.
- 2026-10-08T14:44Z claude: done
