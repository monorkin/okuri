use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// One watch on the desktop's appearance, shared by everything that follows it.
///
/// Colours and icons both change when the theme does, and they live in the same directory —
/// so there is one watch and a list of things to tell, rather than a watch per object all
/// waking up for the same event.
type Listener = Box<dyn Fn() + Send>;

fn listeners() -> &'static Mutex<Vec<Listener>> {
    static LISTENERS: OnceLock<Mutex<Vec<Listener>>> = OnceLock::new();

    LISTENERS.get_or_init(|| Mutex::new(Vec::new()))
}

fn watcher() -> &'static Mutex<Option<notify::RecommendedWatcher>> {
    static WATCHER: OnceLock<Mutex<Option<notify::RecommendedWatcher>>> = OnceLock::new();

    WATCHER.get_or_init(|| Mutex::new(None))
}

/// Long enough for a theme swap to finish, short enough that the window follows the rest of the
/// desktop rather than trailing visibly behind it.
const SETTLE: Duration = Duration::from_millis(150);

/// Calls `listener` whenever the desktop's theme changes.
///
/// `omarchy-theme-set` deletes the theme directory and moves the new one into its place, so the
/// directory above it is what gets watched — the one whose name does not change.
pub fn on_theme_change(listener: impl Fn() + Send + 'static) {
    listeners().lock().unwrap().push(Box::new(listener));

    let mut watcher = watcher().lock().unwrap();

    if watcher.is_some() {
        return;
    }

    let Some(directory) = crate::palette::Palette::omarchy_theme_path() else {
        return;
    };

    *watcher = crate::watcher::watch(&directory, SETTLE, announce);
}

fn announce() {
    for listener in listeners().lock().unwrap().iter() {
        listener();
    }
}
