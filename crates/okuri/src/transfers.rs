use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use okuri_engine::engine::Command;
use okuri_engine::event::Outcome;
use okuri_engine::transfer::{Direction, State, Transfer, TransferId};
use okuri_engine::Event;

use crate::format;

/// The transfer queue, as the toolbar button and its window show it.
///
/// One for the process rather than one per window. The queue is one queue for the whole
/// application, and a transfer started in one window is still moving when that window is
/// looking at something else — or has been closed.
pub struct Transfers {
    pub store: gio::ListStore,
    rows: RefCell<HashMap<TransferId, u32>>,
    observers: RefCell<Vec<Rc<dyn Fn() -> bool>>>,
}

thread_local! {
    static QUEUE: Rc<Transfers> = Transfers::start();
}

pub fn queue() -> Rc<Transfers> {
    QUEUE.with(Rc::clone)
}

impl Transfers {
    fn start() -> Rc<Self> {
        let transfers = Rc::new(Self {
            store: gio::ListStore::new::<glib::BoxedAnyObject>(),
            rows: RefCell::new(HashMap::new()),
            observers: RefCell::new(Vec::new()),
        });

        // Kept for as long as the process runs: the queue outlives every window.
        let queue = Rc::clone(&transfers);

        crate::relay::on_event(move |event| match event.as_ref() {
            Event::TransferAdded(transfer) => queue.add(transfer.clone()),
            Event::TransferProgress { transfer, transferred } => queue.advance(*transfer, *transferred),
            Event::TransferFinished { transfer, outcome } => queue.finish(*transfer, outcome.clone()),
            _ => {}
        })
        .forever();

        transfers
    }

    /// Calls `observer` whenever the queue changes, until it answers `false` — which is how an
    /// observer says the thing it was updating has gone.
    pub fn on_change(&self, observer: impl Fn() -> bool + 'static) {
        self.observers.borrow_mut().push(Rc::new(observer));
    }

    /// How many transfers are still going, which is what the toolbar badge shows.
    pub fn active(&self) -> u32 {
        (0..self.count())
            .filter(|row| self.with_row(*row, |transfer| !transfer.state.is_finished()).unwrap_or(false))
            .count() as u32
    }

    pub fn count(&self) -> u32 {
        self.store.n_items()
    }

    /// Drops everything that has already finished, so the window shows what is happening
    /// rather than what happened.
    pub fn clear_finished(&self) {
        let kept = (0..self.count())
            .filter_map(|row| self.store.item(row))
            .filter(|object| {
                object
                    .downcast_ref::<glib::BoxedAnyObject>()
                    .is_some_and(|object| !object.borrow::<Transfer>().state.is_finished())
            })
            .collect::<Vec<_>>();

        self.store.splice(0, self.count(), &kept);
        self.reindex();
        self.announce();
    }

    pub fn id_at(&self, row: u32) -> Option<TransferId> {
        self.with_row(row, |transfer| transfer.id)
    }

    pub fn cancel(&self, id: TransferId) {
        crate::running::engine().send(Command::CancelTransfer(id));
    }

    pub fn with_row<T>(&self, row: u32, read: impl FnOnce(&Transfer) -> T) -> Option<T> {
        let object = self.store.item(row)?.downcast::<glib::BoxedAnyObject>().ok()?;
        let transfer = object.borrow::<Transfer>();

        Some(read(&transfer))
    }

    fn add(&self, transfer: Transfer) {
        self.store.append(&glib::BoxedAnyObject::new(transfer));
        self.reindex();
        self.announce();
    }

    /// Progress is the one thing that changes often, so it touches the single row it belongs
    /// to rather than replacing the list and losing the scroll position.
    fn advance(&self, id: TransferId, transferred: u64) {
        self.touch(id, |transfer| {
            transfer.transferred = transferred;
            transfer.state = State::Running;
        });
    }

    fn finish(&self, id: TransferId, outcome: Outcome) {
        self.touch(id, |transfer| {
            transfer.state = match outcome {
                Outcome::Done => State::Done,
                Outcome::Failed(reason) => State::Failed(reason),
                Outcome::Cancelled => State::Cancelled,
            };
        });

        self.announce();
    }

    fn touch(&self, id: TransferId, change: impl FnOnce(&mut Transfer)) {
        let Some(row) = self.rows.borrow().get(&id).copied() else {
            return;
        };

        if let Some(object) = self.store.item(row).and_downcast::<glib::BoxedAnyObject>() {
            change(&mut object.borrow_mut::<Transfer>());
        }

        self.store.items_changed(row, 1, 1);
    }

    fn reindex(&self) {
        let rows = (0..self.count())
            .filter_map(|row| Some((self.id_at(row)?, row)))
            .collect();

        *self.rows.borrow_mut() = rows;
    }

    fn announce(&self) {
        // The list is copied before anything is called, so an observer is free to add another
        // without tripping over the borrow it is being called under.
        let observers = self.observers.borrow().clone();
        let alive = observers.into_iter().filter(|observer| observer()).collect::<Vec<_>>();

        *self.observers.borrow_mut() = alive;
    }
}

/// What a row says under its name.
pub fn status(transfer: &Transfer) -> String {
    match &transfer.state {
        State::Queued => "Waiting".to_owned(),
        State::Running => match transfer.total {
            Some(total) => format!(
                "{} of {}",
                format::size(transfer.transferred),
                format::size(total)
            ),
            None => format::size(transfer.transferred),
        },
        State::Done => format::size(transfer.transferred),
        State::Failed(reason) => reason.clone(),
        State::Cancelled => "Cancelled".to_owned(),
    }
}

/// The queue, on screen.
pub fn present(parent: &impl IsA<gtk::Widget>) {
    let transfers = queue();

    let dialog = adw::Dialog::builder()
        .title("Transfers")
        .content_width(560)
        .content_height(420)
        .build();

    let header = adw::HeaderBar::new();
    let clear = gtk::Button::with_label("Clear finished");
    header.pack_end(&clear);

    let progress = gtk::Label::new(None);
    progress.set_xalign(0.0);
    header.set_title_widget(Some(&progress));

    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup({
        let transfers = Rc::clone(&transfers);

        move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
            item.set_child(Some(&TransferCell::new(item, &transfers).root));
            item.set_selectable(false);
            item.set_activatable(false);
        }
    });

    factory.connect_bind({
        let transfers = Rc::clone(&transfers);

        move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
            let Some(cell) = item.child().and_then(|child| TransferCell::from_root(&child)) else {
                return;
            };

            transfers.with_row(item.position(), |transfer| cell.show(transfer));
        }
    });

    let list = gtk::ListView::new(
        Some(gtk::NoSelection::new(Some(transfers.store.clone()))),
        Some(factory),
    );
    list.add_css_class("okuri-transfers");

    let empty = gtk::Label::new(Some("Drag files into the window to upload them."));
    empty.add_css_class("okuri-muted");

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&gtk::ScrolledWindow::builder().child(&list).vexpand(true).build()));
    overlay.add_overlay(&empty);

    let content = adw::ToolbarView::new();
    content.add_top_bar(&header);
    content.set_content(Some(&overlay));
    dialog.set_child(Some(&content));

    // What the header says follows the queue for as long as the dialog is open, and stops
    // being asked the moment it is not.
    let refresh = {
        let (progress, clear, empty) = (progress.downgrade(), clear.downgrade(), empty.downgrade());
        let transfers = Rc::clone(&transfers);

        move || {
            let (Some(progress), Some(clear), Some(empty)) =
                (progress.upgrade(), clear.upgrade(), empty.upgrade())
            else {
                return false;
            };

            let active = transfers.active();

            progress.set_text(&match active {
                0 => "Nothing in progress".to_owned(),
                active => format!("{active} in progress"),
            });
            clear.set_sensitive(transfers.count() > active);
            empty.set_visible(transfers.count() == 0);

            true
        }
    };

    refresh();
    transfers.on_change(refresh);

    clear.connect_clicked({
        let transfers = Rc::clone(&transfers);

        move |_| transfers.clear_finished()
    });

    dialog.present(Some(parent));
}

/// One transfer, as a row of the queue.
struct TransferCell {
    root: gtk::Box,
    name: gtk::Label,
    bar: gtk::ProgressBar,
    status: gtk::Label,
    cancel: gtk::Button,
}

impl TransferCell {
    /// Built once per list item and bound to whichever transfer scrolls into it. The item is
    /// what the Cancel button asks which transfer it is sitting on, because the widgets are
    /// reused and the answer changes under them.
    fn new(item: &gtk::ListItem, transfers: &Rc<Transfers>) -> Self {
        let name = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();

        let bar = gtk::ProgressBar::new();
        bar.add_css_class("okuri-progress");

        let status = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["okuri-muted", "okuri-small"])
            .build();

        let column = gtk::Box::new(gtk::Orientation::Vertical, 5);
        column.set_hexpand(true);
        column.set_valign(gtk::Align::Center);
        column.append(&name);
        column.append(&bar);
        column.append(&status);

        let cancel = gtk::Button::with_label("Cancel");
        cancel.set_valign(gtk::Align::Center);
        cancel.add_css_class("flat");

        cancel.connect_clicked({
            let item = item.downgrade();
            let transfers = Rc::clone(transfers);

            move |_| {
                if let Some(id) = item.upgrade().and_then(|item| transfers.id_at(item.position())) {
                    transfers.cancel(id);
                }
            }
        });

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(10)
            .margin_start(18)
            .margin_end(12)
            .margin_top(8)
            .margin_bottom(8)
            .build();
        root.append(&column);
        root.append(&cancel);

        Self { root, name, bar, status, cancel }
    }

    /// The same widgets back, from the one the list item holds.
    fn from_root(root: &gtk::Widget) -> Option<Self> {
        let root = root.downcast_ref::<gtk::Box>()?.clone();
        let column = root.first_child()?.downcast::<gtk::Box>().ok()?;
        let name = column.first_child()?.downcast::<gtk::Label>().ok()?;
        let bar = name.next_sibling()?.downcast::<gtk::ProgressBar>().ok()?;
        let status = bar.next_sibling()?.downcast::<gtk::Label>().ok()?;
        let cancel = column.next_sibling()?.downcast::<gtk::Button>().ok()?;

        Some(Self { root, name, bar, status, cancel })
    }

    fn show(&self, transfer: &Transfer) {
        let arrow = match transfer.direction {
            Direction::Download => "↓ ",
            Direction::Upload | Direction::Between => "↑ ",
        };

        self.name.set_text(&format!("{arrow}{}", transfer.name));
        self.bar.set_fraction(transfer.fraction().unwrap_or(0.0));
        self.bar.set_visible(!transfer.state.is_finished());
        self.status.set_text(&status(transfer));
        self.cancel.set_visible(!transfer.state.is_finished());
    }
}
