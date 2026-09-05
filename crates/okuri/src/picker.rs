//! What you see before you are connected: everything you have saved, and a way to add more.

use std::cell::{Cell, RefCell};
use std::rc::Weak;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

use crate::window::Window;

/// The line under the title, which changes every few seconds.
///
/// The same shape as Omarchy's tailscale and network panels: a fixed list, an index walking
/// it, and a fade either side of the swap so the words are never caught mid-change.
const PHRASES: [&str; 11] = [
    "Means “sending” in Japanese",
    "Carrying files",
    "Wrapping parcels",
    "Stacking crates",
    "Reading addresses",
    "Sealing boxes",
    "Weighing packages",
    "Sorting the post",
    "Tying string",
    "Stamping labels",
    "Loading the cart",
];

const EVERY: Duration = Duration::from_millis(2800);
const FADE: Duration = Duration::from_millis(180);

pub struct Picker {
    pub root: gtk::Box,
    list: gtk::ListBox,
    frame: gtk::Box,
    empty: gtk::Label,
    subtitle: gtk::Label,
    rows: RefCell<Vec<Saved>>,
    connecting_to: RefCell<String>,
    phrase: Cell<usize>,
    weak: Weak<Window>,
}

/// One saved connection on screen, and the mark that shows it being opened.
struct Saved {
    id: String,
    row: gtk::ListBoxRow,
    spinner: gtk::Spinner,
}

impl Picker {
    pub fn new(weak: Weak<Window>) -> Self {
        let title = gtk::Label::new(Some("Okuri"));
        title.add_css_class("title-1");

        let subtitle = gtk::Label::new(Some(PHRASES[0]));
        subtitle.add_css_class("okuri-muted");
        subtitle.add_css_class("okuri-subtitle");

        let heading = gtk::Box::new(gtk::Orientation::Vertical, 6);
        heading.append(&title);
        heading.append(&subtitle);

        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::Single);

        // Opened by double-clicking, or by Enter, the way a file manager opens anything. A
        // single click selects, and selecting must not be enough to start dialling a server —
        // reaching Edit would mean connecting to whatever you passed over.
        list.set_activate_on_single_click(false);
        list.connect_row_activated({
            let weak = weak.clone();

            move |_, row| {
                let Some(window) = weak.upgrade() else {
                    return;
                };

                let id: Option<String> = window.picker.id_of(row);

                if let Some(id) = id {
                    window.connect_to(&id);
                }
            }
        });

        let empty = gtk::Label::new(Some("No connections yet"));
        empty.add_css_class("okuri-muted");
        empty.set_margin_top(20);
        empty.set_margin_bottom(20);

        let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        frame.append(&list);
        frame.append(&empty);

        let compose = gtk::Button::with_label("New connection");
        compose.add_css_class("suggested-action");
        compose.set_halign(gtk::Align::Center);
        compose.set_action_name(Some("win.new-connection"));

        let column = gtk::Box::new(gtk::Orientation::Vertical, 18);
        column.set_valign(gtk::Align::Center);
        column.append(&heading);
        column.append(&frame);
        column.append(&compose);

        let clamp = adw::Clamp::builder()
            .maximum_size(460)
            .child(&column)
            .margin_start(40)
            .margin_end(40)
            .build();

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_valign(gtk::Align::Fill);
        root.append(&clamp);
        clamp.set_vexpand(true);

        let picker = Self {
            root,
            list,
            frame,
            empty,
            subtitle,
            rows: RefCell::new(Vec::new()),
            connecting_to: RefCell::new(String::new()),
            phrase: Cell::new(0),
            weak: weak.clone(),
        };

        picker.reload();

        // A connection saved in the editor is here the moment it is saved.
        crate::connections::list().on_change({
            let weak = weak.clone();

            move || match weak.upgrade() {
                Some(window) => {
                    window.picker.reload();
                    true
                }
                None => false,
            }
        });

        glib::timeout_add_local(EVERY, move || match weak.upgrade() {
            Some(window) => {
                window.picker.swap_phrase();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });

        picker
    }

    /// Which saved connection is being opened, so the row that was clicked can say so. That
    /// is where whoever clicked it is looking.
    pub fn set_connecting_to(&self, id: &str) {
        if *self.connecting_to.borrow() == id {
            return;
        }

        *self.connecting_to.borrow_mut() = id.to_owned();

        for saved in self.rows.borrow().iter() {
            let opening = saved.id == id;

            saved.spinner.set_spinning(opening);
            saved.spinner.set_visible(opening);

            match opening {
                true => saved.row.add_css_class("opening"),
                false => saved.row.remove_css_class("opening"),
            }
        }
    }

    fn id_of(&self, row: &gtk::ListBoxRow) -> Option<String> {
        self.rows
            .borrow()
            .iter()
            .find(|saved| saved.row == *row)
            .map(|saved| saved.id.clone())
    }

    fn reload(&self) {
        self.list.remove_all();

        let connections = crate::connections::list().entries();
        let opening = self.connecting_to.borrow().clone();
        let mut rows = Vec::new();

        for connection in connections {
            let name = gtk::Label::new(Some(&connection.name));
            name.set_xalign(0.0);

            let summary = gtk::Label::new(Some(&connection.summary()));
            summary.set_xalign(0.0);
            summary.add_css_class("okuri-muted");
            summary.add_css_class("okuri-small");
            summary.set_ellipsize(gtk::pango::EllipsizeMode::End);

            let words = gtk::Box::new(gtk::Orientation::Vertical, 2);
            words.set_hexpand(true);
            words.set_valign(gtk::Align::Center);
            words.append(&name);
            words.append(&summary);

            let spinner = gtk::Spinner::new();
            spinner.set_valign(gtk::Align::Center);
            spinner.set_visible(connection.id == opening);
            spinner.set_spinning(connection.id == opening);

            let edit = gtk::Button::with_label("Edit");
            edit.add_css_class("flat");
            edit.add_css_class("okuri-edit");
            edit.set_valign(gtk::Align::Center);
            edit.connect_clicked({
                let (weak, id) = (self.weak.clone(), connection.id.clone());

                move |_| {
                    if let Some(window) = weak.upgrade() {
                        window.amend_connection(&id);
                    }
                }
            });

            let content = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(10)
                .margin_start(16)
                .margin_end(10)
                .margin_top(10)
                .margin_bottom(10)
                .build();
            content.append(&words);
            content.append(&spinner);
            content.append(&edit);

            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&content));
            row.add_css_class("okuri-connection");

            if connection.id == opening {
                row.add_css_class("opening");
            }

            self.list.append(&row);
            rows.push(Saved { id: connection.id, row, spinner });
        }

        let none = rows.is_empty();
        self.list.set_visible(!none);
        self.empty.set_visible(none);
        self.frame.set_visible(true);

        *self.rows.borrow_mut() = rows;
    }

    /// Fades the subtitle out, changes the words, and fades it back in.
    fn swap_phrase(&self) {
        // Only while it is on screen: a window that is connected has nothing to say here.
        if !self.root.is_mapped() {
            return;
        }

        self.subtitle.add_css_class("okuri-faded");

        let (subtitle, weak) = (self.subtitle.clone(), self.weak.clone());

        glib::timeout_add_local_once(FADE, move || {
            if let Some(window) = weak.upgrade() {
                let next = (window.picker.phrase.get() + 1) % PHRASES.len();

                window.picker.phrase.set(next);
                subtitle.set_text(PHRASES[next]);
            }

            subtitle.remove_css_class("okuri-faded");
        });
    }
}
