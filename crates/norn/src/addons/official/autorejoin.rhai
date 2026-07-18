// autorejoin - rejoin a channel after you are kicked.

const NAME = "autorejoin";
const DESCRIPTION = "rejoin a channel after being kicked";
const VERSION = "1.0";

fn on_kick(m) {
    if m.is_me {
        raw(`JOIN ${m.channel}`);
    }
}
