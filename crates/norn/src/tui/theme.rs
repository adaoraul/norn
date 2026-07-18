//! Colors for the TUI, matching the design mockup's muted-dark palette.

use ratatui::style::Color;

/// Window background.
pub const BG: Color = Color::Rgb(0x12, 0x14, 0x1a);
/// Sidebar / nicklist background.
pub const PANEL: Color = Color::Rgb(0x0e, 0x10, 0x15);
/// Active row background.
pub const ACTIVE_BG: Color = Color::Rgb(0x1c, 0x21, 0x30);
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
/// Dimmer text (timestamps, hints).
pub const DIM2: Color = Color::Rgb(0x5c, 0x63, 0x70);
/// Event-line text.
pub const EVENT: Color = Color::Rgb(0x7d, 0x84, 0x94);
/// Dimmest (separators).
pub const FAINT: Color = Color::Rgb(0x3d, 0x42, 0x50);

/// Default accent (teal).
pub const ACCENT: Color = Color::Rgb(0x6b, 0xac, 0xae);
/// Op (`@`) color, also the >2 unread badge and highlight text.
pub const GOLD: Color = Color::Rgb(0xe0, 0xc0, 0x60);
/// Highlight background for own-nick mentions.
pub const HL_BG: Color = Color::Rgb(0x3a, 0x33, 0x20);

/// The muted hue set used to color nicks deterministically.
const NICK_HUES: &[Color] = &[
    Color::Rgb(0x8f, 0xae, 0x6b), // green
    Color::Rgb(0xc7, 0x8f, 0xd6), // purple
    Color::Rgb(0x6b, 0xac, 0xae), // teal
    Color::Rgb(0xd0, 0x98, 0x5f), // orange
    Color::Rgb(0x6b, 0x8f, 0xae), // blue
    Color::Rgb(0xae, 0x6b, 0x8f), // pink
    Color::Rgb(0x9f, 0xae, 0x6b), // olive
    Color::Rgb(0xae, 0x9f, 0x6b), // tan
    Color::Rgb(0x6b, 0xae, 0x9f), // aqua
    Color::Rgb(0x8f, 0x6b, 0xae), // violet
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
