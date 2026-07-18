//! Addons: react to engine events with commands.
//!
//! Backends implement [`AddonHost`], mapping a normalized [`AddonEvent`] to
//! [`Reaction`]s that the supervisor enacts. Two backends exist: declarative
//! [`Triggers`] (config `[[trigger]]`) and the [`RhaiHost`] scripting backend.
//! Both are driven behind this one boundary, so the supervisor treats them alike.

mod script;
mod triggers;

use std::collections::HashSet;
use std::path::Path;

use irc_engine::{Event, LeaveReason, MessageKind};

use crate::config::TriggerConfig;
use crate::session::NetworkId;
use crate::tui::state::{ctcp_action, mentions, source_nick};

pub use script::RhaiHost;
pub use triggers::{matcher_is_valid, Triggers};

/// A normalized, engine-decoupled view of an event addons can react to.
pub struct AddonEvent {
    /// The network the event came from.
    pub net: NetworkId,
    /// What happened.
    pub kind: AddonEventKind,
}

/// The kinds of event a trigger or script hook can match.
pub enum AddonEventKind {
    /// A channel or private message.
    Message {
        /// The message target (a channel, or our nick for a private message).
        target: String,
        /// The sender.
        nick: String,
        /// The message text (CTCP ACTION unwrapped).
        text: String,
        /// Whether it was a NOTICE.
        notice: bool,
        /// Whether it mentions our nick (and is not from us).
        highlight: bool,
        /// Whether we sent it (guards feedback loops).
        from_self: bool,
    },
    /// Someone joined a channel.
    Join {
        /// The channel.
        channel: String,
        /// Who joined.
        nick: String,
    },
    /// Someone parted a channel.
    Part {
        /// The channel.
        channel: String,
        /// Who left.
        nick: String,
        /// The part reason.
        reason: String,
    },
    /// Someone quit the network.
    Quit {
        /// Who quit.
        nick: String,
        /// The quit reason.
        reason: String,
    },
    /// Someone changed nick.
    NickChange {
        /// Previous nick.
        old: String,
        /// New nick.
        new: String,
    },
}

impl AddonEvent {
    /// Normalize an engine [`Event`] into an [`AddonEvent`], or `None` for events
    /// addons do not react to. `my_nick` is used to fill `highlight`/`from_self`.
    pub fn from_engine(event: &Event, net: NetworkId, my_nick: &str) -> Option<AddonEvent> {
        let kind = match event {
            Event::MessageReceived(msg) => {
                let nick = msg
                    .sender
                    .as_ref()
                    .map(source_nick)
                    .unwrap_or("")
                    .to_string();
                // Unwrap a CTCP ACTION to its inner text (the `/me` flag itself is
                // not yet matched on).
                let text = ctcp_action(&msg.text)
                    .map(str::to_string)
                    .unwrap_or_else(|| msg.text.clone());
                let from_self = nick.eq_ignore_ascii_case(my_nick);
                AddonEventKind::Message {
                    target: msg.target.clone(),
                    highlight: !from_self && mentions(&text, my_nick),
                    nick,
                    text,
                    notice: msg.kind == MessageKind::Notice,
                    from_self,
                }
            }
            Event::MemberJoined { target, who, .. } => AddonEventKind::Join {
                channel: target.clone(),
                nick: who.nick.clone(),
            },
            Event::MemberLeft {
                target,
                who,
                reason,
            } => match reason {
                LeaveReason::Part(r) => AddonEventKind::Part {
                    channel: target.clone(),
                    nick: who.nick.clone(),
                    reason: r.clone(),
                },
                LeaveReason::Quit(r) => AddonEventKind::Quit {
                    nick: who.nick.clone(),
                    reason: r.clone(),
                },
                // A kick is not a trigger kind for now.
                LeaveReason::Kicked { .. } => return None,
            },
            Event::NickChanged { old, new } => AddonEventKind::NickChange {
                old: old.clone(),
                new: new.clone(),
            },
            _ => return None,
        };
        Some(AddonEvent { net, kind })
    }
}

/// The reply target for an event: the sender for a private message (target is our
/// own nick), the channel for a channel message/join/part, else `None`.
pub(crate) fn event_reply_target(kind: &AddonEventKind, my_nick: &str) -> Option<String> {
    match kind {
        AddonEventKind::Message { target, nick, .. } => {
            Some(if target.eq_ignore_ascii_case(my_nick) {
                nick.clone()
            } else {
                target.clone()
            })
        }
        AddonEventKind::Join { channel, .. } | AddonEventKind::Part { channel, .. } => {
            Some(channel.clone())
        }
        _ => None,
    }
}

/// Read-only context passed to a host with each event.
pub struct AddonCtx<'a> {
    /// Our nick on the event's network.
    pub my_nick: &'a str,
    /// The event's network name.
    pub network: &'a str,
}

/// What a host asks the supervisor to enact.
pub enum Reaction {
    /// Send raw IRC lines to a network.
    Send {
        /// The network to send on.
        net: NetworkId,
        /// The lines to send.
        lines: Vec<String>,
    },
    /// Show a local notification (a console line plus the terminal bell).
    Notify {
        /// The originating network.
        net: NetworkId,
        /// The text to show.
        text: String,
    },
}

/// A source of reactions to engine events, behind one boundary.
pub trait AddonHost {
    /// React to one normalized event.
    fn on_event(&mut self, event: &AddonEvent, ctx: &AddonCtx) -> Vec<Reaction>;
}

/// Several hosts driven as one: reactions are concatenated in order.
struct CompositeHost(Vec<Box<dyn AddonHost>>);

impl AddonHost for CompositeHost {
    fn on_event(&mut self, event: &AddonEvent, ctx: &AddonCtx) -> Vec<Reaction> {
        let mut out = Vec::new();
        for host in &mut self.0 {
            out.extend(host.on_event(event, ctx));
        }
        out
    }
}

/// The load status of an addon script (a "plugin").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginStatus {
    /// Enabled, compiled, and active.
    Loaded,
    /// Installed but turned off (not loaded).
    Disabled,
    /// Failed to compile, with the error.
    Failed(String),
}

/// A discovered addon script and its metadata, for `/plugins`.
#[derive(Debug, Clone)]
pub struct PluginInfo {
    /// Display name (`NAME` const, else the filename stem).
    pub name: String,
    /// The filename (the key in the disabled list).
    pub file: String,
    /// One-line description (`DESCRIPTION` const), or empty.
    pub description: String,
    /// Version string (`VERSION` const), or empty.
    pub version: String,
    /// Load status.
    pub status: PluginStatus,
}

/// The result of assembling the addon host: the host plus the discovered plugin
/// list (with status/metadata) for `/plugins` and startup error reporting.
pub struct AddonReport {
    /// The composite host to drive.
    pub host: Box<dyn AddonHost>,
    /// Every discovered addon script.
    pub plugins: Vec<PluginInfo>,
}

/// Assemble the addon host: declarative triggers plus the Rhai plugin scripts
/// under `plugins_dir` (skipping `disabled` filenames). Used at startup/reload.
pub fn build_addon_host(
    triggers: &[TriggerConfig],
    plugins_dir: Option<&Path>,
    disabled: &HashSet<String>,
) -> AddonReport {
    let mut hosts: Vec<Box<dyn AddonHost>> = vec![Box::new(Triggers::from_configs(triggers))];
    let plugins = match plugins_dir {
        Some(dir) => {
            let host = RhaiHost::load(dir, disabled);
            let plugins = host.plugins().to_vec();
            hosts.push(Box::new(host));
            plugins
        }
        None => Vec::new(),
    };
    AddonReport {
        host: Box::new(CompositeHost(hosts)),
        plugins,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct One(Reaction);
    impl AddonHost for One {
        fn on_event(&mut self, event: &AddonEvent, _ctx: &AddonCtx) -> Vec<Reaction> {
            match &self.0 {
                Reaction::Notify { text, .. } => vec![Reaction::Notify {
                    net: event.net,
                    text: text.clone(),
                }],
                Reaction::Send { lines, .. } => vec![Reaction::Send {
                    net: event.net,
                    lines: lines.clone(),
                }],
            }
        }
    }

    #[test]
    fn composite_concatenates_host_reactions() {
        let mut host = CompositeHost(vec![
            Box::new(One(Reaction::Notify {
                net: 0,
                text: "a".into(),
            })),
            Box::new(One(Reaction::Send {
                net: 0,
                lines: vec!["b".into()],
            })),
        ]);
        let ev = AddonEvent {
            net: 0,
            kind: AddonEventKind::Quit {
                nick: "x".into(),
                reason: String::new(),
            },
        };
        let ctx = AddonCtx {
            my_nick: "me",
            network: "n",
        };
        let out = host.on_event(&ev, &ctx);
        assert_eq!(out.len(), 2);
        assert!(matches!(&out[0], Reaction::Notify { text, .. } if text == "a"));
        assert!(matches!(&out[1], Reaction::Send { lines, .. } if lines == &["b"]));
    }
}
