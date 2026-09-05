//! The palette, as the stylesheet the window is painted from.
//!
//! Every colour in the interface comes from here, so a theme change is one provider reloading
//! its stylesheet and the whole window repainting itself. The palette is written out twice:
//! once under Okuri's own names, which the widgets in `style.css` are drawn from, and once
//! under the names Adwaita paints its stock widgets with — so an entry, a switch, a popover
//! or a file chooser follows the theme without anything in here having to know how one is
//! built.

use gtk::gdk;

use crate::palette::{Palette, readable_on};

/// Loads the palette into the display and follows the desktop's theme for as long as the
/// application runs.
///
/// Loaded below the user's own `gtk.css` on purpose. Omarchy ships one per theme that sets the
/// same Adwaita names from the same `colors.toml`, and where it says something more — square
/// corners, say — that is the theme's decision to make, not this one's to undo.
pub fn install() {
    let display = gdk::Display::default().expect("a display");
    let provider = gtk::CssProvider::new();

    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    apply(&provider, &display, &Palette::load());

    // A palette that cannot be read right now leaves the current one alone. During a theme
    // switch there is a moment with no palette on disk at all, and flashing the built-in
    // colours through that gap would be worse than waiting for the new theme to land.
    crate::relay::on_theme_change(move || {
        if let Some(palette) = Palette::current() {
            apply(&provider, &display, &palette);
        }
    })
    .forever();
}

fn apply(provider: &gtk::CssProvider, display: &gdk::Display, palette: &Palette) {
    provider.load_from_string(&stylesheet(palette));

    // Adwaita picks its shadows, borders and default text from this, and a dark palette on a
    // light scheme gets light borders on a dark window.
    adw::StyleManager::default().set_color_scheme(match palette.dark {
        true => adw::ColorScheme::ForceDark,
        false => adw::ColorScheme::ForceLight,
    });

    crate::icons::follow_the_desktop(display);
}

/// The whole stylesheet: the palette as named colours, then the rules drawn with them.
pub fn stylesheet(palette: &Palette) -> String {
    let mut css = String::new();

    for (role, color) in palette.roles() {
        css.push_str(&format!("@define-color okuri_{role} {color};\n"));
    }

    let error_text = readable_on(palette.error, palette.background, palette.bright);

    for (name, value) in [
        ("accent_bg_color", "@okuri_accent"),
        ("accent_fg_color", "@okuri_accent_text"),
        ("accent_color", "@okuri_accent"),
        ("window_bg_color", "@okuri_background"),
        ("window_fg_color", "@okuri_foreground"),
        ("view_bg_color", "@okuri_background"),
        ("view_fg_color", "@okuri_foreground"),
        ("headerbar_bg_color", "@okuri_surface"),
        ("headerbar_fg_color", "@okuri_foreground"),
        ("headerbar_backdrop_color", "@okuri_surface"),
        ("headerbar_border_color", "@okuri_border"),
        ("sidebar_bg_color", "@okuri_surface"),
        ("sidebar_fg_color", "@okuri_foreground"),
        ("sidebar_backdrop_color", "@okuri_surface"),
        ("sidebar_border_color", "@okuri_border"),
        ("card_bg_color", "@okuri_surface"),
        ("card_fg_color", "@okuri_foreground"),
        ("popover_bg_color", "@okuri_elevated"),
        ("popover_fg_color", "@okuri_foreground"),
        ("dialog_bg_color", "@okuri_elevated"),
        ("dialog_fg_color", "@okuri_foreground"),
        // The shade colours are deliberately left to Adwaita: they are translucent, and they
        // are what dims the window behind a dialog. An opaque one blacks the window out.
        ("borders", "@okuri_border"),
        ("scrollbar_outline_color", "@okuri_border"),
        ("dark_fill_bg_color", "@okuri_surface"),
        ("error_bg_color", "@okuri_error"),
        ("error_color", "@okuri_error"),
        ("destructive_bg_color", "@okuri_error"),
        ("destructive_color", "@okuri_error"),
        ("warning_bg_color", "@okuri_warning"),
        ("warning_color", "@okuri_warning"),
        ("success_bg_color", "@okuri_success"),
        ("success_color", "@okuri_success"),
    ] {
        css.push_str(&format!("@define-color {name} {value};\n"));
    }

    for name in ["error_fg_color", "destructive_fg_color", "warning_fg_color", "success_fg_color"] {
        css.push_str(&format!("@define-color {name} {error_text};\n"));
    }

    css.push_str(include_str!("style.css"));

    css
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every colour the palette has is something the stylesheet can be drawn with, under both
    /// the name Okuri uses and the one Adwaita does.
    #[test]
    fn the_stylesheet_names_every_colour_in_the_palette() {
        let palette = Palette::dark();
        let css = stylesheet(&palette);

        for (role, color) in palette.roles() {
            assert!(css.contains(&format!("@define-color okuri_{role} {color};")), "{role}");
        }

        assert!(css.contains("@define-color accent_bg_color @okuri_accent;"));
        assert!(css.contains("@define-color window_bg_color @okuri_background;"));
        assert!(css.contains("@define-color popover_bg_color @okuri_elevated;"));
    }

    /// Nothing in the rules names a colour of its own: a rule with a hex code in it is a
    /// colour that would stay put when the theme changed.
    #[test]
    fn the_rules_only_ever_refer_to_the_palette() {
        let rules = include_str!("style.css");

        assert!(!rules.contains('#'), "style.css names a colour directly");
        assert!(rules.contains("@okuri_"));
    }

    /// A pale error colour wants dark text on it, the same as a pale accent does.
    #[test]
    fn text_on_an_error_surface_is_readable() {
        let palette = Palette::from_omarchy(
            r##"
                bg = "#141010"
                fg = "#dacbe6"
                bright_fg = "#fffbd4"
                accent = "#1d52a1"
                red = "#ffe066"
            "##,
        )
        .unwrap();

        assert!(stylesheet(&palette).contains("@define-color error_fg_color #141010;"));
    }
}
