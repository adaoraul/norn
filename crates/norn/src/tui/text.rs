//! Display-width text helpers.
//!
//! Terminal cells, not characters: a flag, a family emoji or `e` plus a combining
//! accent is one grapheme cluster that occupies one or two cells however many
//! `char`s it is made of. Measuring and cutting per `char` splits such a cluster
//! (leaving a stray modifier) and over-counts its width, so everything that
//! wraps, truncates or places a cursor goes through these functions, which work
//! on whole grapheme clusters.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// The ellipsis shown where text was cut.
pub const ELLIPSIS: &str = "…";

/// The number of terminal cells `s` occupies.
pub fn width(s: &str) -> usize {
    s.graphemes(true).map(UnicodeWidthStr::width).sum()
}

/// The longest prefix of `s` that fits in `cols` cells, never splitting a
/// grapheme cluster.
pub fn prefix_within(s: &str, cols: usize) -> &str {
    let mut used = 0;
    let mut end = 0;
    for (i, g) in s.grapheme_indices(true) {
        let w = UnicodeWidthStr::width(g);
        if used + w > cols {
            break;
        }
        used += w;
        end = i + g.len();
    }
    &s[..end]
}

/// `s` cut to at most `cols` cells, ending in `…` when anything was cut so it is
/// clear the text continues.
pub fn truncate(s: &str, cols: usize) -> String {
    if width(s) <= cols {
        return s.to_string();
    }
    if cols == 0 {
        return String::new();
    }
    format!("{}{ELLIPSIS}", prefix_within(s, cols - 1))
}

/// The byte offset in `s` from which to show the text so that the part ending at
/// `cursor` (a byte offset) fits in `cols` cells: how a one-line input scrolls
/// sideways to keep the cursor in view. 0 when everything up to the cursor fits.
pub fn scroll_start(s: &str, cursor: usize, cols: usize) -> usize {
    let cursor = cursor.min(s.len());
    let mut used = 0;
    let mut start = cursor;
    // Walk back from the cursor, one cluster at a time, while it still fits.
    for (i, g) in s[..cursor].grapheme_indices(true).rev() {
        let w = UnicodeWidthStr::width(g);
        if used + w > cols {
            break;
        }
        used += w;
        start = i;
    }
    start
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_counts_clusters_not_chars() {
        assert_eq!(width("hello"), 5);
        assert_eq!(width("日本"), 4, "wide characters take two cells");
        assert_eq!(width("e\u{301}"), 1, "a combining accent adds nothing");
        // A family emoji is seven chars (three people and two joiners...) but one
        // two-cell cluster; a flag is two regional indicators in two cells.
        assert_eq!(width("👨\u{200d}👩\u{200d}👧"), 2);
        assert_eq!(width("🇧🇷"), 2);
        assert_eq!(
            width("👍🏽"),
            2,
            "a skin tone modifier is part of the cluster"
        );
    }

    #[test]
    fn prefix_within_never_splits_a_cluster() {
        assert_eq!(prefix_within("hello", 3), "hel");
        assert_eq!(prefix_within("日本語", 5), "日本", "no half of a wide char");
        assert_eq!(
            prefix_within("e\u{301}x", 1),
            "e\u{301}",
            "accent stays with its e"
        );
        let family = "👨\u{200d}👩\u{200d}👧!";
        assert_eq!(
            prefix_within(family, 1),
            "",
            "does not fit: nothing, not a fragment"
        );
        assert_eq!(prefix_within(family, 2), "👨\u{200d}👩\u{200d}👧");
        assert_eq!(prefix_within("abc", 0), "");
    }

    #[test]
    fn truncate_ends_in_an_ellipsis_only_when_it_cut() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("exactly", 7), "exactly");
        assert_eq!(truncate("a longer text", 8), "a longe…");
        assert_eq!(width(&truncate("a longer text", 8)), 8);
        assert_eq!(truncate("anything", 1), "…");
        assert_eq!(truncate("anything", 0), "");
        // Wide text: the ellipsis still fits inside the limit.
        let t = truncate("日本語日本語", 7);
        assert!(width(&t) <= 7, "{t:?}");
        assert!(t.ends_with('…'));
    }

    #[test]
    fn scroll_start_keeps_the_cursor_in_view() {
        let s = "0123456789";
        assert_eq!(scroll_start(s, 4, 10), 0, "fits: no scrolling");
        assert_eq!(scroll_start(s, 10, 10), 0);
        assert_eq!(
            scroll_start(s, 10, 4),
            6,
            "shows the last four before the cursor"
        );
        assert_eq!(scroll_start(s, 5, 3), 2);
        assert_eq!(scroll_start("日本語", "日本語".len(), 4), "日".len());
        assert_eq!(scroll_start(s, 0, 4), 0);
    }
}
