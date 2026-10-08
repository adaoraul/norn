//! Line editing on a `String` and a byte-offset cursor.
//!
//! The main input line and every inline edit in the settings, networks and
//! plugin screens share these functions, so the keys behave the same everywhere
//! (Backspace, Delete, Home/End, Ctrl+A/E/U/W, word movement). They work on a
//! `(&mut String, &mut usize)` pair rather than owning the text, so each place
//! keeps the fields it already has.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Clamp `cursor` to a valid char boundary inside `text` (a cursor left over from
/// an earlier, longer text must never panic a slice).
pub fn clamp(text: &str, cursor: usize) -> usize {
    let mut c = cursor.min(text.len());
    while !text.is_char_boundary(c) {
        c -= 1;
    }
    c
}

/// Insert one character at the cursor.
pub fn insert_char(text: &mut String, cursor: &mut usize, c: char) {
    *cursor = clamp(text, *cursor);
    text.insert(*cursor, c);
    *cursor += c.len_utf8();
}

/// Insert a string at the cursor.
pub fn insert_str(text: &mut String, cursor: &mut usize, s: &str) {
    *cursor = clamp(text, *cursor);
    text.insert_str(*cursor, s);
    *cursor += s.len();
}

/// Delete the character before the cursor.
pub fn backspace(text: &mut String, cursor: &mut usize) {
    *cursor = clamp(text, *cursor);
    if let Some(prev) = text[..*cursor].chars().next_back() {
        let start = *cursor - prev.len_utf8();
        text.replace_range(start..*cursor, "");
        *cursor = start;
    }
}

/// Delete the character under the cursor.
pub fn delete(text: &mut String, cursor: &mut usize) {
    *cursor = clamp(text, *cursor);
    if let Some(next) = text[*cursor..].chars().next() {
        text.replace_range(*cursor..*cursor + next.len_utf8(), "");
    }
}

/// Move one character left.
pub fn left(text: &str, cursor: &mut usize) {
    *cursor = clamp(text, *cursor);
    if let Some(prev) = text[..*cursor].chars().next_back() {
        *cursor -= prev.len_utf8();
    }
}

/// Move one character right.
pub fn right(text: &str, cursor: &mut usize) {
    *cursor = clamp(text, *cursor);
    if let Some(next) = text[*cursor..].chars().next() {
        *cursor += next.len_utf8();
    }
}

/// The start of the word before `cursor`: skips spaces, then the word itself.
pub fn word_left(text: &str, cursor: usize) -> usize {
    let mut pos = clamp(text, cursor);
    while let Some(c) = text[..pos]
        .chars()
        .next_back()
        .filter(|c| c.is_whitespace())
    {
        pos -= c.len_utf8();
    }
    while let Some(c) = text[..pos]
        .chars()
        .next_back()
        .filter(|c| !c.is_whitespace())
    {
        pos -= c.len_utf8();
    }
    pos
}

/// The end of the word at or after `cursor`: skips spaces, then the word itself.
pub fn word_right(text: &str, cursor: usize) -> usize {
    let mut pos = clamp(text, cursor);
    while let Some(c) = text[pos..].chars().next().filter(|c| c.is_whitespace()) {
        pos += c.len_utf8();
    }
    while let Some(c) = text[pos..].chars().next().filter(|c| !c.is_whitespace()) {
        pos += c.len_utf8();
    }
    pos
}

/// Delete everything before the cursor (Ctrl+U).
pub fn kill_to_start(text: &mut String, cursor: &mut usize) {
    *cursor = clamp(text, *cursor);
    text.replace_range(..*cursor, "");
    *cursor = 0;
}

/// Delete the word before the cursor (Ctrl+W, Alt+Backspace).
pub fn kill_word_back(text: &mut String, cursor: &mut usize) {
    *cursor = clamp(text, *cursor);
    let start = word_left(text, *cursor);
    text.replace_range(start..*cursor, "");
    *cursor = start;
}

/// Apply an editing key to `text`. Returns whether the key was one of ours; keys
/// that mean something else (Enter, Tab, Ctrl+C, Alt+Left for buffer switching,
/// Alt+digits, function keys, ...) are left alone for the caller.
pub fn handle(text: &mut String, cursor: &mut usize, key: &KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Char('a') if ctrl => *cursor = 0,
        KeyCode::Char('e') if ctrl => *cursor = text.len(),
        KeyCode::Char('u') if ctrl => kill_to_start(text, cursor),
        KeyCode::Char('w') if ctrl => kill_word_back(text, cursor),
        // Word movement: Ctrl+arrows, and readline's Alt+b / Alt+f. Alt+arrows
        // are taken: they switch buffers.
        KeyCode::Left if ctrl => *cursor = word_left(text, *cursor),
        KeyCode::Right if ctrl => *cursor = word_right(text, *cursor),
        KeyCode::Char('b') if alt && !ctrl => *cursor = word_left(text, *cursor),
        KeyCode::Char('f') if alt && !ctrl => *cursor = word_right(text, *cursor),
        KeyCode::Backspace if alt => kill_word_back(text, cursor),
        KeyCode::Backspace => backspace(text, cursor),
        KeyCode::Delete => delete(text, cursor),
        KeyCode::Left if !alt => left(text, cursor),
        KeyCode::Right if !alt => right(text, cursor),
        KeyCode::Home => *cursor = 0,
        KeyCode::End => *cursor = text.len(),
        KeyCode::Char(c) if !ctrl && !alt => insert_char(text, cursor, c),
        _ => return false,
    }
    true
}

/// `text` with a block cursor drawn at `cursor`, for inline edits.
pub fn with_cursor(text: &str, cursor: usize) -> String {
    let c = clamp(text, cursor);
    if c >= text.len() {
        format!("{text}\u{2588}")
    } else {
        // The block replaces the character under the cursor (mid-line), so the
        // line keeps its width as the cursor moves.
        let width = text[c..].chars().next().map_or(0, char::len_utf8);
        format!("{}\u{2588}{}", &text[..c], &text[c + width..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(text: &str, cursor: usize, code: KeyCode, mods: KeyModifiers) -> (String, usize) {
        let (mut t, mut c) = (text.to_string(), cursor);
        handle(&mut t, &mut c, &KeyEvent::new(code, mods));
        (t, c)
    }
    const NONE: KeyModifiers = KeyModifiers::NONE;
    const CTRL: KeyModifiers = KeyModifiers::CONTROL;
    const ALT: KeyModifiers = KeyModifiers::ALT;

    #[test]
    fn insert_and_delete_at_the_cursor_respect_multibyte_characters() {
        let (mut t, mut c) = (String::from("héllo"), 3); // after "hé"
        insert_char(&mut t, &mut c, 'X');
        assert_eq!((t.as_str(), c), ("héXllo", 4));
        backspace(&mut t, &mut c);
        assert_eq!((t.as_str(), c), ("héllo", 3));
        backspace(&mut t, &mut c); // removes the two-byte é
        assert_eq!((t.as_str(), c), ("hllo", 1));
        delete(&mut t, &mut c);
        assert_eq!((t.as_str(), c), ("hlo", 1));
        // At the ends, nothing happens and nothing panics.
        let (mut t, mut c) = (String::from("a"), 0);
        backspace(&mut t, &mut c);
        c = 1;
        delete(&mut t, &mut c);
        assert_eq!((t.as_str(), c), ("a", 1));
    }

    #[test]
    fn a_stale_cursor_is_clamped_instead_of_panicking() {
        let (mut t, mut c) = (String::from("ab"), 99);
        insert_char(&mut t, &mut c, 'c');
        assert_eq!((t.as_str(), c), ("abc", 3));
        let (mut t, mut c) = (String::from("é"), 1); // inside the 2-byte char
        backspace(&mut t, &mut c);
        assert_eq!((t.as_str(), c), ("é", 0));
    }

    #[test]
    fn word_movement_skips_spaces_then_words() {
        let t = "one  two three";
        assert_eq!(word_left(t, t.len()), 9); // start of "three"
        assert_eq!(word_left(t, 9), 5); // start of "two"
        assert_eq!(word_left(t, 5), 0);
        assert_eq!(word_left(t, 0), 0);
        assert_eq!(word_right(t, 0), 3); // end of "one"
        assert_eq!(word_right(t, 3), 8); // end of "two"
        assert_eq!(word_right(t, t.len()), t.len());
    }

    #[test]
    fn kill_keys_cut_before_the_cursor_only() {
        let (t, c) = press("hello big world", 9, KeyCode::Char('w'), CTRL);
        assert_eq!((t.as_str(), c), ("hello  world", 6)); // "big" removed
        let (t, c) = press("hello big world", 9, KeyCode::Char('u'), CTRL);
        assert_eq!((t.as_str(), c), (" world", 0)); // everything before the cursor
    }

    #[test]
    fn readline_and_arrow_keys_move_the_cursor() {
        assert_eq!(press("abc def", 7, KeyCode::Char('a'), CTRL).1, 0);
        assert_eq!(press("abc def", 0, KeyCode::Char('e'), CTRL).1, 7);
        assert_eq!(press("abc def", 7, KeyCode::Left, CTRL).1, 4);
        assert_eq!(press("abc def", 0, KeyCode::Right, CTRL).1, 3);
        assert_eq!(press("abc def", 7, KeyCode::Char('b'), ALT).1, 4);
        assert_eq!(press("abc def", 0, KeyCode::Char('f'), ALT).1, 3);
        assert_eq!(press("abc def", 3, KeyCode::Home, NONE).1, 0);
        assert_eq!(press("abc def", 3, KeyCode::End, NONE).1, 7);
        let (t, c) = press("abc def", 7, KeyCode::Backspace, ALT);
        assert_eq!((t.as_str(), c), ("abc ", 4));
        let (t, c) = press("abc def", 3, KeyCode::Delete, NONE);
        assert_eq!((t.as_str(), c), ("abcdef", 3));
    }

    #[test]
    fn keys_with_other_meanings_are_not_consumed() {
        let mut t = String::from("x");
        let mut c = 1;
        for (code, mods) in [
            (KeyCode::Enter, NONE),
            (KeyCode::Tab, NONE),
            (KeyCode::Esc, NONE),
            (KeyCode::Up, NONE),
            (KeyCode::PageUp, NONE),
            (KeyCode::F(2), NONE),
            (KeyCode::Char('c'), CTRL),
            (KeyCode::Char('k'), CTRL),
            (KeyCode::Left, ALT),  // switches buffer
            (KeyCode::Right, ALT), // switches buffer
            (KeyCode::Char('3'), ALT),
        ] {
            assert!(
                !handle(&mut t, &mut c, &KeyEvent::new(code, mods)),
                "{code:?} {mods:?} should be left to the caller"
            );
        }
        assert_eq!((t.as_str(), c), ("x", 1));
    }

    #[test]
    fn with_cursor_draws_a_block_at_the_cursor() {
        assert_eq!(with_cursor("abc", 3), "abc\u{2588}");
        assert_eq!(with_cursor("abc", 1), "a\u{2588}c");
        assert_eq!(with_cursor("", 0), "\u{2588}");
        assert_eq!(with_cursor("aéb", 1), "a\u{2588}b"); // multibyte under cursor
    }
}
