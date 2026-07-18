//! The Rhai scripting addon backend.
//!
//! Loads `*.rhai` scripts that define event hooks (`on_message`, `on_join`, ...)
//! and call a small host API (`reply`, `send`, `raw`, `notify`, `nick`). Hooks
//! run synchronously; the engine is hardened with operation/size limits so a
//! runaway script aborts instead of hanging the UI loop. Rhai is sandboxed by
//! default (no filesystem or network access is registered).

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rhai::{Array, Dynamic, Engine, ImmutableString, Map, Scope, AST};

use crate::session::NetworkId;

#[cfg(test)]
use super::EMPTY_PRESENCE;
use super::{
    event_reply_target, AddonCtx, AddonEvent, AddonEventKind, AddonHost, PluginInfo, PluginStatus,
    Presence, Reaction,
};

/// Per-invocation state the host API reads/writes while a hook runs. Shared with
/// the registered API closures via `Rc<RefCell<...>>` (single-threaded UI task).
#[derive(Default)]
struct HostState {
    net: NetworkId,
    my_nick: String,
    reply_to: Option<String>,
    reactions: Vec<Reaction>,
    /// Namespace (plugin stem) of the script currently running, so the KV store
    /// closures key writes under the right plugin.
    current_script: String,
    /// Host-owned persistent key/value store: plugin stem -> key -> value.
    store: HashMap<String, HashMap<String, String>>,
    /// Where the store is persisted; `None` for in-memory (tests).
    store_path: Option<PathBuf>,
    /// Whether the store changed since the last flush (debounces disk writes).
    store_dirty: bool,
    /// Our presence on the event's network (for the read-only accessors).
    presence: Presence,
    /// Effective per-plugin config (defaults merged with user overrides): plugin
    /// stem -> key -> value. Read by `cfg(key)`.
    config: HashMap<String, HashMap<String, String>>,
}

/// The `store` handle scripts call methods on (`store.get`/`set`/...). Zero-sized;
/// the real state lives in the shared `HostState`, keyed by the running plugin.
#[derive(Clone)]
struct Store;

/// A compiled script and the hook names it defines.
struct Script {
    label: String,
    ast: AST,
    hooks: HashSet<String>,
    /// Top-level literal constants (`const NAME = ...`), pushed into a hook's
    /// scope so functions can read them (Rhai does not share them into `call_fn`
    /// scopes automatically).
    consts: Vec<(String, Dynamic)>,
}

/// Presence-accessor names; if no loaded script references one, the supervisor
/// skips building the per-event presence snapshot.
const PRESENCE_FNS: &[&str] = &["am_away(", "my_account(", "channels(", "names(", "is_op("];

/// The Rhai scripting host.
pub struct RhaiHost {
    engine: Engine,
    scripts: Vec<Script>,
    state: Rc<RefCell<HostState>>,
    plugins: Vec<PluginInfo>,
    /// Whether any loaded script calls a presence accessor (drives snapshotting).
    uses_presence: bool,
}

impl RhaiHost {
    /// Load every `*.rhai` file under `dir` (sorted). Each is compiled to detect
    /// failures and read metadata; only enabled (not `disabled`) ones that
    /// compile get their hooks registered. Missing dir is fine.
    pub fn load(
        dir: &Path,
        disabled: &HashSet<String>,
        overrides: &BTreeMap<String, BTreeMap<String, String>>,
    ) -> RhaiHost {
        let mut entries: Vec<(String, Result<String, String>)> = Vec::new();
        if dir.is_dir() {
            let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
                Ok(rd) => rd
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().is_some_and(|x| x == "rhai"))
                    .collect(),
                Err(e) => {
                    entries.push(("plugins".to_string(), Err(e.to_string())));
                    Vec::new()
                }
            };
            files.sort();
            for path in files {
                let file = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?")
                    .to_string();
                entries.push((
                    file,
                    std::fs::read_to_string(&path).map_err(|e| e.to_string()),
                ));
            }
        }
        RhaiHost::build(entries, disabled, Some(dir.join("store.toml")), overrides)
    }

    /// Build from in-memory `(file, source)` scripts (for tests). The KV store is
    /// in-memory only (no persistence path).
    #[cfg(test)]
    pub fn from_sources(sources: &[(&str, &str)], disabled: &[&str]) -> RhaiHost {
        RhaiHost::from_sources_cfg(sources, disabled, &BTreeMap::new())
    }

    /// Like [`Self::from_sources`], with per-plugin config overrides (for tests).
    #[cfg(test)]
    pub fn from_sources_cfg(
        sources: &[(&str, &str)],
        disabled: &[&str],
        overrides: &BTreeMap<String, BTreeMap<String, String>>,
    ) -> RhaiHost {
        let entries = sources
            .iter()
            .map(|(f, s)| (f.to_string(), Ok(s.to_string())))
            .collect();
        let disabled: HashSet<String> = disabled.iter().map(|s| s.to_string()).collect();
        RhaiHost::build(entries, &disabled, None, overrides)
    }

    fn build(
        entries: Vec<(String, Result<String, String>)>,
        disabled: &HashSet<String>,
        store_path: Option<PathBuf>,
        overrides: &BTreeMap<String, BTreeMap<String, String>>,
    ) -> RhaiHost {
        let mut host_state = HostState::default();
        if let Some(path) = &store_path {
            if let Ok(txt) = std::fs::read_to_string(path) {
                if let Ok(map) = toml::from_str(&txt) {
                    host_state.store = map;
                }
            }
        }
        host_state.store_path = store_path;
        let state = Rc::new(RefCell::new(host_state));
        let engine = build_engine(state.clone());
        let mut scripts = Vec::new();
        let mut plugins = Vec::new();
        // Effective config per enabled plugin (stem -> key -> value), read by cfg().
        let mut effective: HashMap<String, HashMap<String, String>> = HashMap::new();
        // Whether any enabled script calls a presence accessor (cheap source scan;
        // a false positive only costs an occasional snapshot).
        let mut uses_presence = false;
        for (file, source) in entries {
            let src = match source {
                Ok(src) => src,
                Err(err) => {
                    plugins.push(failed_plugin(&file, err));
                    continue;
                }
            };
            match engine.compile(&src) {
                Ok(ast) => {
                    let (name, description, version) = read_metadata(&ast, &file);
                    let schema = read_config_schema(&ast);
                    let off = disabled.contains(&file);
                    if !off {
                        if PRESENCE_FNS.iter().any(|f| src.contains(f)) {
                            uses_presence = true;
                        }
                        let hooks: HashSet<String> =
                            ast.iter_functions().map(|f| f.name.to_string()).collect();
                        let consts: Vec<(String, Dynamic)> = ast
                            .iter_literal_variables(true, false)
                            .map(|(k, _, v)| (k.to_string(), v))
                            .collect();
                        // Defaults overridden by the user's stored values (only
                        // keys the plugin actually declares).
                        let mut eff: HashMap<String, String> = schema.iter().cloned().collect();
                        if let Some(ov) = overrides.get(&file) {
                            for (k, v) in ov {
                                if eff.contains_key(k) {
                                    eff.insert(k.clone(), v.clone());
                                }
                            }
                        }
                        effective.insert(stem(&file), eff);
                        scripts.push(Script {
                            label: file.clone(),
                            ast,
                            hooks,
                            consts,
                        });
                    }
                    plugins.push(PluginInfo {
                        name,
                        file,
                        description,
                        version,
                        status: if off {
                            PluginStatus::Disabled
                        } else {
                            PluginStatus::Loaded
                        },
                        config: schema,
                    });
                }
                Err(err) => plugins.push(failed_plugin(&file, err.to_string())),
            }
        }
        state.borrow_mut().config = effective;
        // Order for display: loaded, then disabled, then failed; name within.
        let group = |p: &PluginInfo| match p.status {
            PluginStatus::Loaded => 0u8,
            PluginStatus::Disabled => 1,
            PluginStatus::Failed(_) => 2,
        };
        plugins.sort_by(|a, b| {
            group(a)
                .cmp(&group(b))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        RhaiHost {
            engine,
            scripts,
            state,
            plugins,
            uses_presence,
        }
    }

    /// The discovered plugins with their status and metadata.
    pub fn plugins(&self) -> &[PluginInfo] {
        &self.plugins
    }

    /// Whether any loaded script calls a presence accessor. When false the
    /// supervisor can skip building the per-event presence snapshot.
    pub fn uses_presence(&self) -> bool {
        self.uses_presence
    }
}

/// A `PluginInfo` for a script that failed to load.
fn failed_plugin(file: &str, err: String) -> PluginInfo {
    PluginInfo {
        name: stem(file),
        file: file.to_string(),
        description: String::new(),
        version: String::new(),
        status: PluginStatus::Failed(err),
        config: Vec::new(),
    }
}

/// The filename without the `.rhai` extension.
fn stem(file: &str) -> String {
    file.strip_suffix(".rhai").unwrap_or(file).to_string()
}

/// Read `NAME`/`DESCRIPTION`/`VERSION` from a script's top-level consts (without
/// running it). `name` falls back to the filename stem.
fn read_metadata(ast: &AST, file: &str) -> (String, String, String) {
    let (mut name, mut description, mut version) = (String::new(), String::new(), String::new());
    for (key, _is_const, value) in ast.iter_literal_variables(true, false) {
        let text = value
            .clone()
            .into_string()
            .unwrap_or_else(|_| value.to_string());
        match key {
            "NAME" => name = text,
            "DESCRIPTION" => description = text,
            "VERSION" => version = text,
            _ => {}
        }
    }
    if name.is_empty() {
        name = stem(file);
    }
    (name, description, version)
}

/// Read a script's `CONFIG` const (a map of key -> default value) as sorted
/// `(key, default)` pairs, without running it. Empty if absent or not a map.
fn read_config_schema(ast: &AST) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (key, _is_const, value) in ast.iter_literal_variables(true, false) {
        if key == "CONFIG" {
            if let Some(map) = value.try_cast::<Map>() {
                for (k, v) in map {
                    let text = v.clone().into_string().unwrap_or_else(|_| v.to_string());
                    out.push((k.to_string(), text));
                }
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
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
            s.presence = ctx.presence.clone();
        }
        let map = event_map(&event.kind, ctx);
        for script in &self.scripts {
            if !script.hooks.contains(hook) {
                continue;
            }
            // Namespace the KV store to the plugin about to run.
            self.state.borrow_mut().current_script = stem(&script.label);
            let mut scope = Scope::new();
            // Push the script's top-level consts (and the `store` handle) so the
            // hook can read them. NOTE: Rhai scope constants reach only the entry
            // hook, not helper functions it calls (nested script fns get a fresh
            // frame). So a plugin must read a `const` in its hook and pass the
            // value into any helper as an argument (see keepnick.rhai).
            for (name, value) in &script.consts {
                scope.push_constant(name.as_str(), value.clone());
            }
            scope.push_constant("store", Store);
            if let Err(err) =
                self.engine
                    .call_fn::<()>(&mut scope, &script.ast, hook, (map.clone(),))
            {
                let mut s = self.state.borrow_mut();
                let net = s.net;
                s.reactions.push(Reaction::Notify {
                    net,
                    text: format!("plugin {}: {err}", script.label),
                });
            }
        }
        std::mem::take(&mut self.state.borrow_mut().reactions)
    }

    /// Persist the KV store if it changed since the last flush (write-behind, so a
    /// hot `store.set` loop does not rewrite the file on every call).
    fn flush(&mut self) {
        let mut s = self.state.borrow_mut();
        if s.store_dirty {
            persist_store(&s);
            s.store_dirty = false;
        }
    }
}

/// The hook name for an event kind.
fn hook_for(kind: &AddonEventKind) -> &'static str {
    match kind {
        AddonEventKind::Message { .. } => "on_message",
        AddonEventKind::Join { .. } => "on_join",
        AddonEventKind::Part { .. } => "on_part",
        AddonEventKind::Kick { .. } => "on_kick",
        AddonEventKind::Quit { .. } => "on_quit",
        AddonEventKind::NickChange { .. } => "on_nick",
        AddonEventKind::Idle { .. } => "on_idle",
        AddonEventKind::Active => "on_active",
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
    m.insert("by".into(), Dynamic::from(String::new()));
    m.insert("notice".into(), Dynamic::from(false));
    m.insert("highlight".into(), Dynamic::from(false));
    m.insert("from_self".into(), Dynamic::from(false));
    m.insert("is_me".into(), Dynamic::from(false));
    m.insert("seconds".into(), Dynamic::from(0_i64));

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
        AddonEventKind::Kick {
            channel,
            nick,
            by,
            reason,
            is_me,
        } => {
            m.insert("type".into(), Dynamic::from("kick".to_string()));
            m.insert("nick".into(), Dynamic::from(nick.clone()));
            m.insert("channel".into(), Dynamic::from(channel.clone()));
            m.insert("by".into(), Dynamic::from(by.clone()));
            m.insert("reason".into(), Dynamic::from(reason.clone()));
            m.insert("is_me".into(), Dynamic::from(*is_me));
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
        AddonEventKind::Idle { seconds } => {
            m.insert("type".into(), Dynamic::from("idle".to_string()));
            m.insert("seconds".into(), Dynamic::from(*seconds as i64));
        }
        AddonEventKind::Active => {
            m.insert("type".into(), Dynamic::from("active".to_string()));
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
    // cfg(key): this plugin's config value (user override, else declared default,
    // else ""). Lets scripts read tunable settings without hard-coding them.
    let st = state.clone();
    engine.register_fn("cfg", move |key: ImmutableString| -> ImmutableString {
        let s = st.borrow();
        s.config
            .get(&s.current_script)
            .and_then(|m| m.get(key.as_str()))
            .cloned()
            .unwrap_or_default()
            .into()
    });
    // desktop_notify(text): raise an OS desktop notification (host runs it).
    let st = state.clone();
    engine.register_fn("desktop_notify", move |text: ImmutableString| {
        st.borrow_mut().reactions.push(Reaction::Desktop {
            text: text.to_string(),
        });
    });

    // Read-only presence accessors, from the per-event snapshot.
    // am_away(): are we marked away on the event's network?
    let st = state.clone();
    engine.register_fn("am_away", move || -> bool { st.borrow().presence.away });
    // my_account(): our services account ("" if not logged in).
    let st = state.clone();
    engine.register_fn("my_account", move || -> ImmutableString {
        st.borrow()
            .presence
            .account
            .clone()
            .unwrap_or_default()
            .into()
    });
    // channels(): the channels we are in.
    let st = state.clone();
    engine.register_fn("channels", move || -> Array {
        st.borrow()
            .presence
            .channels
            .iter()
            .map(|(name, _)| Dynamic::from(name.clone()))
            .collect()
    });
    // names(channel): the nicks in a channel we are in (empty if not a member).
    let st = state.clone();
    engine.register_fn("names", move |channel: ImmutableString| -> Array {
        let s = st.borrow();
        s.presence
            .channels
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(&channel))
            .map(|(_, members)| {
                members
                    .iter()
                    .map(|(nick, _)| Dynamic::from(nick.clone()))
                    .collect()
            })
            .unwrap_or_default()
    });
    // is_op(channel, nick): does nick hold op (or higher) in channel?
    let st = state.clone();
    engine.register_fn(
        "is_op",
        move |channel: ImmutableString, nick: ImmutableString| -> bool {
            let s = st.borrow();
            s.presence
                .channels
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(&channel))
                .is_some_and(|(_, members)| {
                    members
                        .iter()
                        .any(|(n, op)| *op && n.eq_ignore_ascii_case(&nick))
                })
        },
    );

    // KV store: `store.get/set/del/has/keys`, namespaced per plugin and persisted
    // by the host. Scripts never see the path or touch disk themselves.
    engine.register_type_with_name::<Store>("Store");
    // store.get(key) -> value ("" if unset).
    let st = state.clone();
    engine.register_fn(
        "get",
        move |_s: &mut Store, key: ImmutableString| -> ImmutableString {
            let s = st.borrow();
            s.store
                .get(&s.current_script)
                .and_then(|m| m.get(key.as_str()))
                .cloned()
                .unwrap_or_default()
                .into()
        },
    );
    // store.set(key, value): store a value (persisted on the next flush).
    let st = state.clone();
    engine.register_fn(
        "set",
        move |_s: &mut Store, key: ImmutableString, val: ImmutableString| {
            let mut s = st.borrow_mut();
            let ns = s.current_script.clone();
            s.store
                .entry(ns)
                .or_default()
                .insert(key.to_string(), val.to_string());
            s.store_dirty = true;
        },
    );
    // store.del(key): remove a key (persisted on the next flush).
    let st = state.clone();
    engine.register_fn("del", move |_s: &mut Store, key: ImmutableString| {
        let mut s = st.borrow_mut();
        let ns = s.current_script.clone();
        if let Some(m) = s.store.get_mut(&ns) {
            m.remove(key.as_str());
        }
        s.store_dirty = true;
    });
    // store.has(key) -> bool.
    let st = state.clone();
    engine.register_fn("has", move |_s: &mut Store, key: ImmutableString| -> bool {
        let s = st.borrow();
        s.store
            .get(&s.current_script)
            .is_some_and(|m| m.contains_key(key.as_str()))
    });
    // store.keys() -> array of this plugin's keys.
    let st = state.clone();
    engine.register_fn("keys", move |_s: &mut Store| -> Array {
        let s = st.borrow();
        s.store
            .get(&s.current_script)
            .map(|m| m.keys().map(|k| Dynamic::from(k.clone())).collect())
            .unwrap_or_default()
    });
    engine
}

/// Write the KV store to its file, if a path is set. Best effort: a write error
/// is dropped (the in-memory map stays authoritative for the session).
fn persist_store(s: &HostState) {
    if let Some(path) = &s.store_path {
        if let Ok(txt) = toml::to_string_pretty(&s.store) {
            let _ = std::fs::write(path, txt);
        }
    }
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
            presence: &EMPTY_PRESENCE,
        }
    }

    #[test]
    fn on_message_hook_replies() {
        let mut h = RhaiHost::from_sources(
            &[(
                "greet.rhai",
                r#"fn on_message(m) { reply("hi " + m.nick); }"#,
            )],
            &[],
        );
        assert_eq!(h.plugins().len(), 1);
        assert!(matches!(h.plugins()[0].status, PluginStatus::Loaded));
        let out = h.on_event(&message("#c", "bob", "hey"), &ctx("me"));
        assert!(matches!(&out[0], Reaction::Send { net: 3, lines }
            if lines == &["PRIVMSG #c :hi bob"]));
    }

    #[test]
    fn nick_and_conditional_reply() {
        let mut h = RhaiHost::from_sources(
            &[(
                "t.rhai",
                r#"fn on_message(m) { if m.text.contains(nick()) { reply("you rang?"); } }"#,
            )],
            &[],
        );
        // Mentions "me" -> replies.
        let out = h.on_event(&message("#c", "bob", "hey me"), &ctx("me"));
        assert_eq!(out.len(), 1);
        // No mention -> nothing.
        let out = h.on_event(&message("#c", "bob", "hey you"), &ctx("me"));
        assert!(out.is_empty());
    }

    #[test]
    fn on_kick_hook_sees_is_me() {
        let mut h = RhaiHost::from_sources(
            &[(
                "rejoin.rhai",
                r#"fn on_kick(m) { if m.is_me { raw("JOIN " + m.channel); } }"#,
            )],
            &[],
        );
        let kicked_me = AddonEvent {
            net: 0,
            kind: AddonEventKind::Kick {
                channel: "#rust".into(),
                nick: "me".into(),
                by: "op".into(),
                reason: "bye".into(),
                is_me: true,
            },
        };
        let out = h.on_event(&kicked_me, &ctx("me"));
        assert!(matches!(&out[0], Reaction::Send { lines, .. } if lines == &["JOIN #rust"]));
        // Someone else kicked -> is_me false -> nothing.
        let kicked_other = AddonEvent {
            net: 0,
            kind: AddonEventKind::Kick {
                channel: "#rust".into(),
                nick: "bob".into(),
                by: "op".into(),
                reason: String::new(),
                is_me: false,
            },
        };
        assert!(h.on_event(&kicked_other, &ctx("me")).is_empty());
    }

    #[test]
    fn store_persists_across_events_and_namespaces_by_plugin() {
        // `a` accumulates in its own namespace; `b` reads the SAME key but sees
        // its own (empty) namespace, proving isolation. Both define on_message,
        // so both run per event; scripts are ordered by name (a before b).
        let mut h = RhaiHost::from_sources(
            &[
                (
                    "a.rhai",
                    r#"fn on_message(m) { let n = store.get("k"); store.set("k", n + "x"); reply(store.get("k")); }"#,
                ),
                (
                    "b.rhai",
                    r#"fn on_message(m) { reply("b:" + store.get("k")); }"#,
                ),
            ],
            &[],
        );
        // First event: a -> "x"; b sees its own empty namespace -> "b:".
        let out = h.on_event(&message("#c", "bob", "hi"), &ctx("me"));
        assert!(matches!(&out[0], Reaction::Send { lines, .. } if lines == &["PRIVMSG #c :x"]));
        assert!(matches!(&out[1], Reaction::Send { lines, .. } if lines == &["PRIVMSG #c :b:"]));
        // Second event: a's value persisted -> "xx"; b still isolated -> "b:".
        let out = h.on_event(&message("#c", "bob", "hi"), &ctx("me"));
        assert!(matches!(&out[0], Reaction::Send { lines, .. } if lines == &["PRIVMSG #c :xx"]));
        assert!(matches!(&out[1], Reaction::Send { lines, .. } if lines == &["PRIVMSG #c :b:"]));
    }

    #[test]
    fn store_has_del_and_missing_key() {
        let mut h = RhaiHost::from_sources(
            &[(
                "t.rhai",
                r#"fn on_message(m) {
                       reply(store.has("k").to_string());
                       store.set("k", "v");
                       reply(store.has("k").to_string());
                       store.del("k");
                       reply(store.has("k").to_string());
                       reply("[" + store.get("k") + "]");
                   }"#,
            )],
            &[],
        );
        let out = h.on_event(&message("#c", "bob", "hi"), &ctx("me"));
        let sent: Vec<&str> = out
            .iter()
            .filter_map(|r| match r {
                Reaction::Send { lines, .. } => lines.first().map(|s| s.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            sent,
            vec![
                "PRIVMSG #c :false",
                "PRIVMSG #c :true",
                "PRIVMSG #c :false",
                "PRIVMSG #c :[]",
            ]
        );
    }

    #[test]
    fn store_writes_are_debounced_until_flush() {
        let dir = std::env::temp_dir().join("norn-store-flush-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Reply with the current value, then set it (buffered).
        std::fs::write(
            dir.join("c.rhai"),
            r#"fn on_message(m) { reply(store.get("k")); store.set("k", "v"); }"#,
        )
        .unwrap();
        let store_file = dir.join("store.toml");

        let mut host = RhaiHost::load(&dir, &HashSet::new(), &BTreeMap::new());
        // First event: the store is empty -> reply is blank; the set is buffered.
        let out = host.on_event(&message("#c", "bob", "hi"), &ctx("me"));
        assert!(matches!(&out[0], Reaction::Send { lines, .. } if lines == &["PRIVMSG #c :"]));
        assert!(
            !store_file.exists(),
            "write should be debounced, not on disk"
        );
        // Flush persists it.
        host.flush();
        assert!(store_file.exists(), "flush should write the store");
        // A fresh host loading the same dir sees the flushed value on its next event.
        let mut host2 = RhaiHost::load(&dir, &HashSet::new(), &BTreeMap::new());
        let out = host2.on_event(&message("#c", "bob", "hi"), &ctx("me"));
        assert!(matches!(&out[0], Reaction::Send { lines, .. } if lines == &["PRIVMSG #c :v"]));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn uses_presence_reflects_accessor_calls() {
        // A script that calls a presence accessor is flagged.
        let h = RhaiHost::from_sources(
            &[(
                "p.rhai",
                r#"fn on_message(m) { if am_away() { reply("brb"); } }"#,
            )],
            &[],
        );
        assert!(h.uses_presence());
        // One that does not is not.
        let h = RhaiHost::from_sources(&[("q.rhai", r#"fn on_message(m) { reply("hi"); }"#)], &[]);
        assert!(!h.uses_presence());
        // A disabled presence-using script does not count (it never runs).
        let h = RhaiHost::from_sources(
            &[(
                "p.rhai",
                r##"fn on_message(m) { let x = is_op("#c", "a"); }"##,
            )],
            &["p.rhai"],
        );
        assert!(!h.uses_presence());
    }

    #[test]
    fn desktop_notify_yields_a_desktop_reaction() {
        let mut h = RhaiHost::from_sources(
            &[(
                "d.rhai",
                r#"fn on_message(m) { desktop_notify("hi " + m.nick); }"#,
            )],
            &[],
        );
        let out = h.on_event(&message("#c", "bob", "hey"), &ctx("me"));
        assert_eq!(out.len(), 1);
        assert!(matches!(&out[0], Reaction::Desktop { text } if text == "hi bob"));
    }

    #[test]
    fn presence_accessors_read_the_snapshot() {
        let mut h = RhaiHost::from_sources(
            &[(
                "p.rhai",
                r##"fn on_message(m) {
                       reply("away=" + am_away().to_string());
                       reply("acct=" + my_account());
                       reply("chans=" + channels().len().to_string());
                       reply("op=" + is_op("#rust", "alice").to_string());
                       reply("notop=" + is_op("#rust", "bob").to_string());
                       reply("names=" + names("#rust").len().to_string());
                   }"##,
            )],
            &[],
        );
        let presence = Presence {
            away: true,
            account: Some("svan".into()),
            channels: vec![(
                "#rust".into(),
                vec![("alice".into(), true), ("bob".into(), false)],
            )],
        };
        let ctx = AddonCtx {
            my_nick: "me",
            network: "libera",
            presence: &presence,
        };
        let out = h.on_event(&message("#rust", "bob", "hi"), &ctx);
        let sent: Vec<&str> = out
            .iter()
            .filter_map(|r| match r {
                Reaction::Send { lines, .. } => lines.first().map(|s| s.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            sent,
            vec![
                "PRIVMSG #rust :away=true",
                "PRIVMSG #rust :acct=svan",
                "PRIVMSG #rust :chans=1",
                "PRIVMSG #rust :op=true",
                "PRIVMSG #rust :notop=false",
                "PRIVMSG #rust :names=2",
            ]
        );
    }

    #[test]
    fn idle_and_active_hooks_fire() {
        let mut h = RhaiHost::from_sources(
            &[(
                "a.rhai",
                r#"fn on_idle(m) { raw("AWAY :idle " + m.seconds.to_string()); }
                   fn on_active(m) { raw("AWAY"); }"#,
            )],
            &[],
        );
        let idle = AddonEvent {
            net: 0,
            kind: AddonEventKind::Idle { seconds: 300 },
        };
        let out = h.on_event(&idle, &ctx("me"));
        assert!(matches!(&out[0], Reaction::Send { lines, .. } if lines == &["AWAY :idle 300"]));
        let active = AddonEvent {
            net: 0,
            kind: AddonEventKind::Active,
        };
        let out = h.on_event(&active, &ctx("me"));
        assert!(matches!(&out[0], Reaction::Send { lines, .. } if lines == &["AWAY"]));
    }

    #[test]
    fn cfg_returns_override_else_default() {
        let src = r##"const CONFIG = #{ greeting: "hi", target: "#norn" };
                     fn on_message(m) { reply(cfg("greeting") + " to " + cfg("target")); }"##;
        // No override: declared defaults.
        let mut h = RhaiHost::from_sources(&[("g.rhai", src)], &[]);
        let out = h.on_event(&message("#c", "bob", "yo"), &ctx("me"));
        assert!(matches!(&out[0], Reaction::Send { lines, .. }
            if lines == &["PRIVMSG #c :hi to #norn"]));
        // The declared schema is exposed on PluginInfo, sorted by key.
        assert_eq!(
            h.plugins()[0].config,
            vec![
                ("greeting".to_string(), "hi".to_string()),
                ("target".to_string(), "#norn".to_string()),
            ]
        );
        // With an override on one key, the other keeps its default.
        let mut overrides = BTreeMap::new();
        let mut keys = BTreeMap::new();
        keys.insert("greeting".to_string(), "yo".to_string());
        overrides.insert("g.rhai".to_string(), keys);
        let mut h = RhaiHost::from_sources_cfg(&[("g.rhai", src)], &[], &overrides);
        let out = h.on_event(&message("#c", "bob", "yo"), &ctx("me"));
        assert!(matches!(&out[0], Reaction::Send { lines, .. }
            if lines == &["PRIVMSG #c :yo to #norn"]));
    }

    #[test]
    fn missing_hook_is_skipped() {
        let mut h = RhaiHost::from_sources(&[("t.rhai", r#"fn on_join(m) { notify("j"); }"#)], &[]);
        assert!(h
            .on_event(&message("#c", "bob", "hi"), &ctx("me"))
            .is_empty());
    }

    #[test]
    fn reads_metadata_from_consts() {
        let h = RhaiHost::from_sources(
            &[(
                "nc.rhai",
                r#"const NAME = "nickcolor";
                   const DESCRIPTION = "deterministic nick colors";
                   const VERSION = "1.4";
                   fn on_message(m) {}"#,
            )],
            &[],
        );
        let p = &h.plugins()[0];
        assert_eq!(p.name, "nickcolor");
        assert_eq!(p.description, "deterministic nick colors");
        assert_eq!(p.version, "1.4");
        assert!(matches!(p.status, PluginStatus::Loaded));
    }

    #[test]
    fn disabled_script_is_listed_but_not_run() {
        let mut h = RhaiHost::from_sources(
            &[("greet.rhai", r#"fn on_message(m) { reply("hi"); }"#)],
            &["greet.rhai"],
        );
        assert!(matches!(h.plugins()[0].status, PluginStatus::Disabled));
        assert!(h
            .on_event(&message("#c", "bob", "hi"), &ctx("me"))
            .is_empty());
    }

    #[test]
    fn compile_error_is_captured_not_panic() {
        let h = RhaiHost::from_sources(
            &[("bad.rhai", "fn on_message(m) { this is )( invalid")],
            &[],
        );
        assert_eq!(h.plugins().len(), 1);
        assert!(matches!(h.plugins()[0].status, PluginStatus::Failed(_)));
    }

    #[test]
    fn runtime_error_becomes_a_notify() {
        let mut h =
            RhaiHost::from_sources(&[("t.rhai", "fn on_message(m) { no_such_fn(); }")], &[]);
        let out = h.on_event(&message("#c", "bob", "hi"), &ctx("me"));
        assert!(matches!(&out[0], Reaction::Notify { text, .. }
            if text.contains("plugin t.rhai")));
    }

    #[test]
    fn infinite_loop_is_bounded_by_the_op_limit() {
        // Without the operation cap this would hang; with it the hook aborts and
        // the error surfaces as a notification.
        let mut h = RhaiHost::from_sources(&[("t.rhai", "fn on_message(m) { loop { } }")], &[]);
        let out = h.on_event(&message("#c", "bob", "hi"), &ctx("me"));
        assert!(matches!(&out[0], Reaction::Notify { .. }));
    }
}
