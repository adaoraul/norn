//! Addons: react to engine events with commands.
//!
//! Phase 1 is declarative triggers loaded from config (`[[trigger]]`). Everything
//! is expressed through the [`AddonHost`] trait so a scripting backend (Rhai) can
//! later plug in behind the same boundary. Backends are pure: they map an
//! [`AddonEvent`] to [`Reaction`]s and the supervisor enacts them, which keeps
//! them unit-testable with no I/O.

use irc_engine::{Event, LeaveReason, MessageKind};

use crate::config::TriggerConfig;
use crate::session::NetworkId;
use crate::tui::state::{ctcp_action, mentions, source_nick};

/// A normalized, engine-decoupled view of an event addons can react to.
pub struct AddonEvent {
    /// The network the event came from.
    pub net: NetworkId,
    /// What happened.
    pub kind: AddonEventKind,
}

/// The kinds of event a trigger (or later, a script hook) can match.
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

/// A source of reactions to engine events. Implemented by [`Triggers`] now and by
/// a scripting backend later, behind this one boundary.
pub trait AddonHost {
    /// React to one normalized event.
    fn on_event(&mut self, event: &AddonEvent, ctx: &AddonCtx) -> Vec<Reaction>;
}

/// Which kind of event a trigger matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchKind {
    Message,
    Highlight,
    Notice,
    Join,
    Part,
    Quit,
    Nick,
}

/// A parsed trigger match spec: an event kind and an optional channel filter.
#[derive(Debug, Clone)]
struct Matcher {
    kind: MatchKind,
    channel: Option<String>,
}

impl Matcher {
    /// Parse an `on` string like `"highlight"` or `"join #norn"`.
    fn parse(on: &str) -> Option<Matcher> {
        let mut it = on.split_whitespace();
        let kind = match it.next()?.to_ascii_lowercase().as_str() {
            "message" | "msg" => MatchKind::Message,
            "highlight" | "hl" => MatchKind::Highlight,
            "notice" => MatchKind::Notice,
            "join" => MatchKind::Join,
            "part" => MatchKind::Part,
            "quit" => MatchKind::Quit,
            "nick" => MatchKind::Nick,
            _ => return None,
        };
        Some(Matcher {
            kind,
            channel: it.next().map(str::to_string),
        })
    }

    /// Whether the optional channel filter admits `target`.
    fn channel_ok(&self, target: &str) -> bool {
        self.channel
            .as_ref()
            .is_none_or(|c| c.eq_ignore_ascii_case(target))
    }

    /// If this matcher fits the event, the template variables to substitute
    /// (`nick`, `chan`/`channel`, `msg`/`text`). `chan` is the reply target: the
    /// sender for a private message, else the channel.
    fn match_vars(
        &self,
        kind: &AddonEventKind,
        my_nick: &str,
    ) -> Option<Vec<(&'static str, String)>> {
        use AddonEventKind as K;
        let (nick, chan, msg) = match (self.kind, kind) {
            (
                MatchKind::Message,
                K::Message {
                    target,
                    nick,
                    text,
                    from_self,
                    ..
                },
            ) if !from_self => {
                if !self.channel_ok(target) {
                    return None;
                }
                (
                    nick.clone(),
                    reply_target(target, nick, my_nick),
                    text.clone(),
                )
            }
            (
                MatchKind::Highlight,
                K::Message {
                    target,
                    nick,
                    text,
                    highlight,
                    ..
                },
            ) if *highlight => {
                if !self.channel_ok(target) {
                    return None;
                }
                (
                    nick.clone(),
                    reply_target(target, nick, my_nick),
                    text.clone(),
                )
            }
            (
                MatchKind::Notice,
                K::Message {
                    target,
                    nick,
                    text,
                    notice,
                    from_self,
                    ..
                },
            ) if *notice && !from_self => {
                if !self.channel_ok(target) {
                    return None;
                }
                (
                    nick.clone(),
                    reply_target(target, nick, my_nick),
                    text.clone(),
                )
            }
            (MatchKind::Join, K::Join { channel, nick }) => {
                if !self.channel_ok(channel) {
                    return None;
                }
                (nick.clone(), channel.clone(), String::new())
            }
            (
                MatchKind::Part,
                K::Part {
                    channel,
                    nick,
                    reason,
                },
            ) => {
                if !self.channel_ok(channel) {
                    return None;
                }
                (nick.clone(), channel.clone(), reason.clone())
            }
            (MatchKind::Quit, K::Quit { nick, reason }) => {
                (nick.clone(), String::new(), reason.clone())
            }
            (MatchKind::Nick, K::NickChange { old, new }) => {
                (old.clone(), String::new(), new.clone())
            }
            _ => return None,
        };
        Some(vec![
            ("nick", nick),
            ("chan", chan.clone()),
            ("channel", chan),
            ("msg", msg.clone()),
            ("text", msg),
        ])
    }
}

/// Whether an `on` match spec parses (for validating `/trigger add`).
pub fn matcher_is_valid(on: &str) -> bool {
    Matcher::parse(on).is_some()
}

/// The reply target for a message: the sender for a private message (target is
/// our own nick), otherwise the channel.
fn reply_target(target: &str, sender: &str, my_nick: &str) -> String {
    if target.eq_ignore_ascii_case(my_nick) {
        sender.to_string()
    } else {
        target.to_string()
    }
}

/// A compiled trigger: a matcher plus a command template.
#[derive(Debug, Clone)]
struct Rule {
    matcher: Matcher,
    template: String,
}

/// The declarative-trigger addon host.
#[derive(Debug, Clone, Default)]
pub struct Triggers {
    rules: Vec<Rule>,
}

impl Triggers {
    /// Build from config, dropping disabled entries and unparseable matchers.
    pub fn from_configs(configs: &[TriggerConfig]) -> Triggers {
        let rules = configs
            .iter()
            .filter(|c| c.enabled)
            .filter_map(|c| {
                Matcher::parse(&c.on).map(|matcher| Rule {
                    matcher,
                    template: c.run.clone(),
                })
            })
            .collect();
        Triggers { rules }
    }
}

impl AddonHost for Triggers {
    fn on_event(&mut self, event: &AddonEvent, ctx: &AddonCtx) -> Vec<Reaction> {
        let mut out = Vec::new();
        for rule in &self.rules {
            let Some(mut vars) = rule.matcher.match_vars(&event.kind, ctx.my_nick) else {
                continue;
            };
            vars.push(("me", ctx.my_nick.to_string()));
            vars.push(("net", ctx.network.to_string()));
            // The reply target is the `chan` var (already the sender for a PM).
            let target = vars
                .iter()
                .find(|(k, _)| *k == "chan")
                .map(|(_, v)| v.clone())
                .filter(|v| !v.is_empty());
            for segment in expand_template(&rule.template, &vars) {
                enact_segment(&segment, event.net, target.as_deref(), &mut out);
            }
        }
        out
    }
}

/// Turn one expanded template segment into a reaction: a `notify` action becomes
/// a local notification; anything else runs through `translate` as a command.
fn enact_segment(segment: &str, net: NetworkId, target: Option<&str>, out: &mut Vec<Reaction>) {
    let body = segment.strip_prefix('/').unwrap_or(segment);
    if let Some(text) = body
        .strip_prefix("notify ")
        .or_else(|| (body == "notify").then_some(""))
    {
        out.push(Reaction::Notify {
            net,
            text: text.trim().to_string(),
        });
        return;
    }
    // Commands go through `translate`; ensure the leading slash it expects.
    let line = if segment.starts_with('/') {
        segment.to_string()
    } else {
        format!("/{segment}")
    };
    let mut current = target.map(str::to_string);
    let translated = crate::input::translate(&line, &mut current);
    if !translated.lines.is_empty() {
        out.push(Reaction::Send {
            net,
            lines: translated.lines,
        });
    }
}

/// Expand a template into command segments: split on `;`, substitute `$name`
/// variables (and `$$` -> `$`) in each.
fn expand_template(template: &str, vars: &[(&'static str, String)]) -> Vec<String> {
    template
        .split(';')
        .filter_map(|seg| {
            let seg = seg.trim();
            (!seg.is_empty()).then(|| substitute(seg, vars))
        })
        .collect()
}

/// Substitute `$name` placeholders from `vars` (longest ASCII-word name),
/// treating `$$` as a literal `$` and dropping unknown names.
fn substitute(seg: &str, vars: &[(&'static str, String)]) -> String {
    let bytes = seg.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < seg.len() {
        if bytes[i] == b'$' {
            if i + 1 < seg.len() && bytes[i + 1] == b'$' {
                out.push('$');
                i += 2;
                continue;
            }
            let start = i + 1;
            let mut j = start;
            while j < seg.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if j > start {
                let name = &seg[start..j];
                if let Some((_, value)) = vars.iter().find(|(k, _)| *k == name) {
                    out.push_str(value);
                }
                i = j;
                continue;
            }
        }
        let ch = seg[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(on: &str, run: &str) -> TriggerConfig {
        TriggerConfig {
            on: on.into(),
            run: run.into(),
            enabled: true,
        }
    }

    fn msg_event(target: &str, nick: &str, text: &str, my_nick: &str) -> AddonEvent {
        let from_self = nick.eq_ignore_ascii_case(my_nick);
        AddonEvent {
            net: 0,
            kind: AddonEventKind::Message {
                target: target.into(),
                nick: nick.into(),
                text: text.into(),
                notice: false,
                highlight: !from_self && mentions(text, my_nick),
                from_self,
            },
        }
    }

    fn ctx<'a>(my_nick: &'a str) -> AddonCtx<'a> {
        AddonCtx {
            my_nick,
            network: "libera",
        }
    }

    #[test]
    fn substitute_named_vars_and_literal_dollar() {
        let vars = vec![
            ("nick", "alice".to_string()),
            ("msg", "hi there".to_string()),
        ];
        assert_eq!(
            substitute("$nick said: $msg", &vars),
            "alice said: hi there"
        );
        assert_eq!(substitute("cost $$5", &vars), "cost $5");
        assert_eq!(substitute("unknown $nope here", &vars), "unknown  here");
    }

    #[test]
    fn highlight_trigger_runs_a_reply() {
        let mut t = Triggers::from_configs(&[cfg("highlight", "msg $chan you rang $nick?")]);
        let ev = msg_event("#rust", "alice", "hey svan look", "svan");
        let reactions = t.on_event(&ev, &ctx("svan"));
        assert_eq!(reactions.len(), 1);
        let Reaction::Send { net, lines } = &reactions[0] else {
            panic!("expected Send, got a notify");
        };
        assert_eq!(*net, 0);
        assert_eq!(lines, &["PRIVMSG #rust :you rang alice?"]);
    }

    #[test]
    fn notify_trigger_produces_a_notification() {
        let mut t = Triggers::from_configs(&[cfg("highlight", "notify $nick: $msg")]);
        let ev = msg_event("#rust", "alice", "ping svan", "svan");
        let reactions = t.on_event(&ev, &ctx("svan"));
        assert!(
            matches!(&reactions[0], Reaction::Notify { text, .. } if text == "alice: ping svan")
        );
    }

    #[test]
    fn self_messages_do_not_trigger() {
        let mut t = Triggers::from_configs(&[cfg("message", "notify saw $nick")]);
        let ev = msg_event("#rust", "svan", "talking to myself", "svan");
        assert!(t.on_event(&ev, &ctx("svan")).is_empty());
    }

    #[test]
    fn join_trigger_greets_only_the_named_channel() {
        let mut t = Triggers::from_configs(&[cfg("join #norn", "msg $chan welcome $nick!")]);
        let hit = AddonEvent {
            net: 0,
            kind: AddonEventKind::Join {
                channel: "#norn".into(),
                nick: "bob".into(),
            },
        };
        let reactions = t.on_event(&hit, &ctx("svan"));
        assert!(matches!(&reactions[0], Reaction::Send { lines, .. }
            if lines == &["PRIVMSG #norn :welcome bob!"]));
        // A different channel does not match.
        let miss = AddonEvent {
            net: 0,
            kind: AddonEventKind::Join {
                channel: "#other".into(),
                nick: "bob".into(),
            },
        };
        assert!(t.on_event(&miss, &ctx("svan")).is_empty());
    }

    #[test]
    fn private_message_replies_to_the_sender() {
        // A PM: target is our nick, so $chan is the sender (the reply target).
        let mut t = Triggers::from_configs(&[cfg("message", "msg $chan hi $nick")]);
        let ev = msg_event("svan", "carol", "hello", "svan");
        let reactions = t.on_event(&ev, &ctx("svan"));
        assert!(matches!(&reactions[0], Reaction::Send { lines, .. }
            if lines == &["PRIVMSG carol :hi carol"]));
    }

    #[test]
    fn chained_template_runs_each_command() {
        let mut t = Triggers::from_configs(&[cfg("highlight", "msg $chan a;msg $chan b")]);
        let ev = msg_event("#rust", "alice", "svan?", "svan");
        let reactions = t.on_event(&ev, &ctx("svan"));
        assert_eq!(reactions.len(), 2);
    }
}
