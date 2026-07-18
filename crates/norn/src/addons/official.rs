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
];

/// Look up a bundled plugin by name.
pub fn find(name: &str) -> Option<&'static OfficialPlugin> {
    OFFICIAL.iter().find(|p| p.name.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::{
        AddonCtx, AddonEvent, AddonEventKind, AddonHost, PluginStatus, Reaction, RhaiHost,
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
        };
        let out = host.on_event(&ev, &ctx);
        assert!(matches!(&out[0], Reaction::Send { lines, .. } if lines == &["JOIN #norn"]));
    }
}
