// responder - reply to trigger phrases (a tiny factoid bot).
// Edit RULES: a lowercase substring maps to a reply. Then `/plugins reload`.

const NAME = "responder";
const DESCRIPTION = "reply to trigger phrases";
const VERSION = "1.0";

const RULES = #{
    "!hello": "hi there!",
    "!norn": "norn is a terminal IRCv3 client",
};

fn on_message(m) {
    if m.from_self { return; }
    let text = m.text.to_lower();
    for key in RULES.keys() {
        if text.contains(key) {
            reply(RULES[key]);
        }
    }
}
