// autoaway - mark yourself away after a spell of keyboard inactivity, and come
// back automatically when you return. The idle threshold is the `idle_secs`
// client setting (/set idle_secs 300); this fires once you cross it.
//
// The away message is configurable: /plugins config autoaway (or edit CONFIG).

const NAME = "autoaway";
const DESCRIPTION = "set away when idle, back when active";
const VERSION = "1.0";

// User-tunable config (edit here, or via /plugins config autoaway).
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
