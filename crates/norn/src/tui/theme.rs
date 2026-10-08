//! Colors for the TUI, matching the design mockup's muted-dark palette.

use ratatui::style::Color;

/// Window background.
pub const BG: Color = Color::Rgb(0x12, 0x14, 0x1a);
/// Sidebar / nicklist background.
pub const PANEL: Color = Color::Rgb(0x0e, 0x10, 0x15);
/// Active row background.
pub const ACTIVE_BG: Color = Color::Rgb(0x1c, 0x21, 0x30);
/// Panel separator rules.
pub const BORDER: Color = Color::Rgb(0x23, 0x28, 0x3a);
/// Modal / selection border.
pub const BORDER_BRIGHT: Color = Color::Rgb(0x2e, 0x35, 0x50);

/// Default body text.
pub const TEXT: Color = Color::Rgb(0xaa, 0xb0, 0xbe);
/// Bright text (active labels, own input).
pub const BRIGHT: Color = Color::Rgb(0xe8, 0xeb, 0xf2);
/// Slightly-bright text (buffer name).
pub const BRIGHT2: Color = Color::Rgb(0xc8, 0xcd, 0xd8);
/// Dim text (topics, headers).
pub const DIM: Color = Color::Rgb(0x8b, 0x93, 0xa8);
/// Dimmer text (timestamps, hints). Still readable: at least 4.5:1 on every
/// background it is drawn on.
pub const DIM2: Color = Color::Rgb(0x83, 0x8b, 0x9a);
/// Event-line text.
pub const EVENT: Color = Color::Rgb(0x8a, 0x91, 0xa1);
/// Decorative rules and separators only. Far below text contrast on purpose, so
/// never use it for anything the user has to read.
pub const FAINT: Color = Color::Rgb(0x3d, 0x42, 0x50);

/// Default accent (teal).
pub const ACCENT: Color = Color::Rgb(0x6b, 0xac, 0xae);

/// Resolve a theme name to its accent color. Unknown names fall back to teal.
/// The names here are the valid values for `/set theme` and the config
/// `[client] theme` key.
pub fn accent_for(name: &str) -> Color {
    match name.to_ascii_lowercase().as_str() {
        "amber" | "gold" => Color::Rgb(0xe0, 0xc0, 0x60),
        "green" => Color::Rgb(0x8f, 0xae, 0x6b),
        "blue" => Color::Rgb(0x6b, 0x8f, 0xae),
        "purple" | "violet" => Color::Rgb(0xc7, 0x8f, 0xd6),
        "pink" => Color::Rgb(0xae, 0x6b, 0x8f),
        _ => ACCENT, // teal
    }
}

/// The theme names `accent_for` recognizes, for help text and completion.
pub const THEME_NAMES: &[&str] = &["teal", "amber", "green", "blue", "purple", "pink"];
/// Op (`@`) color, also the >2 unread badge and highlight text.
pub const GOLD: Color = Color::Rgb(0xe0, 0xc0, 0x60);
/// Error / failure color (muted red).
pub const RED: Color = Color::Rgb(0xd0, 0x6b, 0x6b);
/// Highlight background for own-nick mentions.
pub const HL_BG: Color = Color::Rgb(0x3a, 0x33, 0x20);

/// The muted hue set used to color nicks deterministically.
const NICK_HUES: &[Color] = &[
    Color::Rgb(0x8f, 0xae, 0x6b), // green
    Color::Rgb(0xc7, 0x8f, 0xd6), // purple
    Color::Rgb(0x6b, 0xac, 0xae), // teal
    Color::Rgb(0xd0, 0x98, 0x5f), // orange
    Color::Rgb(0x6b, 0x8f, 0xd6), // blue
    Color::Rgb(0xae, 0x6b, 0x8f), // pink
    Color::Rgb(0xd6, 0xc4, 0x6b), // yellow
    Color::Rgb(0xb4, 0xbf, 0xe0), // lavender
    Color::Rgb(0x9a, 0x7c, 0xc0), // violet
];

/// A deterministic color for a nick (hash into the muted hue set).
pub fn nick_color(nick: &str) -> Color {
    let mut hash: u32 = 2166136261;
    for b in nick.bytes() {
        hash ^= b as u32;
        hash = hash.wrapping_mul(16777619);
    }
    NICK_HUES[(hash as usize) % NICK_HUES.len()]
}

/// WCAG relative luminance of an RGB color (other color kinds count as black).
fn luminance(c: Color) -> f64 {
    let Color::Rgb(r, g, b) = c else {
        return 0.0;
    };
    let lin = |v: u8| {
        let s = f64::from(v) / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// WCAG contrast ratio between two colors, from 1.0 (identical) to 21.0.
pub fn contrast(a: Color, b: Color) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AA: f64 = 4.5;

    #[test]
    fn contrast_matches_known_values() {
        let black = Color::Rgb(0, 0, 0);
        let white = Color::Rgb(255, 255, 255);
        assert!((contrast(black, white) - 21.0).abs() < 0.01);
        assert!((contrast(white, white) - 1.0).abs() < 0.01);
    }

    #[test]
    fn text_colors_meet_aa_on_the_main_backgrounds() {
        let text = [
            ("TEXT", TEXT),
            ("BRIGHT", BRIGHT),
            ("BRIGHT2", BRIGHT2),
            ("DIM", DIM),
            ("DIM2", DIM2),
            ("EVENT", EVENT),
            ("ACCENT", ACCENT),
            ("GOLD", GOLD),
            ("RED", RED),
        ];
        for (name, fg) in text {
            for (bg_name, bg) in [("BG", BG), ("PANEL", PANEL)] {
                let ratio = contrast(fg, bg);
                assert!(ratio >= AA, "{name} on {bg_name} is {ratio:.2}:1");
            }
        }
    }

    #[test]
    fn text_drawn_on_the_active_row_meets_aa() {
        // Sidebar badges, the activity bar and selected rows sit on ACTIVE_BG.
        for (name, fg) in [
            ("TEXT", TEXT),
            ("BRIGHT", BRIGHT),
            ("DIM", DIM),
            ("DIM2", DIM2),
            ("EVENT", EVENT),
            ("GOLD", GOLD),
        ] {
            let ratio = contrast(fg, ACTIVE_BG);
            assert!(ratio >= AA, "{name} on ACTIVE_BG is {ratio:.2}:1");
        }
    }

    #[test]
    fn every_accent_theme_is_readable() {
        for name in THEME_NAMES {
            let ratio = contrast(accent_for(name), BG);
            assert!(ratio >= AA, "accent {name} is {ratio:.2}:1 on BG");
        }
    }

    #[test]
    fn nick_hues_are_readable_and_distinct() {
        for (i, hue) in NICK_HUES.iter().enumerate() {
            let ratio = contrast(*hue, BG);
            assert!(ratio >= AA, "nick hue {i} is {ratio:.2}:1 on BG");
        }
        let rgb = |c: Color| match c {
            Color::Rgb(r, g, b) => (f64::from(r), f64::from(g), f64::from(b)),
            _ => (0.0, 0.0, 0.0),
        };
        for (i, a) in NICK_HUES.iter().enumerate() {
            for (j, b) in NICK_HUES.iter().enumerate().skip(i + 1) {
                let ((ar, ag, ab), (br, bg, bb)) = (rgb(*a), rgb(*b));
                let dist = ((ar - br).powi(2) + (ag - bg).powi(2) + (ab - bb).powi(2)).sqrt();
                assert!(
                    dist >= 40.0,
                    "nick hues {i} and {j} are only {dist:.0} apart"
                );
            }
        }
    }

    #[test]
    fn faint_is_only_for_decoration() {
        // Documented, not accidental: FAINT is below text contrast by design.
        assert!(contrast(FAINT, BG) < AA);
    }
}
