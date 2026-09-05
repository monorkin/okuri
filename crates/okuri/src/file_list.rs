use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gtk::prelude::*;
use gtk::{gio, glib};
use okuri_core::{Column, Entry, Permissions, RemotePath, Sort};
use okuri_engine::transfer::Endpoint;
use okuri_engine::{Event, SessionId, TransferId};

use crate::format;
use crate::kinds::Kind;
use crate::relay::Subscription;

/// The rows of the file list.
///
/// Holds the listing for whichever folder is open and nothing else: what happens when you
/// double-click is the window's business, not the model's. The rows themselves live in a
/// `ListStore` the views draw from, each one a [`Row`] with everything it shows worked out.
pub struct FileList {
    pub store: gio::ListStore,
    path: RefCell<RemotePath>,
    working: Cell<bool>,

    /// Which connection this list is showing, once one is open.
    ///
    /// Every event says which session it came from, and a list that ignored that would redraw
    /// itself from any connection's news — which is what a second list beside this one would
    /// make happen on its first listing.
    session: Cell<Option<SessionId>>,

    /// What the server last said is here.
    ///
    /// Shared with the rows rather than copied into them: sorting is done on the handles, and a
    /// folder of fifty thousand files would otherwise hold every name twice over — once here
    /// and once in the row drawing it.
    entries: RefCell<Vec<Arc<Entry>>>,

    /// What is on its way here. Shown alongside the real entries so that dropping a file puts
    /// it in the list at once, with a bar that fills — rather than nothing happening until the
    /// upload finishes and the folder is listed again.
    arriving: RefCell<Vec<Arriving>>,

    /// Who wants to know when the path, the count or the waiting changes.
    observers: RefCell<Vec<Rc<dyn Fn()>>>,

    /// The views' selection, once there is one, so a row redrawn for its progress can be
    /// picked again afterwards.
    selection: RefCell<Option<gtk::MultiSelection>>,

    /// What has been asked to go and not yet confirmed gone. Cleared by the listing that
    /// follows a deletion, or by the failure that follows one that did not happen.
    leaving: RefCell<std::collections::HashMap<String, Departure>>,
    subscriptions: RefCell<Vec<Subscription>>,
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

/// One line of the list, with everything it draws worked out already.
///
/// A row is bound to a widget every time it scrolls into view, so the columns are written when
/// the listing is built rather than re-derived every time the view passes over them.
#[derive(Clone)]
pub struct Row {
    pub entry: Arc<Entry>,
    pub kind: Kind,
    pub size: String,
    pub modified: String,
    pub permissions: String,
    pub fraction: Option<f64>,
    /// How this file has been asked to go, when the server has not yet said it has. Drawn as
    /// already half gone, so the moment between the click and the listing that confirms it
    /// is not a moment where nothing seems to have happened.
    pub leaving: Option<Departure>,
}

/// Why a file is on its way out of the folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Departure {
    Deleted,
    Moved,
}

impl Departure {
    /// What the size column says while it is happening.
    pub fn label(self) -> &'static str {
        match self {
            Self::Deleted => "Deleting",
            Self::Moved => "Moving",
        }
    }
}

impl Row {
    fn new(fraction: Option<f64>, now: time::OffsetDateTime, entry: Arc<Entry>) -> Self {
        Self {
            kind: crate::kinds::of(&entry.name, entry.kind.is_folder()),
            size: match entry.kind.is_folder() {
                true => String::new(),
                false => format::size(entry.size),
            },
            modified: match entry.modified {
                Some(modified) => format::modified(format::local(modified), now),
                None => String::new(),
            },
            permissions: match entry.permissions {
                Some(permissions) => permissions.to_symbolic(),
                None => String::new(),
            },
            fraction,
            leaving: None,
            entry,
        }
    }

    /// Whether the row is on its way in or out, and so drawn as not quite there.
    pub fn pending(&self) -> bool {
        self.uploading() || self.leaving.is_some()
    }

    pub fn is_folder(&self) -> bool {
        self.entry.kind.is_folder()
    }

    pub fn uploading(&self) -> bool {
        self.fraction.is_some()
    }

    pub fn icon(&self) -> gio::ThemedIcon {
        crate::icons::for_kind(self.kind)
    }
}

/// Everything known about one row, for showing it on its own.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub name: String,
    pub kind: &'static str,
    pub size: String,
    pub modified: String,
    pub permissions: String,
    pub is_folder: bool,
    /// The mode, for spelling out as nine answers rather than nine characters.
    pub mode: Option<Permissions>,
}

impl Facts {
    pub fn icon(&self) -> gio::ThemedIcon {
        crate::icons::for_kind(crate::kinds::of(&self.name, self.is_folder))
    }
}

impl FileList {
    /// Subscribes once the list exists, and hears every update on the interface thread.
    pub fn new() -> Rc<Self> {
        let list = Rc::new(Self {
            store: gio::ListStore::new::<glib::BoxedAnyObject>(),
            path: RefCell::new(RemotePath::root()),
            working: Cell::new(false),
            session: Cell::new(None),
            entries: RefCell::new(Vec::new()),
            arriving: RefCell::new(Vec::new()),
            observers: RefCell::new(Vec::new()),
            selection: RefCell::new(None),
            leaving: RefCell::new(std::collections::HashMap::new()),
            subscriptions: RefCell::new(Vec::new()),
        });

        // Every event carries the session it came from, and a list beside this one has to
        // ignore the other's news rather than redraw itself with it — so the filtering happens
        // here, where the answer to "which connection am I" can actually be read.
        let weak = Rc::downgrade(&list);
        let news = crate::relay::on_event(move |event| {
            if let Some(list) = weak.upgrade() {
                list.receive(event);
            }
        });

        // Sorting and hidden files are display settings, held once and read from here.
        let weak = Rc::downgrade(&list);
        let redraw = crate::relay::on_view_change(move || {
            if let Some(list) = weak.upgrade() {
                list.rebuild();
            }
        });

        list.subscriptions.borrow_mut().extend([news, redraw]);

        list
    }

    /// Calls `observer` whenever the path, the count, or the waiting changes.
    pub fn on_change(&self, observer: impl Fn() + 'static) {
        self.observers.borrow_mut().push(Rc::new(observer));
    }

    /// Tells the list which selection the views share, so a redrawn row stays picked.
    pub fn follow_selection(&self, selection: &gtk::MultiSelection) {
        *self.selection.borrow_mut() = Some(selection.clone());
    }

    /// Notes that these files have been asked to go, so they look it until they are.
    pub fn expect_departure(&self, names: &[String], how: Departure) {
        self.leaving
            .borrow_mut()
            .extend(names.iter().map(|name| (name.clone(), how)));
        self.rebuild();
    }

    pub fn path(&self) -> RemotePath {
        self.path.borrow().clone()
    }

    pub fn working(&self) -> bool {
        self.working.get()
    }

    pub fn count(&self) -> u32 {
        self.store.n_items()
    }

    /// Starts showing a connection, and stops showing whatever was there before.
    ///
    /// Told which one rather than working it out from whichever connection opened last. Every
    /// window hears every connection open, so a list that adopted the newest would swap to the
    /// server the window next door had just reached.
    pub fn follow(&self, session: Option<SessionId>) {
        if self.session.get() == session {
            return;
        }

        self.session.set(session);
        self.empty();
    }

    /// The row a type-ahead search should land on, or nothing when nothing matches.
    pub fn find(&self, prefix: &str, after: Option<u32>) -> Option<u32> {
        let prefix = prefix.to_lowercase();

        if prefix.is_empty() {
            return None;
        }

        let names = (0..self.count())
            .map(|row| self.with_row(row, |row| row.entry.name.to_lowercase()).unwrap_or_default())
            .collect::<Vec<_>>();

        // Wraps around, so holding down a letter cycles through everything starting with it.
        let start = after.map(|row| row as usize + 1).unwrap_or(0).min(names.len());

        (start..names.len())
            .chain(0..start)
            .find(|row| names[*row].starts_with(&prefix))
            .map(|row| row as u32)
    }

    pub fn name_at(&self, row: u32) -> Option<String> {
        self.with_row(row, |row| row.entry.name.clone())
    }

    pub fn is_folder_at(&self, row: u32) -> bool {
        self.with_row(row, Row::is_folder).unwrap_or(false)
    }

    /// The full path of a row, which is what a drop onto it means.
    pub fn path_of(&self, row: u32) -> Option<RemotePath> {
        let name = self.name_at(row)?;

        self.path.borrow().join(&name).ok()
    }

    pub fn facts_at(&self, row: u32) -> Option<Facts> {
        self.with_row(row, |row| Facts {
            name: row.entry.name.clone(),
            kind: row.kind.label,
            size: row.size.clone(),
            modified: row.modified.clone(),
            permissions: row.permissions.clone(),
            is_folder: row.is_folder(),
            mode: row.entry.permissions,
        })
    }

    /// Reads one row, if there is one.
    pub fn with_row<T>(&self, row: u32, read: impl FnOnce(&Row) -> T) -> Option<T> {
        let object = self.store.item(row)?.downcast::<glib::BoxedAnyObject>().ok()?;
        let row = object.borrow::<Row>();

        Some(read(&row))
    }

    fn receive(&self, event: &Event) {
        match event {
            Event::Listing { session, path, entries } => {
                self.replace(*session, path.clone(), entries.clone());
            }

            Event::Working { session, working } => self.set_busy(*session, *working),

            Event::Disconnected { session } => self.close(*session),

            // A file that has been dropped belongs on screen immediately, filling as it goes.
            Event::TransferAdded(transfer) => {
                let Endpoint::Remote { path, .. } = &transfer.to else {
                    return;
                };

                let (Some(folder), Some(name)) = (path.parent(), path.name()) else {
                    return;
                };

                let Some(session) = transfer.session() else {
                    return;
                };

                self.expect(
                    session,
                    Arriving {
                        transfer: transfer.id,
                        folder,
                        name: name.to_owned(),
                        transferred: 0,
                        total: transfer.total,
                        landed: false,
                    },
                );
            }

            // Progress and completion carry no session, and need none: they can only ever name
            // a transfer this list already agreed to show.
            Event::TransferProgress { transfer, transferred } => {
                self.advance(*transfer, *transferred);
            }

            Event::TransferFinished { transfer, .. } => self.arrived(*transfer),

            // A deletion that did not happen leaves the files where they were, so they stop
            // looking as though they were going.
            Event::Failed { concern: okuri_engine::Concern::Session(session), .. }
                if self.showing(*session) && !self.leaving.borrow().is_empty() =>
            {
                self.leaving.borrow_mut().clear();
                self.rebuild();
            }

            _ => {}
        }
    }

    /// Whether news from `session` is news about what this list is showing.
    fn showing(&self, session: SessionId) -> bool {
        self.session.get() == Some(session)
    }

    /// Puts the list back to an empty root.
    ///
    /// What is on screen belongs to whichever connection was open before, and leaving it there
    /// means a moment where a new server appears to hold another one's files — and a moment is
    /// long enough to click something.
    fn empty(&self) {
        self.arriving.borrow_mut().clear();
        self.entries.borrow_mut().clear();
        *self.path.borrow_mut() = RemotePath::root();
        self.rebuild();
    }

    fn close(&self, session: SessionId) {
        if self.showing(session) {
            self.session.set(None);
            self.empty();
        }
    }

    fn set_busy(&self, session: SessionId, working: bool) {
        if self.showing(session) {
            self.working.set(working);
            self.announce();
        }
    }

    fn replace(&self, session: SessionId, path: RemotePath, entries: Vec<Entry>) {
        if !self.showing(session) {
            return;
        }

        // A fresh listing of a folder is the truth about it, so anything that was standing in
        // for a file there has done its job.
        self.arriving
            .borrow_mut()
            .retain(|arriving| !(arriving.landed && arriving.folder == path));

        *self.path.borrow_mut() = path;
        *self.entries.borrow_mut() = entries.into_iter().map(Arc::new).collect();
        self.leaving.borrow_mut().clear();
        self.rebuild();
    }

    /// Notes a file that is on its way here, so it shows up the moment it is dropped.
    fn expect(&self, session: SessionId, arriving: Arriving) {
        if !self.showing(session) {
            return;
        }

        self.arriving.borrow_mut().push(arriving);
        self.rebuild();
    }

    /// Moves one row's progress along.
    ///
    /// Deliberately not a rebuild: progress arrives many times a second, and replacing every
    /// row on each one would take the selection and the scroll position with it. Only the row
    /// that moved is touched.
    fn advance(&self, transfer: TransferId, transferred: u64) {
        let mut moved = None;

        for arriving in self.arriving.borrow_mut().iter_mut() {
            if arriving.transfer == transfer {
                arriving.transferred = transferred;
                moved = Some((arriving.folder.clone(), arriving.name.clone(), arriving.fraction()));
            }
        }

        let Some((folder, name, fraction)) = moved else {
            return;
        };

        if folder == *self.path.borrow() {
            self.redraw(&name, fraction);
        }
    }

    /// Marks a file as landed, which drops its progress bar but leaves the row in place. The
    /// listing that follows carries the real entry, with the size and time the server gave it.
    fn arrived(&self, transfer: TransferId) {
        let mut landed = None;

        for arriving in self.arriving.borrow_mut().iter_mut() {
            if arriving.transfer == transfer {
                arriving.landed = true;
                landed = Some((arriving.folder.clone(), arriving.name.clone()));
            }
        }

        let Some((folder, name)) = landed else {
            return;
        };

        if folder == *self.path.borrow() {
            self.redraw(&name, None);
        }
    }

    /// Changes one row's progress and has the view draw that row again.
    ///
    /// A fresh object goes in rather than the same one changed in place: a list view that is
    /// told a row changed keeps the widget it already has when the object is the one it was
    /// built for, so the change never reaches the screen. The selection follows the object,
    /// so a row that was picked is picked again once the swap is done.
    fn redraw(&self, name: &str, fraction: Option<f64>) {
        let position = (0..self.count())
            .find(|row| self.with_row(*row, |row| row.entry.name == name).unwrap_or(false));

        let Some(position) = position else {
            return;
        };

        let Some(mut row) = self.with_row(position, Row::clone) else {
            return;
        };

        row.fraction = fraction;

        let selection = self.selection.borrow().clone();
        let picked = selection.as_ref().is_some_and(|selection| selection.is_selected(position));

        self.store.splice(position, 1, &[glib::BoxedAnyObject::new(row)]);

        if picked && let Some(selection) = selection {
            selection.select_item(position, false);
        }
    }

    /// Rebuilds the rows on screen from what the server said and what is on its way.
    ///
    /// A listing arrives whole rather than row by row, so the store is replaced whole and
    /// avoids pretending otherwise.
    fn rebuild(&self) {
        let settings = crate::view::current();

        let mut rows = merge(
            &self.entries.borrow(),
            &self.arriving.borrow(),
            &self.path.borrow(),
            settings.show_hidden,
            Sort {
                column: column_named(&settings.sort_column),
                descending: settings.sort_descending,
            },
        );

        let leaving = self.leaving.borrow();

        for row in &mut rows {
            row.leaving = leaving.get(&row.entry.name).copied();
        }

        let objects = rows.into_iter().map(glib::BoxedAnyObject::new).collect::<Vec<_>>();

        self.store.splice(0, self.store.n_items(), &objects);
        self.announce();
    }

    fn announce(&self) {
        // The list is copied before anything is called, so an observer is free to add another
        // without tripping over the borrow it is being called under.
        let observers = self.observers.borrow().clone();

        for observer in observers {
            observer();
        }
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
    entries: &[Arc<Entry>],
    arriving: &[Arriving],
    here: &RemotePath,
    showing_hidden: bool,
    sort: Sort,
) -> Vec<Row> {
    // Cloned handles, not files: this is a list of pointers being sorted.
    let mut entries = entries.to_vec();
    sort.apply_to(&mut entries);

    // One clock reading for the whole listing, so two files saved a second apart do not end up
    // described relative to different "now"s.
    let now = format::now();

    let landing_here = |name: &str| {
        arriving
            .iter()
            .find(|arriving| arriving.folder == *here && arriving.name == name)
    };

    let mut rows = entries
        .into_iter()
        .filter(|entry| showing_hidden || !entry.is_hidden())
        .map(|entry| Row::new(landing_here(&entry.name).and_then(Arriving::fraction), now, entry))
        .collect::<Vec<_>>();

    for arriving in arriving {
        if arriving.folder != *here || rows.iter().any(|row| row.entry.name == arriving.name) {
            continue;
        }

        if !showing_hidden && arriving.name.starts_with('.') {
            continue;
        }

        rows.push(Row::new(
            arriving.fraction(),
            now,
            Arc::new(Entry::file(&arriving.name, arriving.total.unwrap_or_default())),
        ));
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
        let entries = entries.iter().cloned().map(Arc::new).collect::<Vec<_>>();

        merge(&entries, arriving, &RemotePath::parse(here).unwrap(), false, Sort::by_name())
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
        assert!(rows[0].is_folder());
    }

    /// The columns are worked out once, when the row is built, so the view reads them rather
    /// than working them out on every pass.
    #[test]
    fn a_row_carries_its_columns_ready_to_draw() {
        let rows = merged(&[Entry::file("invoice.pdf", 250_000)], &[], "/");

        assert_eq!(rows[0].kind.label, "PDF document");
        assert_eq!(rows[0].size, "244 KB");
        assert!(!rows[0].uploading());
    }
}
