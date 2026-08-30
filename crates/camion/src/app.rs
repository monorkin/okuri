use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use camion_core::RemotePath;
use camion_engine::engine::Command;
use camion_engine::secrets::{EncryptedFile, InMemory, Keyring};
use camion_engine::{Answer, Engine, Event, Question, SecretStore, SessionId};
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QString, QStringList};

use crate::format;

/// What QML talks to.
///
/// Holds the current connection and folder, turns clicks and keystrokes into engine commands,
/// and puts questions from the engine on screen. It contains no logic about how to reach a
/// server and none about how to draw a list — only about what the person asked for.
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
        #[qml_singleton]
        #[qproperty(bool, connected)]
        #[qproperty(bool, connecting)]
        #[qproperty(QString, label)]
        #[qproperty(QString, path)]
        #[qproperty(bool, at_root)]
        #[qproperty(bool, can_rename)]
        #[qproperty(bool, can_create_folder)]
        #[qproperty(bool, rename_is_a_copy)]
        #[qproperty(QString, message)]
        /// Where the files being dragged live, for whatever they are dropped on.
        #[qproperty(QStringList, drag_urls)]
        /// What a drag inside the window is carrying. Held here rather than in one corner of
        /// the interface, because the folder rows and the breadcrumb are both places to drop
        /// it and neither owns the other.
        #[qproperty(QStringList, moving)]
        /// The folder the drag started in. Held because it can be navigated away from before
        /// the files are put down, which is what makes the breadcrumb useful mid-drag.
        #[qproperty(QString, moving_from)]
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
        type App = super::AppRust;

        #[qinvokable]
        fn connect_to(self: Pin<&mut App>, id: QString);
        #[qinvokable]
        fn disconnect(self: Pin<&mut App>);

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

        /// Moves whatever is being dragged into a folder on the same connection.
        #[qinvokable]
        fn move_into(self: Pin<&mut App>, folder: QString);

        #[qinvokable]
        fn end_move(self: Pin<&mut App>);

        #[qinvokable]
        fn answer(self: Pin<&mut App>, accepted: bool, first: QString, second: QString);
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
    connected: bool,
    connecting: bool,
    label: QString,
    path: QString,
    at_root: bool,
    can_rename: bool,
    can_create_folder: bool,
    rename_is_a_copy: bool,
    message: QString,
    drag_urls: QStringList,
    moving: QStringList,
    moving_from: QString,
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

    engine: Engine,
    session: Option<SessionId>,
    /// What the open session was opened from, which is what knows whether the desktop can
    /// reach this destination on its own.
    connection: Option<camion_engine::Connection>,
    /// Where this connection's root sits on the server, as the server names it.
    home: String,
    folder: RemotePath,
    pending: Option<Arc<Event>>,
}

impl Default for AppRust {
    fn default() -> Self {
        Self {
            connected: false,
            connecting: false,
            label: QString::default(),
            path: QString::from("/"),
            at_root: true,
            can_rename: false,
            can_create_folder: false,
            rename_is_a_copy: false,
            message: QString::default(),
            drag_urls: QStringList::default(),
            moving: QStringList::default(),
            moving_from: QString::default(),
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

            engine: Engine::start(secret_store(), crate::bus::emitter()),
            session: None,
            connection: None,
            home: String::new(),
            folder: RemotePath::root(),
            pending: None,
        }
    }
}

/// The desktop's keyring when one is running, and a passphrase-encrypted file when none is.
///
/// The choice is made once at startup rather than per connection: it is a property of the
/// machine, and a connection that works today should not start asking differently tomorrow
/// because a daemon happened to be slow.
fn secret_store() -> Arc<dyn SecretStore> {
    if Keyring::is_available() {
        return Arc::new(Keyring);
    }

    match EncryptedFile::default_path() {
        // The passphrase is asked for by the engine on the first connection that needs a
        // secret, so an empty one here only ever opens an empty store.
        Some(path) => match EncryptedFile::open(path, "") {
            Ok(store) => Arc::new(store),
            Err(_) => Arc::new(InMemory::default()),
        },
        None => Arc::new(InMemory::default()),
    }
}

impl cxx_qt::Initialize for qobject::App {
    fn initialize(mut self: Pin<&mut Self>) {
        let thread = self.as_mut().qt_thread();

        crate::bus::listen(move |event| {
            let event = Arc::clone(event);

            let _ = thread.queue(move |app| app.receive(event));
        });

        // `camion production-web` opens that connection straight away, which is what you want
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

        self.as_mut().rust_mut().connection = Some(connection.clone());
        self.as_mut().set_connecting(true);
        self.rust().engine.send(Command::Connect(Box::new(connection)));
    }

    pub fn disconnect(mut self: Pin<&mut Self>) {
        if let Some(session) = self.rust().session {
            self.as_mut().rust_mut().engine.send(Command::Disconnect(session));
        }
    }

    pub fn open(self: Pin<&mut Self>, name: QString) {
        let folder = self.rust().folder.clone();

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
        if let Some(parent) = self.rust().folder.parent() {
            self.go_to(parent);
        }
    }

    pub fn refresh(&self) {
        self.command(Command::Refresh);
    }

    pub fn create_folder(&self, name: QString) {
        let name = name.to_string();

        self.command(move |session| Command::CreateFolder { session, name: name.clone() });
    }

    pub fn rename(&self, from: QString, to: QString) {
        let (from, to) = (from.to_string(), to.to_string());

        self.command(move |session| Command::Rename {
            session,
            from: from.clone(),
            to: to.clone(),
        });
    }

    pub fn remove(&self, names: QStringList) {
        let names = strings(&names);

        self.command(move |session| Command::Delete { session, names: names.clone() });
    }

    pub fn drop_urls(&self, urls: QStringList) {
        let sources = strings(&urls)
            .iter()
            .filter_map(|url| format::path_from_url(url))
            .collect::<Vec<PathBuf>>();

        if sources.is_empty() {
            return;
        }

        let into = self.rust().folder.clone();

        self.command(move |session| Command::Upload {
            session,
            into: into.clone(),
            sources: sources.clone(),
        });
    }

    pub fn download(&self, names: QStringList, folder: QString) {
        let names = strings(&names);
        let Some(into) = format::path_from_url(&folder.to_string()) else {
            return;
        };

        self.command(move |session| Command::Download {
            session,
            names: names.clone(),
            into: into.clone(),
        });
    }

    pub fn begin_move(mut self: Pin<&mut Self>, names: QStringList) {
        let folder = QString::from(&self.rust().folder.to_string());

        // Worked out now rather than when the pointer reaches the edge: a drag that leaves the
        // window is taken over by the desktop, and by then there is no chance to add anything
        // to what it carries.
        let urls = self
            .remote_urls(&strings(&names))
            .unwrap_or_default();

        self.as_mut().set_drag_urls(urls);
        self.as_mut().set_moving_from(folder);
        self.as_mut().set_moving(names);
    }

    pub fn move_into(mut self: Pin<&mut Self>, folder: QString) {
        let names = strings(&self.moving().clone());
        let (Ok(into), Ok(from)) = (
            RemotePath::parse(&folder.to_string()),
            RemotePath::parse(&self.moving_from().to_string()),
        ) else {
            return;
        };

        self.as_mut().set_moving(QStringList::default());

        if names.is_empty() {
            return;
        }

        self.command(move |session| Command::Move {
            session,
            from: from.clone(),
            names: names.clone(),
            into: into.clone(),
        });
    }

    pub fn end_move(mut self: Pin<&mut Self>) {
        self.as_mut().set_moving(QStringList::default());
    }

    /// The addresses of the named files, if the desktop speaks this destination's protocol.
    fn remote_urls(&self, names: &[String]) -> Option<QStringList> {
        let destination = &self.rust().connection.as_ref()?.destination;
        let folder = self.rust().folder.clone();
        let home = self.rust().home.trim_end_matches('/').to_owned();
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
        self.rust().engine.send(Command::CancelTransfer(
            camion_engine::TransferId(id.max(0) as u64),
        ));
    }

    pub fn answer(mut self: Pin<&mut Self>, accepted: bool, first: QString, second: QString) {
        let pending = self.as_mut().rust_mut().pending.take();

        if let Some(Event::Ask(prompt)) = pending.as_deref() {
            let answer = if !accepted {
                Answer::Decline
            } else if self.rust().question_wants_pair {
                Answer::Pair {
                    id: first.to_string(),
                    secret: second.to_string(),
                }
            } else if self.rust().question_wants_text {
                Answer::Text(first.to_string())
            } else {
                Answer::Accept
            };

            prompt.answer(answer);
        }

        self.as_mut().set_asking(false);
    }

    pub fn dismiss_message(mut self: Pin<&mut Self>) {
        self.as_mut().set_message(QString::default());
    }

    pub fn breadcrumb(&self) -> QStringList {
        self.rust()
            .folder
            .ancestors()
            .iter()
            .map(|ancestor| QString::from(&ancestor.to_string()))
            .collect()
    }

    fn go_to(self: Pin<&mut Self>, path: RemotePath) {
        self.command(move |session| Command::Open { session, path: path.clone() });
    }

    /// Sends a command for whichever connection is open. With none open there is nothing the
    /// interface could have asked for, so there is nothing to report either.
    fn command(&self, build: impl Fn(SessionId) -> Command) {
        if let Some(session) = self.rust().session {
            self.rust().engine.send(build(session));
        }
    }

    fn complain(mut self: Pin<&mut Self>, message: impl std::fmt::Display) {
        self.as_mut().set_message(QString::from(&message.to_string()));
    }

    fn receive(mut self: Pin<&mut Self>, event: Arc<Event>) {
        match event.as_ref() {
            Event::Connected { session, label, capabilities, home } => {
                self.as_mut().rust_mut().session = Some(*session);
                self.as_mut().rust_mut().home = home.clone();
                self.as_mut().set_connecting(false);
                self.as_mut().set_connected(true);
                self.as_mut().set_label(QString::from(label));
                self.as_mut().set_can_rename(capabilities.rename.is_available());
                self.as_mut()
                    .set_rename_is_a_copy(capabilities.rename.needs_warning());
                self.as_mut()
                    .set_can_create_folder(capabilities.create_folder.is_available());
            }

            Event::ConnectionFailed { reason, .. } => {
                self.as_mut().set_connecting(false);
                self.as_mut().complain(reason);
            }

            Event::Disconnected { .. } => {
                self.as_mut().rust_mut().session = None;
                self.as_mut().rust_mut().connection = None;
                self.as_mut().rust_mut().home = String::new();
                self.as_mut().set_connected(false);
                self.as_mut().set_label(QString::default());
                self.as_mut().rust_mut().folder = RemotePath::root();
                self.as_mut().set_path(QString::from("/"));
                self.as_mut().set_at_root(true);
            }

            Event::Listing { path, .. } => {
                let at_root = path.is_root();

                self.as_mut().rust_mut().folder = path.clone();
                self.as_mut().set_path(QString::from(&path.to_string()));
                self.as_mut().set_at_root(at_root);
            }

            Event::Failed { message } => self.as_mut().complain(message),

            Event::Ask(prompt) => {
                self.as_mut().pose(&prompt.question);
                self.as_mut().rust_mut().pending = Some(Arc::clone(&event));
                self.as_mut().set_asking(true);
            }

            _ => {}
        }
    }

    /// Turns a question from the engine into the words the dialog shows.
    fn pose(mut self: Pin<&mut Self>, question: &Question) {
        let asked = describe(question);

        self.as_mut().set_question_title(QString::from(&asked.title));
        self.as_mut().set_question_body(QString::from(&asked.body));
        self.as_mut().set_question_detail(QString::from(&asked.detail));
        self.as_mut().set_question_accept(QString::from(&asked.accept));
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
                "Camion has never connected to {host} before. Check the {algorithm} fingerprint \
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
            body: "Camion keeps your passwords in an encrypted file on this machine.".to_owned(),
            accept: "Unlock".to_owned(),
            wants_text: true,
            is_secret: true,
            ..Asked::default()
        },

        Question::Overwrite { name } => Asked {
            title: format!("Replace {name}?"),
            body: format!("{name} is already there."),
            accept: "Replace".to_owned(),
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

