use std::sync::{Mutex, OnceLock, RwLock};

use serde::{Deserialize, Serialize};

/// How the file list is displayed, remembered between sessions.
///
/// One place holds this rather than each object keeping its own copy, because the menu, the
/// list, and the column headers all need the same answer and must never disagree about it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub mode: Mode,
    /// How large icons are drawn, as a step rather than a pixel count: one control changes
    /// both views, and each view turns the step into the size that suits it.
    pub size_step: i32,
    pub columns: Columns,
    pub sort_column: String,
    pub sort_descending: bool,
    pub show_hidden: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    #[default]
    List,
    Grid,
}

impl Mode {
    pub fn name(&self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Grid => "grid",
        }
    }

    pub fn parse(name: &str) -> Self {
        match name {
            "grid" => Self::Grid,
            _ => Self::List,
        }
    }

    pub fn other(&self) -> Self {
        match self {
            Self::List => Self::Grid,
            Self::Grid => Self::List,
        }
    }
}

/// Which columns the list shows. The name is not among them: a file list without names is not
/// a file list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Columns {
    pub size: bool,
    pub kind: bool,
    pub modified: bool,
    pub permissions: bool,
}

impl Default for Columns {
    fn default() -> Self {
        Self {
            size: true,
            kind: false,
            modified: true,
            // Off by default: half the destinations have no permissions to show.
            permissions: false,
        }
    }
}

impl Columns {
    pub fn set(&mut self, name: &str, visible: bool) {
        match name {
            "size" => self.size = visible,
            "kind" => self.kind = visible,
            "modified" => self.modified = visible,
            "permissions" => self.permissions = visible,
            _ => {}
        }
    }
}

/// Five steps, each noticeably different from the last rather than a slider nobody can aim.
///
/// A grid is about looking at things and a list is about reading them, so the same step means
/// a different number of pixels in each — and a list row grows with its icon, or the icon
/// would be clipped by a row that stayed where it was.
const GRID_ICONS: [i32; 5] = [48, 64, 88, 112, 144];
const LIST_ICONS: [i32; 5] = [16, 20, 24, 32, 40];

pub const STEPS: i32 = 5;

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: Mode::default(),
            size_step: 1,
            columns: Columns::default(),
            sort_column: "name".to_owned(),
            sort_descending: false,
            show_hidden: false,
        }
    }
}

impl Settings {
    fn path() -> Option<std::path::PathBuf> {
        Some(camion_engine::config::config_home()?.join("camion/view.toml"))
    }

    fn load() -> Self {
        Self::path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|contents| toml::from_str(&contents).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        if let Err(reason) = self.write() {
            // Not fatal — the window still looks the way it was just asked to. But silently
            // forgetting it every time the application starts is the kind of thing people
            // spend an afternoon on before finding out the directory is not writable.
            crate::bus::publish(camion_engine::Event::Failed {
                message: format!("your display settings could not be saved: {reason}"),
            });
        }
    }

    fn write(&self) -> std::io::Result<()> {
        let Some(path) = Self::path() else {
            return Ok(());
        };

        let contents = toml::to_string_pretty(self)
            .map_err(|error| std::io::Error::other(error.to_string()))?;

        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)?;
        }

        std::fs::write(path, contents)
    }

    fn step(&self) -> usize {
        self.size_step.clamp(0, STEPS - 1) as usize
    }

    pub fn grid_icon(&self) -> i32 {
        GRID_ICONS[self.step()]
    }

    pub fn list_icon(&self) -> i32 {
        LIST_ICONS[self.step()]
    }

    /// Tall enough for the icon and the name, whichever needs more room.
    pub fn row_height(&self) -> i32 {
        (self.list_icon() + 12).max(30)
    }

    /// The next step up or down, stopping at the ends rather than wrapping around.
    pub fn resized(&self, by: i32) -> i32 {
        (self.size_step + by).clamp(0, STEPS - 1)
    }

    pub fn can_grow(&self) -> bool {
        self.size_step < STEPS - 1
    }

    pub fn can_shrink(&self) -> bool {
        self.size_step > 0
    }
}

fn settings() -> &'static RwLock<Settings> {
    static SETTINGS: OnceLock<RwLock<Settings>> = OnceLock::new();

    SETTINGS.get_or_init(|| RwLock::new(Settings::load()))
}

type Listener = std::sync::Arc<dyn Fn() + Send + Sync>;

fn listeners() -> &'static Mutex<Vec<Listener>> {
    static LISTENERS: OnceLock<Mutex<Vec<Listener>>> = OnceLock::new();

    LISTENERS.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn current() -> Settings {
    settings().read().unwrap().clone()
}

/// Changes a setting, writes it down, and tells everything that draws with it.
pub fn update(change: impl FnOnce(&mut Settings)) {
    {
        let mut settings = settings().write().unwrap();
        change(&mut settings);
        settings.save();
    }

    // The list is copied before anything is called, so a listener is free to add another one
    // without deadlocking on the lock it is being called under.
    let listeners = listeners().lock().unwrap().clone();

    for listener in listeners {
        listener();
    }
}

pub fn on_change(listener: impl Fn() + Send + Sync + 'static) {
    listeners().lock().unwrap().push(std::sync::Arc::new(listener));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_are_switched_by_name() {
        let mut columns = Columns::default();

        assert!(columns.size);
        assert!(!columns.kind);

        columns.set("kind", true);
        assert!(columns.kind);

        columns.set("size", false);
        assert!(!columns.size);

        // A name from an older settings file changes nothing rather than panicking.
        columns.set("nothing-like-this", true);
        assert_eq!(columns, Columns { size: false, kind: true, ..Columns::default() });
    }

    #[test]
    fn the_mode_button_offers_the_other_one() {
        assert_eq!(Mode::List.other(), Mode::Grid);
        assert_eq!(Mode::Grid.other(), Mode::List);
        assert_eq!(Mode::parse("grid"), Mode::Grid);
        assert_eq!(Mode::parse("anything else"), Mode::List);
    }

    fn at_step(size_step: i32) -> Settings {
        Settings { size_step, ..Settings::default() }
    }

    #[test]
    fn resizing_steps_through_the_sizes_and_stops_at_the_ends() {
        let smallest = at_step(0);

        assert_eq!(smallest.resized(-1), 0);
        assert_eq!(smallest.resized(1), 1);
        assert!(!smallest.can_shrink());
        assert!(smallest.can_grow());

        let largest = at_step(STEPS - 1);

        assert_eq!(largest.resized(1), STEPS - 1);
        assert!(!largest.can_grow());
        assert!(largest.can_shrink());
    }

    /// One control changes both views, so a step has to mean something in each of them.
    #[test]
    fn a_step_sizes_both_views_and_the_row_grows_with_its_icon() {
        let sizes = (0..STEPS)
            .map(at_step)
            .map(|settings| {
                (settings.grid_icon(), settings.list_icon(), settings.row_height())
            })
            .collect::<Vec<_>>();

        for pair in sizes.windows(2) {
            assert!(pair[1].0 > pair[0].0, "the grid icon did not grow");
            assert!(pair[1].1 > pair[0].1, "the list icon did not grow");
        }

        for (_, icon, row) in sizes {
            assert!(row > icon, "a row of {row} cannot hold an icon of {icon}");
        }
    }

    /// A hand-edited file should not be able to index off the end of the sizes.
    #[test]
    fn a_step_from_outside_the_range_is_brought_back_into_it() {
        assert_eq!(at_step(99).grid_icon(), GRID_ICONS[GRID_ICONS.len() - 1]);
        assert_eq!(at_step(-7).list_icon(), LIST_ICONS[0]);
    }

    #[test]
    fn settings_survive_a_round_trip_through_the_file() {
        let saved = Settings {
            mode: Mode::Grid,
            size_step: 3,
            columns: Columns { size: false, kind: true, modified: true, permissions: true },
            sort_column: "modified".to_owned(),
            sort_descending: true,
            show_hidden: true,
        };

        let written = toml::to_string_pretty(&saved).unwrap();

        assert_eq!(toml::from_str::<Settings>(&written).unwrap(), saved);
    }

    /// A settings file from an older version, or one edited by hand, should start the
    /// application rather than stop it.
    #[test]
    fn anything_missing_falls_back_to_the_default() {
        let partial: Settings = toml::from_str("mode = \"grid\"").unwrap();

        assert_eq!(partial.mode, Mode::Grid);
        assert_eq!(partial.size_step, Settings::default().size_step);
        assert_eq!(partial.sort_column, "name");
    }
}
