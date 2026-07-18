//! Translate user input lines into outgoing IRC lines.
//!
//! Plain text is sent as a message to the current target (the last channel
//! joined or set with `/msg`). Lines starting with `/` are commands. Used by the
//! `--plain` renderer; the TUI has its own key handling.

/// Translate one input line into zero or more IRC lines, plus whether the user
/// asked to quit.
pub fn translate(input: &str, current: &mut Option<String>) -> (Vec<String>, bool) {
    let input = input.trim();
    if input.is_empty() {
        return (Vec::new(), false);
    }

    let Some(rest) = input.strip_prefix('/') else {
        // Plain text: message the current target.
        return match current {
            Some(target) => (vec![format!("PRIVMSG {target} :{input}")], false),
            None => {
                eprintln!("no target; use /join #channel or /msg <target> <text>");
                (Vec::new(), false)
            }
        };
    };

    let mut parts = rest.splitn(2, ' ');
    let command = parts.next().unwrap_or("").to_ascii_lowercase();
    let arg = parts.next().unwrap_or("").trim();

    match command.as_str() {
        "join" => match arg.split_whitespace().next() {
            Some(channel) => {
                *current = Some(channel.to_string());
                (vec![format!("JOIN {channel}")], false)
            }
            None => {
                eprintln!("usage: /join #channel");
                (Vec::new(), false)
            }
        },
        "part" => {
            let channel = if arg.is_empty() {
                current.clone().unwrap_or_default()
            } else {
                arg.split_whitespace().next().unwrap_or("").to_string()
            };
            if channel.is_empty() {
                eprintln!("usage: /part [#channel]");
                (Vec::new(), false)
            } else {
                (vec![format!("PART {channel}")], false)
            }
        }
        "msg" => {
            let mut split = arg.splitn(2, ' ');
            let target = split.next().unwrap_or("");
            let text = split.next().unwrap_or("").trim();
            if target.is_empty() || text.is_empty() {
                eprintln!("usage: /msg <target> <text>");
                (Vec::new(), false)
            } else {
                *current = Some(target.to_string());
                (vec![format!("PRIVMSG {target} :{text}")], false)
            }
        }
        "nick" => {
            if arg.is_empty() {
                eprintln!("usage: /nick <newnick>");
                (Vec::new(), false)
            } else {
                (vec![format!("NICK {arg}")], false)
            }
        }
        "names" => {
            let channel = if arg.is_empty() {
                current.clone().unwrap_or_default()
            } else {
                arg.to_string()
            };
            (
                vec![format!("NAMES {channel}").trim_end().to_string()],
                false,
            )
        }
        "raw" => {
            if arg.is_empty() {
                eprintln!("usage: /raw <line>");
                (Vec::new(), false)
            } else {
                (vec![arg.to_string()], false)
            }
        }
        "quit" => {
            let reason = if arg.is_empty() { "norn" } else { arg };
            (vec![format!("QUIT :{reason}")], true)
        }
        other => {
            eprintln!("unknown command: /{other}");
            (Vec::new(), false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::translate;

    #[test]
    fn plain_text_needs_a_target() {
        let mut current = None;
        assert_eq!(translate("hello", &mut current).0, Vec::<String>::new());
    }

    #[test]
    fn join_sets_current_target_and_plain_text_messages_it() {
        let mut current = None;
        assert_eq!(
            translate("/join #rust", &mut current).0,
            vec!["JOIN #rust".to_string()]
        );
        assert_eq!(current.as_deref(), Some("#rust"));
        assert_eq!(
            translate("hi all", &mut current).0,
            vec!["PRIVMSG #rust :hi all".to_string()]
        );
    }

    #[test]
    fn msg_and_nick_and_raw() {
        let mut current = None;
        assert_eq!(
            translate("/msg bob hey there", &mut current).0,
            vec!["PRIVMSG bob :hey there".to_string()]
        );
        assert_eq!(
            translate("/nick newnick", &mut current).0,
            vec!["NICK newnick".to_string()]
        );
        assert_eq!(
            translate("/raw WHOIS bob", &mut current).0,
            vec!["WHOIS bob".to_string()]
        );
    }

    #[test]
    fn quit_flags_shutdown() {
        let mut current = None;
        let (lines, quit) = translate("/quit bye", &mut current);
        assert_eq!(lines, vec!["QUIT :bye".to_string()]);
        assert!(quit);
    }
}
