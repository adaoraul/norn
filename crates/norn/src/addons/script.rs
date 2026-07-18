//! The Rhai scripting addon backend.
//!
//! Loads `*.rhai` scripts that define event hooks (`on_message`, `on_join`, ...)
//! and call a small host API (`reply`, `send`, `raw`, `notify`, `nick`). Hooks
//! run synchronously; the engine is hardened with operation/size limits so a
//! runaway script aborts instead of hanging the UI loop. Rhai is sandboxed by
//! default (no filesystem or network access is registered).

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rhai::{Dynamic, Engine, ImmutableString, Map, Scope, AST};

use crate::session::NetworkId;

use super::{event_reply_target, AddonCtx, AddonEvent, AddonEventKind, AddonHost, Reaction};

/// The event hooks a script may define.
const HOOKS: &[&str] = &["on_message", "on_join", "on_part", "on_quit", "on_nick"];

/// Per-invocation state the host API reads/writes while a hook runs. Shared with
/// the registered API closures via `Rc<RefCell<...>>` (single-threaded UI task).
#[derive(Default)]
struct HostState {
    net: NetworkId,
    my_nick: String,
    reply_to: Option<String>,
    reactions: Vec<Reaction>,
}

/// A compiled script and the hook names it defines.
struct Script {
    label: String,
    ast: AST,
    hooks: HashSet<String>,
}

/// The Rhai scripting host.
pub struct RhaiHost {
    engine: Engine,
    scripts: Vec<Script>,
    state: Rc<RefCell<HostState>>,
    loaded: Vec<String>,
    errors: Vec<String>,
}

impl RhaiHost {
    /// Load and compile every `*.rhai` file under `dir` (sorted for determinism).
    /// Missing dir is fine; read/compile errors are collected, not fatal.
    pub fn load(dir: &Path) -> RhaiHost {
        let mut entries: Vec<(String, String)> = Vec::new();
        let mut errors: Vec<String> = Vec::new();
        if dir.is_dir() {
            let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
                Ok(rd) => rd
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().is_some_and(|x| x == "rhai"))
                    .collect(),
                Err(e) => {
                    errors.push(format!("addons dir: {e}"));
                    Vec::new()
                }
            };
            files.sort();
            for path in files {
                let label = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?")
                    .to_string();
                match std::fs::read_to_string(&path) {
                    Ok(src) => entries.push((label, src)),
                    Err(e) => errors.push(format!("{label}: {e}")),
                }
            }
        }
        RhaiHost::build(entries, errors)
    }

    /// Build from in-memory `(label, source)` scripts (for tests).
    #[cfg(test)]
    pub fn from_sources(sources: &[(&str, &str)]) -> RhaiHost {
        let entries = sources
            .iter()
            .map(|(l, s)| (l.to_string(), s.to_string()))
            .collect();
        RhaiHost::build(entries, Vec::new())
    }

    fn build(entries: Vec<(String, String)>, mut errors: Vec<String>) -> RhaiHost {
        let state = Rc::new(RefCell::new(HostState::default()));
        let engine = build_engine(state.clone());
        let mut scripts = Vec::new();
        let mut loaded = Vec::new();
        for (label, src) in entries {
            match engine.compile(&src) {
                Ok(ast) => {
                    let hooks: HashSet<String> =
                        ast.iter_functions().map(|f| f.name.to_string()).collect();
                    let active: Vec<&str> = HOOKS
                        .iter()
                        .copied()
                        .filter(|h| hooks.contains(*h))
                        .collect();
                    loaded.push(if active.is_empty() {
                        format!("{label} (no hooks)")
                    } else {
                        format!("{label} ({})", active.join(", "))
                    });
                    scripts.push(Script { label, ast, hooks });
                }
                Err(err) => errors.push(format!("{label}: {err}")),
            }
        }
        RhaiHost {
            engine,
            scripts,
            state,
            loaded,
            errors,
        }
    }

    /// Summary lines for successfully loaded scripts (`label (hooks)`).
    pub fn loaded(&self) -> &[String] {
        &self.loaded
    }

    /// Compile/load error lines.
    pub fn errors(&self) -> &[String] {
        &self.errors
    }
}

impl AddonHost for RhaiHost {
    fn on_event(&mut self, event: &AddonEvent, ctx: &AddonCtx) -> Vec<Reaction> {
        let hook = hook_for(&event.kind);
        if !self.scripts.iter().any(|s| s.hooks.contains(hook)) {
            return Vec::new();
        }
        // Prime the shared state before any script runs; the borrow ends here so
        // the API closures can borrow it during the calls.
        {
            let mut s = self.state.borrow_mut();
            s.net = event.net;
            s.my_nick = ctx.my_nick.to_string();
            s.reply_to = event_reply_target(&event.kind, ctx.my_nick);
            s.reactions.clear();
        }
        let map = event_map(&event.kind, ctx);
        for script in &self.scripts {
            if !script.hooks.contains(hook) {
                continue;
            }
            let mut scope = Scope::new();
            if let Err(err) =
                self.engine
                    .call_fn::<()>(&mut scope, &script.ast, hook, (map.clone(),))
            {
                let mut s = self.state.borrow_mut();
                let net = s.net;
                s.reactions.push(Reaction::Notify {
                    net,
                    text: format!("addon {}: {err}", script.label),
                });
            }
        }
        std::mem::take(&mut self.state.borrow_mut().reactions)
    }
}

/// The hook name for an event kind.
fn hook_for(kind: &AddonEventKind) -> &'static str {
    match kind {
        AddonEventKind::Message { .. } => "on_message",
        AddonEventKind::Join { .. } => "on_join",
        AddonEventKind::Part { .. } => "on_part",
        AddonEventKind::Quit { .. } => "on_quit",
        AddonEventKind::NickChange { .. } => "on_nick",
    }
}

/// Build the event object passed to a hook. Every common key is always present
/// (with a default) so scripts never hit a missing property.
fn event_map(kind: &AddonEventKind, ctx: &AddonCtx) -> Map {
    let mut m = Map::new();
    m.insert("my_nick".into(), Dynamic::from(ctx.my_nick.to_string()));
    m.insert("network".into(), Dynamic::from(ctx.network.to_string()));
    m.insert("nick".into(), Dynamic::from(String::new()));
    m.insert("text".into(), Dynamic::from(String::new()));
    m.insert("channel".into(), Dynamic::from(String::new()));
    m.insert("reason".into(), Dynamic::from(String::new()));
    m.insert("new".into(), Dynamic::from(String::new()));
    m.insert("notice".into(), Dynamic::from(false));
    m.insert("highlight".into(), Dynamic::from(false));
    m.insert("from_self".into(), Dynamic::from(false));

    let reply_to = event_reply_target(kind, ctx.my_nick).unwrap_or_default();
    match kind {
        AddonEventKind::Message {
            nick,
            text,
            notice,
            highlight,
            from_self,
            ..
        } => {
            m.insert(
                "type".into(),
                Dynamic::from(if *notice { "notice" } else { "message" }.to_string()),
            );
            m.insert("nick".into(), Dynamic::from(nick.clone()));
            m.insert("text".into(), Dynamic::from(text.clone()));
            m.insert("channel".into(), Dynamic::from(reply_to));
            m.insert("notice".into(), Dynamic::from(*notice));
            m.insert("highlight".into(), Dynamic::from(*highlight));
            m.insert("from_self".into(), Dynamic::from(*from_self));
        }
        AddonEventKind::Join { nick, channel } => {
            m.insert("type".into(), Dynamic::from("join".to_string()));
            m.insert("nick".into(), Dynamic::from(nick.clone()));
            m.insert("channel".into(), Dynamic::from(channel.clone()));
        }
        AddonEventKind::Part {
            nick,
            channel,
            reason,
        } => {
            m.insert("type".into(), Dynamic::from("part".to_string()));
            m.insert("nick".into(), Dynamic::from(nick.clone()));
            m.insert("channel".into(), Dynamic::from(channel.clone()));
            m.insert("reason".into(), Dynamic::from(reason.clone()));
        }
        AddonEventKind::Quit { nick, reason } => {
            m.insert("type".into(), Dynamic::from("quit".to_string()));
            m.insert("nick".into(), Dynamic::from(nick.clone()));
            m.insert("reason".into(), Dynamic::from(reason.clone()));
        }
        AddonEventKind::NickChange { old, new } => {
            m.insert("type".into(), Dynamic::from("nick".to_string()));
            m.insert("nick".into(), Dynamic::from(old.clone()));
            m.insert("new".into(), Dynamic::from(new.clone()));
        }
    }
    m
}

/// Build a hardened, sandboxed engine with the host API registered against the
/// shared state.
fn build_engine(state: Rc<RefCell<HostState>>) -> Engine {
    let mut engine = Engine::new();
    // Bound runtime so a runaway hook aborts rather than hanging the UI loop.
    engine.set_max_operations(200_000);
    engine.set_max_call_levels(64);
    engine.set_max_expr_depths(64, 64);
    engine.set_max_string_size(16 * 1024);
    engine.set_max_array_size(4096);
    engine.set_max_map_size(4096);

    // reply(text): message the event's reply target (channel, or PM sender).
    let st = state.clone();
    engine.register_fn("reply", move |text: ImmutableString| {
        let mut s = st.borrow_mut();
        if let Some(target) = s.reply_to.clone() {
            let net = s.net;
            s.reactions.push(Reaction::Send {
                net,
                lines: vec![format!("PRIVMSG {target} :{text}")],
            });
        }
    });
    // send(target, text): message an explicit target.
    let st = state.clone();
    engine.register_fn(
        "send",
        move |target: ImmutableString, text: ImmutableString| {
            let mut s = st.borrow_mut();
            let net = s.net;
            s.reactions.push(Reaction::Send {
                net,
                lines: vec![format!("PRIVMSG {target} :{text}")],
            });
        },
    );
    // raw(line): send a raw IRC line.
    let st = state.clone();
    engine.register_fn("raw", move |line: ImmutableString| {
        let mut s = st.borrow_mut();
        let net = s.net;
        s.reactions.push(Reaction::Send {
            net,
            lines: vec![line.to_string()],
        });
    });
    // notify(text): local notification (console line + bell).
    let st = state.clone();
    engine.register_fn("notify", move |text: ImmutableString| {
        let mut s = st.borrow_mut();
        let net = s.net;
        s.reactions.push(Reaction::Notify {
            net,
            text: text.to_string(),
        });
    });
    // nick(): our nick on the event's network.
    let st = state.clone();
    engine.register_fn("nick", move || -> ImmutableString {
        st.borrow().my_nick.clone().into()
    });
    engine
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(target: &str, nick: &str, text: &str) -> AddonEvent {
        AddonEvent {
            net: 3,
            kind: AddonEventKind::Message {
                target: target.into(),
                nick: nick.into(),
                text: text.into(),
                notice: false,
                highlight: false,
                from_self: false,
            },
        }
    }

    fn ctx(my_nick: &str) -> AddonCtx<'_> {
        AddonCtx {
            my_nick,
            network: "libera",
        }
    }

    #[test]
    fn on_message_hook_replies() {
        let mut h = RhaiHost::from_sources(&[(
            "greet.rhai",
            r#"fn on_message(m) { reply("hi " + m.nick); }"#,
        )]);
        assert!(h.errors().is_empty());
        assert_eq!(h.loaded().len(), 1);
        let out = h.on_event(&message("#c", "bob", "hey"), &ctx("me"));
        assert!(matches!(&out[0], Reaction::Send { net: 3, lines }
            if lines == &["PRIVMSG #c :hi bob"]));
    }

    #[test]
    fn nick_and_conditional_reply() {
        let mut h = RhaiHost::from_sources(&[(
            "t.rhai",
            r#"fn on_message(m) { if m.text.contains(nick()) { reply("you rang?"); } }"#,
        )]);
        // Mentions "me" -> replies.
        let out = h.on_event(&message("#c", "bob", "hey me"), &ctx("me"));
        assert_eq!(out.len(), 1);
        // No mention -> nothing.
        let out = h.on_event(&message("#c", "bob", "hey you"), &ctx("me"));
        assert!(out.is_empty());
    }

    #[test]
    fn missing_hook_is_skipped() {
        let mut h = RhaiHost::from_sources(&[("t.rhai", r#"fn on_join(m) { notify("j"); }"#)]);
        assert!(h
            .on_event(&message("#c", "bob", "hi"), &ctx("me"))
            .is_empty());
    }

    #[test]
    fn compile_error_is_captured_not_panic() {
        let h = RhaiHost::from_sources(&[("bad.rhai", "fn on_message(m) { this is )( invalid")]);
        assert!(h.loaded().is_empty());
        assert_eq!(h.errors().len(), 1);
        assert!(h.errors()[0].starts_with("bad.rhai:"));
    }

    #[test]
    fn runtime_error_becomes_a_notify() {
        let mut h = RhaiHost::from_sources(&[("t.rhai", "fn on_message(m) { no_such_fn(); }")]);
        let out = h.on_event(&message("#c", "bob", "hi"), &ctx("me"));
        assert!(matches!(&out[0], Reaction::Notify { text, .. }
            if text.contains("addon t.rhai")));
    }

    #[test]
    fn infinite_loop_is_bounded_by_the_op_limit() {
        // Without the operation cap this would hang; with it the hook aborts and
        // the error surfaces as a notification.
        let mut h = RhaiHost::from_sources(&[("t.rhai", "fn on_message(m) { loop { } }")]);
        let out = h.on_event(&message("#c", "bob", "hi"), &ctx("me"));
        assert!(matches!(&out[0], Reaction::Notify { .. }));
    }
}
