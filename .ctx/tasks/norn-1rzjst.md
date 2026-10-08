---
id: "norn-1rzjst"
title: "Manually verify the TUI review fixes in a real terminal"
status: "done"
priority: "p1"
tags: ["verification"]
parent: "norn-a4k4me"
blocked_by: []
refs: ["docs/USER_GUIDE.md"]
created: "2026-10-08T14:44:26Z"
updated: "2026-10-08T15:42:16Z"
closed: "2026-10-08T15:42:16Z"
started_by: ~
---

All 11 phases have automated tests (311 pass, drawn through ratatui's TestBackend) but nobody has run the app in a real terminal against a real server. Check, then close this task and the epic norn-a4k4me:

- connect to a server: quit/nick/join lines, a refused command (e.g. /join a +i channel) shows a red !! line, the MOTD appears in the status buffer
- the sidebar shows the state glyph and, on a slow link, the lag
- /join from the console shows an error; /join #chan switches once joined
- Ctrl+C needs two presses; x on a network and Delete on an alias need two
- an 80x24 terminal and a 50x10 terminal both render (the second says terminal too small)
- paste several lines: it asks first
- scroll up in a busy channel: the view holds still; Ctrl+F finds text; Alt+U goes to the first unread line
- NO_COLOR=1 norn, and TERM=xterm-16color norn, are readable (selected row visible)
- a message with mIRC bold/colour codes renders styled (e.g. /raw PRIVMSG #chan :\x02hi\x02 from another client)

## Acceptance

## Log
- 2026-10-08T14:44Z claude: created
- 2026-10-08T15:42Z adaoraul: done
