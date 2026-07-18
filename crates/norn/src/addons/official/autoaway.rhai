// autoaway - mark yourself away after a spell of keyboard inactivity, and come
// back automatically when you return. The idle threshold is the `idle_secs`
// client setting (/set idle_secs 300); this fires once you cross it.

const NAME = "autoaway";
const DESCRIPTION = "set away when idle, back when active";
const VERSION = "1.0";

// The away message to use.
const MESSAGE = "auto-away (idle)";

fn on_idle(m) {
    if !am_away() {
        raw(`AWAY :${MESSAGE}`);
    }
}

fn on_active(m) {
    if am_away() {
        raw("AWAY");
    }
}
