//! The file list: the whole point of the application.
//!
//! A plain list — or a grid — that you can drop files onto and drive from the keyboard. The
//! shortcuts are the ones every file manager already uses, because the promise is that you do
//! not have to learn this.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib, graphene};

use crate::file_list::{FileList, Row};
use crate::view::{Mode, Settings};
use crate::window::Window;

/// What a drag of Okuri's own carries, and what its folders look for.
pub const MOVE_MIME: &str = "application/x-okuri-move";

/// What a drag from a file manager carries.
const URI_LIST: &str = "text/uri-list";

/// Long enough not to fire while passing over on the way somewhere else, short enough that
/// waiting on purpose does not feel like waiting.
const SPRING: Duration = Duration::from_millis(1200);

/// How long a pause ends one typed search and starts the next.
const TYPING: Duration = Duration::from_millis(800);

/// The columns on the right, in order: the name takes whatever is left, so turning a column
/// off widens the names rather than leaving a gap.
struct Column {
    role: &'static str,
    title: &'static str,
    width: i32,
    right: bool,
}

const COLUMNS: [Column; 4] = [
    Column { role: "size", title: "Size", width: 100, right: true },
    Column { role: "kind", title: "Type", width: 110, right: false },
    Column { role: "modified", title: "Modified", width: 120, right: true },
    Column { role: "permissions", title: "Permissions", width: 100, right: true },
];

fn showing(settings: &Settings) -> Vec<&'static Column> {
    COLUMNS
        .iter()
        .filter(|column| match column.role {
            "size" => settings.columns.size,
            "kind" => settings.columns.kind,
            "modified" => settings.columns.modified,
            _ => settings.columns.permissions,
        })
        .collect()
}

pub struct Browser {
    pub root: gtk::Box,
    header: gtk::Box,
    stack: gtk::Stack,
    list: gtk::ListView,
    grid: gtk::GridView,
    selection: gtk::MultiSelection,
    files: Rc<FileList>,
    waiting: gtk::Box,
    empty: gtk::Label,
    menu: gtk::PopoverMenu,
    typed: Rc<RefCell<Typed>>,
    weak: Weak<Window>,
}

/// The letters typed so far, which is how you find a file in a long listing without reaching
/// for the mouse.
#[derive(Default)]
struct Typed {
    text: String,
    timer: Option<glib::SourceId>,
}

impl Browser {
    pub fn new(weak: Weak<Window>, files: Rc<FileList>) -> Self {
        let selection = gtk::MultiSelection::new(Some(files.store.clone()));

        let list = gtk::ListView::new(Some(selection.clone()), None::<gtk::ListItemFactory>);
        list.add_css_class("okuri-files");
        list.set_vexpand(true);

        let grid = gtk::GridView::new(Some(selection.clone()), None::<gtk::ListItemFactory>);
        grid.add_css_class("okuri-files");
        grid.set_min_columns(1);
        grid.set_max_columns(32);
        grid.set_vexpand(true);

        for view in [list.upcast_ref::<gtk::Widget>(), grid.upcast_ref()] {
            // The folder you are looking at is somewhere to put things too. This is what
            // catches a drop onto the empty space below the rows, which otherwise landed
            // nowhere after opening a folder to put something in.
            let files = Rc::clone(&files);
            spring_loaded(view, weak.clone(), false, move || Some(files.path().to_string()));

            // Right-clicking nothing in particular offers the folder's own menu.
            let gesture = gtk::GestureClick::new();
            gesture.set_button(3);
            gesture.connect_pressed({
                let weak = weak.clone();

                move |gesture, _, x, y| {
                    let Some(window) = weak.upgrade() else {
                        return;
                    };

                    let point = gesture.widget().and_then(|view| {
                        view.compute_point(&window.browser.root, &graphene::Point::new(x as f32, y as f32))
                    });

                    window.browser.open_menu(&window, None, point);
                }
            });
            view.add_controller(gesture);
        }

        list.connect_activate({
            let weak = weak.clone();

            move |_, row| {
                if let Some(window) = weak.upgrade() {
                    window.open_row(row);
                }
            }
        });

        grid.connect_activate({
            let weak = weak.clone();

            move |_, row| {
                if let Some(window) = weak.upgrade() {
                    window.open_row(row);
                }
            }
        });

        selection.connect_selection_changed({
            let weak = weak.clone();

            move |_, _, _| {
                if let Some(window) = weak.upgrade() {
                    window.sync_selection_actions();
                }
            }
        });

        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        stack.add_named(&gtk::ScrolledWindow::builder().child(&list).build(), Some("list"));
        stack.add_named(&gtk::ScrolledWindow::builder().child(&grid).build(), Some("grid"));

        // Waiting, where whoever is waiting is looking. A listing can take seconds on a slow
        // server, and an empty window with a small mark turning in the corner reads as a
        // folder with nothing in it.
        let spinner = gtk::Spinner::new();
        spinner.set_size_request(28, 28);
        spinner.set_spinning(true);

        let loading = gtk::Label::new(Some("Loading…"));
        loading.add_css_class("okuri-muted");

        let waiting = gtk::Box::new(gtk::Orientation::Vertical, 12);
        waiting.set_halign(gtk::Align::Center);
        waiting.set_valign(gtk::Align::Center);
        waiting.set_can_target(false);
        waiting.append(&spinner);
        waiting.append(&loading);

        let empty = gtk::Label::new(Some("This folder is empty.\nDrag files here to upload them."));
        empty.set_justify(gtk::Justification::Center);
        empty.set_halign(gtk::Align::Center);
        empty.set_valign(gtk::Align::Center);
        empty.set_can_target(false);
        empty.add_css_class("okuri-muted");

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&stack));
        overlay.add_overlay(&waiting);
        overlay.add_overlay(&empty);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        header.add_css_class("okuri-header");

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("okuri-browser");
        root.append(&header);
        root.append(&overlay);

        // Files dropped from a file manager, read as the `text/uri-list` every file manager
        // offers rather than through GTK's typed deserialisers, which is the one path already
        // known to work here because Okuri's own drags take it too.
        //
        // Okuri's own drags carry addresses as well, so that a file can be dropped onto the
        // desktop; those are not for us — a drag that started in Okuri is a move, wherever it
        // started — so this declines them and the folder underneath catches the drop instead.
        //
        // Move is accepted as well as copy. A file manager offers only a move for a plain
        // drag, and a target that speaks only copy is one the compositor cancels the drop for
        // without a word. Nothing is moved either way: the file is read from the disk and the
        // disk is left alone, whatever the drag was called.
        let dropped = gtk::DropTargetAsync::new(
            Some(gdk::ContentFormats::new(&[URI_LIST])),
            gdk::DragAction::COPY | gdk::DragAction::MOVE,
        );
        dropped.connect_accept(|_, drop| {
            let formats = drop.formats();

            formats.contain_mime_type(URI_LIST) && !formats.contain_mime_type(MOVE_MIME)
        });
        dropped.connect_drag_enter(|_, drop, _, _| offered(drop));
        dropped.connect_drag_motion(|_, drop, _, _| offered(drop));
        dropped.connect_drop({
            let weak = weak.clone();

            move |_, drop, _, _| {
                let (drop, weak) = (drop.clone(), weak.clone());

                glib::spawn_future_local(async move {
                    let Some(window) = weak.upgrade() else {
                        return;
                    };

                    match read(&drop, URI_LIST).await {
                        Ok(list) => {
                            drop.finish(offered(&drop));
                            window.upload(dropped_paths(&list));
                        }
                        Err(reason) => {
                            drop.finish(gdk::DragAction::empty());
                            window.complain(format!("that drop could not be read: {reason}"));
                        }
                    }
                });

                true
            }
        });
        root.add_controller(dropped);

        let menu = gtk::PopoverMenu::from_model(None::<&gio::MenuModel>);
        menu.set_parent(&root);
        menu.set_has_arrow(false);
        menu.set_halign(gtk::Align::Start);

        // The shortcuts that only make sense with the list in front of you, and so only fire
        // while it has the focus. Anything typed into a dialog's entry is left to the entry.
        let shortcuts = gtk::ShortcutController::new();
        shortcuts.set_scope(gtk::ShortcutScope::Local);

        for (keys, action) in [
            ("BackSpace", "win.up"),
            ("Delete", "win.delete"),
            ("F2", "win.rename"),
            ("<Control>x", "win.cut"),
            ("<Control>v", "win.paste"),
        ] {
            shortcuts.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string(keys),
                Some(gtk::NamedAction::new(action)),
            ));
        }

        root.add_controller(shortcuts);

        let typed = Rc::new(RefCell::new(Typed::default()));

        // Anything else printable is treated as type-ahead.
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed({
            let weak = weak.clone();

            move |_, key, _, state| {
                if state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK) {
                    return glib::Propagation::Proceed;
                }

                let (Some(character), Some(window)) =
                    (key.to_unicode().filter(|character| !character.is_control()), weak.upgrade())
                else {
                    return glib::Propagation::Proceed;
                };

                window.browser.type_ahead(character);

                glib::Propagation::Stop
            }
        });
        root.add_controller(keys);

        Self { root, header, stack, list, grid, selection, files, waiting, empty, menu, typed, weak }
    }

    /// Redraws both views and the headings for the settings as they now are.
    pub fn apply_settings(&self, settings: &Settings) {
        self.stack.set_visible_child_name(match settings.mode {
            Mode::Grid => "grid",
            Mode::List => "list",
        });
        self.header.set_visible(settings.mode == Mode::List);

        self.list.set_factory(Some(&self.list_factory(settings)));
        self.grid.set_factory(Some(&self.grid_factory(settings)));

        self.rebuild_header(settings);
    }

    pub fn focus(&self) {
        match crate::view::current().mode {
            Mode::Grid => self.grid.grab_focus(),
            Mode::List => self.list.grab_focus(),
        };
    }

    pub fn render_waiting(&self) {
        let working = self.files.working();
        let count = self.files.count();

        self.waiting.set_visible(working && count == 0);
        self.empty.set_visible(!working && count == 0);
    }

    pub fn selected_positions(&self) -> Vec<u32> {
        let selected = self.selection.selection();

        (0..selected.size()).map(|each| selected.nth(each as u32)).collect()
    }

    pub fn selected_names(&self) -> Vec<String> {
        self.selected_positions()
            .into_iter()
            .filter_map(|row| self.files.name_at(row))
            .collect()
    }

    pub fn select_only(&self, row: u32) {
        self.selection.select_item(row, true);
    }

    /// The right-click menu.
    ///
    /// What it offers follows the connection's capabilities and the selection, so an object
    /// store that cannot rename shows the item greyed rather than failing once you click it.
    /// Right-clicking a row that is not part of the selection selects it first, the way every
    /// file manager does — otherwise the menu would act on something you cannot see.
    fn open_menu(&self, window: &Rc<Window>, row: Option<u32>, at: Option<graphene::Point>) {
        match row {
            Some(row) if !self.selection.is_selected(row) => self.select_only(row),
            Some(_) => {}
            None => {
                self.selection.unselect_all();
            }
        }

        window.sync_selection_actions();

        let rows = self.selected_positions().len();
        let one = rows == 1;

        let menu = gio::Menu::new();

        let first = gio::Menu::new();
        first.append(Some("Open"), Some("win.open"));
        first.append(
            Some(&match one {
                true => "Download…".to_owned(),
                false => format!("Download {rows} items…"),
            }),
            Some("win.download"),
        );
        menu.append_section(None, &first);

        let second = gio::Menu::new();
        second.append(Some("Rename…"), Some("win.rename"));
        second.append(
            Some(&match one {
                true => "Delete".to_owned(),
                false => format!("Delete {rows} items"),
            }),
            Some("win.delete"),
        );
        menu.append_section(None, &second);

        let third = gio::Menu::new();
        third.append(Some("New folder…"), Some("win.new-folder"));
        third.append(Some("Refresh"), Some("win.refresh"));
        menu.append_section(None, &third);

        self.menu.set_menu_model(Some(&menu));

        if let Some(at) = at {
            self.menu
                .set_pointing_to(Some(&gdk::Rectangle::new(at.x() as i32, at.y() as i32, 1, 1)));
        }

        self.menu.popup();
    }

    /// One more letter of the name being looked for.
    fn type_ahead(&self, character: char) {
        let mut typed = self.typed.borrow_mut();

        if let Some(timer) = typed.timer.take() {
            timer.remove();
        }

        typed.text.push(character);

        let text = typed.text.clone();
        let cleared = Rc::clone(&self.typed);

        typed.timer = Some(glib::timeout_add_local_once(TYPING, move || {
            let mut typed = cleared.borrow_mut();
            typed.text.clear();
            typed.timer = None;
        }));

        drop(typed);

        if let Some(found) = self.files.find(&text, None) {
            self.reveal(found);
        }
    }

    /// Selects a row and brings it into view, in whichever view is showing.
    fn reveal(&self, row: u32) {
        self.select_only(row);

        match crate::view::current().mode {
            Mode::Grid => self.grid.scroll_to(row, gtk::ListScrollFlags::FOCUS, None),
            Mode::List => self.list.scroll_to(row, gtk::ListScrollFlags::FOCUS, None),
        }
    }

    /// The column headings, which double as the sort control.
    fn rebuild_header(&self, settings: &Settings) {
        while let Some(child) = self.header.first_child() {
            self.header.remove(&child);
        }

        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_size_request(settings.list_icon(), -1);
        self.header.append(&spacer);

        let name = heading("Name", "name", false, settings);
        name.set_hexpand(true);
        self.header.append(&name);

        for column in showing(settings) {
            let button = heading(column.title, column.role, column.right, settings);
            button.set_size_request(column.width, -1);
            self.header.append(&button);
        }
    }

    /// The rows, as a factory for the list. Built afresh whenever the settings change, since
    /// which columns a row has is decided when the row is.
    fn list_factory(&self, settings: &Settings) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        let columns = showing(settings).into_iter().map(|column| column.role).collect::<Vec<_>>();

        factory.connect_setup({
            let settings = settings.clone();
            let weak = self.weak.clone();

            move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
                let cell = ListCell::new(&settings);

                attach(&cell.root, item, &weak);
                item.set_child(Some(&cell.root));
            }
        });

        factory.connect_bind({
            let settings = settings.clone();

            move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");

                let (Some(cell), Some(object)) = (
                    item.child().and_then(|child| ListCell::from_root(&child, &columns)),
                    item.item().and_downcast::<glib::BoxedAnyObject>(),
                ) else {
                    return;
                };

                cell.show(&object.borrow::<Row>(), &settings);
            }
        });

        factory
    }

    /// The cells, as a factory for the grid.
    ///
    /// The same rows, the same selection, and the same double-click as the list — only laid
    /// out for looking rather than for reading.
    fn grid_factory(&self, settings: &Settings) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();

        factory.connect_setup({
            let settings = settings.clone();
            let weak = self.weak.clone();

            move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
                let cell = GridCell::new(&settings);

                attach(&cell.root, item, &weak);
                item.set_child(Some(&cell.root));
            }
        });

        factory.connect_bind({
            let settings = settings.clone();

            move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");

                let (Some(cell), Some(object)) = (
                    item.child().and_then(|child| GridCell::from_root(&child)),
                    item.item().and_downcast::<glib::BoxedAnyObject>(),
                ) else {
                    return;
                };

                cell.show(&object.borrow::<Row>(), &settings);
            }
        });

        factory
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        // A popover is parented by hand, so it has to be unparented by hand or GTK complains
        // about a widget disposed with a child still attached.
        self.menu.unparent();
    }
}

/// One column heading. Shows which way the list is sorted when it is the one sorting it.
fn heading(title: &str, role: &'static str, right: bool, settings: &Settings) -> gtk::Button {
    let sorting = settings.sort_column == role;

    let label = gtk::Label::new(Some(title));

    // The one mark saying which column the list is ordered by, and which way.
    let arrow = gtk::Image::from_icon_name(match settings.sort_descending {
        true => "pan-down-symbolic",
        false => "pan-up-symbolic",
    });
    arrow.set_visible(sorting);

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.set_halign(match right {
        true => gtk::Align::End,
        false => gtk::Align::Start,
    });
    content.append(&label);
    content.append(&arrow);

    let button = gtk::Button::new();
    button.set_child(Some(&content));
    button.add_css_class("flat");

    // Re-sorts by this column, flipping the direction when it is already the sorted one —
    // which is what clicking a column heading is expected to do.
    button.connect_clicked(move |_| {
        crate::view::update(|settings| {
            settings.sort_descending = settings.sort_column == role && !settings.sort_descending;
            settings.sort_column = role.to_owned();
        });
    });

    button
}

/// Everything the pointer can do to a row, attached once when the row's widget is built.
///
/// The widget is reused for row after row as the list scrolls, so nothing here remembers
/// which row it is on: each gesture asks the list item at the moment it fires.
fn attach(widget: &gtk::Box, item: &gtk::ListItem, weak: &Weak<Window>) {
    // A drag is a system drag from the moment it begins. It cannot be anything else: the
    // instant the pointer leaves the window the compositor owns it, so a drag that might leave
    // has to be able to from the outset. The same drag is what the folders and breadcrumbs in
    // this window accept, by the marker it carries. One gesture, one drag, whichever side of
    // the edge it ends on.
    let source = gtk::DragSource::new();

    // Copy, and only copy. A file manager offered a move will take the original away from
    // the server, which is not what dragging a file into a folder is asking for.
    source.set_actions(gdk::DragAction::COPY);

    source.connect_prepare({
        let (item, weak) = (item.downgrade(), weak.clone());

        move |_, _, _| {
            let (Some(item), Some(window)) = (item.upgrade(), weak.upgrade()) else {
                return None;
            };

            // Picking happens as the drag begins: a drag of something not yet picked is a
            // drag of that one thing, and a drag of something picked is a drag of everything
            // picked with it.
            let row = item.position();

            if !window.browser.selection.is_selected(row) {
                window.browser.select_only(row);
            }

            let carry = window.begin_move(window.browser.selected_names())?;

            let mut providers = vec![gdk::ContentProvider::for_bytes(
                MOVE_MIME,
                &glib::Bytes::from_owned(carry.payload.into_bytes()),
            )];

            // Addresses only for destinations the desktop can open. Offering them for an
            // object store makes a drop fail instead of being refused.
            if !carry.urls.is_empty() {
                providers.push(gdk::ContentProvider::for_bytes(
                    "text/uri-list",
                    &glib::Bytes::from_owned(carry.urls.join("\r\n").into_bytes()),
                ));
            }

            Some(gdk::ContentProvider::new_union(&providers))
        }
    });

    // What the pointer carries while dragging: the file, as a chip, or how many when it is
    // several. Not the row — a row's worth of columns under the pointer hides where it is
    // going.
    source.connect_drag_begin({
        let (item, weak) = (item.downgrade(), weak.clone());

        move |_, drag| {
            let (Some(item), Some(window)) = (item.upgrade(), weak.upgrade()) else {
                return;
            };

            let selected = window.browser.selected_positions().len();
            let row = item.item().and_downcast::<glib::BoxedAnyObject>();

            let chip = match (selected, row) {
                (count, _) if count > 1 => chip(None, &format!("{count} items")),
                (_, Some(row)) => {
                    let row = row.borrow::<Row>();

                    chip(Some(&row.icon()), &row.entry.name)
                }
                _ => return,
            };

            gtk::DragIcon::for_drag(drag).set_child(Some(&chip));
            drag.set_hotspot(-8, -8);
        }
    });

    source.connect_drag_end({
        let weak = weak.clone();

        move |_, _, _| {
            if let Some(window) = weak.upgrade() {
                window.end_move();
            }
        }
    });

    widget.add_controller(source);

    let gesture = gtk::GestureClick::new();
    gesture.set_button(3);
    gesture.connect_pressed({
        let (item, weak) = (item.downgrade(), weak.clone());

        move |gesture, _, x, y| {
            let (Some(item), Some(window)) = (item.upgrade(), weak.upgrade()) else {
                return;
            };

            gesture.set_state(gtk::EventSequenceState::Claimed);

            let point = gesture.widget().and_then(|cell| {
                cell.compute_point(&window.browser.root, &graphene::Point::new(x as f32, y as f32))
            });

            window.browser.open_menu(&window, Some(item.position()), point);
        }
    });
    widget.add_controller(gesture);

    // A folder is somewhere to put things, so it accepts what is dragged onto it, lights up
    // while it is under the pointer, and opens if you hold there.
    let (item, files) = (item.downgrade(), weak.clone());

    spring_loaded(widget, weak.clone(), true, move || {
        let (item, window) = (item.upgrade()?, files.upgrade()?);
        let row = item.position();

        match window.files.is_folder_at(row) && !item.is_selected() {
            true => Some(window.files.path_of(row)?.to_string()),
            false => None,
        }
    });
}

/// What a drag looks like: an icon and a name on a pill.
fn chip(icon: Option<&gio::ThemedIcon>, text: &str) -> gtk::Box {
    let chip = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    chip.add_css_class("okuri-chip");

    if let Some(icon) = icon {
        let image = gtk::Image::from_gicon(icon);
        image.set_pixel_size(20);
        chip.append(&image);
    }

    let label = gtk::Label::new(Some(text));
    label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    label.set_max_width_chars(32);
    chip.append(&label);

    chip
}

/// A folder that opens if you hold something over it.
///
/// Dragging is a single gesture, so anywhere you might want to put a file has to be reachable
/// without letting go. Hovering opens the folder and the drag carries on inside it, which is
/// how something gets from one branch of a tree to another in one go.
///
/// `folder` answers with where a drop would go, or nothing when this is not somewhere to drop
/// right now — a file rather than a folder, or a folder that is itself being dragged. Asked
/// every time rather than once, because the widget under it is reused for row after row.
pub fn spring_loaded(
    widget: &impl IsA<gtk::Widget>,
    weak: Weak<Window>,
    opens: bool,
    folder: impl Fn() -> Option<String> + 'static,
) {
    let folder = Rc::new(folder);
    let waiting: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));

    // What the drag actually carries. A system drag is matched by its mime types, and the
    // drag that leaves this window is the same one that lands inside it.
    let target = gtk::DropTargetAsync::new(
        Some(gdk::ContentFormats::new(&[MOVE_MIME])),
        gdk::DragAction::COPY,
    );

    target.connect_accept({
        let folder = Rc::clone(&folder);

        move |_, drop| drop.formats().contain_mime_type(MOVE_MIME) && folder().is_some()
    });

    target.connect_drag_enter({
        let (folder, waiting, weak) = (Rc::clone(&folder), Rc::clone(&waiting), weak.clone());

        move |_, _, _, _| {
            if opens {
                let (folder, weak, done) = (Rc::clone(&folder), weak.clone(), Rc::clone(&waiting));

                let timer = glib::timeout_add_local_once(SPRING, move || {
                    *done.borrow_mut() = None;

                    if let (Some(folder), Some(window)) = (folder(), weak.upgrade()) {
                        window.open_path(&folder);
                    }
                });

                if let Some(earlier) = waiting.borrow_mut().replace(timer) {
                    earlier.remove();
                }
            }

            gdk::DragAction::COPY
        }
    });

    target.connect_drag_leave({
        let waiting = Rc::clone(&waiting);

        move |_, _| {
            if let Some(timer) = waiting.borrow_mut().take() {
                timer.remove();
            }
        }
    });

    // What the drop is carrying comes from the drop, not from this window: it may have been
    // picked up in another one, whose window this is not.
    target.connect_drop({
        let waiting = Rc::clone(&waiting);

        move |_, drop, _, _| {
            if let Some(timer) = waiting.borrow_mut().take() {
                timer.remove();
            }

            let Some(folder) = folder() else {
                return false;
            };

            let (drop, weak) = (drop.clone(), weak.clone());

            glib::spawn_future_local(async move {
                let Some(window) = weak.upgrade() else {
                    return;
                };

                match read(&drop, MOVE_MIME).await {
                    Ok(payload) => {
                        drop.finish(gdk::DragAction::COPY);
                        window.move_into(&payload, &folder);
                    }
                    Err(reason) => {
                        drop.finish(gdk::DragAction::empty());
                        window.complain(format!("that drop could not be read: {reason}"));
                    }
                }
            });

            true
        }
    });

    widget.add_controller(target);
}

/// The action to answer a file manager's drag with: a copy when it offers one, and its move
/// when that is all it offers.
fn offered(drop: &gdk::Drop) -> gdk::DragAction {
    match drop.actions().contains(gdk::DragAction::COPY) {
        true => gdk::DragAction::COPY,
        false => gdk::DragAction::MOVE,
    }
}

/// The local files in a `text/uri-list`. Anything that is not on this machine is left out:
/// there is nothing here to read it from.
fn dropped_paths(list: &str) -> Vec<std::path::PathBuf> {
    list.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|uri| gio::File::for_uri(uri).path())
        .collect()
}

/// Reads what a drop is carrying, in one of its formats, to the end.
async fn read(drop: &gdk::Drop, mime: &str) -> Result<String, String> {
    let (stream, _) = drop
        .read_future(&[mime], glib::Priority::DEFAULT)
        .await
        .map_err(|error| error.to_string())?;

    let mut payload = Vec::new();

    loop {
        let bytes = stream
            .read_bytes_future(64 * 1024, glib::Priority::DEFAULT)
            .await
            .map_err(|error| error.to_string())?;

        if bytes.is_empty() {
            break;
        }

        payload.extend_from_slice(&bytes);
    }

    String::from_utf8(payload).map_err(|error| error.to_string())
}

/// One line of the list, with the columns you asked for.
struct ListCell {
    root: gtk::Box,
    icon: gtk::Image,
    name: gtk::Label,
    bar: gtk::ProgressBar,
    columns: Vec<(&'static str, gtk::Label)>,
}

impl ListCell {
    fn new(settings: &Settings) -> Self {
        let icon = gtk::Image::new();
        icon.set_pixel_size(settings.list_icon());

        let name = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();

        let bar = gtk::ProgressBar::new();
        bar.add_css_class("okuri-progress");

        let middle = gtk::Box::new(gtk::Orientation::Vertical, 3);
        middle.set_hexpand(true);
        middle.set_valign(gtk::Align::Center);
        middle.append(&name);
        middle.append(&bar);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(10)
            .margin_start(14)
            .margin_end(14)
            .height_request(settings.row_height())
            .build();
        root.append(&icon);
        root.append(&middle);

        let columns = showing(settings)
            .into_iter()
            .map(|column| {
                let label = gtk::Label::builder()
                    .xalign(match column.right {
                        true => 1.0,
                        false => 0.0,
                    })
                    .width_request(column.width)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .css_classes(["okuri-muted"])
                    .build();

                root.append(&label);

                (column.role, label)
            })
            .collect();

        Self { root, icon, name, bar, columns }
    }

    /// The same widgets back, from the one the list item holds.
    fn from_root(root: &gtk::Widget, roles: &[&'static str]) -> Option<Self> {
        let root = root.downcast_ref::<gtk::Box>()?.clone();
        let icon = root.first_child()?.downcast::<gtk::Image>().ok()?;
        let middle = icon.next_sibling()?.downcast::<gtk::Box>().ok()?;
        let name = middle.first_child()?.downcast::<gtk::Label>().ok()?;
        let bar = name.next_sibling()?.downcast::<gtk::ProgressBar>().ok()?;

        let mut columns = Vec::new();
        let mut next = middle.next_sibling();

        for role in roles {
            let label = next?.downcast::<gtk::Label>().ok()?;
            next = label.next_sibling();
            columns.push((*role, label));
        }

        Some(Self { root, icon, name, bar, columns })
    }

    fn show(&self, row: &Row, settings: &Settings) {
        self.icon.set_from_gicon(&row.icon());
        self.icon.set_pixel_size(settings.list_icon());
        self.name.set_text(&row.entry.name);
        self.bar.set_visible(row.uploading());
        self.bar.set_fraction(row.fraction.unwrap_or_default());

        // Something still on its way is not there yet, and looks it.
        match row.uploading() {
            true => self.root.add_css_class("okuri-uploading"),
            false => self.root.remove_css_class("okuri-uploading"),
        }

        for (role, label) in &self.columns {
            label.set_text(match *role {
                "size" if row.uploading() => "Uploading",
                "size" => &row.size,
                "kind" => row.kind.label,
                "modified" => &row.modified,
                _ => &row.permissions,
            });
        }
    }
}

/// One cell of the grid.
struct GridCell {
    root: gtk::Box,
    icon: gtk::Image,
    name: gtk::Label,
    bar: gtk::ProgressBar,
}

impl GridCell {
    fn new(settings: &Settings) -> Self {
        let size = settings.grid_icon();

        let icon = gtk::Image::new();
        icon.set_pixel_size(size);

        let name = gtk::Label::builder()
            .justify(gtk::Justification::Center)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(2)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            // A wrapped label asks for as much width as its longest name; capped, so the
            // cells stay the size the icons set rather than the size the names do.
            .max_width_chars(1)
            .hexpand(true)
            .build();

        let bar = gtk::ProgressBar::new();
        bar.add_css_class("okuri-progress");
        bar.set_margin_start(8);
        bar.set_margin_end(8);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(4)
            .margin_end(4)
            .width_request(size + 36)
            .build();
        root.append(&icon);
        root.append(&name);
        root.append(&bar);

        Self { root, icon, name, bar }
    }

    fn from_root(root: &gtk::Widget) -> Option<Self> {
        let root = root.downcast_ref::<gtk::Box>()?.clone();
        let icon = root.first_child()?.downcast::<gtk::Image>().ok()?;
        let name = icon.next_sibling()?.downcast::<gtk::Label>().ok()?;
        let bar = name.next_sibling()?.downcast::<gtk::ProgressBar>().ok()?;

        Some(Self { root, icon, name, bar })
    }

    fn show(&self, row: &Row, settings: &Settings) {
        self.icon.set_from_gicon(&row.icon());
        self.icon.set_pixel_size(settings.grid_icon());
        self.name.set_text(&row.entry.name);
        self.bar.set_visible(row.uploading());
        self.bar.set_fraction(row.fraction.unwrap_or_default());

        match row.uploading() {
            true => self.root.add_css_class("okuri-uploading"),
            false => self.root.remove_css_class("okuri-uploading"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file manager's drag is a list of URLs, one per line, with the odd comment; only the
    /// ones on this machine are files that can be read.
    #[test]
    fn a_dropped_list_becomes_the_local_paths_in_it() {
        let list = "# dragged from a file manager\r\nfile:///home/me/notes.txt\r\nfile:///home/me/my%20file.txt\r\nsftp://elsewhere/notes.txt\r\n\r\n";

        assert_eq!(
            dropped_paths(list),
            vec![
                std::path::PathBuf::from("/home/me/notes.txt"),
                std::path::PathBuf::from("/home/me/my file.txt"),
            ]
        );
    }
}
