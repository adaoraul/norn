# norn plugin developer guide

norn plugins are [Rhai](https://rhai.rs) scripts. A plugin defines *hook*
functions that norn calls when something happens (a message arrives, someone
joins, you go idle), and those hooks call a *host API* to react - send a line,
store some state, raise a notification.

Rhai is a small, sandboxed scripting language embedded in norn. You do not need
to know it deeply; the examples here cover most of what plugins use.

## Contents

- [Where plugins live](#where-plugins-live)
- [Anatomy of a plugin](#anatomy-of-a-plugin)
- [Hooks](#hooks)
- [The event object](#the-event-object)
- [Host API](#host-api)
- [Per-plugin config](#per-plugin-config)
- [The const gotcha](#the-const-gotcha)
- [Sandbox and lifecycle](#sandbox-and-lifecycle)
- [Worked examples](#worked-examples)

## Where plugins live

Scripts are `*.rhai` files in the `plugins` folder next to your config file -
`~/.config/norn/plugins/`. norn loads every `.rhai` file there at startup and on
`/plugins reload`.

Manage them from the `/plugins` manager (**F4**) or the input line:

| Command                      | What it does                                            |
| ---------------------------- | ------------------------------------------------------- |
| `/plugins` (or **F4**)       | Open the manager: loaded and available plugins.         |
| `/plugins ls`                | List installed plugins with status and version.         |
| `/plugins available`         | List the bundled official plugins you can install.      |
| `/plugins install <name>`    | Copy a bundled plugin into your `plugins` folder.       |
| `/plugins reload`            | Recompile scripts and rebuild the host (after editing). |
| `/plugins enable\|disable <name>` | Load / stop loading a script.                      |
| `/plugins config <name>`     | Edit a plugin's config values.                          |

The bundled plugins (`autoop`, `keepnick`, `responder`, `autorejoin`, `seen`,
`autoaway`) double as worked examples - install one and read it.

## Anatomy of a plugin

A minimal plugin is some metadata plus one hook:

```rhai
const NAME = "greeter";
const DESCRIPTION = "greet people when they join";
const VERSION = "1.0";

fn on_join(m) {
    if m.nick != nick() {          // don't greet yourself
        send(m.channel, `welcome, ${m.nick}`);
    }
}
```

The top-level `const`s are read *without executing the script* and populate the
`/plugins` list:

| Const         | Purpose                                                        |
| ------------- | -------------------------------------------------------------- |
| `NAME`        | Display name (falls back to the filename stem).                |
| `DESCRIPTION` | One-line description shown in the manager.                     |
| `VERSION`     | Any version string.                                            |
| `CONFIG`      | A map of tunable keys and defaults (see [config](#per-plugin-config)). |

## Hooks

Define any of these functions; norn calls the ones you define. Each takes a
single event object, conventionally named `m`.

| Hook           | Fires when...                                              |
| -------------- | --------------------------------------------------------- |
| `on_message(m)`| A channel message or PM arrives (also NOTICEs).           |
| `on_join(m)`   | Someone joins a channel.                                  |
| `on_part(m)`   | Someone leaves a channel.                                 |
| `on_kick(m)`   | Someone is kicked from a channel.                         |
| `on_quit(m)`   | Someone quits the network.                                |
| `on_nick(m)`   | Someone changes nick.                                     |
| `on_idle(m)`   | You cross the keyboard-idle threshold (`idle_secs`).      |
| `on_active(m)` | You resume activity after being idle.                     |

`on_idle` / `on_active` are client-local (based on your own keystrokes) and fire
once per registered network. They are disabled when `idle_secs` is `0`.

## The event object

Every hook receives a map. **Every field below is always present** (with a
default), so you never hit a missing-property error; which ones are meaningful
depends on the event `type`.

| Field       | Type   | Meaning                                                          |
| ----------- | ------ | --------------------------------------------------------------- |
| `type`      | string | `"message"`, `"notice"`, `"join"`, `"part"`, `"kick"`, `"quit"`, `"nick"`, `"idle"`, or `"active"`. |
| `my_nick`   | string | Your current nick on this network.                              |
| `network`   | string | The network name.                                               |
| `nick`      | string | The actor's nick. For a nick change, the **old** nick.          |
| `channel`   | string | The channel (for a message/notice, the reply target - the channel, or the sender for a PM). |
| `text`      | string | The message body (message/notice).                              |
| `reason`    | string | Part / kick / quit reason.                                      |
| `new`       | string | The **new** nick, on a nick change.                             |
| `by`        | string | Who did the kicking, on a kick.                                 |
| `notice`    | bool   | True if the message was a NOTICE.                               |
| `highlight` | bool   | True if the message mentioned your nick (and was not from you). |
| `from_self` | bool   | True if you sent the message.                                   |
| `is_me`     | bool   | True if *you* were the one kicked.                              |
| `seconds`   | int    | Idle duration, on `on_idle`.                                    |

Guard against reacting to your own messages with `if m.from_self { return; }`.

## Host API

These functions and objects are available in every hook.

### Sending

| Call                     | Effect                                                         |
| ------------------------ | ------------------------------------------------------------- |
| `reply(text)`            | Message the event's reply target (the channel, or the PM sender). |
| `send(target, text)`     | Message an explicit target (a channel or a nick).             |
| `raw(line)`              | Send a raw IRC line verbatim (e.g. `MODE #x +o bob`).         |
| `notify(text)`           | Local notification: a console line plus the terminal bell.    |
| `desktop_notify(text)`   | Raise an OS desktop notification (via `notify-send`).         |

### Identity and config

| Call         | Returns                                                            |
| ------------ | ----------------------------------------------------------------- |
| `nick()`     | Your current nick on the event's network (a string).              |
| `cfg(key)`   | This plugin's config value for `key` (see [config](#per-plugin-config)). |

`nick()` is a getter and takes no argument - to *change* your nick, use
`raw("NICK newnick")`.

### Presence (read-only)

These read a snapshot of the network taken when the event fired:

| Call                    | Returns                                                       |
| ----------------------- | ------------------------------------------------------------ |
| `am_away()`             | Whether you are marked away (bool).                          |
| `my_account()`          | Your services account, or `""` if not logged in.            |
| `channels()`            | An array of the channels you are in.                        |
| `names(channel)`        | An array of the nicks in a channel you are in (empty if not a member). |
| `is_op(channel, nick)`  | Whether `nick` holds op or higher in `channel` (bool).      |

### Persistent KV store

`store` is a per-plugin key-value store that survives reloads and restarts.
Each plugin has its own namespace (keyed by the script's filename stem), so keys
never collide between plugins. Values are strings.

| Call                 | Effect                                                    |
| -------------------- | --------------------------------------------------------- |
| `store.get(key)`     | The value, or `""` if unset.                              |
| `store.set(key, val)`| Store a value.                                            |
| `store.del(key)`     | Remove a key.                                             |
| `store.has(key)`     | Whether the key exists (bool).                            |
| `store.keys()`       | An array of this plugin's keys.                           |

Writes are held in memory and flushed to disk on a timer and at shutdown, so you
can `store.set` freely without worrying about per-call disk cost. See
[lifecycle](#sandbox-and-lifecycle).

## Per-plugin config

Declare tunable values with a `CONFIG` map and read them with `cfg(key)`:

```rhai
const CONFIG = #{
    message: "auto-away (idle)",
    minutes: "10",
};

fn on_idle(m) {
    raw(`AWAY :${cfg("message")}`);
}
```

The declared entries are the defaults. A user overrides them without touching
the script - via `/plugins config <name>` or the `[plugin_config.<file>]` table
in the config file:

```toml
[plugin_config."autoaway.rhai"]
message = "away from keyboard"
```

`cfg(key)` returns the user override if set, otherwise the declared default,
otherwise `""`. All config values are strings; parse them yourself if you need a
number.

## The const gotcha

A top-level `const` is readable **inside a hook**, but **not inside a helper
function the hook calls** - Rhai gives each called function a fresh scope that
does not inherit the caller's constants. So read the const in the hook and pass
its value in as an argument:

```rhai
const WANT = "yournick";

// WRONG: `grab` cannot see WANT.
// fn grab() { if nick() != WANT { raw(`NICK ${WANT}`); } }

// RIGHT: the hook reads WANT and passes it in.
fn grab(want) {
    if nick() != want { raw(`NICK ${want}`); }
}

fn on_quit(m) { grab(WANT); }
```

This is a real trap - `keepnick` shipped broken this way until a test caught it.
When a helper needs a const, hand it the value.

## Sandbox and lifecycle

**Loading.** Scripts load from `~/.config/norn/plugins/*.rhai` at startup and on
`/plugins reload`. A disabled plugin is still compiled (so its metadata shows and
compile failures are reported) but its hooks are not registered.

**Errors.**

- A *compile* error marks the plugin failed in `/plugins`; it does not run, and
  the others are unaffected.
- A *runtime* error in a hook is caught and surfaced as a notification; it does
  not crash norn or the other plugins.

**The store file.** All plugins' state lives in one file,
`~/.config/norn/plugins/store.toml`, namespaced per plugin. Writes are debounced:
`store.set`/`store.del` update memory immediately and are flushed to disk on an
idle timer (roughly every 5 seconds) and once more at shutdown. The tradeoff is
that an abrupt crash can lose the last few seconds of writes - fine for the kind
of state plugins keep (counters, last-seen times), and much cheaper than
rewriting the file on every call.

**Resource limits.** Each hook runs under caps so a runaway script aborts instead
of hanging the client: at most 200,000 operations, 64 call levels, expression
depth 64, strings up to 16 KB, and arrays / maps up to 4096 elements. There is no
filesystem or network access from a script - the host API is the whole surface.

## Worked examples

These are the bundled plugins. Install any with `/plugins install <name>` and
read the copy in your `plugins` folder.

### seen - the KV store

Records each nick's last line, and answers `!seen <nick>`:

```rhai
const NAME = "seen";
const DESCRIPTION = "track when nicks were last seen (KV store demo)";
const VERSION = "1.0";

fn on_message(m) {
    if m.from_self { return; }

    // Answer a "!seen <nick>" query.
    if m.text.starts_with("!seen ") {
        let who = m.text.sub_string(6);
        who.trim();                       // Rhai's trim mutates in place
        let last = store.get(who.to_lower());
        if last == "" {
            reply(`I have not seen ${who}.`);
        } else {
            reply(`${who} was last seen saying: ${last}`);
        }
        return;
    }

    // Otherwise record this nick's latest line.
    store.set(m.nick.to_lower(), m.text);
}
```

Note the `from_self` guard, the `store.get`/`store.set` round-trip, and Rhai
string methods (`starts_with`, `sub_string`, `trim`, `to_lower`).

### autoaway - config and presence

Marks you away after idle and back on activity, with a configurable message:

```rhai
const NAME = "autoaway";
const DESCRIPTION = "set away when idle, back when active";
const VERSION = "1.0";

const CONFIG = #{
    message: "auto-away (idle)",
};

fn on_idle(m) {
    if !am_away() {
        raw(`AWAY :${cfg("message")}`);
    }
}

fn on_active(m) {
    if am_away() {
        raw("AWAY");
    }
}
```

This shows `on_idle`/`on_active`, the `am_away()` presence check, and `CONFIG` +
`cfg()`.

### responder - matching messages

A tiny factoid bot driven by a map:

```rhai
const NAME = "responder";
const DESCRIPTION = "reply to trigger phrases";
const VERSION = "1.0";

const RULES = #{
    "!hello": "hi there!",
    "!norn": "norn is a terminal IRCv3 client",
};

fn on_message(m) {
    if m.from_self { return; }
    let text = m.text.to_lower();
    for key in RULES.keys() {
        if text.contains(key) {
            reply(RULES[key]);
        }
    }
}
```

Because `RULES` is read directly inside the hook (not in a helper), the
[const gotcha](#the-const-gotcha) does not bite here.

For the rest - `autoop` (`on_join`, checking the joiner against `in` trust
lists), `keepnick` (the const gotcha in the flesh), `autorejoin` (`on_kick` with
`m.is_me`) - read the installed scripts.
