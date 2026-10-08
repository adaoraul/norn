//! Colour modes: what the terminal can show, and what the user asked for.
//!
//! The interface is drawn in 24-bit colour. Terminals that cannot show that, and
//! people who do not want colour (`NO_COLOR`), get the same screen run through
//! [`apply`] once it is drawn: every cell's colours are reduced to the 256-colour
//! cube, the 16 ANSI colours, or removed. Doing it as one pass over the finished
//! buffer covers every style in the program, including the arbitrary colours of
//! mIRC-formatted text, without each place having to know about colour modes.
//!
//! Meaning that colour carries is kept some other way where it matters: selected
//! rows and highlights become reverse video, dim text becomes the terminal's
//! "dim", warnings and errors become bold (and the `!!`, `▌` and `▶` markers are
//! text anyway).

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

use super::theme;

/// How many colours to draw with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    /// 24-bit colour, as designed.
    TrueColor,
    /// The 256-colour palette.
    Ansi256,
    /// The 16 ANSI colours.
    Ansi16,
    /// No colour: the terminal's own, with bold/dim/reverse for emphasis.
    None,
}

/// The `color_mode` setting's values.
pub const COLOR_MODE_NAMES: &[&str] = &["auto", "truecolor", "256", "16", "none"];

/// The parts of the process environment that decide `auto`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorEnv {
    /// `NO_COLOR` is set to a non-empty value (<https://no-color.org>).
    pub no_color: bool,
    /// The `COLORTERM` value.
    pub colorterm: String,
    /// The `TERM` value.
    pub term: String,
}

impl Default for ColorEnv {
    /// A capable terminal: what tests and a fresh `App` assume.
    fn default() -> Self {
        ColorEnv {
            no_color: false,
            colorterm: "truecolor".to_string(),
            term: "xterm-256color".to_string(),
        }
    }
}

impl ColorEnv {
    /// Read the real environment.
    pub fn from_process() -> Self {
        ColorEnv {
            no_color: std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()),
            colorterm: std::env::var("COLORTERM").unwrap_or_default(),
            term: std::env::var("TERM").unwrap_or_default(),
        }
    }
}

/// Decide the mode from the `color_mode` setting and the environment. An explicit
/// setting wins over the environment; `auto` follows `NO_COLOR`, then
/// `COLORTERM`, then `TERM`.
pub fn resolve(setting: &str, env: &ColorEnv) -> ColorMode {
    match setting {
        "truecolor" => ColorMode::TrueColor,
        "256" => ColorMode::Ansi256,
        "16" => ColorMode::Ansi16,
        "none" => ColorMode::None,
        _ => {
            if env.no_color {
                ColorMode::None
            } else if matches!(env.colorterm.as_str(), "truecolor" | "24bit") {
                ColorMode::TrueColor
            } else if env.term == "dumb" {
                ColorMode::None
            } else if env.term.contains("256color") {
                ColorMode::Ansi256
            } else {
                ColorMode::Ansi16
            }
        }
    }
}

/// The 16 ANSI colours with the RGB values terminals usually give them, for
/// finding the nearest.
const ANSI16: [(Color, (u8, u8, u8)); 16] = [
    (Color::Black, (0, 0, 0)),
    (Color::Red, (205, 0, 0)),
    (Color::Green, (0, 205, 0)),
    (Color::Yellow, (205, 205, 0)),
    (Color::Blue, (0, 0, 238)),
    (Color::Magenta, (205, 0, 205)),
    (Color::Cyan, (0, 205, 205)),
    (Color::Gray, (229, 229, 229)),
    (Color::DarkGray, (127, 127, 127)),
    (Color::LightRed, (255, 0, 0)),
    (Color::LightGreen, (0, 255, 0)),
    (Color::LightYellow, (255, 255, 0)),
    (Color::LightBlue, (92, 92, 255)),
    (Color::LightMagenta, (255, 0, 255)),
    (Color::LightCyan, (0, 255, 255)),
    (Color::White, (255, 255, 255)),
];

/// The levels of each axis of the 256-colour 6x6x6 cube.
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn dist2(a: (u8, u8, u8), b: (u8, u8, u8)) -> i32 {
    let d = |x: u8, y: u8| (i32::from(x) - i32::from(y)).pow(2);
    d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
}

/// The nearest of the 16 ANSI colours.
fn nearest_ansi16(rgb: (u8, u8, u8)) -> Color {
    ANSI16
        .iter()
        .min_by_key(|(_, c)| dist2(rgb, *c))
        .map_or(Color::Reset, |(color, _)| *color)
}

/// The nearest colour in the 256-colour palette (the 6x6x6 cube or the 24-step
/// grey ramp, whichever is closer).
fn nearest_256(rgb: (u8, u8, u8)) -> u8 {
    let level = |v: u8| {
        CUBE.iter()
            .enumerate()
            .min_by_key(|(_, l)| (i32::from(v) - i32::from(**l)).abs())
            .map_or(0, |(i, _)| i)
    };
    let (ri, gi, bi) = (level(rgb.0), level(rgb.1), level(rgb.2));
    let cube_rgb = (CUBE[ri], CUBE[gi], CUBE[bi]);
    let cube_index = 16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8;

    // Greys: 232..=255 are 8, 18, ... 238.
    let avg = ((u32::from(rgb.0) + u32::from(rgb.1) + u32::from(rgb.2)) / 3) as i32;
    let step = (((avg - 8) as f32 / 10.0).round() as i32).clamp(0, 23);
    let grey = (8 + 10 * step) as u8;
    let grey_index = 232 + step as u8;

    if dist2(rgb, (grey, grey, grey)) < dist2(rgb, cube_rgb) {
        grey_index
    } else {
        cube_index
    }
}

/// The RGB of a 256-colour palette index.
fn indexed_rgb(index: u8) -> (u8, u8, u8) {
    match index {
        0..=15 => ANSI16[index as usize].1,
        16..=231 => {
            let i = index - 16;
            (
                CUBE[(i / 36) as usize],
                CUBE[((i / 6) % 6) as usize],
                CUBE[(i % 6) as usize],
            )
        }
        232..=255 => {
            let g = 8 + 10 * (index - 232);
            (g, g, g)
        }
    }
}

/// `color` reduced to what `mode` can show. `Reset` (the terminal default) stays.
pub fn quantize(color: Color, mode: ColorMode) -> Color {
    match (mode, color) {
        (_, Color::Reset) => Color::Reset,
        (ColorMode::None, _) => Color::Reset,
        (ColorMode::TrueColor, c) => c,
        (ColorMode::Ansi256, Color::Rgb(r, g, b)) => Color::Indexed(nearest_256((r, g, b))),
        (ColorMode::Ansi256, c) => c,
        (ColorMode::Ansi16, Color::Rgb(r, g, b)) => nearest_ansi16((r, g, b)),
        (ColorMode::Ansi16, Color::Indexed(i)) if i >= 16 => nearest_ansi16(indexed_rgb(i)),
        (ColorMode::Ansi16, Color::Indexed(i)) => ANSI16[i as usize].0,
        (ColorMode::Ansi16, c) => c,
    }
}

/// Backgrounds that mark "this is the selected / highlighted thing".
fn is_highlight_bg(c: Color) -> bool {
    c == theme::ACTIVE_BG || c == theme::BORDER_BRIGHT || c == theme::HL_BG
}

/// Backgrounds that are just the window's own paint.
fn is_paint_bg(c: Color) -> bool {
    c == theme::BG || c == theme::PANEL
}

/// Foregrounds that mean "quieter than normal text".
fn is_quiet_fg(c: Color) -> bool {
    c == theme::DIM || c == theme::DIM2 || c == theme::EVENT || c == theme::FAINT
}

/// Foregrounds that mean "look here" (warnings, errors, mentions, ops).
fn is_loud_fg(c: Color) -> bool {
    c == theme::GOLD || c == theme::RED
}

/// Run the drawn screen through the colour mode and the `paint_background`
/// choice. With `paint_background` off, the window and panel backgrounds are
/// dropped so the terminal's own shows through (highlights keep theirs).
pub fn apply(buf: &mut Buffer, mode: ColorMode, paint_background: bool) {
    if mode == ColorMode::TrueColor && paint_background {
        return; // as drawn
    }
    for cell in &mut buf.content {
        if !paint_background && is_paint_bg(cell.bg) {
            cell.bg = Color::Reset;
        }
        match mode {
            ColorMode::TrueColor => {}
            ColorMode::Ansi256 => {
                cell.fg = quantize(cell.fg, mode);
                cell.bg = quantize(cell.bg, mode);
            }
            ColorMode::Ansi16 | ColorMode::None => {
                // Dark-on-dark highlights collapse to the same colour in 16
                // colours (and to nothing without any): keep the meaning as
                // reverse video instead.
                if is_highlight_bg(cell.bg) {
                    cell.bg = Color::Reset;
                    cell.modifier.insert(Modifier::REVERSED);
                }
                if mode == ColorMode::None {
                    if is_quiet_fg(cell.fg) {
                        cell.modifier.insert(Modifier::DIM);
                    } else if is_loud_fg(cell.fg) {
                        cell.modifier.insert(Modifier::BOLD);
                    }
                }
                cell.fg = quantize(cell.fg, mode);
                cell.bg = quantize(cell.bg, mode);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(no_color: bool, colorterm: &str, term: &str) -> ColorEnv {
        ColorEnv {
            no_color,
            colorterm: colorterm.into(),
            term: term.into(),
        }
    }

    #[test]
    fn auto_follows_no_color_then_colorterm_then_term() {
        let r = |e: &ColorEnv| resolve("auto", e);
        assert_eq!(
            r(&env(true, "truecolor", "xterm-256color")),
            ColorMode::None
        );
        assert_eq!(r(&env(false, "truecolor", "xterm")), ColorMode::TrueColor);
        assert_eq!(r(&env(false, "24bit", "xterm")), ColorMode::TrueColor);
        assert_eq!(r(&env(false, "", "xterm-256color")), ColorMode::Ansi256);
        assert_eq!(r(&env(false, "", "screen")), ColorMode::Ansi16);
        assert_eq!(r(&env(false, "", "linux")), ColorMode::Ansi16);
        assert_eq!(r(&env(false, "", "dumb")), ColorMode::None);
        assert_eq!(r(&env(false, "", "")), ColorMode::Ansi16);
    }

    #[test]
    fn an_explicit_setting_overrides_the_environment() {
        let no_color = env(true, "", "dumb");
        assert_eq!(resolve("truecolor", &no_color), ColorMode::TrueColor);
        assert_eq!(resolve("256", &no_color), ColorMode::Ansi256);
        assert_eq!(resolve("16", &no_color), ColorMode::Ansi16);
        let capable = ColorEnv::default();
        assert_eq!(resolve("none", &capable), ColorMode::None);
        assert_eq!(resolve("auto", &capable), ColorMode::TrueColor);
        assert_eq!(
            resolve("nonsense", &capable),
            ColorMode::TrueColor,
            "falls back to auto"
        );
    }

    #[test]
    fn rgb_maps_to_the_nearest_256_colour() {
        // Pure colours land on the cube; greys prefer the finer grey ramp.
        assert_eq!(nearest_256((255, 0, 0)), 196);
        assert_eq!(nearest_256((0, 255, 0)), 46);
        assert_eq!(nearest_256((0, 0, 255)), 21);
        assert_eq!(nearest_256((255, 255, 255)), 231);
        assert_eq!(nearest_256((0, 0, 0)), 16);
        let grey = nearest_256((128, 128, 128));
        assert!((232..=255).contains(&grey), "{grey}");
        // The mapping is close: the chosen entry is near the colour asked for.
        for rgb in [
            (18, 20, 26),
            (170, 176, 190),
            (107, 172, 174),
            (224, 192, 96),
        ] {
            let got = indexed_rgb(nearest_256(rgb));
            assert!(dist2(rgb, got) < 40 * 40 * 3, "{rgb:?} -> {got:?}");
        }
    }

    #[test]
    fn rgb_maps_to_the_nearest_ansi_colour() {
        assert_eq!(nearest_ansi16((0, 0, 0)), Color::Black);
        assert_eq!(nearest_ansi16((250, 250, 250)), Color::White);
        assert_eq!(nearest_ansi16((255, 10, 10)), Color::LightRed);
        assert_eq!(nearest_ansi16((200, 10, 10)), Color::Red);
        assert_eq!(nearest_ansi16((10, 10, 200)), Color::Blue);
        assert_eq!(nearest_ansi16((130, 130, 130)), Color::DarkGray);
    }

    #[test]
    fn quantize_reduces_per_mode_and_keeps_the_terminal_default() {
        let c = Color::Rgb(255, 0, 0);
        assert_eq!(quantize(c, ColorMode::TrueColor), c);
        assert_eq!(quantize(c, ColorMode::Ansi256), Color::Indexed(196));
        assert_eq!(quantize(c, ColorMode::Ansi16), Color::LightRed);
        assert_eq!(quantize(c, ColorMode::None), Color::Reset);
        for mode in [
            ColorMode::TrueColor,
            ColorMode::Ansi256,
            ColorMode::Ansi16,
            ColorMode::None,
        ] {
            assert_eq!(quantize(Color::Reset, mode), Color::Reset);
        }
        // An indexed colour from elsewhere is reduced too.
        assert_eq!(
            quantize(Color::Indexed(9), ColorMode::Ansi16),
            Color::LightRed
        );
        assert_eq!(
            quantize(Color::Indexed(196), ColorMode::Ansi16),
            Color::LightRed
        );
        assert_eq!(
            quantize(Color::Indexed(196), ColorMode::Ansi256),
            Color::Indexed(196)
        );
    }

    fn buffer_with(cells: &[(Color, Color)]) -> Buffer {
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, cells.len() as u16, 1));
        for (i, (fg, bg)) in cells.iter().enumerate() {
            let cell = &mut buf[(i as u16, 0)];
            cell.set_char('x');
            cell.fg = *fg;
            cell.bg = *bg;
        }
        buf
    }

    #[test]
    fn truecolor_with_the_background_painted_changes_nothing() {
        let cells = [(theme::TEXT, theme::BG), (theme::GOLD, theme::ACTIVE_BG)];
        let mut buf = buffer_with(&cells);
        let before = buf.clone();
        apply(&mut buf, ColorMode::TrueColor, true);
        assert_eq!(buf, before);
    }

    #[test]
    fn not_painting_the_background_lets_the_terminals_show_through() {
        let mut buf = buffer_with(&[
            (theme::TEXT, theme::BG),
            (theme::TEXT, theme::PANEL),
            (theme::TEXT, theme::ACTIVE_BG),
            (theme::TEXT, theme::HL_BG),
        ]);
        apply(&mut buf, ColorMode::TrueColor, false);
        assert_eq!(buf[(0, 0)].bg, Color::Reset);
        assert_eq!(buf[(1, 0)].bg, Color::Reset);
        // Highlights keep their own colour: they are not the window's paint.
        assert_eq!(buf[(2, 0)].bg, theme::ACTIVE_BG);
        assert_eq!(buf[(3, 0)].bg, theme::HL_BG);
        assert_eq!(buf[(0, 0)].fg, theme::TEXT, "foregrounds untouched");
    }

    #[test]
    fn no_colour_leaves_no_colour_but_keeps_the_meaning() {
        let mut buf = buffer_with(&[
            (theme::TEXT, theme::BG),
            (theme::BRIGHT, theme::ACTIVE_BG), // the selected row
            (theme::GOLD, theme::HL_BG),       // a mention
            (theme::DIM2, theme::BG),          // a timestamp
            (theme::RED, theme::BG),           // an error
            (Color::Rgb(1, 2, 3), Color::Rgb(4, 5, 6)), // anything else
        ]);
        apply(&mut buf, ColorMode::None, true);
        for i in 0..6 {
            let cell = &buf[(i, 0)];
            assert_eq!(cell.fg, Color::Reset, "cell {i}");
            assert_eq!(cell.bg, Color::Reset, "cell {i}");
        }
        assert!(
            buf[(1, 0)].modifier.contains(Modifier::REVERSED),
            "selection"
        );
        assert!(
            buf[(2, 0)].modifier.contains(Modifier::REVERSED),
            "highlight"
        );
        assert!(
            buf[(2, 0)].modifier.contains(Modifier::BOLD),
            "gold is loud"
        );
        assert!(buf[(3, 0)].modifier.contains(Modifier::DIM), "quiet text");
        assert!(buf[(4, 0)].modifier.contains(Modifier::BOLD), "errors");
        assert!(buf[(0, 0)].modifier.is_empty(), "plain text stays plain");
    }

    #[test]
    fn sixteen_colours_keep_the_selection_visible_and_use_only_ansi_colours() {
        let mut buf = buffer_with(&[
            (theme::BRIGHT, theme::ACTIVE_BG),
            (theme::ACCENT, theme::PANEL),
            (Color::Rgb(255, 128, 0), Color::Reset),
        ]);
        apply(&mut buf, ColorMode::Ansi16, true);
        // The dark selection background would be black like everything else:
        // reverse video says "selected" instead.
        assert!(buf[(0, 0)].modifier.contains(Modifier::REVERSED));
        for i in 0..3 {
            for c in [buf[(i, 0)].fg, buf[(i, 0)].bg] {
                assert!(
                    !matches!(c, Color::Rgb(..) | Color::Indexed(_)),
                    "cell {i} still has {c:?}"
                );
            }
        }
    }

    #[test]
    fn two_hundred_fifty_six_colours_use_only_indexed_or_default() {
        let mut buf = buffer_with(&[
            (theme::TEXT, theme::BG),
            (theme::GOLD, theme::ACTIVE_BG),
            (Color::Reset, Color::Reset),
        ]);
        apply(&mut buf, ColorMode::Ansi256, true);
        for i in 0..3 {
            for c in [buf[(i, 0)].fg, buf[(i, 0)].bg] {
                assert!(matches!(c, Color::Indexed(_) | Color::Reset), "{c:?}");
            }
        }
        // The active row stays distinguishable from the window in 256 colours.
        assert_ne!(buf[(0, 0)].bg, buf[(1, 0)].bg);
        assert!(buf[(1, 0)].modifier.is_empty(), "no reverse video needed");
    }
}
