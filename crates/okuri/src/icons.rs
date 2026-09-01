use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// File icons from the desktop's own icon theme.
///
/// Okuri draws the same icons the file manager does, because a file list that invents its own
/// symbols for "a document" is a file list you have to learn. The lookup follows the XDG icon
/// theme specification, which matters more than it sounds: themes routinely define almost
/// nothing and inherit the rest, so resolving `Yaru-yellow` without walking its `Inherits` line
/// finds no mimetype icons at all.
pub struct Icons {
    themes: Vec<PathBuf>,
    resolved: Mutex<HashMap<String, String>>,
}

/// Where themes live, most specific first.
const ROOTS: [&str; 3] = ["/usr/local/share/icons", "/usr/share/icons", "/usr/share/pixmaps"];

/// The directories inside a theme worth looking in, and the sizes worth preferring. A file list
/// row is small, so a 48px icon downscales well without the blur of a 16px one blown up.
const CONTEXTS: [&str; 5] = ["mimetypes", "places", "apps", "devices", "status"];
const SIZES: [&str; 8] = ["48x48", "64x64", "32x32", "96x96", "128x128", "256x256", "24x24", "16x16"];

impl Icons {
    /// Builds the search path once: the current theme, everything it inherits, and `hicolor`,
    /// which the specification requires every theme to fall back to.
    pub fn new() -> Self {
        let mut themes = Vec::new();

        // Taken from the front, because `Inherits` is written in order of preference and a
        // stack would search the last-named theme first — which is how an icon comes back from
        // the fallback theme when the one the person chose has its own.
        let mut pending = std::collections::VecDeque::from([current_theme()]);
        let mut seen = std::collections::HashSet::new();

        while let Some(name) = pending.pop_front() {
            if !seen.insert(name.clone()) {
                continue;
            }

            for directory in theme_directories(&name) {
                pending.extend(inherited(&directory));
                themes.push(directory);
            }
        }

        themes.extend(theme_directories("hicolor"));

        Self::in_themes(themes)
    }

    /// A lookup over exactly these theme directories, in this order.
    ///
    /// The empty list is a real configuration: a machine with no icon theme installed answers
    /// nothing for everything, and the interface draws its own mark instead.
    pub fn in_themes(themes: Vec<PathBuf>) -> Self {
        Self { themes, resolved: Mutex::new(HashMap::new()) }
    }

    /// The icon for a file, as a `file://` URL, or an empty string when the theme has nothing.
    ///
    /// An empty answer is a real one: the interface falls back to its own mark rather than
    /// showing a broken image.
    pub fn for_file(&self, name: &str, is_folder: bool) -> String {
        for candidate in icon_names(name, is_folder) {
            if let Some(found) = self.find(&candidate) {
                return found;
            }
        }

        String::new()
    }

    /// Looks one icon name up, remembering the answer. A listing asks for the same handful of
    /// names over and over, and each miss is a walk through every theme on the machine.
    fn find(&self, name: &str) -> Option<String> {
        if let Some(known) = self.resolved.lock().unwrap().get(name) {
            return match known.is_empty() {
                true => None,
                false => Some(known.clone()),
            };
        }

        let found = self.search(name).unwrap_or_default();
        self.resolved
            .lock()
            .unwrap()
            .insert(name.to_owned(), found.clone());

        match found.is_empty() {
            true => None,
            false => Some(found),
        }
    }

    fn search(&self, name: &str) -> Option<String> {
        for theme in &self.themes {
            for context in CONTEXTS {
                for candidate in candidates(theme, context, name) {
                    if candidate.is_file() {
                        return Some(url_of(&candidate));
                    }
                }
            }
        }

        None
    }
}

impl Default for Icons {
    fn default() -> Self {
        Self::new()
    }
}

/// Where one icon might be inside one theme, best first.
///
/// Themes disagree about the order of the two directory levels: GNOME writes `48x48/places`,
/// KDE writes `places/48`. Both are allowed — the specification only says a theme lists its own
/// directories — so both are tried, and a theme like Breeze resolves rather than silently
/// finding nothing.
fn candidates(theme: &Path, context: &str, name: &str) -> Vec<PathBuf> {
    let mut candidates = vec![
        // Scalable first: one file that looks right at any size.
        theme.join("scalable").join(context).join(format!("{name}.svg")),
        theme.join(context).join("scalable").join(format!("{name}.svg")),
    ];

    for size in SIZES {
        for extension in ["svg", "png"] {
            candidates.push(theme.join(size).join(context).join(format!("{name}.{extension}")));
            candidates.push(
                theme
                    .join(context)
                    .join(size.split_once('x').map(|(size, _)| size).unwrap_or(size))
                    .join(format!("{name}.{extension}")),
            );
        }
    }

    candidates
}

/// The icon names to try for a file, most specific first.
fn icon_names(name: &str, is_folder: bool) -> Vec<String> {
    crate::kinds::of(name, is_folder)
        .icons
        .iter()
        .map(|name| name.to_string())
        // Every list ends the same way, so an unknown extension still gets a document rather
        // than nothing at all.
        .chain(["text-x-generic".to_owned(), "application-x-generic".to_owned()])
        .collect()
}

/// The icon theme the desktop is using.
///
/// Omarchy states it outright; otherwise GTK's settings file does. Adwaita is the last resort
/// because it is the one theme a desktop is nearly certain to have.
fn current_theme() -> String {
    omarchy_theme()
        .or_else(gtk_theme)
        .unwrap_or_else(|| "Adwaita".to_owned())
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

fn gtk_theme() -> Option<String> {
    let config = home()?.join(".config");

    ["gtk-4.0", "gtk-3.0"].iter().find_map(|version| {
        let settings = std::fs::read_to_string(config.join(version).join("settings.ini")).ok()?;

        setting(&settings, "gtk-icon-theme-name")
    })
}

fn setting(settings: &str, key: &str) -> Option<String> {
    settings.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;

        match name.trim() == key {
            true => Some(value.trim().to_owned()),
            false => None,
        }
    })
}

/// Every directory a theme of this name occupies. A theme can be split across the system and
/// the user's own directory, and both halves count.
fn theme_directories(name: &str) -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Some(home) = home() {
        roots.push(home.join(".local/share/icons"));
        roots.push(home.join(".icons"));
    }

    roots.extend(ROOTS.iter().map(PathBuf::from));

    roots
        .into_iter()
        .map(|root| root.join(name))
        .filter(|directory| directory.is_dir())
        .collect()
}

fn inherited(theme: &Path) -> Vec<String> {
    let Ok(index) = std::fs::read_to_string(theme.join("index.theme")) else {
        return Vec::new();
    };

    setting(&index, "Inherits")
        .map(|names| names.split(',').map(|name| name.trim().to_owned()).collect())
        .unwrap_or_default()
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn url_of(path: &Path) -> String {
    format!("file://{}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_matched_by_its_extension() {
        assert_eq!(icon_names("harbour.jpg", false)[0], "image-x-generic");
        assert_eq!(icon_names("talk.mp3", false)[0], "audio-x-generic");
        assert_eq!(icon_names("invoice.PDF", false)[0], "application-pdf");
        assert_eq!(icon_names("main.rs", false)[0], "text-x-script");
        assert_eq!(icon_names("site.tar.gz", false)[0], "package-x-generic");
    }

    #[test]
    fn a_folder_asks_for_a_folder() {
        assert_eq!(icon_names("documents", true)[0], "folder");
    }

    #[test]
    fn anything_unrecognised_still_gets_a_document() {
        assert_eq!(icon_names("mystery", false), vec!["text-x-generic", "application-x-generic"]);
        assert_eq!(icon_names("data.wat", false), vec!["text-x-generic", "application-x-generic"]);
    }

    #[test]
    fn every_kind_falls_back_to_something_generic() {
        for name in ["a.jpg", "b.mp4", "c.pdf", "d.zip", "e.rs", "f"] {
            let names = icon_names(name, false);

            assert_eq!(
                names.last().map(String::as_str),
                Some("application-x-generic"),
                "{name} has no last resort"
            );
        }
    }

    /// GNOME and KDE nest their directories the other way around, and a lookup that only knows
    /// one of the two orders quietly finds nothing in half the themes on a machine.
    #[test]
    fn both_directory_layouts_are_looked_in() {
        let theme = Path::new("/themes/Example");
        let paths = candidates(theme, "places", "folder")
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();

        assert!(paths.contains(&"/themes/Example/48x48/places/folder.svg".to_owned()));
        assert!(paths.contains(&"/themes/Example/places/48/folder.svg".to_owned()));
        assert!(paths.contains(&"/themes/Example/scalable/places/folder.svg".to_owned()));
        assert!(paths.contains(&"/themes/Example/places/scalable/folder.svg".to_owned()));
    }

    #[test]
    fn settings_files_are_read_by_key() {
        let settings = "[Settings]\ngtk-icon-theme-name=Papirus\ngtk-theme-name = Adwaita\n";

        assert_eq!(setting(settings, "gtk-icon-theme-name"), Some("Papirus".to_owned()));
        assert_eq!(setting(settings, "gtk-theme-name"), Some("Adwaita".to_owned()));
        assert_eq!(setting(settings, "absent"), None);
    }

    #[test]
    fn an_inherits_line_becomes_a_list_of_themes() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("index.theme"),
            "[Icon Theme]\nName=Yaru-yellow\nInherits=Yaru,Humanity,hicolor\n",
        )
        .unwrap();

        assert_eq!(inherited(directory.path()), vec!["Yaru", "Humanity", "hicolor"]);
        assert!(inherited(&directory.path().join("nothing-here")).is_empty());
    }

    /// The lookup has to survive a machine with no icon theme at all rather than showing
    /// broken images, so an empty answer is a valid one.
    #[test]
    fn a_missing_icon_is_an_empty_answer_not_a_panic() {
        let icons = Icons::in_themes(Vec::new());

        assert_eq!(icons.for_file("notes.txt", false), "");
        assert_eq!(icons.for_file("documents", true), "");
    }

    /// A listing asks for the same handful of names over and over, and every miss is a walk
    /// through every theme on the machine. Asserted by taking the file away between the two
    /// lookups: only an answer that was remembered can still be given.
    #[test]
    fn a_second_lookup_of_the_same_name_is_answered_from_memory() {
        let directory = tempfile::tempdir().unwrap();
        let mimetypes = directory.path().join("48x48/mimetypes");
        let icon = mimetypes.join("text-x-generic.png");

        std::fs::create_dir_all(&mimetypes).unwrap();
        std::fs::write(&icon, b"").unwrap();

        let icons = Icons::in_themes(vec![directory.path().to_path_buf()]);

        let first = icons.for_file("notes.txt", false);
        assert!(first.ends_with("text-x-generic.png"), "{first}");

        std::fs::remove_file(&icon).unwrap();

        assert_eq!(icons.for_file("changelog.txt", false), first);
    }
}
