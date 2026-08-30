//! What the window is showing, as plain data.
//!
//! Held apart from the Qt object on purpose. The rules here — what a refused connection does to
//! the toolbar, what disconnecting has to forget, which folder the breadcrumb is describing —
//! are the only real decisions the interface makes, and welded to a `QObject` they can only be
//! checked by opening the application and looking. Here they can be read, and tested.
//!
//! The bridge above does nothing but copy these fields onto properties.

use camion_core::RemotePath;
use camion_engine::{Connection, Event, SessionId};

#[derive(Debug, Default)]
pub struct Screen {
    /// The open connection, once there is one.
    pub session: Option<SessionId>,

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

    /// The last thing worth telling the person at the keyboard, or empty.
    pub message: String,
}

impl Screen {
    /// A connection is being opened. Held from here rather than from the first event, because
    /// the destination is what says whether a drag out of the window can be handed to the file
    /// manager — and a drag can start before anything has been listed.
    pub fn connecting_to(&mut self, connection: Connection) {
        self.connection = Some(connection);
        self.connecting = true;
    }

    /// Folds one event into what the window shows.
    pub fn receive(&mut self, event: &Event) {
        match event {
            Event::Connecting { .. } => self.connecting = true,

            Event::Connected { session, label, capabilities, home } => {
                self.session = Some(*session);
                self.home = home.clone();
                self.connecting = false;
                self.connected = true;
                self.label = label.clone();
                self.can_rename = capabilities.rename.is_available();
                self.rename_is_a_copy = capabilities.rename.needs_warning();
                self.can_create_folder = capabilities.create_folder.is_available();
            }

            Event::ConnectionFailed { reason, .. } => {
                self.connecting = false;
                self.message = reason.clone();
            }

            // Everything about the connection goes, including the destination: a stale one
            // would still be answering questions about where files can be dragged to.
            Event::Disconnected { .. } => {
                self.session = None;
                self.connection = None;
                self.home = String::new();
                self.connected = false;
                self.label = String::new();
                self.folder = RemotePath::root();
            }

            Event::Listing { path, .. } => self.folder = path.clone(),

            Event::Failed { message } => self.message = message.clone(),

            _ => {}
        }
    }

    pub fn at_root(&self) -> bool {
        self.folder.is_root()
    }

    pub fn path(&self) -> String {
        self.folder.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camion_core::{Capabilities, Entry};
    use camion_providers::Destination;

    fn connected(capabilities: Capabilities) -> Screen {
        let mut screen = Screen::default();

        screen.connecting_to(Connection::new("Scratch", Destination::Memory));
        screen.receive(&Event::Connected {
            session: SessionId(1),
            label: "scratch".to_owned(),
            capabilities,
            home: "/home/camion".to_owned(),
        });

        screen
    }

    #[test]
    fn connecting_and_arriving() {
        let screen = connected(Capabilities::filesystem());

        assert!(screen.connected);
        assert!(!screen.connecting);
        assert_eq!(screen.label, "scratch");
        assert_eq!(screen.home, "/home/camion");
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
        screen.connecting_to(Connection::new("Scratch", Destination::Memory));

        screen.receive(&Event::ConnectionFailed {
            connection: "scratch".to_owned(),
            reason: "the password was refused".to_owned(),
        });

        assert!(!screen.connecting);
        assert!(!screen.connected);
        assert_eq!(screen.message, "the password was refused");
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

        screen.receive(&Event::Failed { message: "the server hung up".to_owned() });

        assert_eq!(screen.message, "the server hung up");
    }
}
