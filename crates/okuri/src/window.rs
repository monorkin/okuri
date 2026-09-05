//! One window onto one server.
//!
//! Everything a window has is its own: the connection, the folder, the selection, the questions
//! being asked about it. What it shares with the others is the engine underneath, the transfer
//! queue, and the list of saved connections — the things that would be wrong to have two of.
//!
//! Every rule about what the window shows lives in [`Screen`], where it can be tested; this
//! object turns clicks and keystrokes into engine commands and copies the answers onto widgets.

use std::cell::{Ref, RefCell};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use adw::prelude::*;
use gtk::{gio, glib};
use okuri_core::RemotePath;
use okuri_engine::engine::Command;
use okuri_engine::transfer::Place;
use okuri_engine::{Answer, Attempt, Concern, Event, SessionId};

use crate::browser::Browser;
use crate::details::Details;
use crate::dialogs::{self, Reply};
use crate::file_list::FileList;
use crate::picker::Picker;
use crate::relay::Subscription;
use crate::screen::{Carried, Screen};
use crate::view::{self, Mode};

thread_local! {
    /// Every open window, oldest first. Owned here rather than by any one of them, because a
    /// window that owned the list would take the list with it when it closed.
    static WINDOWS: RefCell<Vec<Rc<Window>>> = const { RefCell::new(Vec::new()) };
}

/// The application's shortcuts, named after what they do. Installed once per process.
pub fn install_shortcuts(app: &adw::Application) {
    for (action, keys) in [
        ("win.up", &["<Alt>Up"][..]),
        ("win.refresh", &["F5", "<Control>r"]),
        ("win.new-folder", &["<Control>n"]),
        ("win.new-window", &["<Control><Shift>n"]),
        ("win.show-hidden", &["<Control>h"]),
        ("win.zoom-in", &["<Control>equal", "<Control>plus"]),
        ("win.zoom-out", &["<Control>minus"]),
    ] {
        app.set_accels_for_action(action, keys);
    }
}

/// Opens another window.
pub fn open(app: &adw::Application) {
    let window = Rc::new_cyclic(|weak| Window::build(app, weak.clone()));

    WINDOWS.with(|windows| windows.borrow_mut().push(Rc::clone(&window)));

    window.apply_view_settings();
    window.render();
    window.gtk.present();

    // The first window is the one Okuri opened with, and the only one there is when the
    // command line is read. `okuri production-web` opens that connection straight away, which
    // is what you want from a launcher, a keybinding, or a terminal you are already standing in.
    static OPENED: AtomicBool = AtomicBool::new(false);

    if !OPENED.swap(true, Ordering::SeqCst)
        && let Some(id) = std::env::args().nth(1)
    {
        window.connect_to(&id);
    }
}

/// What a drag is carrying, kept for a paste or a drop.
#[derive(Clone, Debug)]
pub struct Carry {
    /// What the drop reads back, wherever it lands.
    pub payload: String,
    /// Where the files live, for anything outside Okuri that can open them. Empty for a
    /// destination the desktop cannot reach on its own.
    pub urls: Vec<String>,
}

pub struct Window {
    pub gtk: adw::ApplicationWindow,
    app: adw::Application,
    weak: Weak<Window>,

    /// What the window is showing. Every rule about it lives in [`Screen`]; this object only
    /// copies the answers onto widgets.
    screen: RefCell<Screen>,

    /// The questions waiting to be answered, oldest first.
    ///
    /// A queue rather than one slot: two connections opening at once ask two questions, and
    /// overwriting the first would drop its prompt — which answers it by declining, so one of
    /// the two connections would fail for no reason anybody could see.
    pending: RefCell<VecDeque<Arc<Event>>>,
    asking: RefCell<bool>,

    pub files: Rc<FileList>,

    /// What is being dragged or has been cut, until it is dropped or pasted.
    ///
    /// Worked out when the gesture starts rather than when the pointer reaches the edge: a
    /// drag that leaves the window is taken over by the desktop, and by then there is no chance
    /// to add anything to what it carries.
    carried: RefCell<Option<Carry>>,

    /// The path the breadcrumb was last drawn for, so it is only redrawn when that moves.
    trail: RefCell<Option<(String, String)>>,

    up: gtk::Button,
    refresh: gtk::Button,
    title: gtk::Stack,
    breadcrumb: Breadcrumb,
    spinner: gtk::Spinner,
    new_connection: gtk::Button,
    display: adw::SplitButton,
    transfers: gtk::Button,
    transfers_count: gtk::Label,
    disconnect: gtk::Button,

    stack: gtk::Stack,
    pub picker: Picker,
    pub browser: Browser,
    notice: Notice,

    /// The panel showing one file, while it is open.
    details: RefCell<Option<Rc<Details>>>,

    actions: Actions,
    subscriptions: RefCell<Vec<Subscription>>,
}

/// The window's actions, kept so their enabled and checked states can follow the connection.
struct Actions {
    new_folder: gio::SimpleAction,
    open: gio::SimpleAction,
    download: gio::SimpleAction,
    rename: gio::SimpleAction,
    delete: gio::SimpleAction,
    columns: gio::SimpleAction,
    sort: gio::SimpleAction,
    show_hidden: gio::SimpleAction,
    zoom_in: gio::SimpleAction,
    zoom_out: gio::SimpleAction,
}

impl Window {
    fn build(app: &adw::Application, weak: Weak<Window>) -> Self {
        let gtk = adw::ApplicationWindow::builder()
            .application(app)
            .title("Okuri")
            .default_width(960)
            .default_height(640)
            .width_request(520)
            .height_request(360)
            .build();

        let files = FileList::new();

        let up = gtk::Button::from_icon_name("go-previous-symbolic");
        up.set_tooltip_text(Some("Parent folder"));
        up.set_action_name(Some("win.up"));

        let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh.set_tooltip_text(Some("Refresh"));
        refresh.set_action_name(Some("win.refresh"));

        // The path takes the whole width between the buttons, reading from the left the way
        // a path does; the header's loose centering below is what lets a title do that.
        let breadcrumb = Breadcrumb::new();
        let title = gtk::Stack::new();
        title.set_hexpand(true);
        title.add_named(&adw::WindowTitle::new("Okuri", ""), Some("name"));
        title.add_named(&breadcrumb.root, Some("trail"));
        title.set_margin_start(4);
        title.set_margin_end(4);

        // Only for work that has somewhere else to be shown: connecting says so in the row
        // that was clicked, and a first listing says so in the middle of the window.
        let spinner = gtk::Spinner::new();

        let new_connection = gtk::Button::from_icon_name("list-add-symbolic");
        new_connection.set_tooltip_text(Some("New connection"));
        new_connection.set_action_name(Some("win.new-connection"));

        // The view mode and the options that go with it belong together, so they are one
        // control with a seam rather than two buttons that happen to be adjacent.
        let display = adw::SplitButton::new();
        display.set_popover(Some(&view_menu()));

        let transfers_count = gtk::Label::new(None);
        let transfers_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        transfers_content.append(&gtk::Label::new(Some("↑")));
        transfers_content.append(&transfers_count);

        let transfers = gtk::Button::new();
        transfers.set_child(Some(&transfers_content));
        transfers.add_css_class("flat");
        transfers.set_tooltip_text(Some("Transfers"));
        transfers.set_action_name(Some("win.show-transfers"));

        // Always here, connected or not. A second window is how you reach a second server, so
        // hiding it until you have reached the first one has it missing exactly when somebody
        // wants two things open side by side.
        let new_window = gtk::Button::from_icon_name("window-new-symbolic");
        new_window.set_tooltip_text(Some("New window"));
        new_window.set_action_name(Some("win.new-window"));

        // Eject: the same mark a file manager puts beside a mounted server.
        let disconnect = gtk::Button::from_icon_name("media-eject-symbolic");
        disconnect.set_tooltip_text(Some("Disconnect"));
        disconnect.set_action_name(Some("win.disconnect"));

        // A plain toolbar rather than a header bar: a header bar centres its title and
        // gives it no more than it asks for, and the path wants the whole width between the
        // buttons. The handle keeps it draggable. No close button: the window manager closes
        // windows, and on Omarchy nothing draws one of its own.
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.add_css_class("toolbar");

        for widget in [
            up.upcast_ref::<gtk::Widget>(),
            refresh.upcast_ref(),
            title.upcast_ref(),
            spinner.upcast_ref(),
            new_connection.upcast_ref(),
            display.upcast_ref(),
            transfers.upcast_ref(),
            new_window.upcast_ref(),
            disconnect.upcast_ref(),
        ] {
            bar.append(widget);
        }

        let header = gtk::WindowHandle::new();
        header.set_child(Some(&bar));

        let picker = Picker::new(weak.clone());
        let browser = Browser::new(weak.clone(), Rc::clone(&files));
        let notice = Notice::new(weak.clone());

        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        stack.add_named(&picker.root, Some("picker"));
        stack.add_named(&browser.root, Some("browser"));

        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.append(&stack);
        column.append(&notice.revealer);

        let content = adw::ToolbarView::new();
        content.add_top_bar(&header);
        content.set_content(Some(&column));
        gtk.set_content(Some(&content));

        let actions = Actions::install(&gtk, &weak);

        display.connect_clicked(|_| view::update(|settings| settings.mode = settings.mode.other()));

        // Closing a window closes what it had open. Leaving the session behind would leave a
        // live SSH connection belonging to a window nobody can see any more.
        gtk.connect_close_request({
            let weak = weak.clone();

            move |_| {
                if let Some(window) = weak.upgrade() {
                    window.disconnect();
                    WINDOWS.with(|windows| {
                        windows.borrow_mut().retain(|each| !Rc::ptr_eq(each, &window));
                    });
                }

                glib::Propagation::Proceed
            }
        });

        let window = Self {
            gtk,
            app: app.clone(),
            weak: weak.clone(),
            screen: RefCell::new(Screen::default()),
            pending: RefCell::new(VecDeque::new()),
            asking: RefCell::new(false),
            files,
            carried: RefCell::new(None),
            trail: RefCell::new(None),
            up,
            refresh,
            title,
            breadcrumb,
            spinner,
            new_connection,
            display,
            transfers,
            transfers_count,
            disconnect,
            stack,
            picker,
            browser,
            notice,
            details: RefCell::new(None),
            actions,
            subscriptions: RefCell::new(Vec::new()),
        };

        window.subscribe(&weak);

        window
    }

    /// Hears the engine, the display settings, and the queue, for as long as the window is
    /// open. Dropping the window drops the subscriptions with it.
    fn subscribe(&self, weak: &Weak<Window>) {
        let events = crate::relay::on_event({
            let weak = weak.clone();

            move |event| {
                if let Some(window) = weak.upgrade() {
                    window.receive(event);
                }
            }
        });

        let settings = crate::relay::on_view_change({
            let weak = weak.clone();

            move || {
                if let Some(window) = weak.upgrade() {
                    window.apply_view_settings();
                }
            }
        });

        self.subscriptions.borrow_mut().extend([events, settings]);

        crate::transfers::queue().on_change({
            let weak = weak.clone();

            move || match weak.upgrade() {
                Some(window) => {
                    window.render_transfers();
                    true
                }
                None => false,
            }
        });

        self.files.on_change({
            let weak = weak.clone();

            move || {
                if let Some(window) = weak.upgrade() {
                    window.render_waiting();
                }
            }
        });
    }

    pub fn screen(&self) -> Ref<'_, Screen> {
        self.screen.borrow()
    }

    /// Whether this is the oldest window open, which is where a question that belongs to no
    /// connection is asked. Somebody has to ask it, and exactly one of them may.
    fn primary(&self) -> bool {
        WINDOWS.with(|windows| {
            windows.borrow().first().is_some_and(|first| std::ptr::eq(&**first, self))
        })
    }

    pub fn another(&self) {
        open(&self.app);
    }

    /// Connections are re-read here rather than remembered, so one saved a moment ago in the
    /// editor is connectable without anything having to be told about it.
    pub fn connect_to(&self, id: &str) {
        let Some(connection) = crate::store::load().find(id).cloned() else {
            self.complain(format!("there is no connection called {id}"));
            return;
        };

        let attempt = Attempt::next();

        self.screen.borrow_mut().connecting_to(attempt, connection.clone());
        self.render();

        crate::running::engine().send(Command::Connect {
            attempt,
            connection: Box::new(connection),
        });
    }

    /// Asked for under an attempt of its own, so the questions it puts up are asked by this
    /// window and not by whichever one happens to be listening.
    pub fn change_credentials(&self, id: &str) {
        let Some(connection) = crate::store::load().find(id).cloned() else {
            self.complain(format!("there is no connection called {id}"));
            return;
        };

        let attempt = Attempt::next();
        self.screen.borrow_mut().attempt = Some(attempt);

        crate::running::engine().send(Command::ChangeCredentials {
            attempt,
            connection: Box::new(connection),
        });
    }

    pub fn disconnect(&self) {
        if let Some(session) = self.screen.borrow().session {
            crate::running::engine().send(Command::Disconnect(session));
        }
    }

    /// Asks for everything the destination knows about a file, for the panel showing one.
    pub fn describe(&self, name: &str) {
        // Cleared first: what is on screen belongs to whichever file was looked at before.
        {
            let mut screen = self.screen.borrow_mut();
            screen.described = Default::default();
            screen.describing = true;
        }

        let name = name.to_owned();
        self.command(move |session| Command::Describe { session, name });
    }

    /// Asks who can read a file, answered by the `shared*` fields of the screen.
    pub fn share(&self, name: &str) {
        // Cleared first, so a panel opened on a second file never shows the first one's answer
        // while the server is still being asked.
        {
            let mut screen = self.screen.borrow_mut();
            screen.shared_public = None;
            screen.shared_url = String::new();
            screen.signed_url = String::new();
        }

        let name = name.to_owned();
        self.command(move |session| Command::Share { session, name });
    }

    /// Changes who can read a file, then reports where it stands.
    pub fn reshare(&self, name: &str, public: bool) {
        let name = name.to_owned();

        self.command(move |session| Command::Reshare { session, name, public });
    }

    /// Changes a file's mode.
    pub fn set_permissions(&self, name: &str, mode: u32) {
        let name = name.to_owned();
        let mode = mode & 0o777;

        self.command(move |session| Command::SetPermissions { session, name, mode });
    }

    /// Signs a link to a file that works for a week without an account.
    pub fn sign_link(&self, name: &str) {
        let name = name.to_owned();

        self.command(move |session| Command::SignLink { session, name });
    }

    pub fn open(&self, name: &str) {
        let folder = self.screen.borrow().folder.clone();

        match folder.join(name) {
            Ok(path) => self.go_to(path),
            Err(error) => self.complain(error.to_string()),
        }
    }

    pub fn open_path(&self, path: &str) {
        match RemotePath::parse(path) {
            Ok(path) => self.go_to(path),
            Err(error) => self.complain(error.to_string()),
        }
    }

    pub fn up(&self) {
        let parent = self.screen.borrow().folder.parent();

        if let Some(parent) = parent {
            self.go_to(parent);
        }
    }

    pub fn refresh(&self) {
        self.command(Command::Refresh);
    }

    pub fn create_folder(&self, name: &str) {
        let name = name.to_owned();

        self.command(move |session| Command::CreateFolder { session, name });
    }

    pub fn rename(&self, from: &str, to: &str) {
        let (from, to) = (from.to_owned(), to.to_owned());

        self.command(move |session| Command::Rename { session, from, to });
    }

    pub fn remove(&self, names: Vec<String>) {
        self.command(move |session| Command::Delete { session, names });
    }

    /// Files dropped from a file manager.
    pub fn upload(&self, sources: Vec<PathBuf>) {
        if sources.is_empty() {
            self.complain("nothing that was dropped is a file on this machine");
            return;
        }

        let into = self.screen.borrow().folder.clone();

        self.command(move |session| Command::Upload { session, into, sources });
    }

    pub fn download(&self, names: Vec<String>, into: PathBuf) {
        self.command(move |session| Command::Download { session, names, into });
    }

    /// Writes down what is being dragged, for the drag to carry.
    ///
    /// Goes into the drag itself rather than being read back off this window when a drop
    /// lands, because a drop can land in another window — and that window cannot ask this one
    /// what was picked up. Kept here as well so pasting, which has no drop to read, has
    /// somewhere to get it from.
    pub fn begin_move(&self, names: Vec<String>) -> Option<Carry> {
        let screen = self.screen.borrow();
        let session = screen.session?;

        if names.is_empty() {
            return None;
        }

        let urls = remote_urls(&screen, &names).unwrap_or_default();
        let carried = Carried { session, folder: screen.folder.clone(), names };
        let carry = Carry { payload: carried.payload(), urls };

        drop(screen);
        *self.carried.borrow_mut() = Some(carry.clone());

        Some(carry)
    }

    pub fn carrying(&self) -> Option<Carry> {
        self.carried.borrow().clone()
    }

    /// Puts whatever `payload` describes into `folder` on this window's connection.
    ///
    /// What is being moved comes from the drop rather than from this window, because the drop
    /// may have started in another one. It is also what makes a drop safe to offer to several
    /// targets at once: nothing is being consumed, so a target that turns out not to be the one
    /// meant leaves nothing missing behind it.
    pub fn move_into(&self, payload: &str, folder: &str) {
        let Some(session) = self.screen.borrow().session else {
            return;
        };

        let Some(carried) = Carried::parse(payload) else {
            self.complain("that drop did not say what it was carrying");
            return;
        };

        let Ok(path) = RemotePath::parse(folder) else {
            self.complain(format!("{folder} is not a folder this connection has"));
            return;
        };

        let from = Place::new(carried.session, carried.folder);
        let into = Place::new(session, path);

        crate::running::engine().send(Command::Move { from, names: carried.names, into });
    }

    pub fn end_move(&self) {
        *self.carried.borrow_mut() = None;
    }

    pub fn dismiss_message(&self) {
        self.screen.borrow_mut().message = String::new();
        self.render();
    }

    /// The breadcrumb: every folder from the root down to the one that is open.
    pub fn breadcrumb(&self) -> Vec<RemotePath> {
        self.screen.borrow().folder.ancestors()
    }

    /// What double-clicking, or pressing Enter, means.
    ///
    /// A folder opens. A file cannot — there is nothing on this machine to open, and fetching
    /// it silently would be a download nobody asked for — so it shows what is known about it
    /// and offers to bring it down.
    pub fn open_row(self: &Rc<Self>, row: u32) {
        if self.files.is_folder_at(row) {
            if let Some(name) = self.files.name_at(row) {
                self.open(&name);
            }
        } else if let Some(facts) = self.files.facts_at(row) {
            self.browser.select_only(row);

            let details = Details::show(self, facts);
            *self.details.borrow_mut() = Some(details);
        }
    }

    pub fn details(&self) -> Option<Rc<Details>> {
        self.details.borrow().clone()
    }

    pub fn details_closed(&self) {
        *self.details.borrow_mut() = None;
    }

    /// What the menu and the shortcuts may do follows what is picked as much as what the
    /// connection can do: renaming is for one thing, downloading for any number.
    pub fn sync_selection_actions(&self) {
        let selected = self.browser.selected_positions();
        let one = selected.len() == 1;
        let any = !selected.is_empty();
        let on_folder = selected.first().is_some_and(|row| self.files.is_folder_at(*row));
        let screen = self.screen.borrow();

        self.actions.open.set_enabled(one && on_folder);
        self.actions.download.set_enabled(any);
        self.actions.rename.set_enabled(one && screen.can_rename);
        self.actions.delete.set_enabled(any);
    }

    pub fn prompt_new_folder(self: &Rc<Self>) {
        if !self.screen.borrow().can_create_folder {
            return;
        }

        let weak = self.weak.clone();

        dialogs::name(&self.gtk, "New folder", "Folder name", "Create", "", "", move |name| {
            if let Some(window) = weak.upgrade() {
                window.create_folder(&name);
            }
        });
    }

    pub fn prompt_rename(self: &Rc<Self>) {
        let Some(original) = self.browser.selected_names().into_iter().next() else {
            return;
        };

        if !self.screen.borrow().can_rename {
            return;
        }

        let warning = match self.screen.borrow().rename_is_a_copy {
            true => {
                "On this kind of storage a rename is a copy and a delete, which takes as long \
                 as the file is big."
            }
            false => "",
        };

        let weak = self.weak.clone();
        let was = original.clone();

        dialogs::name(&self.gtk, "Rename", "Name", "Rename", warning, &original, move |name| {
            if name != was && let Some(window) = weak.upgrade() {
                window.rename(&was, &name);
            }
        });
    }

    pub fn confirm_delete(self: &Rc<Self>) {
        let names = self.browser.selected_names();

        if names.is_empty() {
            return;
        }

        let what = match names.len() {
            1 => format!("Delete {}?", names[0]),
            count => format!("Delete {count} items?"),
        };

        let weak = self.weak.clone();

        dialogs::confirm(
            &self.gtk,
            &what,
            "There is no trash on a remote server — this cannot be undone.",
            "Delete",
            move || {
                if let Some(window) = weak.upgrade() {
                    window.remove(names.clone());
                }
            },
        );
    }

    /// Asks where to put the selected files, then brings them down.
    pub fn download_selected(self: &Rc<Self>) {
        let names = self.browser.selected_names();

        if names.is_empty() {
            return;
        }

        let weak = self.weak.clone();
        let chooser = gtk::FileDialog::builder().title("Download to").modal(true).build();
        let parent = self.gtk.clone();

        glib::spawn_future_local(async move {
            let Ok(folder) = chooser.select_folder_future(Some(&parent)).await else {
                return;
            };

            if let (Some(path), Some(window)) = (folder.path(), weak.upgrade()) {
                window.download(names, path);
            }
        });
    }

    pub fn cut(&self) {
        self.begin_move(self.browser.selected_names());
    }

    pub fn paste(&self) {
        if let Some(carry) = self.carrying() {
            let folder = self.screen.borrow().path();

            self.move_into(&carry.payload, &folder);
        }
    }

    pub fn compose_connection(self: &Rc<Self>) {
        crate::editor::compose(self);
    }

    pub fn amend_connection(self: &Rc<Self>, id: &str) {
        crate::editor::amend(self, id);
    }

    pub fn complain(&self, message: impl std::fmt::Display) {
        self.screen.borrow_mut().complain(message);
        self.render();
    }

    fn go_to(&self, path: RemotePath) {
        self.command(move |session| Command::Open { session, path });
    }

    /// Sends a command for whichever connection is open. With none open there is nothing the
    /// interface could have asked for, so there is nothing to report either.
    fn command(&self, build: impl FnOnce(SessionId) -> Command) {
        let session = self.screen.borrow().session;

        if let Some(session) = session {
            crate::running::engine().send(build(session));
        }
    }

    fn receive(self: &Rc<Self>, event: &Arc<Event>) {
        self.screen.borrow_mut().receive(event);
        self.render();

        if let Event::Ask(prompt) = event.as_ref()
            && self.ours(prompt.concern)
        {
            self.pending.borrow_mut().push_back(Arc::clone(event));
            self.ask_the_next_question();
        }
    }

    /// Whether a question is this window's to ask.
    ///
    /// Stricter than what [`Screen`] shows, and deliberately so: a message in two windows is
    /// repetition, but a question in two windows is one of them left holding a dialog about
    /// something the other has already answered.
    fn ours(&self, concern: Concern) -> bool {
        match concern {
            Concern::Everyone => self.primary(),
            Concern::Attempt(attempt) => self.screen.borrow().attempt == Some(attempt),
            Concern::Session(session) => self.screen.borrow().session == Some(session),
        }
    }

    /// Puts the oldest unanswered question on screen, if none is up already.
    fn ask_the_next_question(self: &Rc<Self>) {
        if *self.asking.borrow() {
            return;
        }

        let Some(waiting) = self.pending.borrow().front().cloned() else {
            return;
        };

        let Event::Ask(prompt) = waiting.as_ref() else {
            return;
        };

        *self.asking.borrow_mut() = true;

        let asked = dialogs::describe(&prompt.question);
        let weak = self.weak.clone();

        dialogs::ask(&self.gtk, &asked, move |reply| {
            let Some(window) = weak.upgrade() else {
                return;
            };

            let answer = match reply {
                Reply::Declined => Answer::Decline,
                Reply::Alternative => Answer::KeepBoth,
                Reply::Accepted { first, second } if asked.wants_pair => {
                    Answer::Pair { id: first, secret: second }
                }
                Reply::Accepted { first, .. } if asked.wants_text => Answer::Text(first),
                Reply::Accepted { .. } => Answer::Accept,
            };

            window.reply(answer);
        });
    }

    /// Answers the question on screen and moves on to whatever is behind it.
    fn reply(self: &Rc<Self>, answer: Answer) {
        let asked = self.pending.borrow_mut().pop_front();

        if let Some(Event::Ask(prompt)) = asked.as_deref() {
            prompt.answer(answer);
        }

        *self.asking.borrow_mut() = false;
        self.ask_the_next_question();
    }

    /// Copies what the window is showing onto its widgets.
    fn render(&self) {
        let screen = self.screen.borrow();
        let connected = screen.connected;

        self.gtk.set_title(Some(&match connected {
            true => format!("{} — Okuri", screen.label),
            false => "Okuri".to_owned(),
        }));

        self.files.follow(screen.session);

        self.up.set_sensitive(connected && !screen.at_root());
        self.refresh.set_sensitive(connected);
        self.title.set_visible_child_name(match connected {
            true => "trail",
            false => "name",
        });
        self.new_connection.set_visible(!connected);
        self.display.set_visible(connected);
        self.disconnect.set_visible(connected);

        let page = match connected {
            true => "browser",
            false => "picker",
        };

        // Keys go to the list from the moment it appears, so nothing has to be clicked first.
        if self.stack.visible_child_name().as_deref() != Some(page) {
            self.stack.set_visible_child_name(page);

            if connected {
                self.browser.focus();
            }
        }

        self.actions.new_folder.set_enabled(screen.can_create_folder);
        self.notice.show(&screen.message, screen.message_is_grave);
        self.picker.set_connecting_to(&screen.connecting_id());

        let trail = (screen.label.clone(), screen.path());
        drop(screen);

        if self.trail.borrow().as_ref() != Some(&trail) {
            *self.trail.borrow_mut() = Some(trail);
            self.breadcrumb.reload(self);
        }

        self.sync_selection_actions();
        self.render_transfers();
        self.render_waiting();

        let details = self.details.borrow().clone();

        if let Some(details) = details {
            details.refresh(self);
        }
    }

    fn render_transfers(&self) {
        let queue = crate::transfers::queue();
        let active = queue.active();

        self.transfers_count.set_visible(active > 0);
        self.transfers_count.set_text(&active.to_string());

        // The queue is one queue for the whole application, so a window with nothing open can
        // still be the one you happen to be looking at while a transfer started somewhere
        // else is running.
        self.transfers
            .set_sensitive(self.screen.borrow().connected || queue.count() > 0);

        match active > 0 {
            true => self.transfers.add_css_class("suggested-action"),
            false => self.transfers.remove_css_class("suggested-action"),
        }
    }

    fn render_waiting(&self) {
        self.spinner.set_spinning(self.files.working() && self.files.count() > 0);
        self.browser.render_waiting();
    }

    /// Copies the display settings onto the controls that show them.
    fn apply_view_settings(&self) {
        let settings = view::current();
        let grid = settings.mode == Mode::Grid;

        self.display.set_icon_name(match grid {
            true => "view-list-symbolic",
            false => "view-grid-symbolic",
        });
        self.display.set_tooltip_text(Some(match grid {
            true => "List view",
            false => "Grid view",
        }));

        let direction = match settings.sort_descending {
            true => "desc",
            false => "asc",
        };

        self.actions
            .sort
            .set_state(&format!("{}:{direction}", settings.sort_column).to_variant());
        self.actions.show_hidden.set_state(&settings.show_hidden.to_variant());
        self.actions.zoom_in.set_enabled(settings.can_grow());
        self.actions.zoom_out.set_enabled(settings.can_shrink());
        self.actions.columns.set_enabled(!grid);

        self.browser.apply_settings(&settings);
    }
}

impl Actions {
    fn install(gtk: &adw::ApplicationWindow, weak: &Weak<Window>) -> Self {
        let plain = |name: &str, act: fn(&Rc<Window>)| {
            let action = gio::SimpleAction::new(name, None);
            let weak = weak.clone();

            action.connect_activate(move |_, _| {
                if let Some(window) = weak.upgrade() {
                    act(&window);
                }
            });

            gtk.add_action(&action);

            action
        };

        plain("up", |window| window.up());
        plain("refresh", |window| window.refresh());
        plain("new-window", |window| window.another());
        plain("new-connection", |window| window.compose_connection());
        plain("disconnect", |window| window.disconnect());
        plain("show-transfers", |window| crate::transfers::present(&window.gtk));
        plain("cut", |window| window.cut());
        plain("paste", |window| window.paste());

        let open = plain("open", |window| {
            if let Some(row) = window.browser.selected_positions().into_iter().next() {
                window.open_row(row);
            }
        });
        let download = plain("download", |window| window.download_selected());
        let delete = plain("delete", |window| window.confirm_delete());
        let new_folder = plain("new-folder", |window| window.prompt_new_folder());
        let rename = plain("rename", |window| window.prompt_rename());
        let columns = plain("columns", |window| dialogs::columns(&window.gtk));
        let zoom_in = plain("zoom-in", |_| view::update(|settings| settings.size_step = settings.resized(1)));
        let zoom_out = plain("zoom-out", |_| view::update(|settings| settings.size_step = settings.resized(-1)));

        // Stateful, so the menu draws them as the radio and the tick they are.
        let sort = gio::SimpleAction::new_stateful("sort", Some(glib::VariantTy::STRING), &"name:asc".to_variant());
        sort.connect_activate(|_, chosen| {
            let Some(chosen) = chosen.and_then(String::from_variant) else {
                return;
            };

            let (column, direction) = chosen.split_once(':').unwrap_or((&chosen, "asc"));
            let (column, descending) = (column.to_owned(), direction == "desc");

            view::update(move |settings| {
                settings.sort_column = column;
                settings.sort_descending = descending;
            });
        });
        gtk.add_action(&sort);

        let show_hidden = gio::SimpleAction::new_stateful("show-hidden", None, &false.to_variant());
        show_hidden.connect_activate(|_, _| view::update(|settings| settings.show_hidden = !settings.show_hidden));
        gtk.add_action(&show_hidden);

        Self { new_folder, open, download, rename, delete, columns, sort, show_hidden, zoom_in, zoom_out }
    }
}

/// How the list is shown: how big, in what order, and with which columns.
fn view_menu() -> gtk::PopoverMenu {
    let menu = gio::Menu::new();

    // Icon size, as two steps on one line rather than a slider — and it changes both views,
    // so it sits above the rest rather than inside either one.
    let size = gio::Menu::new();
    let stepper = gio::MenuItem::new(None, None);
    stepper.set_attribute_value("custom", Some(&"size".to_variant()));
    size.append_item(&stepper);
    menu.append_section(None, &size);

    let sort = gio::Menu::new();

    for (label, target) in [
        ("A–Z", "name:asc"),
        ("Z–A", "name:desc"),
        ("Last Modified", "modified:desc"),
        ("First Modified", "modified:asc"),
        ("Size", "size:desc"),
        ("Type", "kind:asc"),
    ] {
        let item = gio::MenuItem::new(Some(label), None);
        item.set_action_and_target_value(Some("win.sort"), Some(&target.to_variant()));
        sort.append_item(&item);
    }

    menu.append_section(Some("Sort"), &sort);

    let rest = gio::Menu::new();
    rest.append(Some("Show Hidden Files"), Some("win.show-hidden"));
    rest.append(Some("Visible Columns…"), Some("win.columns"));
    menu.append_section(None, &rest);

    let label = gtk::Label::new(Some("Icon Size"));
    label.set_xalign(0.0);
    label.set_hexpand(true);

    let smaller = gtk::Button::from_icon_name("zoom-out-symbolic");
    smaller.set_tooltip_text(Some("Smaller"));
    smaller.set_action_name(Some("win.zoom-out"));

    let larger = gtk::Button::from_icon_name("zoom-in-symbolic");
    larger.set_tooltip_text(Some("Larger"));
    larger.set_action_name(Some("win.zoom-in"));

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.set_margin_start(10);
    row.set_margin_end(4);
    row.append(&label);

    for button in [&smaller, &larger] {
        button.add_css_class("flat");
        button.add_css_class("circular");
        row.append(button);
    }

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.add_child(&row, "size");

    popover
}

/// The addresses of the named files, if the desktop speaks this destination's protocol.
fn remote_urls(screen: &Screen, names: &[String]) -> Option<Vec<String>> {
    let destination = &screen.connection.as_ref()?.destination;
    let home = screen.home.trim_end_matches('/');
    let mut urls = Vec::new();

    for name in names {
        // The whole path as the server names it. What is shown in the window is relative to
        // wherever the connection starts, which is rarely the server's own root.
        let path = format!("{home}{}", screen.folder.join(name).ok()?);

        urls.push(destination.url_for(&path)?);
    }

    Some(urls)
}

/// Something worth saying, said once along the bottom rather than in a dialog you have to
/// dismiss before you can carry on.
///
/// Coloured like what it is. A failure on a tinted strip the same shade as everything else is
/// something you scroll past; a failure has to be the loudest thing on screen for as long as it
/// is there.
struct Notice {
    revealer: gtk::Revealer,
    strip: gtk::Box,
    label: gtk::Label,
}

impl Notice {
    fn new(weak: Weak<Window>) -> Self {
        let label = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .wrap(true)
            // Two lines is enough for any sentence worth reading here, and stops one long
            // answer from a server pushing the file list off the window.
            .lines(2)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();

        let dismiss = gtk::Button::with_label("Dismiss");
        dismiss.add_css_class("flat");
        dismiss.set_valign(gtk::Align::Center);
        dismiss.connect_clicked(move |_| {
            if let Some(window) = weak.upgrade() {
                window.dismiss_message();
            }
        });

        let strip = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        strip.add_css_class("okuri-notice");
        strip.append(&label);
        strip.append(&dismiss);

        let revealer = gtk::Revealer::builder()
            .child(&strip)
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .transition_duration(120)
            .reveal_child(false)
            .build();

        Self { revealer, strip, label }
    }

    fn show(&self, text: &str, grave: bool) {
        if text.is_empty() {
            self.revealer.set_reveal_child(false);
            return;
        }

        self.label.set_text(text);

        match grave {
            true => self.strip.remove_css_class("good"),
            false => self.strip.add_css_class("good"),
        }

        self.revealer.set_reveal_child(true);
    }
}

/// The path, one clickable folder at a time.
struct Breadcrumb {
    root: gtk::ScrolledWindow,
    row: gtk::Box,
}

impl Breadcrumb {
    fn new() -> Self {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);

        let root = gtk::ScrolledWindow::builder()
            .child(&row)
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_width(true)
            .hexpand(true)
            .css_classes(["okuri-trail"])
            .build();

        Self { root, row }
    }

    fn reload(&self, window: &Window) {
        while let Some(child) = self.row.first_child() {
            self.row.remove(&child);
        }

        let crumbs = window.breadcrumb();
        let here = window.screen().path();
        let label = window.screen().label.clone();
        let last = crumbs.len().saturating_sub(1);

        for (index, crumb) in crumbs.into_iter().enumerate() {
            if index > 0 {
                let separator = gtk::Label::new(Some("/"));
                separator.add_css_class("okuri-muted");
                self.row.append(&separator);
            }

            // The root has no name of its own, so it is shown as the connection, under the
            // mark every file manager uses for where a path starts.
            let name = match index {
                0 => label.clone(),
                _ => crumb.name().unwrap_or_default().to_owned(),
            };

            let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);

            if index == 0 {
                content.append(&gtk::Image::from_icon_name("go-home-symbolic"));
            }

            content.append(&gtk::Label::new(Some(&name)));

            let button = gtk::Button::new();
            button.set_child(Some(&content));
            button.add_css_class("flat");
            button.add_css_class("okuri-crumb");

            // The folder that is open is the one you are reading, and reads like it.
            match index == last {
                true => button.add_css_class("current"),
                false => button.set_sensitive(true),
            }

            let path = crumb.to_string();

            button.connect_clicked({
                let weak = window.weak.clone();
                let path = path.clone();

                move |_| {
                    if let Some(window) = weak.upgrade() {
                        window.open_path(&path);
                    }
                }
            });

            // Dropping onto a crumb is how something goes back up, and holding over one opens
            // it — so a file can go from one branch of the tree to another without ever being
            // let go of.
            let target = path != here;

            crate::browser::spring_loaded(&button, window.weak.clone(), true, move || {
                match target {
                    true => Some(path.clone()),
                    false => None,
                }
            });

            self.row.append(&button);
        }

        // Scrolled to the end once laid out, so the folder that is open is the one in view.
        let root = self.root.clone();

        glib::idle_add_local_once(move || {
            let adjustment = root.hadjustment();
            adjustment.set_value(adjustment.upper() - adjustment.page_size());
        });
    }
}
