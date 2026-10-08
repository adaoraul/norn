---
title: "norn user guide"
summary: "Everything you need to run norn day to day: starting it, the config file, connecting to networks, settings, the command set, keybindings, and the features that…"
verified: "2026-10-08"
---
# norn user guide

Everything you need to run norn day to day: starting it, the config file,
connecting to networks, settings, the command set, keybindings, and the features
that are not obvious from the command list.

For writing plugins, see the [plugin developer guide](PLUGINS.md).

## Contents

- [Running norn](#running-norn)
- [The config file](#the-config-file)
- [Networks and connecting](#networks-and-connecting)
- [Settings](#settings)
- [Command reference](#command-reference)
- [Keybindings](#keybindings)
- [Features](#features)

## Running norn

```sh
norn [FLAGS]
```

The command-line flags define at most one ad-hoc network and pick the config
file. Anything durable belongs in the config file.

| Flag                     | Default                        | Meaning                                                  |
| ------------------------ | ------------------------------ | -------------------------------------------------------- |
| `--config <path>`        | `~/.config/norn/config.toml`   | Path to the TOML config file.                            |
| `--plain`                | off                            | Use the plain line renderer instead of the full TUI.     |
| `--server <host>`        | -                              | Server hostname; defines a single ad-hoc network.        |
| `--port <port>`          | `6697`                         | Server port.                                             |
| `--nick <nick>`          | -                              | Nickname to register.                                    |
| `--user <ident>`         | the nick                       | Username / ident.                                        |
| `--realname <gecos>`     | the nick                       | Realname / GECOS.                                        |
| `--sasl-account <name>`  | -                              | SASL account; the password comes from `NORN_PASSWORD`.   |
| `--sasl-mech <mech>`     | `plain`                        | SASL mechanism: `plain` or `scram`.                      |
| `--no-tls`               | off                            | Connect in plaintext (for local test servers only).      |
| `--join <#a,#b,...>`     | -                              | Channels to auto-join on connect (comma-separated).      |

The ad-hoc network from these flags is *not* saved. To persist a network, put it
in the config file or add it in-app with `/network add` (which does save).

## The config file

The config file is TOML at `~/.config/norn/config.toml` (or `--config <path>`).
norn rewrites it when you change settings, aliases, triggers, networks, or
plugin state in-app, so you can manage most of it without editing by hand - but
it stays readable and hand-editable while norn is not running.

```toml
# Client-wide UI preferences. See "Settings" for every key.
[client]
timestamps = true
theme = "teal"
scrollback_lines = 5000

# One [[network]] block per network. See "Networks and connecting".
[[network]]
name = "libera"
host = "irc.libera.chat"
port = 6697
tls  = true
nick = "yournick"
sasl_account = "yournick"
sasl_mech = "scram"
password_command = "pass irc/libera"
auto_join = ["#rust", "#ratatui"]
auto_connect = true

# Command aliases: name = expansion.
[aliases]
j = "join $1"
bye = "quit see you"

# Declarative triggers: run a command when an event fires.
[[trigger]]
on = "highlight"
run = "notify $nick: $msg"

# Plugin scripts to keep installed but not load.
disabled_plugins = ["urlgrab"]

# Per-plugin config overrides: [plugin_config.<script-file>].
[plugin_config."autoaway.rhai"]
message = "idle"
```

Passwords are never written to this file. See
[Networks and connecting](#networks-and-connecting).

## Networks and connecting

Each `[[network]]` block declares one network. Fields:

| Field              | Type            | Default   | Notes                                                              |
| ------------------ | --------------- | --------- | ------------------------------------------------------------------ |
| `name`             | string          | required  | Display name, used in buffers and `/connect`.                      |
| `host`             | string          | required  | Server hostname.                                                   |
| `port`             | integer         | `6697`    | Server port.                                                       |
| `tls`              | bool            | `true`    | Use TLS.                                                           |
| `nick`             | string          | required  | Nickname to register.                                              |
| `user`             | string          | the nick  | Username / ident.                                                  |
| `realname`         | string          | the nick  | Realname / GECOS.                                                  |
| `sasl_account`     | string          | -         | SASL account name (requires a password source).                   |
| `sasl_mech`        | `plain`/`scram` | `plain`   | SASL mechanism.                                                   |
| `password_command` | string          | -         | Shell command whose stdout is the password.                       |
| `auto_join`        | array of string | -         | Channels to join after registration.                              |
| `auto_connect`     | bool            | `true`    | Dial on launch; if `false`, stays defined but idle.               |
| `identify`         | bool            | `false`   | Auto-identify to NickServ when prompted (fallback; see below).    |

### Passwords

norn never stores a password inline. When a network needs one, it is resolved at
connect time from, in order:

1. `password_command` - a shell command whose standard output is the password
   (e.g. `pass irc/libera`).
2. The `NORN_PASSWORD` environment variable.

### SASL

- `plain` - the password is base64-encoded and sent. **TLS only.**
- `scram` - SCRAM-SHA-256 salted challenge-response; the password is never sent,
  and norn verifies the server's signature before trusting the login.

### NickServ auto-identify

Set `identify = true` to have norn respond to a NickServ identify prompt using
the same resolved password. This is a *fallback*: it stays dormant when SASL has
already logged you in, and covers networks or situations where SASL is not
available.

### Managing networks live

- `/network ls` - list defined networks.
- `/network add <name> host=<h> nick=<n> [key=value ...]` - define a network and
  save it. Recognized keys mirror the fields above: `host`, `port`, `tls=on|off`,
  `nick`, `user`, `realname`, `sasl_account`, `sasl_mech=plain|scram`,
  `password_command`, `join=<#a,#b>`, `auto_connect=on|off`, `identify=on|off`.
- `/network rm <name>` - remove a definition.
- `/network show <name>` - show a network's settings and live state.
- `/networks` (or **F3**) - the interactive networks manager: a list with live
  connection markers and an edit form. Passwords are never entered here - set
  `password_command` instead.
- `/connect <name>` - dial an idle or newly defined network.
- `/disconnect [reason]` - disconnect but keep the definition.
- `/reconnect` - drop and redial the active network.

## Settings

Client-wide UI preferences live in `[client]`. Change one with `/set <key>
<value>`, or open the searchable `/settings` screen (**F2**). Every change
applies live and auto-saves.

| Key                | Type            | Default | What it does                                                        |
| ------------------ | --------------- | ------- | ------------------------------------------------------------------- |
| `timestamps`       | bool            | `on`    | Show a timestamp in front of each message.                          |
| `nick_colors`      | bool            | `on`    | Color nicks by a per-nick hue (off = one muted color).              |
| `theme`            | enum            | `teal`  | Accent color: `teal`, `amber`, `green`, `blue`, `purple`, `pink`.   |
| `nicklist`         | bool            | `on`    | Show the channel nicklist by default.                               |
| `completion_char`  | string          | `:`     | Character after a nick completed at line start (then a space).      |
| `beep_on_highlight`| bool            | `off`   | Ring the terminal bell when a message highlights your nick.         |
| `scrollback_lines` | int (100-1e6)   | `5000`  | Maximum lines kept per buffer.                                      |
| `idle_secs`        | int (0-86400)   | `300`   | Seconds of inactivity before plugins get `on_idle`; `0` disables.   |

`/set` with no arguments lists the current values.

## Command reference

Type commands in the input line prefixed with `/`. `/help` opens a searchable
panel; `/help <command>` jumps to a command's details. Aliases are shown in
parentheses.

### Chat

| Command                 | Summary                                                        |
| ----------------------- | -------------------------------------------------------------- |
| `/msg <target> <text>` (`/m`) | Message a user or channel without switching to its buffer. |
| `/me <action>`          | Send a CTCP ACTION to the current target.                      |
| `/query <nick>` (`/q`)  | Open (or focus) a private message buffer.                      |
| `/notice <target> <text>` | Send an IRC NOTICE.                                           |

### Channel

| Command                    | Summary                                            |
| -------------------------- | -------------------------------------------------- |
| `/join #channel` (`/j`)    | Join a channel and make it current.                |
| `/part [#channel]`         | Leave a channel (current if omitted).              |
| `/names [#channel]`        | List a channel's members.                          |
| `/topic [text]`            | View the topic, or set it with text.               |
| `/kick <nick> [reason]`    | Kick a user from the current channel (needs ops).  |
| `/mode <args>`             | Apply channel or user modes (e.g. `+o bob`).       |
| `/invite <nick> [#chan]`   | Invite a user to a channel.                         |
| `/close` (`/wc`)           | Close the active buffer; parts if it is a channel. |

### You

| Command                | Summary                                              |
| ---------------------- | ---------------------------------------------------- |
| `/nick <newnick>`      | Change your nickname.                                |
| `/away [message]`      | Set away with a message, or clear it with no arg.    |
| `/whois <nick>`        | Request WHOIS information about a user.              |

### Networks

| Command                     | Summary                                            |
| --------------------------- | -------------------------------------------------- |
| `/network ls\|add\|rm\|show` (`/net`) | Manage network definitions (saved to config). |
| `/networks`                 | Open the interactive networks manager.             |
| `/connect <name>` (`/server`) | Connect a defined network by name.               |
| `/disconnect [reason]`      | Disconnect the active network, keeping it defined. |
| `/reconnect`                | Drop and redial the active network.                |

### Client

| Command                     | Summary                                                  |
| --------------------------- | -------------------------------------------------------- |
| `/set [key value]`          | List settings, or change one (applied live, auto-saved). |
| `/settings`                 | Open the settings screen.                                |
| `/alias [name expansion]`   | List or define command aliases.                          |
| `/unalias <name>`           | Remove an alias.                                         |
| `/trigger ls\|add\|rm`      | Manage declarative event triggers.                       |
| `/plugins [ls\|available\|install\|reload\|enable\|disable\|config]` | Open or manage Rhai plugins. |
| `/clear`                    | Clear the active buffer's scrollback.                    |
| `/raw <line>` (`/quote`)    | Send a raw IRC line verbatim.                            |
| `/help [command]` (`/h`)    | Open the help panel.                                     |
| `/quit [reason]`            | Disconnect all networks and exit.                       |

## Keybindings

### In the chat window (normal mode)

| Key                 | Action                                                        |
| ------------------- | ------------------------------------------------------------- |
| `Ctrl+C`            | Quit (disconnect all and exit).                               |
| `Ctrl+K`            | Open the buffer switcher (type to filter, arrows, Enter).     |
| `F2`                | Open the settings screen.                                     |
| `F3`                | Open the networks manager.                                    |
| `F4`                | Open the plugins manager.                                     |
| `F9`                | Toggle the nicklist.                                          |
| `Alt+Left` / `Alt+Right` | Previous / next buffer.                                  |
| `Alt+1` .. `Alt+9`  | Switch to buffer N.                                           |
| `Tab`               | Autocomplete nicks / commands / arguments (repeat to cycle).  |
| `Esc`               | Clear the completion menu.                                    |
| `Enter`             | Send the message or run the command.                          |
| `Left` / `Right`    | Move the cursor.                                              |
| `Home` / `End`      | Jump to start / end of the input.                             |
| `Up` / `Down`       | Recall previous / next input from history.                    |
| `PageUp` / `PageDown` | Scroll the buffer (fetches `CHATHISTORY` when you reach the top). |

The help panel opens with `/help` (there is no dedicated key for it).

### In the manager screens

- **Buffer switcher** (`Ctrl+K`): type to filter, `Up`/`Down` to select, `Enter`
  to switch, `Esc` to cancel.
- **Settings** (`F2`): type to filter, `Up`/`Down` to move, `Enter` to edit
  (toggle a bool, cycle an enum, or type a value), `Delete` to remove an alias,
  `Esc` to close.
- **Networks** (`F3`): `Up`/`Down` to move, `Enter`/`Right` to edit or create,
  `c` to connect, `d` to disconnect, `x` to remove, `Esc` to close.
- **Plugins** (`F4`): `Up`/`Down` to move, `Space`/`Enter` to enable/disable, `c`
  to open the config editor, `Esc` to close.
- **Help** (`/help`): type to filter, `Up`/`Down` to move, `Enter` to insert the
  command into the input, `Right`/`Left` to move between the list and detail
  panes, `Esc` to close.

## Features

### Buffers and tabs

The sidebar lists the global console, then each network's status buffer, then its
channels and query (PM) buffers. Commands act on the active buffer's network.
Move around with `Alt+Left`/`Alt+Right`, `Alt+1..9`, or the `Ctrl+K` fuzzy
switcher. `/join`, `/query`, `/part`, and `/close` open and close buffers. The
console and network status buffers cannot be closed.

Channel buffers show joins, parts, kicks, quits and nick changes (a quit or nick
change appears in every channel you share with that person, and in their query
buffer). Your own nick changes and away state are announced too, and the
network's status buffer says why a connection was abandoned (for example, every
nickname in use).

### Completion

`Tab` completes. In a channel it completes member nicks; at the start of a line a
completed nick gets your `completion_char` (default `:`) and a space appended
(`nick: `). After a `/` it completes command names, then subcommands and
arguments (nicks, channels, network names). Repeat `Tab` to cycle matches.

### Scrollback and history

Each buffer keeps up to `scrollback_lines` lines (default 5000), trimmed as it
grows. `PageUp`/`PageDown` scroll; reaching the top requests older messages over
IRCv3 `CHATHISTORY` where the server supports it, paginating on demand. Replayed
history is rendered with its original timestamps, not the reconnect time.

### Aliases

`/alias <name> <expansion>` defines a shortcut. The expansion may use `$1`..`$9`
and `$*` for arguments, chain commands with `;`, and has its trailing args
appended when it contains no placeholder. Example:

```
/alias slap me slaps $1 around a bit
```

Aliases cannot shadow the structural commands (e.g. `set`, `network`, `connect`,
`plugins`). Remove one with `/unalias`.

### Triggers

Triggers run a command automatically when an event fires:

```
/trigger add highlight = notify $nick: $msg
/trigger add join #norn = msg #norn welcome, $nick
```

- **Events:** `highlight`, `message` (`msg`), `notice`, `join`, `part`, `kick`,
  `quit`, `nick`. Each can be scoped to a channel by appending it (`join #norn`).
- **Template variables:** `$nick` (who), `$chan` (the reply target - the channel,
  or the sender for a PM), `$msg` / `$text` (the message), `$me` (your nick).
- Chain commands with `;`, and use `notify <text>` to raise a local notification.
- Messages you send yourself are skipped, so triggers do not feed back on
  themselves.

List with `/trigger ls`, remove with `/trigger rm <number>`. For anything beyond
templates, use [plugins](PLUGINS.md).

### Notifications

- **Local** - a console line plus the terminal bell, from a `notify` trigger
  action or a plugin calling `notify(...)`. Set `beep_on_highlight` to ring the
  bell on any highlight.
- **Desktop** - plugins can raise an OS desktop notification (via `notify-send`)
  with `desktop_notify(...)`.

### Plugins

Rhai scripts in `~/.config/norn/plugins/*.rhai` extend norn with event hooks and
a host API. Manage them with `/plugins` (or **F4**): list, install the bundled
examples, enable/disable, reload after editing, and edit per-plugin config. See
the [plugin developer guide](PLUGINS.md) to write your own.
