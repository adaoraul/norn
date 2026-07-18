// keepnick - reclaim your preferred nick when it becomes free.
// Set WANT to your registered nick, then `/plugins reload`.

const NAME = "keepnick";
const DESCRIPTION = "reclaim your preferred nick when it frees up";
const VERSION = "1.0";

const WANT = "yournick";

// Grab WANT if the freed nick is it and we are not already using it.
fn grab(freed) {
    if freed == WANT && nick() != WANT {
        raw(`NICK ${WANT}`);
    }
}

fn on_quit(m) { grab(m.nick); }       // someone quit
fn on_nick(m) { grab(m.nick); }       // m.nick is the vacated (old) nick
