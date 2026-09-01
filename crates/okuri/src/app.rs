use std::path::PathBuf;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;

use okuri_core::RemotePath;

use crate::screen::{Carried, Screen};
use okuri_engine::engine::Command;
use okuri_engine::transfer::Place;
use okuri_engine::{Answer, Attempt, Event, Question, SessionId};
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QString, QStringList};

use crate::format;

/// What QML talks to.
///
/// Holds the current connection and folder, turns clicks and keystrokes into engine commands,
/// and puts questions from the engine on screen. It contains no logic about how to reach a
/// server and none about how to draw a list — only about what the person asked for.
///
/// One per window rather than one per application. Two windows are two connections, two open
/// folders and two selections, and the engine has always been able to hold several sessions at
/// once — this was the only thing insisting there be one of everything.
#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        /// Which connection this window has open, or zero. Read by the file list, so that a
        /// listing arriving for another window's connection is not drawn over this one's.
        #[qproperty(i64, session)]
        /// Whether this is the window Okuri opened with. It takes the connection named on the
        /// command line, and it is where a question that belongs to no connection is asked.
        #[qproperty(bool, primary)]
        #[qproperty(bool, connected)]
        #[qproperty(bool, connecting)]
        /// Which saved connection is being opened, or empty. The row that was clicked is where
        /// somebody is looking, so it is where the waiting has to be shown.
        #[qproperty(QString, connecting_to)]
        #[qproperty(QString, label)]
        #[qproperty(QString, path)]
        /// Where the open folder sits on the server, which is not what the breadcrumb shows.
        #[qproperty(QString, absolute_path)]
        #[qproperty(bool, at_root)]
        #[qproperty(bool, can_rename)]
        #[qproperty(bool, can_create_folder)]
        #[qproperty(bool, rename_is_a_copy)]
        /// Whether this destination can hand files to people with no account.
        #[qproperty(bool, can_share)]
        /// Whether a file's mode can be changed here.
        #[qproperty(bool, can_set_permissions)]
        /// What the last answer about a shared file said, for the panel showing it.
        #[qproperty(QString, shared_name)]
        #[qproperty(bool, shared_is_public)]
        /// Whether the store would say at all. A file whose permissions cannot be read is not
        /// the same as a private one, and the switch must not claim otherwise.
        #[qproperty(bool, shared_is_known)]
        #[qproperty(QString, shared_why_not)]
        #[qproperty(QString, shared_url)]
        /// A link that works for a week without an account, once one has been asked for.
        #[qproperty(QString, signed_url)]
        /// Everything else the destination said about the file being looked at, as label and
        /// value in the order it arrived.
        #[qproperty(QStringList, facts)]
        /// Whether that answer is still on its way, and which rows it will fill when it lands.
        #[qproperty(bool, describing)]
        #[qproperty(QStringList, expected_facts)]
        #[qproperty(QString, message)]
        /// Whether the message is bad news, so the strip can be coloured like it.
        #[qproperty(bool, message_is_grave)]
        /// Where the files being dragged live, for whatever they are dropped on.
        #[qproperty(QStringList, drag_urls)]
        /// What a drag is carrying, as the text it carries it in.
        ///
        /// Goes into the drag itself rather than being read back off this object when a drop
        /// lands, because a drop can land in another window — and that window cannot ask this
        /// one what was picked up. Kept here so the drag can be handed it, and so pasting,
        /// which has no drop to read, has somewhere to get it from.
        #[qproperty(QString, drag_payload)]
        #[qproperty(bool, asking)]
        #[qproperty(QString, question_title)]
        #[qproperty(QString, question_body)]
        #[qproperty(QString, question_detail)]
        #[qproperty(bool, question_wants_text)]
        #[qproperty(bool, question_wants_pair)]
        #[qproperty(QString, question_first_label)]
        #[qproperty(QString, question_second_label)]
        #[qproperty(bool, question_is_secret)]
        #[qproperty(bool, question_is_grave)]
        #[qproperty(QString, question_accept)]
        /// The third choice, when the question has one. Empty when it does not.
        #[qproperty(QString, question_alternative)]
        type App = super::AppRust;

        #[qinvokable]
        fn connect_to(self: Pin<&mut App>, id: QString);
        #[qinvokable]
        fn disconnect(self: Pin<&mut App>);

        /// Asks for a saved connection's credentials again and keeps what is given.
        #[qinvokable]
        fn change_credentials(self: Pin<&mut App>, id: QString);

        /// Asks for everything the destination knows about a file.
        #[qinvokable]
        fn describe(self: Pin<&mut App>, name: QString);

        /// Asks who can read a file, answered by `shared*` and the `sharedChanged` signal.
        #[qinvokable]
        fn share(self: Pin<&mut App>, name: QString);

        /// Changes who can read a file, then reports where it stands.
        #[qinvokable]
        fn reshare(self: Pin<&mut App>, name: QString, is_public: bool);

        /// Signs a link to a file that works for a week without an account.
        #[qinvokable]
        fn sign_link(self: Pin<&mut App>, name: QString);

        /// Changes a file's mode.
        #[qinvokable]
        fn set_permissions(self: Pin<&mut App>, name: QString, mode: i32);

        #[qinvokable]
        fn open(self: Pin<&mut App>, name: QString);
        #[qinvokable]
        fn open_path(self: Pin<&mut App>, path: QString);
        #[qinvokable]
        fn up(self: Pin<&mut App>);
        #[qinvokable]
        fn refresh(self: &App);

        #[qinvokable]
        fn create_folder(self: &App, name: QString);
        #[qinvokable]
        fn rename(self: &App, from: QString, to: QString);
        #[qinvokable]
        fn remove(self: &App, names: QStringList);

        /// Files dropped from a file manager, as `file://` URLs.
        #[qinvokable]
        fn drop_urls(self: &App, urls: QStringList);
        #[qinvokable]
        fn download(self: &App, names: QStringList, folder: QString);
        #[qinvokable]
        fn cancel_transfer(self: &App, id: i64);

        /// Notes what a drag is carrying, so anywhere it might be dropped knows — inside the
        /// window and out of it alike.
        #[qinvokable]
        fn begin_move(self: Pin<&mut App>, names: QStringList);

        /// Puts what a drop is carrying into a folder on this window's connection.
        ///
        /// The payload comes from the drop itself, because the drag may have started in another
        /// window. Whether that means renaming the files or carrying their bytes across is the
        /// engine's to decide from the two connections.
        #[qinvokable]
        fn move_into(self: Pin<&mut App>, payload: QString, folder: QString);

        #[qinvokable]
        fn end_move(self: Pin<&mut App>);

        #[qinvokable]
        fn answer(self: Pin<&mut App>, accepted: bool, first: QString, second: QString);

        /// Takes the question's third choice, for the questions that offer one.
        #[qinvokable]
        fn answer_alternative(self: Pin<&mut App>);

        #[qinvokable]
        fn dismiss_message(self: Pin<&mut App>);

        /// The breadcrumb: every folder from the root down to the one that is open.
        #[qinvokable]
        fn breadcrumb(self: &App) -> QStringList;
    }

    impl cxx_qt::Threading for App {}

    impl cxx_qt::Initialize for App {}
}

pub struct AppRust {
    session: i64,
    primary: bool,
    connected: bool,
    connecting: bool,
    connecting_to: QString,
    label: QString,
    path: QString,
    absolute_path: QString,
    at_root: bool,
    can_rename: bool,
    can_create_folder: bool,
    rename_is_a_copy: bool,
    message: QString,
    message_is_grave: bool,
    drag_urls: QStringList,
    drag_payload: QString,
    asking: bool,
    question_title: QString,
    question_body: QString,
    question_detail: QString,
    question_wants_text: bool,
    question_wants_pair: bool,
    question_first_label: QString,
    question_second_label: QString,
    question_is_secret: bool,
    question_is_grave: bool,
    question_accept: QString,
    question_alternative: QString,

    can_share: bool,
    can_set_permissions: bool,
    shared_name: QString,
    shared_is_public: bool,
    shared_is_known: bool,
    shared_why_not: QString,
    shared_url: QString,
    signed_url: QString,
    facts: QStringList,
    describing: bool,
    expected_facts: QStringList,

    /// What the window is showing. Every rule about it lives in [`Screen`], where it can be
    /// tested; this object only copies the answers onto properties.
    screen: Screen,
    /// The questions waiting to be answered, oldest first.
    ///
    /// A queue rather than one slot: two connections opening at once ask two questions, and
    /// overwriting the first would drop its prompt — which answers it by declining, so one of
    /// the two connections would fail for no reason anybody could see.
    pending: VecDeque<Arc<Event>>,
}

impl Default for AppRust {
    fn default() -> Self {
        Self {
            session: 0,
            primary: false,
            connected: false,
            connecting: false,
            connecting_to: QString::default(),
            label: QString::default(),
            path: QString::from("/"),
            absolute_path: QString::from("/"),
            at_root: true,
            can_rename: false,
            can_create_folder: false,
            rename_is_a_copy: false,
            message: QString::default(),
            message_is_grave: true,
            drag_urls: QStringList::default(),
            drag_payload: QString::default(),
            asking: false,
            question_title: QString::default(),
            question_body: QString::default(),
            question_detail: QString::default(),
            question_wants_text: false,
            question_wants_pair: false,
            question_first_label: QString::default(),
            question_second_label: QString::default(),
            question_is_secret: false,
            question_is_grave: false,
            question_accept: QString::default(),
            question_alternative: QString::default(),

            can_share: false,
            can_set_permissions: false,
            shared_name: QString::default(),
            shared_is_public: false,
            shared_is_known: false,
            shared_why_not: QString::default(),
            shared_url: QString::default(),
            signed_url: QString::default(),
            facts: QStringList::default(),
            describing: false,
            expected_facts: QStringList::default(),

            screen: Screen::default(),
            pending: VecDeque::new(),
        }
    }
}

impl cxx_qt::Initialize for qobject::App {
    fn initialize(mut self: Pin<&mut Self>) {
        let thread = self.as_mut().qt_thread();

        // Kept for as long as the process runs, not for as long as the window does. A window
        // that has been closed has had its `App` destroyed with it, and `queue` on a dead
        // object is dropped rather than delivered — so an unsubscribe here would buy nothing
        // and cost a listener that has to outlive whatever is holding it.
        crate::bus::listen(move |event| {
            let event = Arc::clone(event);

            crate::qt::queue(&thread, move |app| app.receive(event));
        })
        .forever();

        // The first window is the one Okuri opened with, and the only one there is when the
        // command line is read.
        static OPENED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

        if OPENED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }

        self.as_mut().set_primary(true);

        // `okuri production-web` opens that connection straight away, which is what you want
        // from a launcher, a keybinding, or a terminal you are already standing in.
        if let Some(id) = std::env::args().nth(1) {
            self.connect_to(QString::from(&id));
        }
    }
}

impl qobject::App {
    /// Connections are re-read here rather than remembered, so one saved a moment ago in the
    /// editor is connectable without anything having to be told about it.
    pub fn connect_to(mut self: Pin<&mut Self>, id: QString) {
        let Some(connection) = crate::store::load().find(&id.to_string()).cloned() else {
            self.as_mut().complain(format!("there is no connection called {id}"));
            return;
        };

        let attempt = Attempt::next();

        self.as_mut().rust_mut().screen.connecting_to(attempt, connection.clone());
        self.as_mut().show();

        crate::running::engine().send(Command::Connect {
            attempt,
            connection: Box::new(connection),
        });
    }

    /// Asks for everything the destination knows about a file, for the panel showing one.
    pub fn describe(mut self: Pin<&mut Self>, name: QString) {
        // Cleared first: what is on screen belongs to whichever file was looked at before.
        self.as_mut().rust_mut().screen.described = Default::default();
        self.as_mut().rust_mut().screen.describing = true;

        let name = name.to_string();
        self.command(move |session| Command::Describe { session, name });
    }

    pub fn share(mut self: Pin<&mut Self>, name: QString) {
        // Cleared first, so a panel opened on a second file never shows the first one's answer
        // while the server is still being asked.
        self.as_mut().set_shared_name(name.clone());
        self.as_mut().set_shared_url(QString::default());
        self.as_mut().rust_mut().screen.shared_public = None;
        self.as_mut().rust_mut().screen.signed_url = String::new();

        let name = name.to_string();
        self.command(move |session| Command::Share { session, name });
    }

    pub fn reshare(self: Pin<&mut Self>, name: QString, is_public: bool) {
        let name = name.to_string();

        self.command(move |session| Command::Reshare { session, name, public: is_public });
    }

    pub fn set_permissions(self: Pin<&mut Self>, name: QString, mode: i32) {
        let name = name.to_string();
        let mode = u32::try_from(mode).unwrap_or_default() & 0o777;

        self.command(move |session| Command::SetPermissions { session, name, mode });
    }

    pub fn sign_link(self: Pin<&mut Self>, name: QString) {
        let name = name.to_string();

        self.command(move |session| Command::SignLink { session, name });
    }

    /// Asked for under an attempt of its own, so the questions it puts up are asked by this
    /// window and not by whichever one happens to be listening.
    pub fn change_credentials(mut self: Pin<&mut Self>, id: QString) {
        let Some(connection) = crate::store::load().find(&id.to_string()).cloned() else {
            self.as_mut().complain(format!("there is no connection called {id}"));
            return;
        };

        let attempt = Attempt::next();
        self.as_mut().rust_mut().screen.attempt = Some(attempt);

        crate::running::engine().send(Command::ChangeCredentials {
            attempt,
            connection: Box::new(connection),
        });
    }

    pub fn disconnect(self: Pin<&mut Self>) {
        if let Some(session) = self.rust().screen.session {
            crate::running::engine().send(Command::Disconnect(session));
        }
    }

    pub fn open(self: Pin<&mut Self>, name: QString) {
        let folder = self.rust().screen.folder.clone();

        match folder.join(&name.to_string()) {
            Ok(path) => self.go_to(path),
            Err(error) => self.complain(error.to_string()),
        }
    }

    pub fn open_path(self: Pin<&mut Self>, path: QString) {
        match RemotePath::parse(&path.to_string()) {
            Ok(path) => self.go_to(path),
            Err(error) => self.complain(error.to_string()),
        }
    }

    pub fn up(self: Pin<&mut Self>) {
        if let Some(parent) = self.rust().screen.folder.parent() {
            self.go_to(parent);
        }
    }

    pub fn refresh(&self) {
        self.command(Command::Refresh);
    }

    pub fn create_folder(&self, name: QString) {
        let name = name.to_string();

        self.command(move |session| Command::CreateFolder { session, name });
    }

    pub fn rename(&self, from: QString, to: QString) {
        let (from, to) = (from.to_string(), to.to_string());

        self.command(move |session| Command::Rename { session, from, to });
    }

    pub fn remove(&self, names: QStringList) {
        let names = strings(&names);

        self.command(move |session| Command::Delete { session, names });
    }

    pub fn drop_urls(&self, urls: QStringList) {
        let sources = strings(&urls)
            .iter()
            .filter_map(|url| format::path_from_url(url))
            .collect::<Vec<PathBuf>>();

        if sources.is_empty() {
            return;
        }

        let into = self.rust().screen.folder.clone();

        self.command(move |session| Command::Upload { session, into, sources });
    }

    pub fn download(&self, names: QStringList, folder: QString) {
        let names = strings(&names);
        let Some(into) = format::path_from_url(&folder.to_string()) else {
            return;
        };

        self.command(move |session| Command::Download { session, names, into });
    }

    /// Writes down what is being dragged, for the drag to carry.
    ///
    /// Worked out now rather than when the pointer reaches the edge: a drag that leaves the
    /// window is taken over by the desktop, and by then there is no chance to add anything to
    /// what it carries — including the answer to which window it came from.
    pub fn begin_move(mut self: Pin<&mut Self>, names: QStringList) {
        let Some(session) = self.rust().screen.session else {
            return;
        };

        let names = strings(&names);
        let urls = self.remote_urls(&names).unwrap_or_default();

        let carried = Carried {
            session,
            folder: self.rust().screen.folder.clone(),
            names,
        };

        self.as_mut().set_drag_urls(urls);
        self.as_mut().set_drag_payload(QString::from(&carried.payload()));
    }

    /// Puts whatever `payload` describes into `folder` on this window's connection.
    ///
    /// What is being moved comes from the drop rather than from this object, because the drop
    /// may have started in another window — and that window's `App` is not this one. It is also
    /// what makes a drop safe to offer to several targets at once: nothing is being consumed,
    /// so a target that turns out not to be the one meant leaves nothing missing behind it.
    pub fn move_into(mut self: Pin<&mut Self>, payload: QString, folder: QString) {
        let Some(session) = self.rust().screen.session else {
            return;
        };

        let Some(carried) = Carried::parse(&payload.to_string()) else {
            self.as_mut()
                .complain("that drop did not say what it was carrying");
            return;
        };

        let Ok(path) = RemotePath::parse(&folder.to_string()) else {
            self.as_mut()
                .complain(format!("{folder} is not a folder this connection has"));
            return;
        };

        let from = Place::new(carried.session, carried.folder);
        let into = Place::new(session, path);

        crate::running::engine().send(Command::Move { from, names: carried.names, into });
    }

    pub fn end_move(mut self: Pin<&mut Self>) {
        self.as_mut().set_drag_payload(QString::default());
        self.as_mut().set_drag_urls(QStringList::default());
    }

    /// The addresses of the named files, if the desktop speaks this destination's protocol.
    fn remote_urls(&self, names: &[String]) -> Option<QStringList> {
        let destination = &self.rust().screen.connection.as_ref()?.destination;
        let folder = self.rust().screen.folder.clone();
        let home = self.rust().screen.home.trim_end_matches('/').to_owned();
        let mut urls = Vec::new();

        for name in names {
            // The whole path as the server names it. What is shown in the window is relative
            // to wherever the connection starts, which is rarely the server's own root.
            let path = format!("{home}{}", folder.join(name).ok()?);

            urls.push(QString::from(&destination.url_for(&path)?));
        }

        Some(urls.into_iter().collect())
    }

    pub fn cancel_transfer(&self, id: i64) {
        // A row with no transfer under it answers `-1`, which is not a transfer to cancel.
        if let Ok(id) = u64::try_from(id) {
            crate::running::engine()
                .send(Command::CancelTransfer(okuri_engine::TransferId(id)));
        }
    }

    pub fn answer(self: Pin<&mut Self>, accepted: bool, first: QString, second: QString) {
        let answer = if !accepted {
            Answer::Decline
        } else if self.rust().question_wants_pair {
            Answer::Pair { id: first.to_string(), secret: second.to_string() }
        } else if self.rust().question_wants_text {
            Answer::Text(first.to_string())
        } else {
            Answer::Accept
        };

        self.reply(answer);
    }

    /// Takes the question's third choice, for the questions that offer one.
    pub fn answer_alternative(self: Pin<&mut Self>) {
        self.reply(Answer::KeepBoth);
    }

    /// Answers the question on screen and moves on to whatever is behind it.
    fn reply(mut self: Pin<&mut Self>, answer: Answer) {
        let asked = self.as_mut().rust_mut().pending.pop_front();

        if let Some(Event::Ask(prompt)) = asked.as_deref() {
            prompt.answer(answer);
        }

        self.ask_the_next_question();
    }

    /// Puts the oldest unanswered question on screen, or takes the dialog away when there are
    /// none left.
    fn ask_the_next_question(mut self: Pin<&mut Self>) {
        let Some(waiting) = self.rust().pending.front().cloned() else {
            self.as_mut().set_asking(false);
            return;
        };

        if let Event::Ask(prompt) = waiting.as_ref() {
            self.as_mut().pose(&prompt.question);
            self.as_mut().set_asking(true);
        }
    }

    pub fn dismiss_message(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().screen.message = String::new();
        self.show();
    }

    pub fn breadcrumb(&self) -> QStringList {
        self.rust()
            .screen
            .folder
            .ancestors()
            .iter()
            .map(|ancestor| QString::from(&ancestor.to_string()))
            .collect()
    }

    fn go_to(self: Pin<&mut Self>, path: RemotePath) {
        self.command(move |session| Command::Open { session, path });
    }

    /// Sends a command for whichever connection is open. With none open there is nothing the
    /// interface could have asked for, so there is nothing to report either.
    fn command(&self, build: impl FnOnce(SessionId) -> Command) {
        if let Some(session) = self.rust().screen.session {
            crate::running::engine().send(build(session));
        }
    }

    fn complain(mut self: Pin<&mut Self>, message: impl std::fmt::Display) {
        self.as_mut().rust_mut().screen.complain(message);
        self.show();
    }

    fn receive(mut self: Pin<&mut Self>, event: Arc<Event>) {
        self.as_mut().rust_mut().screen.receive(&event);
        self.as_mut().show();

        if let Event::Ask(prompt) = event.as_ref()
            && self.ours(prompt.concern)
        {
            self.as_mut().rust_mut().pending.push_back(Arc::clone(&event));
            self.ask_the_next_question();
        }
    }

    /// Whether a question is this window's to ask.
    ///
    /// Stricter than what [`Screen`] shows, and deliberately so: a message in two windows is
    /// repetition, but a question in two windows is one of them left holding a dialog about
    /// something the other has already answered. A question belonging to no connection is the
    /// first window's, because somebody has to ask it and exactly one of them may.
    fn ours(&self, concern: okuri_engine::Concern) -> bool {
        match concern {
            okuri_engine::Concern::Everyone => self.rust().primary,
            okuri_engine::Concern::Attempt(attempt) => {
                self.rust().screen.attempt == Some(attempt)
            }
            okuri_engine::Concern::Session(session) => {
                self.rust().screen.session == Some(session)
            }
        }
    }

    /// Copies what the window is showing onto the properties QML binds to.
    ///
    /// Assigned through the generated setters rather than replaced wholesale, so Qt emits a
    /// change signal for what actually moved and nothing else redraws.
    fn show(mut self: Pin<&mut Self>) {
        let screen = &self.rust().screen;

        // Zero for no connection. The file list binds to this, and a list still showing the
        // session it had is a list drawing another window's folder.
        let session = screen.session.map(|it| it.0 as i64).unwrap_or_default();
        let (connecting, connected) = (screen.connecting, screen.connected);
        let connecting_to = screen.connecting_id();
        let (label, message) = (screen.label.clone(), screen.message.clone());
        let grave = screen.message_is_grave;
        let (can_rename, is_a_copy) = (screen.can_rename, screen.rename_is_a_copy);
        let can_create_folder = screen.can_create_folder;
        let (path, at_root) = (screen.path(), screen.at_root());
        let absolute = screen.absolute_path();
        let can_share = screen.can_share;
        let can_set_permissions = screen.can_set_permissions;
        let (shared_public, shared_url) = (screen.shared_public, screen.shared_url.clone());
        let shared_why_not = screen.shared_why_not.clone();
        let signed_url = screen.signed_url.clone();

        // Label and value alternating, because a list of pairs is the one shape that survives
        // the trip into QML without inventing a type for it.
        let describing = screen.describing;
        let expected = screen
            .expected()
            .into_iter()
            .map(QString::from)
            .collect::<QStringList>();

        let facts = screen
            .described
            .rows()
            .iter()
            .flat_map(|(label, said)| [QString::from(*label), QString::from(said)])
            .collect::<QStringList>();

        self.as_mut().set_session(session);
        self.as_mut().set_connecting(connecting);
        self.as_mut().set_connecting_to(QString::from(&connecting_to));
        self.as_mut().set_connected(connected);
        self.as_mut().set_label(QString::from(&label));
        self.as_mut().set_can_rename(can_rename);
        self.as_mut().set_rename_is_a_copy(is_a_copy);
        self.as_mut().set_can_create_folder(can_create_folder);
        self.as_mut().set_path(QString::from(&path));
        self.as_mut().set_absolute_path(QString::from(&absolute));
        self.as_mut().set_at_root(at_root);
        self.as_mut().set_message(QString::from(&message));
        self.as_mut().set_message_is_grave(grave);
        self.as_mut().set_can_share(can_share);
        self.as_mut().set_can_set_permissions(can_set_permissions);
        self.as_mut().set_shared_is_public(shared_public.unwrap_or_default());
        self.as_mut().set_shared_is_known(shared_public.is_some());
        self.as_mut().set_shared_why_not(QString::from(&shared_why_not));
        self.as_mut().set_shared_url(QString::from(&shared_url));
        self.as_mut().set_signed_url(QString::from(&signed_url));
        self.as_mut().set_facts(facts);
        self.as_mut().set_describing(describing);
        self.as_mut().set_expected_facts(expected);
    }

    /// Turns a question from the engine into the words the dialog shows.
    fn pose(mut self: Pin<&mut Self>, question: &Question) {
        let asked = describe(question);

        self.as_mut().set_question_title(QString::from(&asked.title));
        self.as_mut().set_question_body(QString::from(&asked.body));
        self.as_mut().set_question_detail(QString::from(&asked.detail));
        self.as_mut().set_question_accept(QString::from(&asked.accept));
        self.as_mut()
            .set_question_alternative(QString::from(&asked.alternative));
        self.as_mut().set_question_wants_text(asked.wants_text);
        self.as_mut().set_question_wants_pair(asked.wants_pair);
        self.as_mut()
            .set_question_first_label(QString::from(&asked.first_label));
        self.as_mut()
            .set_question_second_label(QString::from(&asked.second_label));
        self.as_mut().set_question_is_secret(asked.is_secret);
        self.as_mut().set_question_is_grave(asked.is_grave);
    }
}

struct Asked {
    title: String,
    body: String,
    detail: String,
    alternative: String,
    accept: String,
    wants_text: bool,
    wants_pair: bool,
    first_label: String,
    second_label: String,
    is_secret: bool,
    is_grave: bool,
}

impl Default for Asked {
    fn default() -> Self {
        Self {
            title: String::new(),
            body: String::new(),
            detail: String::new(),
            alternative: String::new(),
            accept: "Continue".to_owned(),
            wants_text: false,
            wants_pair: false,
            first_label: String::new(),
            second_label: String::new(),
            is_secret: false,
            is_grave: false,
        }
    }
}

fn describe(question: &Question) -> Asked {
    match question {
        Question::UnknownHostKey { host, algorithm, fingerprint } => Asked {
            title: format!("{host} is new"),
            body: format!(
                "Okuri has never connected to {host} before. Check the {algorithm} fingerprint \
                 against the one the server's administrator gave you."
            ),
            detail: fingerprint.clone(),
            accept: "Trust and connect".to_owned(),
            ..Asked::default()
        },

        Question::ChangedHostKey { host, algorithm, fingerprint } => Asked {
            title: format!("{host} is not the machine it was"),
            body: format!(
                "The {algorithm} key {host} presented is not the one on file. This happens when a \
                 server is rebuilt — and it is also what an eavesdropper looks like. Do not \
                 continue unless you know the server changed."
            ),
            detail: fingerprint.clone(),
            accept: "Connect anyway".to_owned(),
            is_grave: true,
            ..Asked::default()
        },

        Question::Password { connection } => Asked {
            title: format!("Password for {connection}"),
            accept: "Connect".to_owned(),
            wants_text: true,
            is_secret: true,
            ..Asked::default()
        },

        Question::KeyPair { connection } => Asked {
            title: format!("Keys for {connection}"),
            body: "This kind of storage signs every request with an access key and its secret."
                .to_owned(),
            accept: "Connect".to_owned(),
            wants_pair: true,
            first_label: "Access key".to_owned(),
            second_label: "Secret".to_owned(),
            is_secret: true,
            ..Asked::default()
        },

        Question::KeyPassphrase { path } => Asked {
            title: "Unlock your SSH key".to_owned(),
            body: format!("{path} is encrypted."),
            accept: "Unlock".to_owned(),
            wants_text: true,
            is_secret: true,
            ..Asked::default()
        },

        Question::Passphrase => Asked {
            title: "Unlock your saved passwords".to_owned(),
            body: "Okuri keeps your passwords in an encrypted file on this machine.".to_owned(),
            accept: "Unlock".to_owned(),
            wants_text: true,
            is_secret: true,
            ..Asked::default()
        },

        Question::Overwrite { name } => Asked {
            title: format!("Replace {name}?"),
            body: format!("{name} is already there."),
            accept: "Replace".to_owned(),
            alternative: "Keep both".to_owned(),
            ..Asked::default()
        },
    }
}

fn strings(list: &QStringList) -> Vec<String> {
    cxx_qt_lib::QList::<QString>::from(list)
        .iter()
        .map(QString::to_string)
        .collect()
}

