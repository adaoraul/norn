// autoop - give ops to trusted nicks when they join.
// Edit TRUSTED (nicks) and CHANNELS below, then `/plugins reload`.

const NAME = "autoop";
const DESCRIPTION = "op trusted nicks when they join";
const VERSION = "1.0";

const TRUSTED = ["alice", "bob"];
const CHANNELS = ["#norn"];

fn on_join(m) {
    if (m.nick in TRUSTED) && (m.channel in CHANNELS) {
        raw(`MODE ${m.channel} +o ${m.nick}`);
    }
}
