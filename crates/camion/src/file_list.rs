use std::pin::Pin;

use camion_core::{Column, Entry, RemotePath, Sort};
use camion_engine::transfer::Endpoint;
use camion_engine::{Event, TransferId};
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{
    QByteArray, QHash, QHashPair_i32_QByteArray, QList, QModelIndex, QString, QVariant,
};

use crate::format;

/// The rows of the file list.
///
/// Holds the listing for whichever folder is open and nothing else: what happens when you
/// double-click is the application's business, not the model's.
#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qvariant.h");
        type QVariant = cxx_qt_lib::QVariant;
        include!("cxx-qt-lib/qmodelindex.h");
        type QModelIndex = cxx_qt_lib::QModelIndex;
        include!("cxx-qt-lib/qhash.h");
        type QHash_i32_QByteArray = cxx_qt_lib::QHash<cxx_qt_lib::QHashPair_i32_QByteArray>;
        include!("cxx-qt-lib/qlist.h");
        type QList_i32 = cxx_qt_lib::QList<i32>;
    }

    unsafe extern "C++Qt" {
        include!(<QtCore/QAbstractListModel>);

        type QAbstractListModel = crate::qt::qobject::QAbstractListModel;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[base = QAbstractListModel]
        #[qproperty(QString, path)]
        #[qproperty(bool, working)]
        #[qproperty(i32, count)]
        type FileList = super::FileListRust;

        #[qinvokable]
        #[cxx_override]
        fn data(self: &FileList, index: &QModelIndex, role: i32) -> QVariant;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &FileList, parent: &QModelIndex) -> i32;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &FileList) -> QHash_i32_QByteArray;

        /// The row a type-ahead search should land on, or -1 when nothing matches.
        #[qinvokable]
        fn find(self: &FileList, prefix: QString, after: i32) -> i32;

        #[qinvokable]
        fn name_at(self: &FileList, row: i32) -> QString;

        #[qinvokable]
        fn is_folder_at(self: &FileList, row: i32) -> bool;

        /// The icon of a row, which is what a drag of it should look like.
        #[qinvokable]
        fn icon_at(self: &FileList, row: i32) -> QString;
    }

    unsafe extern "RustQt" {
        #[inherit]
        #[cxx_name = "beginResetModel"]
        unsafe fn begin_reset_model(self: Pin<&mut FileList>);

        #[inherit]
        #[cxx_name = "endResetModel"]
        unsafe fn end_reset_model(self: Pin<&mut FileList>);

        #[inherit]
        #[cxx_name = "dataChanged"]
        unsafe fn data_changed(
            self: Pin<&mut FileList>,
            top_left: &QModelIndex,
            bottom_right: &QModelIndex,
            roles: &QList_i32,
        );

        #[inherit]
        fn index(self: &FileList, row: i32, column: i32, parent: &QModelIndex) -> QModelIndex;
    }

    impl cxx_qt::Threading for FileList {}

    impl cxx_qt::Initialize for FileList {}
}

/// `Qt::UserRole`, where roles a model defines for itself are allowed to start.
const USER_ROLE: i32 = 0x0100;

const NAME: i32 = USER_ROLE;
const SIZE: i32 = USER_ROLE + 1;
const MODIFIED: i32 = USER_ROLE + 2;
const IS_FOLDER: i32 = USER_ROLE + 3;
const PERMISSIONS: i32 = USER_ROLE + 4;
const KIND: i32 = USER_ROLE + 5;
const ICON: i32 = USER_ROLE + 6;
const UPLOADING: i32 = USER_ROLE + 7;
const FRACTION: i32 = USER_ROLE + 8;

pub struct FileListRust {
    path: QString,
    working: bool,
    count: i32,

    /// What the server last said is here.
    entries: Vec<Entry>,

    /// What is on its way here. Shown alongside the real entries so that dropping a file puts
    /// it in the list at once, with a bar that fills — rather than nothing happening until the
    /// upload finishes and the folder is listed again.
    arriving: Vec<Arriving>,

    /// The two merged, filtered, and sorted: exactly the rows on screen.
    rows: Vec<Row>,

    icons: crate::icons::Icons,
}

struct Arriving {
    transfer: TransferId,
    folder: RemotePath,
    name: String,
    transferred: u64,
    total: Option<u64>,
    /// Whether the transfer has finished. The row stays until the folder is listed again and
    /// the real entry takes its place — removing it the moment the upload ends would blink the
    /// file out of the list and back in.
    landed: bool,
}

impl Arriving {
    fn fraction(&self) -> Option<f64> {
        if self.landed {
            return None;
        }

        Some(match self.total {
            Some(total) if total > 0 => (self.transferred as f64 / total as f64).min(1.0),
            _ => 0.0,
        })
    }
}

struct Row {
    entry: Entry,
    icon: String,
    fraction: Option<f64>,
}

impl Default for FileListRust {
    fn default() -> Self {
        Self {
            path: QString::from("/"),
            working: false,
            count: 0,
            entries: Vec::new(),
            arriving: Vec::new(),
            rows: Vec::new(),
            icons: crate::icons::Icons::new(),
        }
    }
}

impl cxx_qt::Initialize for qobject::FileList {
    /// Subscribes once the object exists, and queues every update back onto the interface
    /// thread. Nothing here ever touches the engine's threads directly.
    fn initialize(self: Pin<&mut Self>) {
        let thread = self.qt_thread();

        crate::bus::listen(move |event| match event.as_ref() {
            Event::Listing { path, entries, .. } => {
                let path = path.clone();
                let entries = entries.clone();

                let _ = thread.queue(move |model| model.replace(path, entries));
            }
            Event::Working { working, .. } => {
                let working = *working;

                let _ = thread.queue(move |mut model| model.as_mut().set_working(working));
            }
            Event::Disconnected { .. } => {
                let _ = thread.queue(|model| model.replace(RemotePath::root(), Vec::new()));
            }

            // A file that has been dropped belongs on screen immediately, filling as it goes.
            Event::TransferAdded(transfer) => {
                let Endpoint::Remote { path, .. } = &transfer.to else {
                    return;
                };

                let (Some(folder), Some(name)) = (path.parent(), path.name()) else {
                    return;
                };

                let arriving = Arriving {
                    transfer: transfer.id,
                    folder,
                    name: name.to_owned(),
                    transferred: 0,
                    total: transfer.total,
                    landed: false,
                };

                let _ = thread.queue(move |model| model.expect(arriving));
            }

            Event::TransferProgress { transfer, transferred } => {
                let (id, transferred) = (*transfer, *transferred);

                let _ = thread.queue(move |model| model.advance(id, transferred));
            }

            Event::TransferFinished { transfer, .. } => {
                let id = *transfer;

                let _ = thread.queue(move |model| model.arrived(id));
            }

            _ => {}
        });

        // Icons come from the desktop's theme, so switching themes has to change them here as
        // well. Without this the window keeps drawing the icons of a theme that is gone.
        let thread = self.qt_thread();

        crate::desktop::on_theme_change(move || {
            let _ = thread.queue(|model| model.reload_icons());
        });

        // Sorting and hidden files are display settings, held once and read from here.
        let thread = self.qt_thread();

        crate::view::on_change(move || {
            let _ = thread.queue(|model| model.rebuild());
        });
    }
}

impl qobject::FileList {
    pub fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let Some(row) = self.rust().rows.get(index.row() as usize) else {
            return QVariant::default();
        };

        let text = |value: String| QVariant::from(&QString::from(&value));

        match role {
            NAME => text(row.entry.name.clone()),
            SIZE => text(match row.entry.kind.is_folder() {
                true => String::new(),
                false => format::size(row.entry.size),
            }),
            MODIFIED => text(match row.entry.modified {
                Some(modified) => format::modified(modified, time::OffsetDateTime::now_utc()),
                None => String::new(),
            }),
            IS_FOLDER => QVariant::from(&row.entry.kind.is_folder()),
            PERMISSIONS => text(match row.entry.permissions {
                Some(permissions) => permissions.to_symbolic(),
                None => String::new(),
            }),
            KIND => text(format::kind(&row.entry.name, row.entry.kind.is_folder()).to_owned()),
            ICON => text(row.icon.clone()),
            UPLOADING => QVariant::from(&row.fraction.is_some()),
            FRACTION => QVariant::from(&row.fraction.unwrap_or_default()),
            _ => QVariant::default(),
        }
    }

    pub fn row_count(&self, _parent: &QModelIndex) -> i32 {
        self.rust().rows.len() as i32
    }

    pub fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut names = QHash::<QHashPair_i32_QByteArray>::default();

        names.insert(NAME, QByteArray::from("name"));
        names.insert(SIZE, QByteArray::from("size"));
        names.insert(MODIFIED, QByteArray::from("modified"));
        names.insert(IS_FOLDER, QByteArray::from("isFolder"));
        names.insert(PERMISSIONS, QByteArray::from("permissions"));
        names.insert(KIND, QByteArray::from("kind"));
        names.insert(ICON, QByteArray::from("icon"));
        names.insert(UPLOADING, QByteArray::from("uploading"));
        names.insert(FRACTION, QByteArray::from("fraction"));

        names
    }

    pub fn find(&self, prefix: QString, after: i32) -> i32 {
        let prefix = prefix.to_string().to_lowercase();

        if prefix.is_empty() {
            return -1;
        }

        let names = self
            .rust()
            .rows
            .iter()
            .map(|row| row.entry.name.to_lowercase())
            .collect::<Vec<_>>();
        let start = (after + 1).max(0) as usize;

        // Wraps around, so holding down a letter cycles through everything starting with it.
        (start..names.len())
            .chain(0..start.min(names.len()))
            .find(|row| names[*row].starts_with(&prefix))
            .map(|row| row as i32)
            .unwrap_or(-1)
    }

    pub fn name_at(&self, row: i32) -> QString {
        QString::from(
            &self
                .rust()
                .rows
                .get(row.max(0) as usize)
                .map(|row| row.entry.name.clone())
                .unwrap_or_default(),
        )
    }

    pub fn icon_at(&self, row: i32) -> QString {
        QString::from(
            &self
                .rust()
                .rows
                .get(row.max(0) as usize)
                .map(|row| row.icon.clone())
                .unwrap_or_default(),
        )
    }

    pub fn is_folder_at(&self, row: i32) -> bool {
        self.rust()
            .rows
            .get(row.max(0) as usize)
            .is_some_and(|row| row.entry.kind.is_folder())
    }

    /// Re-reads the desktop's icon theme and redraws with it.
    fn reload_icons(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().icons = crate::icons::Icons::new();
        self.rebuild();
    }

    fn replace(mut self: Pin<&mut Self>, path: RemotePath, entries: Vec<Entry>) {
        // A fresh listing of a folder is the truth about it, so anything that was standing in
        // for a file there has done its job.
        self.as_mut()
            .rust_mut()
            .arriving
            .retain(|arriving| !(arriving.landed && arriving.folder == path));

        self.as_mut().set_path(QString::from(&path.to_string()));
        self.as_mut().rust_mut().entries = entries;
        self.rebuild();
    }

    /// Notes a file that is on its way here, so it shows up the moment it is dropped.
    fn expect(mut self: Pin<&mut Self>, arriving: Arriving) {
        self.as_mut().rust_mut().arriving.push(arriving);
        self.rebuild();
    }

    /// Moves one row's progress along.
    ///
    /// Deliberately not a rebuild: progress arrives many times a second, and resetting the
    /// model on each one would tear down every row on screen, taking the selection and the
    /// scroll position with it. Only the row that moved is touched.
    fn advance(mut self: Pin<&mut Self>, transfer: TransferId, transferred: u64) {
        let mut moved = None;

        for arriving in self.as_mut().rust_mut().arriving.iter_mut() {
            if arriving.transfer == transfer {
                arriving.transferred = transferred;
                    moved = Some((
                    arriving.folder.clone(),
                    arriving.name.clone(),
                    arriving.fraction(),
                ));
            }
        }

        let Some((folder, name, fraction)) = moved else {
            return;
        };

        let here = RemotePath::parse(&self.path().to_string()).unwrap_or_default();

        if folder != here {
            return;
        }

        let row = self
            .rust()
            .rows
            .iter()
            .position(|row| row.entry.name == name);

        let Some(row) = row else {
            return;
        };

        self.as_mut().rust_mut().rows[row].fraction = fraction;

        let at = self.index(row as i32, 0, &QModelIndex::default());
        unsafe { self.as_mut().data_changed(&at, &at, &QList::<i32>::default()) };
    }

    /// Marks a file as landed, which drops its progress bar but leaves the row in place. The
    /// listing that follows carries the real entry, with the size and time the server gave it.
    fn arrived(mut self: Pin<&mut Self>, transfer: TransferId) {
        let mut landed = false;

        for arriving in self.as_mut().rust_mut().arriving.iter_mut() {
            if arriving.transfer == transfer {
                arriving.landed = true;
                landed = true;
            }
        }

        if landed {
            self.rebuild();
        }
    }

    /// Rebuilds the rows on screen from what the server said and what is on its way.
    ///
    /// A listing arrives whole rather than row by row, so a reset says exactly what happened
    /// and avoids pretending otherwise.
    fn rebuild(mut self: Pin<&mut Self>) {
        let here = RemotePath::parse(&self.path().to_string()).unwrap_or_default();

        let settings = crate::view::current();

        let rows = merge(
            &self.rust().entries,
            &self.rust().arriving,
            &here,
            settings.show_hidden,
            Sort {
                column: column_named(&settings.sort_column),
                descending: settings.sort_descending,
            },
            &self.rust().icons,
        );

        let count = rows.len() as i32;

        unsafe { self.as_mut().begin_reset_model() };
        self.as_mut().rust_mut().rows = rows;
        unsafe { self.as_mut().end_reset_model() };

        self.as_mut().set_count(count);
    }
}

fn column_named(name: &str) -> Column {
    match name {
        "size" => Column::Size,
        "modified" => Column::Modified,
        "kind" => Column::Kind,
        _ => Column::Name,
    }
}

/// What the server said and what is on its way, as one list of rows.
///
/// A file being uploaded over one that is already here belongs on that row rather than on a
/// second row with the same name; anything genuinely new goes at the end, where new things
/// appear rather than jumping into the middle of a list you are reading.
fn merge(
    entries: &[Entry],
    arriving: &[Arriving],
    here: &RemotePath,
    showing_hidden: bool,
    sort: Sort,
    icons: &crate::icons::Icons,
) -> Vec<Row> {
    let mut entries = entries.to_vec();
    sort.apply(&mut entries);

    let landing_here = |name: &str| {
        arriving
            .iter()
            .find(|arriving| arriving.folder == *here && arriving.name == name)
    };

    let mut rows = entries
        .into_iter()
        .filter(|entry| showing_hidden || !entry.is_hidden())
        .map(|entry| Row {
            icon: icons.for_file(&entry.name, entry.kind.is_folder()),
            fraction: landing_here(&entry.name).and_then(Arriving::fraction),
            entry,
        })
        .collect::<Vec<_>>();

    for arriving in arriving {
        if arriving.folder != *here || rows.iter().any(|row| row.entry.name == arriving.name) {
            continue;
        }

        if !showing_hidden && arriving.name.starts_with('.') {
            continue;
        }

        rows.push(Row {
            entry: Entry::file(&arriving.name, arriving.total.unwrap_or_default()),
            icon: icons.for_file(&arriving.name, false),
            fraction: arriving.fraction(),
        });
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arriving(name: &str, folder: &str, transferred: u64, total: u64) -> Arriving {
        Arriving {
            transfer: TransferId(1),
            folder: RemotePath::parse(folder).unwrap(),
            name: name.to_owned(),
            transferred,
            total: Some(total),
            landed: false,
        }
    }

    fn merged(entries: &[Entry], arriving: &[Arriving], here: &str) -> Vec<Row> {
        merge(
            entries,
            arriving,
            &RemotePath::parse(here).unwrap(),
            false,
            Sort::by_name(),
            &crate::icons::Icons { themes: Vec::new(), resolved: Default::default() },
        )
    }

    #[test]
    fn a_file_on_its_way_appears_at_once_with_its_progress() {
        let rows = merged(
            &[Entry::folder("documents")],
            &[arriving("harbour.jpg", "/", 25, 100)],
            "/",
        );

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].entry.name, "harbour.jpg");
        assert_eq!(rows[1].fraction, Some(0.25));
        assert_eq!(rows[0].fraction, None);
    }

    #[test]
    fn uploading_over_a_file_that_is_already_here_stays_one_row() {
        let rows = merged(
            &[Entry::file("notes.txt", 10)],
            &[arriving("notes.txt", "/", 5, 10)],
            "/",
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].fraction, Some(0.5));
    }

    #[test]
    fn a_file_landing_in_another_folder_is_not_shown_here() {
        let rows = merged(&[], &[arriving("harbour.jpg", "/photos", 0, 100)], "/");

        assert!(rows.is_empty());
    }

    #[test]
    fn an_upload_of_unknown_size_still_gets_a_row() {
        let mut unknown = arriving("stream.bin", "/", 0, 0);
        unknown.total = None;

        let rows = merged(&[], &[unknown], "/");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].fraction, Some(0.0));
    }

    /// The row has to survive the gap between an upload finishing and the folder being listed
    /// again, or the file blinks out of the list and back in.
    #[test]
    fn a_file_that_has_landed_keeps_its_row_but_loses_its_bar() {
        let mut landed = arriving("harbour.jpg", "/", 100, 100);
        landed.landed = true;

        let rows = merged(&[], &[landed], "/");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].entry.name, "harbour.jpg");
        assert_eq!(rows[0].fraction, None);
    }

    #[test]
    fn hidden_files_stay_hidden_whether_they_are_here_or_on_their_way() {
        let rows = merged(
            &[Entry::file(".bashrc", 1), Entry::file("notes.txt", 1)],
            &[arriving(".hidden", "/", 0, 10)],
            "/",
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].entry.name, "notes.txt");
    }

    #[test]
    fn rows_keep_the_sort_the_listing_was_given() {
        let rows = merged(
            &[Entry::file("b", 1), Entry::folder("a"), Entry::file("a", 1)],
            &[],
            "/",
        );

        let names = rows.iter().map(|row| row.entry.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names, vec!["a", "a", "b"]);
        assert!(rows[0].entry.kind.is_folder());
    }
}
