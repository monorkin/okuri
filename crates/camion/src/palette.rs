use std::path::{Path, PathBuf};

use crate::color::Color;

/// The colours the whole interface is painted from.
///
/// Camion follows the desktop rather than shipping a look of its own: on Omarchy the palette is
/// the current theme's, and everywhere else it is a plain dark or light set. Nothing in the UI
/// names a colour directly, so switching themes is only ever a matter of replacing this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub dark: bool,
    pub background: Color,
    pub surface: Color,
    pub elevated: Color,
    pub foreground: Color,
    pub bright: Color,
    pub muted: Color,
    pub accent: Color,
    pub accent_text: Color,
    pub selection: Color,
    /// What to write on a selected row. Worked out against the selection rather than against
    /// the accent: a theme is free to make those two different colours, and text chosen for
    /// one of them can be unreadable on the other.
    pub selection_text: Color,
    pub border: Color,
    pub error: Color,
    pub warning: Color,
    pub success: Color,
}

impl Palette {
    /// The current Omarchy theme, or a built-in palette when Omarchy is not installed.
    ///
    /// The built-in one still follows the desktop rather than always being dark: a light
    /// desktop that happens not to run Omarchy should not get a dark window.
    pub fn load() -> Self {
        Self::current().unwrap_or_else(|| match prefers_dark() {
            true => Self::dark(),
            false => Self::light(),
        })
    }

    /// The current Omarchy theme, if there is one that can be read right now.
    ///
    /// Switching themes deletes the theme directory and moves the new one into its place, so
    /// there is a moment where there is no palette at all. Telling that apart from "Omarchy
    /// isn't installed" is what stops the window flashing a stranger's colours mid-switch.
    pub fn current() -> Option<Self> {
        Self::omarchy_colors_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|contents| Self::from_omarchy(&contents))
    }

    /// Where `omarchy-theme-set` leaves the palette of the current theme.
    pub fn omarchy_colors_path() -> Option<PathBuf> {
        Some(Self::state_home()?.join("omarchy/current/theme/colors.toml"))
    }

    /// The directory to watch: the symlink target moves wholesale when the theme changes, so
    /// watching the file alone would miss the switch.
    pub fn omarchy_theme_path() -> Option<PathBuf> {
        Some(Self::state_home()?.join("omarchy/current"))
    }

    fn config_home() -> Option<PathBuf> {
        match std::env::var_os("XDG_CONFIG_HOME") {
            Some(config) if !config.is_empty() => Some(PathBuf::from(config)),
            _ => Some(Path::new(&std::env::var_os("HOME")?).join(".config")),
        }
    }

    fn state_home() -> Option<PathBuf> {
        match std::env::var_os("XDG_STATE_HOME") {
            Some(state_home) if !state_home.is_empty() => Some(PathBuf::from(state_home)),
            _ => Some(Path::new(&std::env::var_os("HOME")?).join(".local/state")),
        }
    }

    /// Reads an Omarchy `colors.toml`.
    ///
    /// Omarchy 4 themes carry semantic roles (`bg`, `fg`, `accent`) alongside the older ANSI
    /// names, and not every theme in the wild has been migrated — so each role falls back
    /// through the names that have meant the same thing over time.
    pub fn from_omarchy(contents: &str) -> Option<Self> {
        let table: toml::Table = contents.parse().ok()?;

        let color = |names: &[&str]| {
            names
                .iter()
                .filter_map(|name| table.get(*name)?.as_str())
                .find_map(Color::parse)
        };

        let background = color(&["bg", "background", "color0"])?;
        let foreground = color(&["fg", "foreground", "color7"])?;
        let accent = color(&["accent", "selection", "blue", "color4"])?;

        let dark = match table.get("mode").and_then(toml::Value::as_str) {
            Some(mode) => mode.eq_ignore_ascii_case("dark"),
            None => !background.is_light(),
        };

        Some(Self::from_roles(RoleColors {
            dark,
            background,
            foreground,
            accent,
            surface: color(&["lighter_bg", "dark_bg"]),
            elevated: color(&["dark_bg", "darker_bg"]),
            bright: color(&["bright_fg", "light_fg"]),
            muted: color(&["muted", "dark_fg", "color8"]),
            selection: color(&["selection", "selection_background"]),
            error: color(&["red", "color1"]),
            warning: color(&["yellow", "orange", "color3"]),
            success: color(&["green", "color2"]),
        }))
    }

    pub fn dark() -> Self {
        Self::from_roles(RoleColors {
            dark: true,
            background: Color::new(0x16, 0x17, 0x19),
            foreground: Color::new(0xd6, 0xd8, 0xdb),
            accent: Color::new(0x4c, 0x8e, 0xda),
            surface: Some(Color::new(0x1c, 0x1e, 0x21)),
            elevated: Some(Color::new(0x11, 0x12, 0x14)),
            bright: Some(Color::new(0xf2, 0xf4, 0xf6)),
            muted: Some(Color::new(0x86, 0x8b, 0x92)),
            selection: None,
            error: Some(Color::new(0xe0, 0x5a, 0x5a)),
            warning: Some(Color::new(0xd8, 0xa6, 0x57)),
            success: Some(Color::new(0x5f, 0xb3, 0x81)),
        })
    }

    pub fn light() -> Self {
        Self::from_roles(RoleColors {
            dark: false,
            background: Color::new(0xfa, 0xfa, 0xfa),
            foreground: Color::new(0x24, 0x27, 0x2b),
            accent: Color::new(0x2f, 0x6f, 0xb8),
            surface: Some(Color::new(0xf0, 0xf1, 0xf3)),
            elevated: Some(Color::new(0xff, 0xff, 0xff)),
            bright: Some(Color::new(0x0e, 0x10, 0x12)),
            muted: Some(Color::new(0x6b, 0x71, 0x78)),
            selection: None,
            error: Some(Color::new(0xc0, 0x39, 0x39)),
            warning: Some(Color::new(0x9a, 0x6c, 0x11)),
            success: Some(Color::new(0x2f, 0x7d, 0x4f)),
        })
    }

    /// Fills in whatever a theme left unsaid by blending what it did say.
    fn from_roles(roles: RoleColors) -> Self {
        let RoleColors { dark, background, foreground, accent, .. } = roles;
        let lift = if dark { 0.06 } else { 0.04 };

        let surface = roles.surface.unwrap_or_else(|| background.mix(foreground, lift));
        let selection = roles.selection.unwrap_or(accent);

        Self {
            dark,
            background,
            surface,
            elevated: roles.elevated.unwrap_or(surface),
            foreground,
            bright: roles.bright.unwrap_or(foreground),
            muted: roles.muted.unwrap_or_else(|| background.mix(foreground, 0.55)),
            accent,
            accent_text: readable_on(accent, background, roles.bright.unwrap_or(foreground)),
            selection,
            selection_text: readable_on(selection, background, roles.bright.unwrap_or(foreground)),
            border: background.mix(foreground, 0.18),
            error: roles.error.unwrap_or(accent),
            warning: roles.warning.unwrap_or(accent),
            success: roles.success.unwrap_or(accent),
        }
    }
}

struct RoleColors {
    dark: bool,
    background: Color,
    foreground: Color,
    accent: Color,
    surface: Option<Color>,
    elevated: Option<Color>,
    bright: Option<Color>,
    muted: Option<Color>,
    selection: Option<Color>,
    error: Option<Color>,
    warning: Option<Color>,
    success: Option<Color>,
}

/// Whether the desktop asked for a dark appearance.
///
/// GTK writes the preference into a settings file that every desktop honouring the portal
/// keeps up to date, which is readable without talking to a bus. When nothing says otherwise,
/// dark is assumed — it is the safer guess for a tool people keep open next to a terminal.
fn prefers_dark() -> bool {
    let Some(config) = Palette::config_home() else {
        return true;
    };

    ["gtk-4.0", "gtk-3.0"]
        .iter()
        .filter_map(|version| std::fs::read_to_string(config.join(version).join("settings.ini")).ok())
        .find_map(|settings| dark_from_gtk_settings(&settings))
        .unwrap_or(true)
}

fn dark_from_gtk_settings(settings: &str) -> Option<bool> {
    settings.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;

        match key.trim() == "gtk-application-prefer-dark-theme" {
            true => Some(matches!(value.trim(), "1" | "true" | "yes")),
            false => None,
        }
    })
}

/// Whichever of the two candidates stands out more against `background`.
///
/// A themed accent can be anything from navy to lemon, so the label written on top of it cannot
/// be a fixed colour. Neither candidate is the default: the one with more contrast wins, and
/// `preferred` only settles a tie.
fn readable_on(background: Color, preferred: Color, alternative: Color) -> Color {
    let contrast = |candidate: Color| (candidate.luminance() - background.luminance()).abs();

    if contrast(preferred) >= contrast(alternative) {
        preferred
    } else {
        alternative
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AWAKENING: &str = r##"
        accent = "#1d52a1"
        foreground = "#dacbe6"
        background = "#141010"
        color0 = "#141010"
        color1 = "#c92b50"
        mode = "dark"
        bg = "#141010"
        lighter_bg = "#1a1616"
        selection = "#1d52a1"
        muted = "#ab9191"
        fg = "#dacbe6"
        bright_fg = "#fffbd4"
        red = "#c92b50"
        green = "#449392"
        yellow = "#ffff00"
        dark_bg = "#0f0c0c"
    "##;

    #[test]
    fn an_omarchy_theme_maps_onto_the_roles_camion_paints_with() {
        let palette = Palette::from_omarchy(AWAKENING).unwrap();

        assert!(palette.dark);
        assert_eq!(palette.background, Color::parse("#141010").unwrap());
        assert_eq!(palette.surface, Color::parse("#1a1616").unwrap());
        assert_eq!(palette.elevated, Color::parse("#0f0c0c").unwrap());
        assert_eq!(palette.foreground, Color::parse("#dacbe6").unwrap());
        assert_eq!(palette.bright, Color::parse("#fffbd4").unwrap());
        assert_eq!(palette.muted, Color::parse("#ab9191").unwrap());
        assert_eq!(palette.accent, Color::parse("#1d52a1").unwrap());
        assert_eq!(palette.error, Color::parse("#c92b50").unwrap());
    }

    #[test]
    fn a_theme_with_only_ansi_colors_still_works() {
        let palette = Palette::from_omarchy(
            r##"
                color0 = "#101010"
                color4 = "#5577cc"
                color7 = "#e0e0e0"
            "##,
        )
        .unwrap();

        assert!(palette.dark);
        assert_eq!(palette.background, Color::parse("#101010").unwrap());
        assert_eq!(palette.accent, Color::parse("#5577cc").unwrap());
        assert_ne!(palette.surface, palette.background);
        assert_ne!(palette.border, palette.background);
    }

    #[test]
    fn a_light_theme_is_recognised_without_a_mode_line() {
        let palette = Palette::from_omarchy(
            r##"
                bg = "#fdfdfd"
                fg = "#202020"
                accent = "#2f6fb8"
            "##,
        )
        .unwrap();

        assert!(!palette.dark);
    }

    #[test]
    fn the_desktops_dark_preference_is_read_from_its_settings() {
        assert_eq!(
            dark_from_gtk_settings("[Settings]\ngtk-application-prefer-dark-theme=1\n"),
            Some(true)
        );
        assert_eq!(
            dark_from_gtk_settings("[Settings]\ngtk-application-prefer-dark-theme = false\n"),
            Some(false)
        );
        assert_eq!(dark_from_gtk_settings("[Settings]\ngtk-theme-name=Adwaita\n"), None);
        assert_eq!(dark_from_gtk_settings(""), None);
    }

    #[test]
    fn the_built_in_palettes_are_readable_in_both_directions() {
        for palette in [Palette::dark(), Palette::light()] {
            assert_ne!(palette.background, palette.foreground);
            assert_ne!(palette.surface, palette.background);
            assert_ne!(palette.border, palette.background);
            assert_eq!(palette.dark, !palette.background.is_light());
        }
    }

    #[test]
    fn nonsense_is_not_a_palette() {
        assert!(Palette::from_omarchy("this is not toml {{{").is_none());
        assert!(Palette::from_omarchy("unrelated = \"value\"").is_none());
    }

    /// A theme whose selection is nothing like its accent is the case this exists for.
    #[test]
    fn a_selected_row_is_readable_even_when_the_selection_is_not_the_accent() {
        let palette = Palette::from_omarchy(
            r##"
                bg = "#12140f"
                fg = "#c8d0be"
                bright_fg = "#ffffff"
                accent = "#ffe066"
                selection = "#1f3a1f"
            "##,
        )
        .unwrap();

        // A pale accent wants dark text on it; a dark selection wants pale text.
        assert_eq!(palette.accent_text, palette.background);
        assert_eq!(palette.selection_text, Color::parse("#ffffff").unwrap());
    }

    #[test]
    fn accent_labels_stay_readable_whatever_the_accent_is() {
        let navy = Palette::from_omarchy(
            r##"
                bg = "#141010"
                fg = "#dacbe6"
                bright_fg = "#fffbd4"
                accent = "#1d52a1"
            "##,
        )
        .unwrap();
        assert_eq!(navy.accent_text, Color::parse("#fffbd4").unwrap());

        let lemon = Palette::from_omarchy(
            r##"
                bg = "#141010"
                fg = "#dacbe6"
                bright_fg = "#fffbd4"
                accent = "#ffff00"
            "##,
        )
        .unwrap();
        assert_eq!(lemon.accent_text, Color::parse("#141010").unwrap());
    }
}
