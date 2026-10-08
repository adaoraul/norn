//! IRC text formatting: mIRC bold, italic, underline, colour and friends.
//!
//! Messages carry control characters for styling (`\x02` bold, `\x03NN` colour,
//! ...). Left alone they show as stray digits and unprintable boxes, and other
//! control characters (an `ESC` in particular) could drive the terminal itself.
//! This module turns text into styled runs for rendering, or into plain text for
//! everything that must not see the codes: mention detection, search, plugins,
//! the line-mode renderer.
//!
//! It is pure: no terminal types, colours are plain RGB triples.

/// An RGB colour.
pub type Rgb = (u8, u8, u8);

/// The styling in effect for a run of text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Format {
    /// `\x02`.
    pub bold: bool,
    /// `\x1d`.
    pub italic: bool,
    /// `\x1f`.
    pub underline: bool,
    /// `\x1e`.
    pub strike: bool,
    /// `\x16`: swap foreground and background.
    pub reverse: bool,
    /// The foreground set by `\x03` or `\x04`, if any.
    pub fg: Option<Rgb>,
    /// The background set by `\x03` or `\x04`, if any.
    pub bg: Option<Rgb>,
}

#[cfg(test)]
impl Format {
    /// Whether this is the plain, unstyled default.
    pub fn is_plain(&self) -> bool {
        *self == Format::default()
    }
}

/// A run of text with one format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// The visible text.
    pub text: String,
    /// How it is styled.
    pub format: Format,
}

/// The 99-colour mIRC palette: 0-15 the classic colours, 16-98 the extended set.
/// 99 means "default" and has no entry.
const PALETTE: [Rgb; 99] = [
    (0xff, 0xff, 0xff),
    (0x00, 0x00, 0x00),
    (0x00, 0x00, 0x7f),
    (0x00, 0x93, 0x00),
    (0xff, 0x00, 0x00),
    (0x7f, 0x00, 0x00),
    (0x9c, 0x00, 0x9c),
    (0xfc, 0x7f, 0x00),
    (0xff, 0xff, 0x00),
    (0x00, 0xfc, 0x00),
    (0x00, 0x93, 0x93),
    (0x00, 0xff, 0xff),
    (0x00, 0x00, 0xfc),
    (0xff, 0x00, 0xff),
    (0x7f, 0x7f, 0x7f),
    (0xd2, 0xd2, 0xd2),
    (0x47, 0x00, 0x00),
    (0x47, 0x21, 0x00),
    (0x47, 0x47, 0x00),
    (0x32, 0x47, 0x00),
    (0x00, 0x47, 0x00),
    (0x00, 0x47, 0x2c),
    (0x00, 0x47, 0x47),
    (0x00, 0x27, 0x47),
    (0x00, 0x00, 0x47),
    (0x2e, 0x00, 0x47),
    (0x47, 0x00, 0x47),
    (0x47, 0x00, 0x2a),
    (0x74, 0x00, 0x00),
    (0x74, 0x3a, 0x00),
    (0x74, 0x74, 0x00),
    (0x51, 0x74, 0x00),
    (0x00, 0x74, 0x00),
    (0x00, 0x74, 0x49),
    (0x00, 0x74, 0x74),
    (0x00, 0x40, 0x74),
    (0x00, 0x00, 0x74),
    (0x4b, 0x00, 0x74),
    (0x74, 0x00, 0x74),
    (0x74, 0x00, 0x45),
    (0xb5, 0x00, 0x00),
    (0xb5, 0x63, 0x00),
    (0xb5, 0xb5, 0x00),
    (0x7d, 0xb5, 0x00),
    (0x00, 0xb5, 0x00),
    (0x00, 0xb5, 0x71),
    (0x00, 0xb5, 0xb5),
    (0x00, 0x63, 0xb5),
    (0x00, 0x00, 0xb5),
    (0x75, 0x00, 0xb5),
    (0xb5, 0x00, 0xb5),
    (0xb5, 0x00, 0x6b),
    (0xff, 0x00, 0x00),
    (0xff, 0x8c, 0x00),
    (0xff, 0xff, 0x00),
    (0xb2, 0xff, 0x00),
    (0x00, 0xff, 0x00),
    (0x00, 0xff, 0xa0),
    (0x00, 0xff, 0xff),
    (0x00, 0x8c, 0xff),
    (0x00, 0x00, 0xff),
    (0xa5, 0x00, 0xff),
    (0xff, 0x00, 0xff),
    (0xff, 0x00, 0x98),
    (0xff, 0x59, 0x59),
    (0xff, 0xb4, 0x59),
    (0xff, 0xff, 0x71),
    (0xcf, 0xff, 0x60),
    (0x6f, 0xff, 0x6f),
    (0x65, 0xff, 0xc9),
    (0x6d, 0xff, 0xff),
    (0x59, 0xb4, 0xff),
    (0x59, 0x59, 0xff),
    (0xc4, 0x59, 0xff),
    (0xff, 0x66, 0xff),
    (0xff, 0x59, 0xbc),
    (0xff, 0x9c, 0x9c),
    (0xff, 0xd3, 0x9c),
    (0xff, 0xff, 0x9c),
    (0xe2, 0xff, 0x9c),
    (0x9c, 0xff, 0x9c),
    (0x9c, 0xff, 0xdb),
    (0x9c, 0xff, 0xff),
    (0x9c, 0xd3, 0xff),
    (0x9c, 0x9c, 0xff),
    (0xdc, 0x9c, 0xff),
    (0xff, 0x9c, 0xff),
    (0xff, 0x94, 0xd3),
    (0x00, 0x00, 0x00),
    (0x13, 0x13, 0x13),
    (0x28, 0x28, 0x28),
    (0x36, 0x36, 0x36),
    (0x4d, 0x4d, 0x4d),
    (0x65, 0x65, 0x65),
    (0x81, 0x81, 0x81),
    (0x9f, 0x9f, 0x9f),
    (0xbc, 0xbc, 0xbc),
    (0xe2, 0xe2, 0xe2),
    (0xff, 0xff, 0xff),
];

/// Whether `c` is a control character that is never shown: the C0 and C1
/// controls and DEL, except the formatting codes this module interprets.
fn is_dropped_control(c: char) -> bool {
    (c.is_control()) && !matches!(c, '\t')
}

/// Take up to `max` ASCII digits from the front of `s`.
fn take_digits(s: &str, max: usize) -> (&str, &str) {
    let n = s.bytes().take(max).take_while(u8::is_ascii_digit).count();
    s.split_at(n)
}

/// Take exactly six hex digits from the front of `s`, as a colour.
fn take_hex(s: &str) -> Option<(Rgb, &str)> {
    let hex = s.get(..6)?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    Some((((v >> 16) as u8, (v >> 8) as u8, v as u8), &s[6..]))
}

/// A palette entry: `None` for 99 (default) or out of range.
fn palette(index: usize) -> Option<Rgb> {
    PALETTE.get(index).copied()
}

/// Parse `input` into styled runs. Control characters other than the
/// formatting codes are dropped, so nothing in a message can reach the terminal
/// as an escape sequence. Adjacent runs with the same format are merged.
pub fn parse(input: &str) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    let mut format = Format::default();
    let mut text = String::new();
    let flush = |runs: &mut Vec<Run>, text: &mut String, format: Format| {
        if text.is_empty() {
            return;
        }
        match runs.last_mut() {
            Some(last) if last.format == format => last.text.push_str(text),
            _ => runs.push(Run {
                text: std::mem::take(text),
                format,
            }),
        }
        text.clear();
    };

    let mut rest = input;
    while let Some(c) = rest.chars().next() {
        rest = &rest[c.len_utf8()..];
        let toggle = |f: &mut Format| match c {
            '\u{02}' => f.bold = !f.bold,
            '\u{1d}' => f.italic = !f.italic,
            '\u{1f}' => f.underline = !f.underline,
            '\u{1e}' => f.strike = !f.strike,
            '\u{16}' => f.reverse = !f.reverse,
            _ => {}
        };
        match c {
            '\u{02}' | '\u{1d}' | '\u{1f}' | '\u{1e}' | '\u{16}' => {
                flush(&mut runs, &mut text, format);
                toggle(&mut format);
            }
            '\u{0f}' => {
                flush(&mut runs, &mut text, format);
                format = Format::default();
            }
            // \x03 FG[,BG]: one or two digits each; a bare \x03 clears colours.
            '\u{03}' => {
                flush(&mut runs, &mut text, format);
                let (fg, after_fg) = take_digits(rest, 2);
                if fg.is_empty() {
                    format.fg = None;
                    format.bg = None;
                } else {
                    format.fg = fg.parse::<usize>().ok().and_then(palette);
                    rest = after_fg;
                    // A comma belongs to the colour only when a digit follows.
                    if let Some(after_comma) = rest.strip_prefix(',') {
                        let (bg, after_bg) = take_digits(after_comma, 2);
                        if !bg.is_empty() {
                            format.bg = bg.parse::<usize>().ok().and_then(palette);
                            rest = after_bg;
                        }
                    }
                }
            }
            // \x04 RRGGBB[,RRGGBB]: hex colours; a bare \x04 clears colours.
            '\u{04}' => {
                flush(&mut runs, &mut text, format);
                match take_hex(rest) {
                    Some((fg, after_fg)) => {
                        format.fg = Some(fg);
                        rest = after_fg;
                        if let Some((bg, after_bg)) = rest.strip_prefix(',').and_then(take_hex) {
                            format.bg = Some(bg);
                            rest = after_bg;
                        }
                    }
                    None => {
                        format.fg = None;
                        format.bg = None;
                    }
                }
            }
            c if is_dropped_control(c) => {}
            c => text.push(c),
        }
    }
    flush(&mut runs, &mut text, format);
    runs
}

/// `input` as plain text: formatting codes and every other control character
/// removed, nothing else changed.
pub fn strip(input: &str) -> String {
    // Fast path: most messages have no control characters at all.
    if !input.chars().any(|c| c.is_control() && c != '\t') {
        return input.to_string();
    }
    parse(input).into_iter().map(|r| r.text).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(runs: &[Run]) -> Vec<&str> {
        runs.iter().map(|r| r.text.as_str()).collect()
    }

    #[test]
    fn plain_text_is_one_plain_run() {
        let runs = parse("hello world");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "hello world");
        assert!(runs[0].format.is_plain());
        assert!(parse("").is_empty());
    }

    #[test]
    fn toggles_switch_styles_on_and_off() {
        let runs = parse("a\u{02}b\u{02}c\u{1d}d\u{1f}e\u{0f}f");
        assert_eq!(texts(&runs), ["a", "b", "c", "d", "e", "f"]);
        assert!(!runs[0].format.bold);
        assert!(runs[1].format.bold);
        assert!(!runs[2].format.bold && !runs[2].format.italic);
        assert!(runs[3].format.italic);
        assert!(runs[4].format.italic && runs[4].format.underline);
        assert!(runs[5].format.is_plain(), "\\x0f resets everything");
    }

    #[test]
    fn strike_and_reverse_are_recognised() {
        let runs = parse("\u{1e}s\u{16}r");
        assert!(runs[0].format.strike);
        assert!(runs[1].format.strike && runs[1].format.reverse);
    }

    #[test]
    fn colours_take_one_or_two_digits_and_an_optional_background() {
        let runs = parse("\u{03}4red\u{03}04,12both\u{03}plain");
        assert_eq!(texts(&runs), ["red", "both", "plain"]);
        assert_eq!(runs[0].format.fg, Some((0xff, 0x00, 0x00)));
        assert_eq!(runs[0].format.bg, None);
        assert_eq!(runs[1].format.fg, Some((0xff, 0x00, 0x00)));
        assert_eq!(runs[1].format.bg, Some((0x00, 0x00, 0xfc)));
        assert!(runs[2].format.is_plain(), "a bare \\x03 clears the colours");
    }

    #[test]
    fn a_digit_after_the_colour_is_text_and_a_lone_comma_is_text() {
        // Only two digits belong to the colour: the third is the message.
        let runs = parse("\u{03}041!");
        assert_eq!(runs[0].text, "1!");
        // A comma with no digit after it is not a background.
        let runs = parse("\u{03}4,x");
        assert_eq!(runs[0].text, ",x");
        assert_eq!(runs[0].format.bg, None);
    }

    #[test]
    fn colour_99_means_default() {
        let runs = parse("\u{03}4a\u{03}99b");
        assert!(runs[0].format.fg.is_some());
        assert_eq!(runs[1].format.fg, None);
    }

    #[test]
    fn hex_colours_work_with_and_without_a_background() {
        let runs = parse("\u{04}ff8000orange\u{04}00ff00,0000ffgreen\u{04}x");
        assert_eq!(runs[0].format.fg, Some((0xff, 0x80, 0x00)));
        assert_eq!(runs[1].format.fg, Some((0x00, 0xff, 0x00)));
        assert_eq!(runs[1].format.bg, Some((0x00, 0x00, 0xff)));
        assert_eq!(runs[2].text, "x");
        assert!(
            runs[2].format.is_plain(),
            "an invalid hex clears the colours"
        );
    }

    #[test]
    fn terminal_escapes_and_other_controls_never_survive() {
        // An ESC sequence, a bell, a NUL, DEL and a C1 control.
        let nasty = "a\u{1b}[31mb\u{07}c\u{00}d\u{7f}e\u{85}f";
        let plain = strip(nasty);
        assert_eq!(plain, "a[31mbcdef");
        assert!(plain.chars().all(|c| !c.is_control()));
        let runs = parse(nasty);
        assert!(runs.iter().all(|r| r.text.chars().all(|c| !c.is_control())));
        // Tabs are ordinary whitespace and stay.
        assert_eq!(strip("a\tb"), "a\tb");
    }

    #[test]
    fn strip_removes_every_formatting_code_and_nothing_else() {
        let msg = "\u{02}bold\u{02} \u{03}4,2red on blue\u{0f} \u{1d}it\u{1d} \u{04}ff0000x";
        assert_eq!(strip(msg), "bold red on blue it x");
        assert_eq!(strip("no codes here"), "no codes here");
        assert_eq!(strip("ünïcödé 日本"), "ünïcödé 日本");
    }

    #[test]
    fn adjacent_runs_with_the_same_format_are_merged() {
        // A reset that changes nothing does not split the text.
        let runs = parse("ab\u{0f}cd");
        assert_eq!(texts(&runs), ["abcd"]);
    }

    #[test]
    fn the_palette_has_all_99_colours() {
        assert_eq!(PALETTE.len(), 99);
        assert_eq!(palette(0), Some((255, 255, 255)));
        assert_eq!(palette(1), Some((0, 0, 0)));
        assert_eq!(palette(98), Some((255, 255, 255)));
        assert_eq!(palette(99), None);
    }
}
