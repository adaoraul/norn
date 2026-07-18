// keepnick - reclaim your preferred nick when it becomes free.
// Set WANT to your registered nick, then `/plugins reload`.

const NAME = "keepnick";
const DESCRIPTION = "reclaim your preferred nick when it frees up";
const VERSION = "1.0";

const WANT = "yournick";

// Grab `want` if the freed nick is it and we are not already using it.
// Note: a top-level `const` is readable inside hook functions, but NOT inside
// helper functions they call, so the hook reads WANT and passes it in.
fn grab(freed, want) {
    if freed == want && nick() != want {
        raw(`NICK ${want}`);
    }
}

fn on_quit(m) { grab(m.nick, WANT); }       // someone quit
fn on_nick(m) { grab(m.nick, WANT); }       // m.nick is the vacated (old) nick
