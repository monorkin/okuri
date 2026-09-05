//! File icons from the desktop's own icon theme.
//!
//! Okuri draws the same icons the file manager does, because a file list that invents its own
//! symbols for "a document" is a file list you have to learn. GTK does the looking up — the
//! theme, everything it inherits, and `hicolor` underneath — so what lives here is only which
//! theme to look in and which names stand for which files.

use gtk::{gdk, gio};

use crate::kinds::Kind;

/// Points GTK at the icon theme the desktop is using.
///
/// Omarchy states it outright, and is the one desktop that changes it under a running
/// application; anywhere else GTK already reads the desktop's own setting, and is left to it.
pub fn follow_the_desktop(display: &gdk::Display) {
    let Some(name) = omarchy_theme() else {
        return;
    };

    // Only a theme that is actually installed. Naming one that is not leaves GTK with
    // nothing but its handful of built-in icons, and half the toolbar turns into the
    // missing-icon mark — worse than the theme GTK would have chosen on its own.
    let theme = gtk::IconTheme::for_display(display);
    let installed = theme
        .search_path()
        .iter()
        .any(|root| root.join(&name).join("index.theme").is_file());

    if installed {
        // Through the settings rather than the icon theme itself: the display's own theme
        // refuses to be renamed directly, and follows this instead.
        gtk::Settings::for_display(display).set_gtk_icon_theme_name(Some(&name));
    }
}

/// The icon for a file, with the fallbacks the theme is allowed to answer with instead.
pub fn for_kind(kind: Kind) -> gio::ThemedIcon {
    gio::ThemedIcon::from_names(&names_for(kind))
}

/// The icon names to try for a file, most specific first.
pub fn names_for(kind: Kind) -> Vec<&'static str> {
    kind.icons
        .iter()
        .copied()
        // Every list ends the same way, so an unknown extension still gets a document rather
        // than nothing at all.
        .chain(["text-x-generic", "application-x-generic"])
        .collect()
}

fn omarchy_theme() -> Option<String> {
    let state = crate::palette::Palette::omarchy_theme_path()?;
    let named = std::fs::read_to_string(state.join("theme/icons.theme")).ok()?;
    let named = named.trim().to_owned();

    match named.is_empty() {
        true => None,
        false => Some(named),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinds;

    #[test]
    fn a_file_is_matched_by_its_extension() {
        assert_eq!(names_for(kinds::of("harbour.jpg", false))[0], "image-x-generic");
        assert_eq!(names_for(kinds::of("talk.mp3", false))[0], "audio-x-generic");
        assert_eq!(names_for(kinds::of("invoice.PDF", false))[0], "application-pdf");
        assert_eq!(names_for(kinds::of("main.rs", false))[0], "text-x-script");
        assert_eq!(names_for(kinds::of("site.tar.gz", false))[0], "package-x-generic");
    }

    #[test]
    fn a_folder_asks_for_a_folder() {
        assert_eq!(names_for(kinds::of("documents", true))[0], "folder");
    }

    #[test]
    fn anything_unrecognised_still_gets_a_document() {
        assert_eq!(
            names_for(kinds::of("mystery", false)),
            vec!["text-x-generic", "application-x-generic"]
        );
        assert_eq!(
            names_for(kinds::of("data.wat", false)),
            vec!["text-x-generic", "application-x-generic"]
        );
    }

    #[test]
    fn every_kind_falls_back_to_something_generic() {
        for name in ["a.jpg", "b.mp4", "c.pdf", "d.zip", "e.rs", "f"] {
            let names = names_for(kinds::of(name, false));

            assert_eq!(names.last().copied(), Some("application-x-generic"), "{name} has no last resort");
        }
    }
}
