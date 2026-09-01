//! What the window is showing, as plain data.
//!
//! Held apart from the Qt object on purpose. The rules here — what a refused connection does to
//! the toolbar, what disconnecting has to forget, which folder the breadcrumb is describing —
//! are the only real decisions the interface makes, and welded to a `QObject` they can only be
//! checked by opening the application and looking. Here they can be read, and tested.
//!
//! The bridge above does nothing but copy these fields onto properties.

use okuri_core::RemotePath;
use okuri_core::{Details, Ownership, Served, Stored};
use okuri_engine::{Attempt, Concern, Connection, Event, SessionId};

#[derive(Debug, Default)]
pub struct Screen {
    /// The open connection, once there is one.
    pub session: Option<SessionId>,

    /// The connection being opened, until it becomes a session or fails.
    ///
    /// Held because a window with nothing open yet still has to recognise its own news. Every
    /// window hears every event, and while two of them are connecting, "a server is asking for
    /// a password" says nothing about which window should be showing the question.
    pub attempt: Option<Attempt>,

    /// What that session was opened from, which is what knows whether the desktop can reach
    /// this destination on its own.
    pub connection: Option<Connection>,

    /// Where this connection's root sits on the server, as the server names it.
    pub home: String,

    pub folder: RemotePath,
    pub connecting: bool,
    pub connected: bool,
    pub label: String,
    pub can_rename: bool,
    pub rename_is_a_copy: bool,
    pub can_create_folder: bool,

    /// Whether this destination can hand files to people with no account.
    pub can_share: bool,

    /// Whether a file's mode can be changed here.
    pub can_set_permissions: bool,

    /// The last answer about a file's visibility, and its address.
    ///
    /// `None` means the store would not say, which is a different thing from private.
    pub shared_public: Option<bool>,
    pub shared_why_not: String,
    pub shared_url: String,

    /// A signed link, once one has been asked for.
    pub signed_url: String,

    /// Everything else the destination said about the file being looked at.
    pub described: Described,

    /// Whether that answer is still on its way.
    pub describing: bool,

    /// Which of those rows this destination can answer at all, so the panel can put them on
    /// screen before the server replies rather than growing one row at a time.
    pub answerable: Details,

    /// The last thing worth telling the person at the keyboard, or empty.
    pub message: String,

    /// Whether that was bad news. A confirmation shown in the colours of a failure teaches
    /// people to ignore the failures.
    pub message_is_grave: bool,
}

/// What a drag is carrying, written down so it can travel.
///
/// A drag that leaves the window it started in arrives as mime data and nothing else: the window
/// it lands in cannot ask the window it left what was picked up, and with the file list, the
/// breadcrumb and another window all being places to drop, no one of them can hold the answer
/// either. So everything needed to act on a drop goes with the drop — which connection the
/// files are on, which folder they are in, and what they are called.
///
/// Three lines and then the names, because a drag carries text and this is the shape that
/// survives the trip without inventing a format anybody has to look up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Carried {
    pub session: SessionId,
    pub folder: RemotePath,
    pub names: Vec<String>,
}

impl Carried {
    pub fn payload(&self) -> String {
        let mut lines = vec![self.session.0.to_string(), self.folder.to_string()];
        lines.extend(self.names.iter().cloned());

        lines.join("\n")
    }

    /// Reads back what [`Carried::payload`] wrote, or nothing if this drag came from somewhere
    /// that is not Okuri.
    pub fn parse(payload: &str) -> Option<Self> {
        let mut lines = payload.split('\n');

        let session = SessionId(lines.next()?.parse().ok()?);
        let folder = RemotePath::parse(lines.next()?).ok()?;
        let names = lines.map(str::to_owned).filter(|name| !name.is_empty()).collect::<Vec<_>>();

        match names.is_empty() {
            true => None,
            false => Some(Self { session, folder, names }),
        }
    }
}

/// What one file turned out to be, as the destinations that know each part answered.
#[derive(Clone, Debug, Default)]
pub struct Described {
    pub ownership: Option<Ownership>,
    pub link_target: Option<String>,
    pub served: Option<Served>,
    pub stored: Option<Stored>,
}

impl Described {
    /// The parts worth showing, worded here rather than by the adapters — wording is the
    /// interface's business, and a provider that decides what appears on screen is a provider
    /// nobody can restyle.
    pub fn rows(&self) -> Vec<(&'static str, String)> {
        let ownership = self.ownership.clone().unwrap_or_default();
        let served = self.served.clone().unwrap_or_default();
        let stored = self.stored.clone().unwrap_or_default();

        [
            ("Owner", ownership.user),
            ("Group", ownership.group),
            ("Links to", self.link_target.clone()),
            ("Content type", served.content_type),
            ("ETag", served.etag),
            ("Cache control", served.cache_control),
            ("Content encoding", served.content_encoding),
            ("Storage class", stored.class),
            ("Encryption", stored.encryption),
            ("Version", stored.version),
        ]
        .into_iter()
        .filter_map(|(label, said)| said.map(|said| (label, said)))
        .filter(|(_, said)| !said.trim().is_empty())
        .collect()
    }
}

impl Screen {
    /// A connection is being opened. Held from here rather than from the first event, because
    /// the destination is what says whether a drag out of the window can be handed to the file
    /// manager — and a drag can start before anything has been listed.
    pub fn connecting_to(&mut self, attempt: Attempt, connection: Connection) {
        self.attempt = Some(attempt);
        self.connection = Some(connection);
        self.connecting = true;
    }

    /// Whether an event is news for this window.
    ///
    /// [`Concern::Everyone`] is everybody's: a config file that will not parse is not one
    /// window's problem, and a window that stayed quiet about it would be a window showing an
    /// empty list of connections for no stated reason.
    pub fn concerns_us(&self, event: &Event) -> bool {
        match event.concern() {
            Concern::Everyone => true,
            Concern::Attempt(attempt) => self.attempt == Some(attempt),
            Concern::Session(session) => self.session == Some(session),
        }
    }

    /// Which saved connection is being opened, so the row that was clicked can say so.
    pub fn connecting_id(&self) -> String {
        match self.connecting {
            true => self.connection.as_ref().map(|it| it.id.clone()).unwrap_or_default(),
            false => String::new(),
        }
    }

    /// Folds one event into what the window shows.
    ///
    /// Events belonging to another window are dropped here rather than filtered by each caller,
    /// because every one of them is a way for one window to start showing another's files.
    pub fn receive(&mut self, event: &Event) {
        if !self.concerns_us(event) {
            return;
        }

        match event {
            Event::Connecting { .. } => self.connecting = true,

            Event::Connected { session, label, capabilities, home, .. } => {
                self.session = Some(*session);
                self.home = home.clone();
                self.connecting = false;
                self.connected = true;
                self.label = label.clone();
                self.can_rename = capabilities.rename.is_available();
                self.rename_is_a_copy = capabilities.rename.needs_warning();
                self.can_create_folder = capabilities.create_folder.is_available();
                self.can_share = capabilities.sharing;
                self.can_set_permissions = capabilities.permissions;
                self.answerable = capabilities.details;
            }

            Event::ConnectionFailed { reason, .. } => {
                self.attempt = None;
                self.connecting = false;
                self.complain(reason);
            }

            // Everything about the connection goes, including the destination: a stale one
            // would still be answering questions about where files can be dragged to.
            Event::Disconnected { .. } => {
                self.session = None;
                self.attempt = None;
                self.connection = None;
                self.home = String::new();
                self.connected = false;
                self.label = String::new();
                self.folder = RemotePath::root();
                self.can_share = false;
                self.can_set_permissions = false;
                self.shared_public = None;
                self.shared_why_not = String::new();
                self.shared_url = String::new();
                self.signed_url = String::new();
                self.described = Described::default();
                self.answerable = Details::default();
            }

            Event::Listing { path, .. } => self.folder = path.clone(),

            Event::Shared { public, why_not, url, .. } => {
                self.shared_public = *public;
                self.shared_why_not = why_not.clone();
                self.shared_url = url.clone();
            }

            Event::Linked { url, .. } => self.signed_url = url.clone(),

            Event::Described { ownership, link_target, served, stored, .. } => {
                self.describing = false;
                self.described = Described {
                    ownership: ownership.clone(),
                    link_target: link_target.clone(),
                    served: served.clone(),
                    stored: stored.clone(),
                };
            }

            Event::Failed { message, .. } => self.complain(message),

            Event::Notice { message, .. } => {
                self.message = message.clone();
                self.message_is_grave = false;
            }

            _ => {}
        }
    }

    /// The rows this destination could answer, whether or not it has yet.
    ///
    /// Only the ones nearly every file has. Cache headers and encryption are usually absent,
    /// and reserving room for a row that rarely arrives is its own kind of jumping.
    pub fn expected(&self) -> Vec<&'static str> {
        let mut labels = Vec::new();

        if self.answerable.owning {
            labels.extend(["Owner", "Group"]);
        }

        if self.answerable.serving {
            labels.extend(["Content type", "ETag"]);
        }

        if self.answerable.storing {
            labels.push("Storage class");
        }

        labels
    }

    pub fn complain(&mut self, message: impl std::fmt::Display) {
        self.message = message.to_string();
        self.message_is_grave = true;
    }

    pub fn at_root(&self) -> bool {
        self.folder.is_root()
    }

    pub fn path(&self) -> String {
        self.folder.to_string()
    }

    /// Where the open folder sits on the server itself.
    ///
    /// What the window shows is relative to wherever the connection starts, which is rarely the
    /// server's own root — and showing that with a leading slash says it is somewhere it is
    /// not. Anything naming a file to something outside Okuri needs the whole path.
    pub fn absolute_path(&self) -> String {
        format!("{}{}", self.home, self.folder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use okuri_core::{Capabilities, Entry};
    use okuri_providers::Destination;

    const OURS: Attempt = Attempt(1);

    fn connected(capabilities: Capabilities) -> Screen {
        let mut screen = Screen::default();

        screen.connecting_to(OURS, Connection::new("Scratch", Destination::Memory));
        screen.receive(&Event::Connected {
            attempt: OURS,
            session: SessionId(1),
            label: "scratch".to_owned(),
            capabilities,
            home: "/home/okuri".to_owned(),
        });

        screen
    }

    #[test]
    fn connecting_and_arriving() {
        let screen = connected(Capabilities::filesystem());

        assert!(screen.connected);
        assert!(!screen.connecting);
        assert_eq!(screen.label, "scratch");
        assert_eq!(screen.home, "/home/okuri");
        assert_eq!(screen.session, Some(SessionId(1)));
    }

    /// The menu is drawn from what the destination can do, so an object store has to arrive
    /// saying that renaming is really a copy before anybody clicks it.
    #[test]
    fn what_a_destination_can_do_comes_from_its_capabilities() {
        let filesystem = connected(Capabilities::filesystem());

        assert!(filesystem.can_rename);
        assert!(!filesystem.rename_is_a_copy);
        assert!(filesystem.can_create_folder);

        let store = connected(Capabilities::object_store());

        assert!(store.can_rename);
        assert!(store.rename_is_a_copy);
    }

    #[test]
    fn a_refused_connection_stops_waiting_and_says_why() {
        let mut screen = Screen::default();
        screen.connecting_to(OURS, Connection::new("Scratch", Destination::Memory));

        screen.receive(&Event::ConnectionFailed {
            attempt: OURS,
            connection: "scratch".to_owned(),
            reason: "the password was refused".to_owned(),
        });

        assert!(!screen.connecting);
        assert!(!screen.connected);
        assert_eq!(screen.message, "the password was refused");
    }

    /// A connection that starts in a home directory shows `/photos`, and the file really is at
    /// `/home/okuri/photos`. Presenting the first as though it were the second is how somebody
    /// pastes a path into a terminal and finds nothing there.
    #[test]
    fn where_a_folder_is_on_the_server_is_not_where_the_window_says_it_is() {
        let mut screen = connected(Capabilities::filesystem());

        screen.receive(&Event::Listing {
            session: SessionId(1),
            path: RemotePath::parse("/photos").unwrap(),
            entries: Vec::new(),
        });

        assert_eq!(screen.path(), "/photos");
        assert_eq!(screen.absolute_path(), "/home/okuri/photos");
    }

    /// An object store starts at the root of its bucket, so the two are the same thing.
    #[test]
    fn a_connection_that_starts_at_the_root_says_the_same_either_way() {
        let mut screen = Screen::default();
        screen.connecting_to(OURS, Connection::new("Assets", Destination::Memory));

        screen.receive(&Event::Connected {
            attempt: OURS,
            session: SessionId(1),
            label: "assets".to_owned(),
            capabilities: Capabilities::object_store(),
            home: String::new(),
        });
        screen.receive(&Event::Listing {
            session: SessionId(1),
            path: RemotePath::parse("/photos").unwrap(),
            entries: Vec::new(),
        });

        assert_eq!(screen.absolute_path(), "/photos");
    }

    #[test]
    fn a_listing_moves_the_breadcrumb() {
        let mut screen = connected(Capabilities::filesystem());

        assert!(screen.at_root());

        screen.receive(&Event::Listing {
            session: SessionId(1),
            path: RemotePath::parse("/documents/invoices").unwrap(),
            entries: vec![Entry::file("2026-08.pdf", 4096)],
        });

        assert_eq!(screen.path(), "/documents/invoices");
        assert!(!screen.at_root());
    }

    /// Disconnecting has to forget the destination too. Keeping it would leave the window still
    /// able to answer where a file could be dragged to on a server nobody is connected to.
    #[test]
    fn disconnecting_forgets_the_connection_entirely() {
        let mut screen = connected(Capabilities::filesystem());

        screen.receive(&Event::Listing {
            session: SessionId(1),
            path: RemotePath::parse("/photos").unwrap(),
            entries: Vec::new(),
        });

        screen.receive(&Event::Disconnected { session: SessionId(1) });

        assert!(!screen.connected);
        assert_eq!(screen.session, None);
        assert!(screen.connection.is_none());
        assert_eq!(screen.home, "");
        assert_eq!(screen.label, "");
        assert!(screen.at_root());
    }

    #[test]
    fn something_going_wrong_is_worth_saying() {
        let mut screen = Screen::default();

        screen.receive(&Event::Failed {
            concern: Concern::Everyone,
            message: "the server hung up".to_owned(),
        });

        assert_eq!(screen.message, "the server hung up");
        assert!(screen.message_is_grave);
    }

    /// Both use the same strip along the bottom, and a confirmation wearing the colours of a
    /// failure is how people learn to ignore the failures.
    #[test]
    fn a_confirmation_is_not_dressed_as_a_failure() {
        let mut screen = Screen::default();

        screen.receive(&Event::Notice {
            concern: Concern::Everyone,
            message: "Saved.".to_owned(),
        });

        assert_eq!(screen.message, "Saved.");
        assert!(!screen.message_is_grave);
    }

    /// Two windows hear every event, including each other's. Before this, the second window to
    /// connect took the first window's listings and the first window started drawing the second
    /// one's folder.
    #[test]
    fn a_window_ignores_the_other_windows_connection() {
        let mut ours = connected(Capabilities::filesystem());

        ours.receive(&Event::Connected {
            attempt: Attempt(2),
            session: SessionId(2),
            label: "elsewhere".to_owned(),
            capabilities: Capabilities::object_store(),
            home: "/somewhere-else".to_owned(),
        });

        ours.receive(&Event::Listing {
            session: SessionId(2),
            path: RemotePath::parse("/their-photos").unwrap(),
            entries: Vec::new(),
        });

        assert_eq!(ours.session, Some(SessionId(1)));
        assert_eq!(ours.label, "scratch");
        assert_eq!(ours.home, "/home/okuri");
        assert_eq!(ours.path(), "/");
    }

    /// Disconnecting one window leaves the other one connected. Sharing an engine is not the
    /// same as sharing a connection.
    #[test]
    fn a_window_stays_connected_when_another_one_disconnects() {
        let mut ours = connected(Capabilities::filesystem());

        ours.receive(&Event::Disconnected { session: SessionId(2) });

        assert!(ours.connected);
        assert_eq!(ours.session, Some(SessionId(1)));
    }

    /// A password refused while the window next door was connecting is not this window's news,
    /// and a window that reported it would be a window complaining about a server it never
    /// tried to reach.
    #[test]
    fn only_the_window_that_asked_hears_why_a_connection_failed() {
        let mut ours = Screen::default();
        ours.connecting_to(OURS, Connection::new("Scratch", Destination::Memory));

        ours.receive(&Event::ConnectionFailed {
            attempt: Attempt(2),
            connection: "elsewhere".to_owned(),
            reason: "the password was refused".to_owned(),
        });

        assert!(ours.connecting);
        assert_eq!(ours.message, "");
    }

    /// A drag that leaves its window arrives as text and nothing else, so everything needed to
    /// act on it has to survive the round trip.
    #[test]
    fn what_a_drag_carries_survives_leaving_the_window() {
        let carried = Carried {
            session: SessionId(7),
            folder: RemotePath::parse("/documents/invoices").unwrap(),
            names: vec!["2026-08.pdf".to_owned(), "notes.txt".to_owned()],
        };

        assert_eq!(Carried::parse(&carried.payload()), Some(carried));
    }

    /// Anything else dropped on Okuri is not Okuri's to move. A drop that cannot be read is
    /// worth saying so about, which it cannot be if an unreadable one looks like an empty one.
    #[test]
    fn a_drop_from_somewhere_else_carries_nothing() {
        assert_eq!(Carried::parse(""), None);
        assert_eq!(Carried::parse("file:///home/me/harbour.jpg"), None);

        // A connection and a folder, and nothing being carried between them.
        assert_eq!(Carried::parse("7\n/documents"), None);

        // A connection and nothing else.
        assert_eq!(Carried::parse("7"), None);
    }

    /// A config file that will not parse belongs to nobody in particular, and a window that
    /// stayed quiet about it would show an empty list of connections for no stated reason.
    #[test]
    fn a_problem_with_okuri_itself_is_every_windows_news() {
        let mut ours = connected(Capabilities::filesystem());

        ours.receive(&Event::Failed {
            concern: Concern::Everyone,
            message: "your saved connections could not be read".to_owned(),
        });

        assert_eq!(ours.message, "your saved connections could not be read");
    }
}

#[cfg(test)]
mod described {
    use super::*;

    /// Only what the destination actually said. A row with nothing beside it reads as "this
    /// file has none" rather than "nobody was asked".
    #[test]
    fn a_destination_that_says_nothing_shows_nothing() {
        assert!(Described::default().rows().is_empty());
    }

    #[test]
    fn what_each_kind_of_destination_knows_is_worded_here() {
        let sftp = Described {
            ownership: Some(Ownership {
                user: Some("ubuntu".to_owned()),
                group: Some("ubuntu".to_owned()),
            }),
            link_target: Some("/etc/nginx.conf".to_owned()),
            ..Described::default()
        };

        assert_eq!(
            sftp.rows(),
            vec![
                ("Owner", "ubuntu".to_owned()),
                ("Group", "ubuntu".to_owned()),
                ("Links to", "/etc/nginx.conf".to_owned()),
            ]
        );

        let store = Described {
            served: Some(Served {
                content_type: Some("image/jpeg".to_owned()),
                etag: Some("d41d8cd9".to_owned()),
                ..Served::default()
            }),
            stored: Some(Stored {
                class: Some("STANDARD".to_owned()),
                // Left out below, because the store did not say.
                encryption: None,
                version: Some("3HL4kqt".to_owned()),
            }),
            ..Described::default()
        };

        assert_eq!(
            store.rows(),
            vec![
                ("Content type", "image/jpeg".to_owned()),
                ("ETag", "d41d8cd9".to_owned()),
                ("Storage class", "STANDARD".to_owned()),
                ("Version", "3HL4kqt".to_owned()),
            ]
        );
    }
}

#[cfg(test)]
mod waiting {
    use super::*;
    use okuri_core::Capabilities;

    fn connected_to(details: Details) -> Screen {
        let mut screen = Screen { attempt: Some(Attempt(1)), ..Screen::default() };

        screen.receive(&Event::Connected {
            attempt: Attempt(1),
            session: SessionId(1),
            label: "somewhere".to_owned(),
            capabilities: Capabilities { details, ..Capabilities::filesystem() },
            home: String::new(),
        });

        screen
    }

    /// The panel puts these on screen before the server has answered, so the window does not
    /// grow a row at a time as each reply lands.
    #[test]
    fn what_a_destination_can_be_asked_is_known_before_it_is_asked() {
        let file_server = connected_to(Details { owning: true, linking: true, ..Details::none() });

        assert_eq!(file_server.expected(), vec!["Owner", "Group"]);

        let store = connected_to(Details { serving: true, storing: true, ..Details::none() });

        assert_eq!(store.expected(), vec!["Content type", "ETag", "Storage class"]);
    }

    #[test]
    fn a_destination_with_nothing_to_add_reserves_nothing() {
        assert!(connected_to(Details::none()).expected().is_empty());
    }
}
