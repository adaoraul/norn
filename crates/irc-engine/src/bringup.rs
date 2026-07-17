//! Connection bring-up: cap negotiation and the SASL sub-machine.
//!
//! This is sans-I/O and synchronous. It consumes parsed [`Message`]s and
//! produces an ordered list of [`Action`]s (lines to send, events to surface),
//! so it can be driven by scripted transcripts with no socket. The async
//! connection loop in `connection.rs` is the only thing that touches I/O.
//!
//! The machine enforces the hard rules that are easy to get wrong:
//!
//! - `CAP LS 302` always, then `NICK`/`USER` (rule 3).
//! - `CAP LS` may be multiline; buffer until a line without `*` (rule 4).
//! - `CAP REQ` is atomic: a `NAK` enables nothing in that group (rule 5).
//! - `001` is only accepted after `CAP END` is sent (rule 6), and when SASL was
//!   requested, `CAP END` waits for SASL to resolve (rule 7).

use irc_proto::{
    sasl::{decode_b64, response_lines},
    CapName, CapSet, Command, LsAccumulator, Mechanism, Message, SaslError,
};

use crate::event::{DisconnectReason, Event};

/// How many alternate nicknames to try after `433`/`436` before giving up.
const MAX_NICK_ATTEMPTS: usize = 6;

/// What to do if SASL fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaslFailPolicy {
    /// Tear down the connection.
    Abort,
    /// Proceed unauthenticated (send `CAP END` and register anyway).
    Continue,
}

/// Static configuration for a bring-up.
pub struct BringupConfig {
    /// Desired nickname.
    pub nick: String,
    /// Username/ident for the `USER` line.
    pub user: String,
    /// Realname (the `USER` trailing param).
    pub realname: String,
    /// Capability groups to request. Each group is `REQ`'d on its own line and
    /// is therefore ACK/NAK'd atomically (rule 5). Keep groups small.
    pub cap_groups: Vec<Vec<String>>,
    /// SASL mechanisms in preference order (each owns its credentials). When
    /// non-empty and the server offers `sasl`, one is selected (preferring the
    /// server's advertised list) and driven to completion before `CAP END`. On
    /// a `908` supported-mechanisms reply, the next listed candidate is tried.
    pub sasl: Vec<Box<dyn Mechanism>>,
    /// Whether the underlying transport is TLS. PLAIN is refused without it
    /// (rule 9).
    pub tls: bool,
    /// What to do when SASL fails.
    pub sasl_fail_policy: SaslFailPolicy,
}

/// A side effect produced by the machine, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// A raw line to write to the server (no trailing CRLF).
    Send(String),
    /// A semantic event to surface to the UI.
    Emit(Event),
}

/// Bring-up phases. Illegal transitions are simply not produced by the code
/// paths below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Nothing sent yet.
    Idle,
    /// `CAP LS 302` sent; collecting the (possibly multiline) listing.
    LsRequested,
    /// `CAP REQ`(s) sent; awaiting ACK/NAK.
    Negotiating,
    /// SASL exchange underway.
    SaslInProgress,
    /// `CAP END` sent; awaiting `001`.
    AwaitingWelcome,
    /// Registered; cap-notify and the runtime layers are live.
    Registered,
    /// Connection aborted.
    Closed,
}

/// The bring-up state machine.
pub struct BringupMachine {
    config: BringupConfig,
    state: State,
    ls: LsAccumulator,
    new_ls: LsAccumulator,
    available: CapSet,
    enabled: CapSet,
    outstanding_reqs: usize,
    sasl_pending: bool,
    /// Index into `config.sasl` of the mechanism currently being attempted.
    current_mech: Option<usize>,
    /// How many alternate nicks we have tried after `433`/`436`.
    nick_attempts: usize,
    account: Option<String>,
    cap_end_sent: bool,
}

impl BringupMachine {
    /// Build a machine for the given configuration. Call [`start`] to emit the
    /// opening lines.
    ///
    /// [`start`]: BringupMachine::start
    pub fn new(config: BringupConfig) -> Self {
        BringupMachine {
            config,
            state: State::Idle,
            ls: LsAccumulator::new(),
            new_ls: LsAccumulator::new(),
            available: CapSet::new(),
            enabled: CapSet::new(),
            outstanding_reqs: 0,
            sasl_pending: false,
            current_mech: None,
            nick_attempts: 0,
            account: None,
            cap_end_sent: false,
        }
    }

    /// The current phase.
    pub fn state(&self) -> State {
        self.state
    }

    /// Whether registration has completed.
    pub fn is_registered(&self) -> bool {
        self.state == State::Registered
    }

    /// Whether bring-up was aborted (SASL abort policy, or nick exhaustion).
    pub fn is_closed(&self) -> bool {
        self.state == State::Closed
    }

    /// Whether `CAP END` has been sent (used to assert rule 6 in tests).
    pub fn cap_end_sent(&self) -> bool {
        self.cap_end_sent
    }

    /// Emit the opening handshake: `CAP LS 302`, then `NICK`/`USER` (rule 3).
    pub fn start(&mut self) -> Vec<Action> {
        let actions = vec![
            Action::Send("CAP LS 302".to_string()),
            Action::Send(format!("NICK {}", self.config.nick)),
            Action::Send(format!(
                "USER {} 0 * :{}",
                self.config.user, self.config.realname
            )),
        ];
        self.state = State::LsRequested;
        actions
    }

    /// Feed one parsed server message and get the resulting actions.
    pub fn handle(&mut self, msg: &Message) -> Vec<Action> {
        let mut actions = Vec::new();
        match &msg.command {
            Command::Named(name) => match name.as_str() {
                "CAP" => self.handle_cap(msg, &mut actions),
                "AUTHENTICATE" => self.handle_authenticate(msg, &mut actions),
                "PING" => {
                    let token = msg.params.first().cloned().unwrap_or_default();
                    actions.push(Action::Send(format!("PONG :{token}")));
                }
                _ => {}
            },
            Command::Numeric(n) => self.handle_numeric(*n, msg, &mut actions),
        }
        actions
    }

    fn handle_cap(&mut self, msg: &Message, actions: &mut Vec<Action>) {
        let Some(sub) = msg.params.get(1).map(String::as_str) else {
            return;
        };
        match sub {
            "LS" => {
                let (more, list) = parse_cap_list(&msg.params);
                if let Some(set) = self.ls.feed(list, more) {
                    self.available = set;
                    self.request_caps(actions);
                    self.state = State::Negotiating;
                    self.maybe_finish(actions);
                }
            }
            "ACK" => {
                let list = msg.params.last().map(String::as_str).unwrap_or("");
                let acked: Vec<String> = list.split_whitespace().map(String::from).collect();
                for cap in &acked {
                    self.enabled.insert_token(cap);
                }
                if self.state == State::Registered {
                    // cap-notify: a live enable.
                    self.emit_caps_changed(actions);
                } else {
                    self.outstanding_reqs = self.outstanding_reqs.saturating_sub(1);
                    let sasl_acked = acked
                        .iter()
                        .any(|c| CapName::new(c.as_str()) == CapName::new("sasl"));
                    if sasl_acked && !self.config.sasl.is_empty() {
                        self.start_sasl(actions);
                    } else {
                        self.maybe_finish(actions);
                    }
                }
            }
            "NAK" => {
                // Atomic: enable nothing in the group (rule 5).
                if self.state != State::Registered {
                    self.outstanding_reqs = self.outstanding_reqs.saturating_sub(1);
                    self.maybe_finish(actions);
                }
            }
            "NEW" => {
                let (more, list) = parse_cap_list(&msg.params);
                if let Some(offered) = self.new_ls.feed(list, more) {
                    // Request any newly offered cap we want and lack.
                    let mut to_req: Vec<String> = Vec::new();
                    for group in &self.config.cap_groups {
                        for want in group {
                            if offered.contains(want) && !self.enabled.contains(want) {
                                if let Some(adv) = offered.advertised(want) {
                                    to_req.push(adv.to_string());
                                }
                            }
                        }
                    }
                    self.available.extend(offered);
                    for name in to_req {
                        actions.push(Action::Send(format!("CAP REQ :{name}")));
                    }
                }
            }
            "DEL" => {
                let list = msg.params.last().map(String::as_str).unwrap_or("");
                for cap in list.split_whitespace() {
                    self.available.remove(cap);
                    self.enabled.remove(cap);
                }
                if self.state == State::Registered {
                    self.emit_caps_changed(actions);
                }
            }
            _ => {}
        }
    }

    fn handle_authenticate(&mut self, msg: &Message, actions: &mut Vec<Action>) {
        if self.state != State::SaslInProgress {
            return;
        }
        let payload = msg.params.first().map(String::as_str).unwrap_or("+");
        let challenge = if payload == "+" {
            Vec::new()
        } else {
            match decode_b64(payload) {
                Ok(bytes) => bytes,
                Err(_) => {
                    self.sasl_fail(SaslError::InvalidBase64, actions);
                    return;
                }
            }
        };
        let Some(idx) = self.current_mech else {
            return;
        };
        let result = match self.config.sasl.get_mut(idx) {
            Some(mech) => mech.respond(&challenge),
            None => return,
        };
        match result {
            Ok(resp) => {
                for line in response_lines(&resp) {
                    actions.push(Action::Send(line));
                }
            }
            Err(err) => self.sasl_fail(err, actions),
        }
    }

    fn handle_numeric(&mut self, n: u16, msg: &Message, actions: &mut Vec<Action>) {
        match n {
            // 001 RPL_WELCOME: accepted ONLY after CAP END (rule 6).
            1 => {
                if self.state == State::AwaitingWelcome {
                    let nick = msg
                        .params
                        .first()
                        .cloned()
                        .unwrap_or_else(|| self.config.nick.clone());
                    actions.push(Action::Emit(Event::Registered { nick }));
                    self.emit_caps_changed(actions);
                    self.state = State::Registered;
                }
            }
            // 900 RPL_LOGGEDIN carries the account name.
            900 => {
                self.account = msg.params.get(2).cloned();
            }
            // 903 RPL_SASLSUCCESS.
            903 => {
                self.sasl_pending = false;
                let account = self.account.clone().unwrap_or_default();
                actions.push(Action::Emit(Event::AuthResult(Ok(account))));
                self.maybe_finish(actions);
            }
            // 904/905/906: SASL failure.
            904..=906 => {
                let err = match n {
                    905 => SaslError::TooLong,
                    906 => SaslError::Aborted,
                    _ => SaslError::Failed,
                };
                self.sasl_fail(err, actions);
            }
            // 907: already authenticated; treat as resolved.
            907 => {
                self.sasl_pending = false;
                self.maybe_finish(actions);
            }
            // 433/436: the nick is taken; try an alternate before giving up.
            433 | 436 => self.try_alternate_nick(actions),
            // 908 RPL_SASLMECHS: the server lists what it supports; retry with
            // the next configured candidate rather than failing hard.
            908 => {
                let allowed: Vec<String> = msg
                    .params
                    .get(1)
                    .map(|list| list.split(',').map(String::from).collect())
                    .unwrap_or_default();
                match self.select_mechanism(&allowed, self.current_mech) {
                    Some(idx) => {
                        self.current_mech = Some(idx);
                        let name = self.config.sasl[idx].name().to_string();
                        actions.push(Action::Send(format!("AUTHENTICATE {name}")));
                    }
                    None => self.sasl_fail(SaslError::Failed, actions),
                }
            }
            _ => {}
        }
    }

    /// Pick a configured mechanism to try: the first that is usable (PLAIN
    /// requires TLS, rule 9), not excluded, and permitted by `allowed` (an
    /// empty `allowed` means no restriction).
    fn select_mechanism(&self, allowed: &[String], exclude: Option<usize>) -> Option<usize> {
        for (i, mech) in self.config.sasl.iter().enumerate() {
            if Some(i) == exclude {
                continue;
            }
            let name = mech.name();
            if name == "PLAIN" && !self.config.tls {
                continue;
            }
            if allowed.is_empty() || allowed.iter().any(|m| m == name) {
                return Some(i);
            }
        }
        None
    }

    /// Build and send `CAP REQ` lines for the wanted-and-available caps, one
    /// line per group (rule 5), plus `sasl` if configured and offered.
    fn request_caps(&mut self, actions: &mut Vec<Action>) {
        let groups = self.config.cap_groups.clone();
        for group in &groups {
            let names: Vec<String> = group
                .iter()
                .filter_map(|c| self.available.advertised(c).map(String::from))
                .collect();
            if names.is_empty() {
                continue;
            }
            actions.push(Action::Send(format!("CAP REQ :{}", names.join(" "))));
            self.outstanding_reqs += 1;
        }

        // Ensure sasl is requested if configured and offered but not already in
        // a configured group.
        if !self.config.sasl.is_empty() && self.available.contains("sasl") {
            let already = groups
                .iter()
                .flatten()
                .any(|c| CapName::new(c.as_str()) == CapName::new("sasl"));
            if !already {
                let adv = self
                    .available
                    .advertised("sasl")
                    .unwrap_or("sasl")
                    .to_string();
                actions.push(Action::Send(format!("CAP REQ :{adv}")));
                self.outstanding_reqs += 1;
            }
        }
    }

    /// React to a nick-in-use reply by proposing a new nick (the base nick with
    /// trailing underscores), or aborting once the attempts are exhausted. Only
    /// meaningful before registration completes.
    fn try_alternate_nick(&mut self, actions: &mut Vec<Action>) {
        if self.state == State::Registered || self.state == State::Closed {
            return;
        }
        self.nick_attempts += 1;
        if self.nick_attempts > MAX_NICK_ATTEMPTS {
            self.state = State::Closed;
            actions.push(Action::Emit(Event::Disconnected(
                DisconnectReason::NickUnavailable,
            )));
            return;
        }
        let candidate = format!("{}{}", self.config.nick, "_".repeat(self.nick_attempts));
        actions.push(Action::Send(format!("NICK {candidate}")));
    }

    fn start_sasl(&mut self, actions: &mut Vec<Action>) {
        // Prefer a mechanism the server advertised in the sasl cap value; fall
        // back to any usable configured one.
        let advertised: Vec<String> = self
            .available
            .values("sasl")
            .iter()
            .map(|m| (*m).to_string())
            .collect();
        match self.select_mechanism(&advertised, None) {
            Some(idx) => {
                self.current_mech = Some(idx);
                self.sasl_pending = true;
                self.state = State::SaslInProgress;
                let name = self.config.sasl[idx].name().to_string();
                actions.push(Action::Send(format!("AUTHENTICATE {name}")));
            }
            None => {
                // Nothing usable (e.g. only PLAIN offered without TLS, rule 9).
                self.sasl_pending = false;
                actions.push(Action::Emit(Event::AuthResult(Err(SaslError::Failed))));
                self.maybe_finish(actions);
            }
        }
    }

    fn sasl_fail(&mut self, err: SaslError, actions: &mut Vec<Action>) {
        self.sasl_pending = false;
        actions.push(Action::Emit(Event::AuthResult(Err(err))));
        match self.config.sasl_fail_policy {
            SaslFailPolicy::Continue => self.maybe_finish(actions),
            SaslFailPolicy::Abort => {
                self.state = State::Closed;
                actions.push(Action::Emit(Event::Disconnected(
                    DisconnectReason::SaslAbortedByPolicy,
                )));
            }
        }
    }

    /// Send `CAP END` and advance to awaiting `001`, but only once every REQ is
    /// resolved and SASL (if any) has finished (rules 6 and 7). This is the one
    /// path to `AwaitingWelcome`, so `001` can never be honored without it.
    fn maybe_finish(&mut self, actions: &mut Vec<Action>) {
        let negotiating = matches!(self.state, State::Negotiating | State::SaslInProgress);
        if negotiating && self.outstanding_reqs == 0 && !self.sasl_pending {
            actions.push(Action::Send("CAP END".to_string()));
            self.cap_end_sent = true;
            self.state = State::AwaitingWelcome;
        }
    }

    fn emit_caps_changed(&self, actions: &mut Vec<Action>) {
        actions.push(Action::Emit(Event::CapabilitiesChanged {
            available: self.available.clone(),
            enabled: self.enabled.clone(),
        }));
    }
}

/// The phase-1 and phase-2 capabilities a modern client wants, grouped into
/// small atomic `CAP REQ` sets (rule 5). `sasl` is added automatically by the
/// machine when a mechanism is configured, so it is not listed here.
pub fn recommended_caps() -> Vec<Vec<String>> {
    [
        &["server-time", "message-tags", "batch", "labeled-response"][..],
        &["account-tag", "account-notify", "extended-join"][..],
        &["chghost", "away-notify", "setname", "multi-prefix"][..],
        &["msgid", "echo-message"][..],
        &["chathistory"][..],
    ]
    .iter()
    .map(|group| group.iter().map(|c| (*c).to_string()).collect())
    .collect()
}

/// Extract `(more, cap_list)` from a `CAP ... LS/NEW ...` param list. A `*`
/// param before the trailing cap list marks a non-final line (rule 4).
fn parse_cap_list(params: &[String]) -> (bool, &str) {
    if params.len() >= 4 && params[2] == "*" {
        (true, params[3].as_str())
    } else {
        (false, params.get(2).map(String::as_str).unwrap_or(""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irc_proto::sasl::Plain;

    fn record(actions: Vec<Action>, sent: &mut Vec<String>, events: &mut Vec<Event>) {
        for action in actions {
            match action {
                Action::Send(line) => sent.push(line),
                Action::Emit(event) => events.push(event),
            }
        }
    }

    struct Driver {
        machine: BringupMachine,
        sent: Vec<String>,
        events: Vec<Event>,
    }

    impl Driver {
        fn new(config: BringupConfig) -> Self {
            let mut machine = BringupMachine::new(config);
            let mut sent = Vec::new();
            let mut events = Vec::new();
            record(machine.start(), &mut sent, &mut events);
            Driver {
                machine,
                sent,
                events,
            }
        }

        fn feed(&mut self, line: &str) {
            let msg = Message::parse(line).unwrap();
            record(self.machine.handle(&msg), &mut self.sent, &mut self.events);
        }

        fn sent_contains(&self, line: &str) -> bool {
            self.sent.iter().any(|l| l == line)
        }
    }

    fn config(sasl: bool, tls: bool, policy: SaslFailPolicy) -> BringupConfig {
        BringupConfig {
            nick: "adao".into(),
            user: "adao".into(),
            realname: "Adao".into(),
            cap_groups: vec![vec!["multi-prefix".into(), "server-time".into()]],
            sasl: if sasl {
                vec![Box::new(Plain::new("adao", "hunter2")) as Box<dyn Mechanism>]
            } else {
                Vec::new()
            },
            tls,
            sasl_fail_policy: policy,
        }
    }

    #[test]
    fn happy_path_ls_req_ack_sasl_end_welcome() {
        let mut d = Driver::new(config(true, true, SaslFailPolicy::Continue));
        assert!(d.sent_contains("CAP LS 302"));
        assert!(d.sent_contains("NICK adao"));

        d.feed("CAP * LS :multi-prefix sasl server-time");
        assert!(d.sent_contains("CAP REQ :multi-prefix server-time"));
        assert!(d.sent_contains("CAP REQ :sasl"));

        d.feed("CAP * ACK :multi-prefix server-time");
        d.feed("CAP * ACK :sasl");
        assert!(d.sent_contains("AUTHENTICATE PLAIN"));

        d.feed("AUTHENTICATE +");
        assert!(d.sent_contains("AUTHENTICATE AGFkYW8AaHVudGVyMg=="));
        assert!(!d.sent_contains("CAP END")); // rule 7: not before SASL resolves

        d.feed("900 adao adao!u@h adao :You are now logged in as adao");
        d.feed("903 adao :SASL authentication successful");
        assert!(d.sent_contains("CAP END"));

        d.feed("001 adao :Welcome to the network");
        assert!(d.machine.is_registered());

        // Event order: auth result, then Registered, then CapabilitiesChanged.
        assert_eq!(d.events.len(), 3);
        assert_eq!(d.events[0], Event::AuthResult(Ok("adao".to_string())));
        assert_eq!(
            d.events[1],
            Event::Registered {
                nick: "adao".to_string()
            }
        );
        assert!(matches!(d.events[2], Event::CapabilitiesChanged { .. }));
    }

    #[test]
    fn multiline_ls_assembles_before_req() {
        let mut d = Driver::new(config(false, true, SaslFailPolicy::Continue));
        d.feed("CAP * LS * :multi-prefix sasl");
        // Continuation line: no REQ yet (rule 4).
        assert!(!d.sent.iter().any(|l| l.starts_with("CAP REQ")));

        d.feed("CAP * LS :server-time");
        // Final line: now the assembled set drives the REQ.
        assert!(d.sent_contains("CAP REQ :multi-prefix server-time"));
    }

    #[test]
    fn nak_enables_no_cap_in_the_group() {
        let mut cfg = config(false, true, SaslFailPolicy::Continue);
        cfg.cap_groups = vec![vec!["multi-prefix".into(), "away-notify".into()]];
        let mut d = Driver::new(cfg);

        d.feed("CAP * LS :multi-prefix away-notify");
        assert!(d.sent_contains("CAP REQ :multi-prefix away-notify"));

        d.feed("CAP * NAK :multi-prefix away-notify");
        assert!(d.sent_contains("CAP END")); // negotiation still completes

        d.feed("001 adao :Welcome");
        // The CapabilitiesChanged from 001 must show nothing enabled (rule 5).
        let enabled_empty = d
            .events
            .iter()
            .any(|e| matches!(e, Event::CapabilitiesChanged { enabled, .. } if enabled.is_empty()));
        assert!(enabled_empty);
    }

    #[test]
    fn cap_end_required_before_welcome_is_honored() {
        // Rule 6 guard: reach a state where CAP END has NOT been sent (SASL is
        // still pending), then deliver 001 early. It must be ignored.
        let mut d = Driver::new(config(true, true, SaslFailPolicy::Continue));
        d.feed("CAP * LS :sasl");
        d.feed("CAP * ACK :sasl"); // -> SaslInProgress, sasl_pending, no CAP END
        assert!(!d.machine.cap_end_sent());

        d.feed("001 adao :Welcome"); // premature
        assert!(!d.machine.is_registered());
        assert!(!d.sent_contains("CAP END"));
    }

    #[test]
    fn cap_end_waits_for_sasl_result() {
        // Rule 7: CAP END only after 903 when sasl was REQ'd.
        let mut d = Driver::new(config(true, true, SaslFailPolicy::Continue));
        d.feed("CAP * LS :sasl");
        d.feed("CAP * ACK :sasl");
        d.feed("AUTHENTICATE +");
        assert!(!d.sent_contains("CAP END"));

        d.feed("903 adao :ok");
        assert!(d.sent_contains("CAP END"));
    }

    #[test]
    fn sasl_failure_continue_policy_still_registers() {
        let mut d = Driver::new(config(true, true, SaslFailPolicy::Continue));
        d.feed("CAP * LS :sasl");
        d.feed("CAP * ACK :sasl");
        d.feed("AUTHENTICATE +");
        d.feed("904 adao :SASL failed");

        assert!(d
            .events
            .iter()
            .any(|e| matches!(e, Event::AuthResult(Err(SaslError::Failed)))));
        assert!(d.sent_contains("CAP END")); // continue policy proceeds

        d.feed("001 adao :Welcome");
        assert!(d.machine.is_registered());
    }

    #[test]
    fn scram_drives_through_the_bringup_machine() {
        // Proves the Mechanism trait carries a multi-round mechanism end to end,
        // using the RFC 7677 vector. Server messages are base64 on the wire.
        use irc_proto::sasl::{encode_b64, ScramSha256};

        let mut cfg = config(false, true, SaslFailPolicy::Continue);
        cfg.sasl = vec![Box::new(ScramSha256::new(
            "user",
            "pencil",
            "rOprNGfwEbeRWgbNEkqO",
        ))];
        let mut d = Driver::new(cfg);

        d.feed("CAP * LS :sasl");
        d.feed("CAP * ACK :sasl");
        assert!(d.sent_contains("AUTHENTICATE SCRAM-SHA-256"));

        d.feed("AUTHENTICATE +");
        let client_first = encode_b64(b"n,,n=user,r=rOprNGfwEbeRWgbNEkqO");
        assert!(d.sent_contains(&format!("AUTHENTICATE {client_first}")));

        let server_first = "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096";
        d.feed(&format!(
            "AUTHENTICATE {}",
            encode_b64(server_first.as_bytes())
        ));
        let client_final = "c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ=";
        assert!(d.sent_contains(&format!(
            "AUTHENTICATE {}",
            encode_b64(client_final.as_bytes())
        )));

        // Correct server signature: client acknowledges with empty, then 903.
        d.feed(&format!(
            "AUTHENTICATE {}",
            encode_b64(b"v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=")
        ));
        assert!(d.sent_contains("AUTHENTICATE +"));
        assert!(!d.sent_contains("CAP END")); // not until 903

        d.feed("903 user :SASL authentication successful");
        assert!(d.sent_contains("CAP END"));
    }

    #[test]
    fn sasl_failure_abort_policy_disconnects() {
        let mut d = Driver::new(config(true, true, SaslFailPolicy::Abort));
        d.feed("CAP * LS :sasl");
        d.feed("CAP * ACK :sasl");
        d.feed("AUTHENTICATE +");
        d.feed("905 adao :too long");

        assert!(d
            .events
            .iter()
            .any(|e| matches!(e, Event::AuthResult(Err(SaslError::TooLong)))));
        assert!(d.events.iter().any(|e| matches!(
            e,
            Event::Disconnected(DisconnectReason::SaslAbortedByPolicy)
        )));
        assert!(!d.sent_contains("CAP END"));
        assert_eq!(d.machine.state(), State::Closed);
    }

    #[test]
    fn cap_notify_new_and_del_after_registration() {
        // Want chathistory, but the initial LS does not offer it.
        let mut cfg = config(false, true, SaslFailPolicy::Continue);
        cfg.cap_groups = vec![vec!["chathistory".into()]];
        let mut d = Driver::new(cfg);

        d.feed("CAP * LS :server-time");
        d.feed("001 adao :Welcome");
        assert!(d.machine.is_registered());

        // CAP NEW makes it requestable; the machine REQs the advertised name.
        d.feed("CAP * NEW :draft/chathistory");
        assert!(d.sent_contains("CAP REQ :draft/chathistory"));

        let events_before = d.events.len();
        d.feed("CAP * ACK :draft/chathistory");
        let changed_enabled = d.events[events_before..].iter().any(|e| {
            matches!(e, Event::CapabilitiesChanged { enabled, .. } if enabled.contains("chathistory"))
        });
        assert!(changed_enabled);

        // CAP DEL removes it with no REQ.
        let events_before = d.events.len();
        d.feed("CAP * DEL :draft/chathistory");
        let changed_removed = d.events[events_before..].iter().any(|e| {
            matches!(e, Event::CapabilitiesChanged { enabled, .. } if !enabled.contains("chathistory"))
        });
        assert!(changed_removed);
    }

    #[test]
    fn sasl_908_retries_with_a_listed_mechanism() {
        // Prefer EXTERNAL, fall back to PLAIN. The server rejects EXTERNAL with
        // 908 listing only PLAIN, so the machine retries PLAIN, not fails.
        use irc_proto::sasl::{External, Plain};

        let mut cfg = config(false, true, SaslFailPolicy::Continue);
        cfg.sasl = vec![
            Box::new(External::new()) as Box<dyn Mechanism>,
            Box::new(Plain::new("adao", "hunter2")) as Box<dyn Mechanism>,
        ];
        let mut d = Driver::new(cfg);

        d.feed("CAP * LS :sasl");
        d.feed("CAP * ACK :sasl");
        assert!(d.sent_contains("AUTHENTICATE EXTERNAL"));

        // Server: EXTERNAL not available, only PLAIN is.
        d.feed("908 adao PLAIN :are available SASL mechanisms");
        assert!(d.sent_contains("AUTHENTICATE PLAIN"));
        // Not a hard failure: no auth error, negotiation still open.
        assert!(!d
            .events
            .iter()
            .any(|e| matches!(e, Event::AuthResult(Err(_)))));
        assert!(!d.sent_contains("CAP END"));

        // PLAIN then completes normally.
        d.feed("AUTHENTICATE +");
        assert!(d.sent_contains("AUTHENTICATE AGFkYW8AaHVudGVyMg=="));
        d.feed("903 adao :ok");
        assert!(d.sent_contains("CAP END"));
    }

    #[test]
    fn sasl_908_with_no_matching_mechanism_fails() {
        let mut d = Driver::new(config(true, true, SaslFailPolicy::Continue));
        d.feed("CAP * LS :sasl");
        d.feed("CAP * ACK :sasl"); // starts PLAIN
                                   // Server lists only mechanisms we do not have.
        d.feed("908 adao ECDSA-NIST256P-CHALLENGE :are available SASL mechanisms");
        assert!(d
            .events
            .iter()
            .any(|e| matches!(e, Event::AuthResult(Err(SaslError::Failed)))));
    }

    #[test]
    fn plain_without_tls_is_refused() {
        let mut d = Driver::new(config(true, false, SaslFailPolicy::Continue));
        d.feed("CAP * LS :sasl");
        d.feed("CAP * ACK :sasl");
        // PLAIN over a non-TLS transport: no AUTHENTICATE, SASL skipped (rule 9).
        assert!(!d.sent_contains("AUTHENTICATE PLAIN"));
        assert!(d.sent_contains("CAP END"));
    }

    #[test]
    fn recommended_caps_are_small_groups_without_sasl() {
        let groups = recommended_caps();
        assert!(groups.len() > 1, "should be split into small groups");
        // sasl is added by the machine, not listed here.
        assert!(!groups.iter().flatten().any(|c| c == "sasl"));
        assert!(groups.iter().flatten().any(|c| c == "extended-join"));
        assert!(groups.iter().flatten().any(|c| c == "chathistory"));
    }

    #[test]
    fn nick_in_use_retries_with_an_alternate() {
        let mut d = Driver::new(config(false, true, SaslFailPolicy::Continue));
        d.feed("CAP * LS :server-time");
        d.feed("CAP * ACK :server-time");
        assert!(d.sent_contains("CAP END"));

        // Server rejects the nick; the machine proposes the base nick + '_'.
        d.feed("433 * adao :Nickname is already in use");
        assert!(d.sent_contains("NICK adao_"));
        assert!(!d.machine.is_registered());

        // The server accepts the alternate and welcomes it.
        d.feed("001 adao_ :Welcome");
        assert!(d.machine.is_registered());
        assert!(d
            .events
            .iter()
            .any(|e| matches!(e, Event::Registered { nick } if nick == "adao_")));
    }

    #[test]
    fn nick_in_use_gives_up_after_max_attempts() {
        let mut d = Driver::new(config(false, true, SaslFailPolicy::Continue));
        d.feed("CAP * LS :server-time");
        d.feed("CAP * ACK :server-time");

        for _ in 0..=MAX_NICK_ATTEMPTS {
            d.feed("433 * adao :Nickname is already in use");
        }
        assert_eq!(d.machine.state(), State::Closed);
        assert!(d.machine.is_closed());
        assert!(d
            .events
            .iter()
            .any(|e| matches!(e, Event::Disconnected(DisconnectReason::NickUnavailable))));
    }

    #[test]
    fn responds_to_ping_during_bringup() {
        let mut d = Driver::new(config(false, true, SaslFailPolicy::Continue));
        d.feed("PING :abc123");
        assert!(d.sent_contains("PONG :abc123"));
    }
}
