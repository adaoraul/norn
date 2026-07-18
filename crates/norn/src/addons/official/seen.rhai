// seen - remember when each nick was last active, and answer "!seen <nick>".
// A demo of the persistent KV store: state survives reloads and restarts.
// Say "!seen alice" in a channel and the bot replies with alice's last line.

const NAME = "seen";
const DESCRIPTION = "track when nicks were last seen (KV store demo)";
const VERSION = "1.0";

fn on_message(m) {
    if m.from_self { return; }

    // Answer a "!seen <nick>" query.
    if m.text.starts_with("!seen ") {
        let who = m.text.sub_string(6);
        who.trim();                       // trims in place (Rhai trim mutates)
        let last = store.get(who.to_lower());
        if last == "" {
            reply(`I have not seen ${who}.`);
        } else {
            reply(`${who} was last seen saying: ${last}`);
        }
        return;
    }

    // Otherwise record this nick's latest line.
    store.set(m.nick.to_lower(), m.text);
}
