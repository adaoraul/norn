//! Official starter plugins bundled into the binary and installable with
//! `/plugins install <name>`. Each ships with an editable `const` config block at
//! the top; installing copies it into the user's plugins folder to customize.

/// A bundled official plugin script.
pub struct OfficialPlugin {
    /// The plugin name (also the installed filename stem).
    pub name: &'static str,
    /// A one-line description.
    pub description: &'static str,
    /// The script source.
    pub source: &'static str,
}

/// Every bundled official plugin.
pub const OFFICIAL: &[OfficialPlugin] = &[
    OfficialPlugin {
        name: "autoop",
        description: "op trusted nicks when they join",
        source: include_str!("official/autoop.rhai"),
    },
    OfficialPlugin {
        name: "keepnick",
        description: "reclaim your preferred nick when it frees up",
        source: include_str!("official/keepnick.rhai"),
    },
    OfficialPlugin {
        name: "responder",
        description: "reply to trigger phrases",
        source: include_str!("official/responder.rhai"),
    },
    OfficialPlugin {
        name: "autorejoin",
        description: "rejoin a channel after being kicked",
        source: include_str!("official/autorejoin.rhai"),
    },
    OfficialPlugin {
        name: "seen",
        description: "track when nicks were last seen (KV store demo)",
        source: include_str!("official/seen.rhai"),
    },
    OfficialPlugin {
        name: "autoaway",
        description: "set away when idle, back when active",
        source: include_str!("official/autoaway.rhai"),
    },
];

/// Look up a bundled plugin by name.
pub fn find(name: &str) -> Option<&'static OfficialPlugin> {
    OFFICIAL.iter().find(|p| p.name.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::{
        AddonCtx, AddonEvent, AddonEventKind, AddonHost, PluginStatus, Presence, Reaction,
        RhaiHost, EMPTY_PRESENCE,
    };

    #[test]
    fn every_official_plugin_compiles() {
        for plugin in OFFICIAL {
            let host = RhaiHost::from_sources(&[(plugin.name, plugin.source)], &[]);
            assert!(
                matches!(host.plugins()[0].status, PluginStatus::Loaded),
                "{} failed to compile: {:?}",
                plugin.name,
                host.plugins()[0].status
            );
        }
    }

    #[test]
    fn autorejoin_rejoins_on_self_kick() {
        let src = find("autorejoin").unwrap().source;
        let mut host = RhaiHost::from_sources(&[("autorejoin.rhai", src)], &[]);
        let ev = AddonEvent {
            net: 0,
            kind: AddonEventKind::Kick {
                channel: "#norn".into(),
                nick: "me".into(),
                by: "op".into(),
                reason: String::new(),
                is_me: true,
            },
        };
        let ctx = AddonCtx {
            my_nick: "me",
            network: "libera",
            presence: &EMPTY_PRESENCE,
        };
        let out = host.on_event(&ev, &ctx);
        assert!(matches!(&out[0], Reaction::Send { lines, .. } if lines == &["JOIN #norn"]));
    }

    #[test]
    fn seen_records_and_answers() {
        let src = find("seen").unwrap().source;
        let mut host = RhaiHost::from_sources(&[("seen.rhai", src)], &[]);
        let ctx = AddonCtx {
            my_nick: "me",
            network: "libera",
            presence: &EMPTY_PRESENCE,
        };
        let msg = |nick: &str, text: &str| AddonEvent {
            net: 0,
            kind: AddonEventKind::Message {
                target: "#norn".into(),
                nick: nick.into(),
                text: text.into(),
                notice: false,
                highlight: false,
                from_self: false,
            },
        };
        // Alice says something -> recorded, no reply.
        assert!(host.on_event(&msg("alice", "hello world"), &ctx).is_empty());
        // Query "!seen alice" -> reply with her last line.
        let out = host.on_event(&msg("bob", "!seen alice"), &ctx);
        assert!(matches!(&out[0], Reaction::Send { lines, .. }
            if lines[0].contains("last seen saying: hello world")));
        // Query for an unknown nick.
        let out = host.on_event(&msg("bob", "!seen nobody"), &ctx);
        assert!(matches!(&out[0], Reaction::Send { lines, .. }
            if lines[0].contains("have not seen")));
    }

    #[test]
    fn autoaway_sets_and_clears_from_presence() {
        let src = find("autoaway").unwrap().source;
        let mut host = RhaiHost::from_sources(&[("autoaway.rhai", src)], &[]);
        // Not away yet: on_idle sets AWAY.
        let not_away = Presence {
            away: false,
            ..Presence::default()
        };
        let ctx = AddonCtx {
            my_nick: "me",
            network: "libera",
            presence: &not_away,
        };
        let idle = AddonEvent {
            net: 0,
            kind: AddonEventKind::Idle { seconds: 300 },
        };
        let out = host.on_event(&idle, &ctx);
        assert!(matches!(&out[0], Reaction::Send { lines, .. }
            if lines[0].starts_with("AWAY :")));
        // Already away: on_idle again does nothing.
        let away = Presence {
            away: true,
            ..Presence::default()
        };
        let ctx = AddonCtx {
            my_nick: "me",
            network: "libera",
            presence: &away,
        };
        assert!(host.on_event(&idle, &ctx).is_empty());
        // Away: on_active clears it.
        let active = AddonEvent {
            net: 0,
            kind: AddonEventKind::Active,
        };
        let out = host.on_event(&active, &ctx);
        assert!(matches!(&out[0], Reaction::Send { lines, .. } if lines == &["AWAY"]));
    }

    // The following three tests drive a real hook that reads a top-level const, so
    // they double as regression guards for the "consts invisible inside call_fn"
    // bug (fixed in a57dbe5): if consts stopped reaching hooks they would fail.

    #[test]
    fn autoop_ops_trusted_nick_in_trusted_channel() {
        let src = find("autoop").unwrap().source;
        let mut host = RhaiHost::from_sources(&[("autoop.rhai", src)], &[]);
        let ctx = AddonCtx {
            my_nick: "me",
            network: "libera",
            presence: &EMPTY_PRESENCE,
        };
        let join = |channel: &str, nick: &str| AddonEvent {
            net: 0,
            kind: AddonEventKind::Join {
                channel: channel.into(),
                nick: nick.into(),
            },
        };
        // Trusted nick (alice) in a trusted channel (#norn) -> op.
        let out = host.on_event(&join("#norn", "alice"), &ctx);
        assert!(matches!(&out[0], Reaction::Send { lines, .. }
            if lines == &["MODE #norn +o alice"]));
        // Untrusted nick -> nothing.
        assert!(host.on_event(&join("#norn", "carol"), &ctx).is_empty());
        // Trusted nick in an untrusted channel -> nothing.
        assert!(host.on_event(&join("#other", "alice"), &ctx).is_empty());
    }

    #[test]
    fn keepnick_reclaims_the_wanted_nick() {
        let src = find("keepnick").unwrap().source;
        let mut host = RhaiHost::from_sources(&[("keepnick.rhai", src)], &[]);
        // nick() is "me", which differs from the shipped WANT ("yournick").
        let ctx = AddonCtx {
            my_nick: "me",
            network: "libera",
            presence: &EMPTY_PRESENCE,
        };
        // The wanted nick quit -> reclaim it.
        let quit = AddonEvent {
            net: 0,
            kind: AddonEventKind::Quit {
                nick: "yournick".into(),
                reason: String::new(),
            },
        };
        let out = host.on_event(&quit, &ctx);
        assert!(matches!(&out[0], Reaction::Send { lines, .. } if lines == &["NICK yournick"]));
        // A vacated (old) nick from a NICK change is also a chance to grab it.
        let renamed = AddonEvent {
            net: 0,
            kind: AddonEventKind::NickChange {
                old: "yournick".into(),
                new: "somebody".into(),
            },
        };
        let out = host.on_event(&renamed, &ctx);
        assert!(matches!(&out[0], Reaction::Send { lines, .. } if lines == &["NICK yournick"]));
        // Someone else quitting does nothing.
        let other = AddonEvent {
            net: 0,
            kind: AddonEventKind::Quit {
                nick: "bob".into(),
                reason: String::new(),
            },
        };
        assert!(host.on_event(&other, &ctx).is_empty());
    }

    #[test]
    fn responder_replies_to_a_trigger_phrase() {
        let src = find("responder").unwrap().source;
        let mut host = RhaiHost::from_sources(&[("responder.rhai", src)], &[]);
        let ctx = AddonCtx {
            my_nick: "me",
            network: "libera",
            presence: &EMPTY_PRESENCE,
        };
        let msg = |from_self: bool, text: &str| AddonEvent {
            net: 0,
            kind: AddonEventKind::Message {
                target: "#norn".into(),
                nick: "bob".into(),
                text: text.into(),
                notice: false,
                highlight: false,
                from_self,
            },
        };
        // A matching phrase (case-insensitive) gets the mapped reply.
        let out = host.on_event(&msg(false, "hey !HELLO everyone"), &ctx);
        assert!(matches!(&out[0], Reaction::Send { lines, .. }
            if lines == &["PRIVMSG #norn :hi there!"]));
        // Non-matching text -> nothing.
        assert!(host.on_event(&msg(false, "just chatting"), &ctx).is_empty());
        // Our own messages are ignored (no feedback loops).
        assert!(host.on_event(&msg(true, "!hello"), &ctx).is_empty());
    }
}
