---
id: "norn-csggjs"
title: "Control and safety: confirmations, mouse, password_command"
status: "done"
priority: "p2"
tags: []
parent: "norn-a4k4me"
blocked_by: ["norn-wm5cdg"]
refs: ["crates/norn/src/tui/mod.rs"]
created: "2026-10-08T08:47:04Z"
updated: "2026-10-08T12:47:42Z"
closed: "2026-10-08T12:47:42Z"
started_by: ~
---

See the approved plan, phase 6. Run fmt, clippy and tests before done.

plan: i-would-like-us-virtual-steele-a0056f#step-6

## Acceptance

## Log
- 2026-10-08T08:47Z claude: created
- 2026-10-08T08:47Z claude: blocked by norn-wm5cdg
- 2026-10-08T12:43Z claude: started
- 2026-10-08T12:47Z claude: Implemented: App.armed/confirm (Ctrl+C, network x/Delete, alias Delete need a second press within 2s; any other key cancels; prompt in activity bar / panel message), mouse setting + AppAction::SetMouse + startup respect, password_command resolved in the network task via spawn_blocking (NetworkInit::Deferred) with leak-free warnings; cmd_tx now always pushed so ids and senders stay aligned. Startup networks still resolve before the TUI so a pinentry can prompt. fmt, clippy, tests pass.
- 2026-10-08T12:47Z claude: done
