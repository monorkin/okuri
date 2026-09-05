//! One file, on its own, with a way to get it.
//!
//! What opening a file means on a remote server: there is nothing to open in place, so showing
//! what is known about it and offering to bring it down is the honest answer to a double click.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use okuri_core::{Access, Who};

use crate::file_list::Facts;
use crate::window::Window;

/// How long "Copied" stays where the address was.
const SAID: Duration = Duration::from_millis(1600);

const PARTIES: [(Who, &str, &str); 3] = [
    (Who::Owner, "owner", "Owner"),
    (Who::Group, "group", "Group"),
    (Who::Everyone, "everyone", "Everyone"),
];

const VERBS: [(Access, &str, &str); 3] =
    [(Access::Read, "read", "Read"), (Access::Write, "write", "Write"), (Access::Execute, "execute", "Execute")];

pub struct Details {
    dialog: adw::Dialog,
    facts: gtk::Box,
    permissions: gtk::Box,
    ticks: Vec<(Who, Access, gtk::CheckButton)>,
    sharing: gtk::Box,
    visibility: gtk::Label,
    public: gtk::Switch,
    copy_link: gtk::Button,
    copy_signed: gtk::Button,
    download: gtk::Button,

    /// What was double-clicked, as the file list describes it.
    file: RefCell<Facts>,

    /// The rows as last drawn, so they are only redrawn when they change. Redrawing them
    /// destroys whichever of them has the focus, and a dialog with no focus is a dialog that
    /// Escape cannot reach.
    shown: RefCell<Vec<Fact>>,

    /// Which button last put something on the clipboard, so it can say so.
    copied: RefCell<String>,

    /// Whether a signature was asked for in order to copy it, rather than to look at it.
    copy_when_signed: Cell<bool>,

    /// Whether the controls are being set from the server's answer, as opposed to by hand.
    /// A switch moved by the server must not be taken as a request to move it again.
    refreshing: Cell<bool>,

    weak: Weak<Window>,
}

impl Details {
    pub fn show(window: &Rc<Window>, facts: Facts) -> Rc<Self> {
        let icon = gtk::Image::from_gicon(&facts.icon());
        icon.set_pixel_size(48);

        let name = gtk::Label::new(Some(&facts.name));
        name.set_xalign(0.0);
        name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        name.add_css_class("okuri-title");

        let kind = gtk::Label::new(Some(facts.kind));
        kind.set_xalign(0.0);
        kind.add_css_class("okuri-muted");
        kind.add_css_class("okuri-small");

        let words = gtk::Box::new(gtk::Orientation::Vertical, 2);
        words.set_hexpand(true);
        words.set_valign(gtk::Align::Center);
        words.append(&name);
        words.append(&kind);

        let heading = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        heading.append(&icon);
        heading.append(&words);

        let facts_box = gtk::Box::new(gtk::Orientation::Vertical, 8);

        // The mode, as nine answers rather than nine characters of shorthand.
        //
        // Editable where the destination keeps modes at all, which means the file protocols —
        // an object store has nothing to set.
        let permissions = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let mut ticks = Vec::new();

        let captions = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        captions.append(&caption("Permissions", 110));

        for (_, _, verb) in VERBS {
            let label = caption(verb, 78);
            label.add_css_class("okuri-small");
            captions.append(&label);
        }

        permissions.append(&captions);

        for (who, _, party) in PARTIES {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            let label = gtk::Label::new(Some(party));
            label.set_xalign(0.0);
            label.set_size_request(110, -1);
            label.add_css_class("okuri-small");
            row.append(&label);

            for (access, _, _) in VERBS {
                let tick = gtk::CheckButton::new();
                tick.add_css_class("okuri-tick");
                row.append(&tick);
                ticks.push((who, access, tick));
            }

            permissions.append(&row);
        }

        // Only for destinations that can hand a file to somebody with no account, which today
        // means the S3-shaped ones. Everything else has no answer to give.
        //
        // The addresses are copied rather than displayed. Neither is worth reading: one is a
        // path you already know, the other four hundred characters of signature and expiry.
        // The button is what people came for.
        let sharing = gtk::Box::new(gtk::Orientation::Vertical, 10);
        sharing.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        let public_label = gtk::Label::new(Some("Public"));
        public_label.set_xalign(0.0);

        let visibility = gtk::Label::new(None);
        visibility.set_xalign(0.0);
        visibility.set_ellipsize(gtk::pango::EllipsizeMode::End);
        visibility.add_css_class("okuri-muted");
        visibility.add_css_class("okuri-small");

        let public_words = gtk::Box::new(gtk::Orientation::Vertical, 2);
        public_words.set_hexpand(true);
        public_words.append(&public_label);
        public_words.append(&visibility);

        let public = gtk::Switch::new();
        public.set_valign(gtk::Align::Center);

        let public_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        public_row.append(&public_words);
        public_row.append(&public);
        sharing.append(&public_row);

        let copy_link = gtk::Button::with_label("Copy link");
        copy_link.set_hexpand(true);

        // Signs and copies in one go. A link nobody can see is no use as two steps.
        let copy_signed = gtk::Button::with_label("Copy signed link");
        copy_signed.set_hexpand(true);

        let copies = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        copies.set_homogeneous(true);
        copies.append(&copy_link);
        copies.append(&copy_signed);
        sharing.append(&copies);

        let download = gtk::Button::with_label("Download");
        download.add_css_class("suggested-action");

        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            .margin_top(20)
            .margin_bottom(20)
            .margin_start(20)
            .margin_end(20)
            .build();
        column.append(&heading);
        column.append(&facts_box);
        column.append(&permissions);
        column.append(&sharing);
        column.append(&download);

        let content = adw::ToolbarView::new();
        content.add_top_bar(&adw::HeaderBar::new());
        content.set_content(Some(&column));

        let dialog = adw::Dialog::builder()
            .title(&facts.name)
            .content_width(420)
            .child(&content)
            .build();

        let details = Rc::new(Self {
            dialog,
            facts: facts_box,
            permissions,
            ticks,
            sharing,
            visibility,
            public,
            copy_link,
            copy_signed,
            download: download.clone(),
            file: RefCell::new(facts),
            shown: RefCell::new(Vec::new()),
            copied: RefCell::new(String::new()),
            copy_when_signed: Cell::new(false),
            refreshing: Cell::new(false),
            weak: Rc::downgrade(window),
        });

        for (who, access, tick) in &details.ticks {
            tick.connect_toggled({
                let details = Rc::clone(&details);
                let (who, access) = (*who, *access);

                move |tick| {
                    if !details.refreshing.get() {
                        details.permit(who, access, tick.is_active());
                    }
                }
            });
        }

        details.public.connect_state_set({
            let details = Rc::clone(&details);

            move |_, state| {
                if details.refreshing.get() {
                    return glib::Propagation::Proceed;
                }

                if let Some(window) = details.weak.upgrade() {
                    window.reshare(&details.file.borrow().name, state);
                }

                // Left where it is until the server answers, so a store that refuses to change
                // this snaps back instead of lying about it.
                glib::Propagation::Stop
            }
        });

        details.copy_link.connect_clicked({
            let details = Rc::clone(&details);

            move |_| {
                let url = details.weak.upgrade().map(|window| window.screen().shared_url.clone());

                if let Some(url) = url {
                    details.take(&url, "plain");
                }
            }
        });

        details.copy_signed.connect_clicked({
            let details = Rc::clone(&details);

            move |_| {
                details.copy_when_signed.set(true);

                if let Some(window) = details.weak.upgrade() {
                    window.sign_link(&details.file.borrow().name);
                }
            }
        });

        download.connect_clicked({
            let details = Rc::clone(&details);

            move |_| {
                details.dialog.close();

                if let Some(window) = details.weak.upgrade() {
                    window.download_selected();
                }
            }
        });

        details.dialog.connect_closed({
            let weak = Rc::downgrade(window);

            move |_| {
                if let Some(window) = weak.upgrade() {
                    window.details_closed();
                }
            }
        });

        // Asked rather than assumed: whether a file is readable by anybody is a property of
        // the file on the server, not something the listing carries.
        if window.screen().can_share {
            window.share(&details.file.borrow().name);
        }

        // What the listing carries is only what every destination has in common. The rest is
        // asked for now, one file at a time, because this is the moment anybody wants it.
        window.describe(&details.file.borrow().name);

        details.refresh(window);
        details.dialog.present(Some(&window.gtk));

        details
    }

    /// Redraws from what the window knows, which changes as the server answers.
    pub fn refresh(&self, window: &Window) {
        self.refreshing.set(true);

        let screen = window.screen();
        let file = self.file.borrow();
        let copied = self.copied.borrow().clone();

        // Anything the server did not say is left out rather than shown empty — a blank
        // Modified line reads like the file has never been touched.
        let mut facts = vec![
            Fact::plain("Size", &file.size),
            Fact::plain("Modified", &file.modified),
            // Where the file is on the server, not where the breadcrumb says: what the window
            // shows is relative to wherever the connection starts. Worth copying, because a
            // path is something you paste somewhere else.
            Fact::copyable("Where", &screen.absolute_path()),
        ];

        // Whatever else this destination knows. While the answer is still coming, the rows it
        // will fill are already here with nothing in them — otherwise the panel opens short
        // and grows under the pointer as each reply lands.
        match screen.describing {
            true => facts.extend(screen.expected().into_iter().map(Fact::waiting)),
            false => facts.extend(
                screen
                    .described
                    .rows()
                    .into_iter()
                    .map(|(label, said)| Fact::copyable(label, &said)),
            ),
        }

        let facts = facts
            .into_iter()
            .filter(|fact| fact.waiting || !fact.value.is_empty())
            .map(|fact| fact.saying(&copied))
            .collect::<Vec<_>>();

        if *self.shown.borrow() != facts {
            while let Some(child) = self.facts.first_child() {
                self.facts.remove(&child);
            }

            for fact in &facts {
                self.facts.append(&self.fact_row(fact));
            }

            if self.dialog.focus().is_none() {
                self.download.grab_focus();
            }

            *self.shown.borrow_mut() = facts;
        }

        self.permissions.set_visible(!file.permissions.is_empty());

        for (who, access, tick) in &self.ticks {
            tick.set_active(file.mode.is_some_and(|mode| mode.allows(*who, *access)));
            tick.set_sensitive(screen.can_set_permissions);
        }

        self.sharing.set_visible(screen.can_share);
        self.visibility.set_text(match screen.shared_public {
            None => "Okuri cannot tell",
            Some(true) => "Anyone with the address can read it",
            Some(false) => "Only this account's keys can read it",
        });
        self.public.set_sensitive(screen.shared_public.is_some());
        self.public.set_active(screen.shared_public.unwrap_or(false));
        self.public.set_state(screen.shared_public.unwrap_or(false));

        // Why the switch will not move, kept out of the way until it is wanted: the reason is
        // a sentence from the server and would otherwise be the largest thing in the panel.
        self.public.set_tooltip_text(match screen.shared_public {
            None if !screen.shared_why_not.is_empty() => Some(screen.shared_why_not.as_str()),
            _ => None,
        });

        self.copy_link.set_sensitive(!screen.shared_url.is_empty());
        self.copy_link.set_label(match copied == "plain" {
            true => "Copied",
            false => "Copy link",
        });
        self.copy_signed.set_label(match copied == "signed" {
            true => "Copied",
            false => "Copy signed link",
        });

        let signed = screen.signed_url.clone();
        drop(screen);
        drop(file);

        self.refreshing.set(false);

        if self.copy_when_signed.get() && !signed.is_empty() {
            self.copy_when_signed.set(false);
            self.take(&signed, "signed");
        }
    }

    fn fact_row(&self, fact: &Fact) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.append(&caption(&fact.label, 110));

        if fact.waiting {
            // What is not there yet, drawn as the shape of what will be. A bar rather than a
            // spinner: three spinners in a column is a lot of movement for something that is
            // only slow the first time.
            let placeholder = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            placeholder.add_css_class("okuri-placeholder");
            placeholder.set_valign(gtk::Align::Center);
            placeholder.set_halign(gtk::Align::Start);
            row.append(&placeholder);

            return row;
        }

        let value = gtk::Label::new(Some(&fact.shown));
        value.set_xalign(0.0);
        value.set_hexpand(true);
        value.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        value.add_css_class("okuri-small");

        match fact.copies {
            true => {
                let button = gtk::Button::new();
                button.set_child(Some(&value));
                button.add_css_class("flat");
                button.add_css_class("okuri-copyable");
                button.set_hexpand(true);
                button.set_tooltip_text(Some("Copy"));

                button.connect_clicked({
                    let weak = self.weak.clone();
                    let (text, label) = (fact.value.clone(), fact.label.clone());

                    move |_| {
                        let details = weak.upgrade().and_then(|window| window.details());

                        if let Some(details) = details {
                            details.take(&text, &label);
                        }
                    }
                });

                row.append(&button);
            }
            false => row.append(&value),
        }

        row
    }

    /// Puts `text` on the clipboard and says so where the address was.
    fn take(&self, text: &str, which: &str) {
        self.dialog.clipboard().set_text(text);
        *self.copied.borrow_mut() = which.to_owned();

        if let Some(window) = self.weak.upgrade() {
            self.refresh(&window);
        }

        let weak = self.weak.clone();

        glib::timeout_add_local_once(SAID, move || {
            let details = weak.upgrade().and_then(|window| window.details());

            if let (Some(details), Some(window)) = (details, weak.upgrade()) {
                *details.copied.borrow_mut() = String::new();
                details.refresh(&window);
            }
        });
    }

    /// Sends the whole mode, because that is what the protocols take: there is no way to
    /// change one bit on its own, so every other answer has to be sent back exactly as it was.
    ///
    /// What is shown is updated here rather than waiting for the server, since the listing that
    /// comes back afterwards does not know which row this panel is describing. A refusal shows
    /// along the bottom, and reopening the file shows the truth.
    fn permit(&self, who: Who, what: Access, allowed: bool) {
        let mut file = self.file.borrow_mut();
        let current = file.mode.map(|mode| mode.mode()).unwrap_or_default();
        let bit = (what as u32) << (who as u32 * 3);

        let mode = match allowed {
            true => current | bit,
            false => current & !bit,
        };

        file.mode = Some(okuri_core::Permissions(mode));
        let name = file.name.clone();
        drop(file);

        if let Some(window) = self.weak.upgrade() {
            window.set_permissions(&name, mode);
        }
    }
}

/// One line of the panel: a label, what the server said, and what is written there right now —
/// which is "Copied" for a moment after it has been.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Fact {
    label: String,
    value: String,
    shown: String,
    copies: bool,
    waiting: bool,
}

impl Fact {
    fn plain(label: &str, value: &str) -> Self {
        Self::new(label, value, false, false)
    }

    fn copyable(label: &str, value: &str) -> Self {
        Self::new(label, value, true, false)
    }

    fn waiting(label: &str) -> Self {
        Self::new(label, "", false, true)
    }

    fn new(label: &str, value: &str, copies: bool, waiting: bool) -> Self {
        Self {
            label: label.to_owned(),
            value: value.to_owned(),
            shown: value.to_owned(),
            copies,
            waiting,
        }
    }

    fn saying(mut self, copied: &str) -> Self {
        if copied == self.label {
            self.shown = "Copied".to_owned();
        }

        self
    }
}

fn caption(text: &str, width: i32) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_size_request(width, -1);
    label.add_css_class("okuri-muted");
    label.add_css_class("okuri-small");

    label
}
