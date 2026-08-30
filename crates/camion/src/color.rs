use std::fmt;

/// An opaque colour, parsed from the hex strings themes are written in.
///
/// Camion only ever needs to read a palette and blend between two of its colours — separators
/// and hover states are derived rather than declared, so a theme that only defines a background
/// and a foreground still gets a coherent set of surfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Color {
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    /// Accepts `#rgb`, `#rrggbb`, and `#rrggbbaa`. Alpha is dropped: every surface Camion
    /// paints is opaque, and a theme's alpha is about window compositing, not about us.
    pub fn parse(input: &str) -> Option<Self> {
        let digits = input.trim().trim_start_matches('#');

        // Counted and sliced in bytes below, which only lines up with characters while every
        // one of them is ASCII — and a hex digit always is.
        if !digits.is_ascii() {
            return None;
        }

        let expand = |digit: &str| u8::from_str_radix(&digit.repeat(2), 16).ok();
        let byte = |at: usize| u8::from_str_radix(digits.get(at..at + 2)?, 16).ok();

        match digits.len() {
            3 => Some(Self::new(
                expand(&digits[0..1])?,
                expand(&digits[1..2])?,
                expand(&digits[2..3])?,
            )),
            6 | 8 => Some(Self::new(byte(0)?, byte(2)?, byte(4)?)),
            _ => None,
        }
    }

    /// Blends towards `other`, where `amount` is how much of `other` ends up in the result.
    pub fn mix(&self, other: Self, amount: f32) -> Self {
        let blend = |from: u8, to: u8| {
            (f32::from(from) + (f32::from(to) - f32::from(from)) * amount).round() as u8
        };

        Self::new(
            blend(self.red, other.red),
            blend(self.green, other.green),
            blend(self.blue, other.blue),
        )
    }

    /// Perceived brightness, used to decide whether a palette is a light one when the theme
    /// does not say so itself.
    pub fn luminance(&self) -> f32 {
        let channel = |value: u8| f32::from(value) / 255.0;

        0.2126 * channel(self.red) + 0.7152 * channel(self.green) + 0.0722 * channel(self.blue)
    }

    pub fn is_light(&self) -> bool {
        self.luminance() > 0.5
    }
}

impl fmt::Display for Color {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "#{:02x}{:02x}{:02x}", self.red, self.green, self.blue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_that_is_not_hex_is_not_a_colour() {
        assert_eq!(Color::parse("#nope"), None);
        assert_eq!(Color::parse(""), None);

        // Three bytes, but not three characters — slicing this one apart used to panic.
        assert_eq!(Color::parse("#é1"), None);
    }

    #[test]
    fn hex_is_parsed_in_every_length_themes_use() {
        assert_eq!(Color::parse("#1d52a1"), Some(Color::new(0x1d, 0x52, 0xa1)));
        assert_eq!(Color::parse("1d52a1"), Some(Color::new(0x1d, 0x52, 0xa1)));
        assert_eq!(Color::parse("#f0c"), Some(Color::new(0xff, 0x00, 0xcc)));
        assert_eq!(Color::parse("#1d52a1ff"), Some(Color::new(0x1d, 0x52, 0xa1)));
        assert_eq!(Color::parse("not a colour"), None);
        assert_eq!(Color::parse("#12345"), None);
    }

    #[test]
    fn colors_render_back_to_hex() {
        assert_eq!(Color::new(0x14, 0x10, 0x10).to_string(), "#141010");
    }

    #[test]
    fn mixing_walks_from_one_colour_to_the_other() {
        let black = Color::new(0, 0, 0);
        let white = Color::new(255, 255, 255);

        assert_eq!(black.mix(white, 0.0), black);
        assert_eq!(black.mix(white, 1.0), white);
        assert_eq!(black.mix(white, 0.5), Color::new(128, 128, 128));
    }

    #[test]
    fn lightness_tells_a_dark_theme_from_a_light_one() {
        assert!(!Color::new(0x14, 0x10, 0x10).is_light());
        assert!(Color::new(0xff, 0xfb, 0xd4).is_light());
    }
}
