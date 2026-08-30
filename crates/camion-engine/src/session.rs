use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use camion_core::{Provider, RemotePath};
use tokio::sync::Semaphore;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub u64);

impl SessionId {
    pub fn next() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);

        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// One request to look at a folder, held so a late answer can be recognised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Navigation(u64);

/// One open connection.
///
/// Sessions are held in a registry rather than as a single current connection, because the
/// transfer queue can be moving files on one while you browse another, and because the
/// side-by-side view is coming.
pub struct Session {
    pub id: SessionId,
    pub connection: String,
    pub provider: Arc<dyn Provider>,
    path: Mutex<RemotePath>,
    transfers: Arc<Semaphore>,
    /// Which navigation is the current one.
    ///
    /// Every command runs on its own task, so two folders opened in quick succession are two
    /// listings racing each other. The folder asked for last is the one wanted, whichever
    /// server answers first — without this, a slow listing arriving late drops the window into
    /// a folder nobody chose.
    navigation: AtomicU64,
}

impl Session {
    pub fn new(connection: impl Into<String>, provider: Arc<dyn Provider>) -> Self {
        let concurrency = provider.capabilities().transfer_slots;

        Self {
            id: SessionId::next(),
            connection: connection.into(),
            provider,
            path: Mutex::new(RemotePath::root()),
            transfers: Arc::new(Semaphore::new(concurrency)),
            navigation: AtomicU64::new(0),
        }
    }

    pub fn path(&self) -> RemotePath {
        self.path.lock().unwrap().clone()
    }

    pub fn move_to(&self, path: RemotePath) {
        *self.path.lock().unwrap() = path;
    }

    /// Starts a navigation, and returns the token that says whether it is still the current one.
    pub fn navigating(&self) -> Navigation {
        Navigation(self.navigation.fetch_add(1, Ordering::SeqCst) + 1)
    }

    /// Whether this navigation is still the one being waited on, or has been overtaken.
    pub fn is_current(&self, navigation: Navigation) -> bool {
        self.navigation.load(Ordering::SeqCst) == navigation.0
    }

    /// How many transfers this connection will run at once.
    ///
    /// Held per session rather than globally: eight parallel uploads is polite to an object
    /// store and rude to a small SFTP server, and a slow connection should not be able to
    /// starve a fast one.
    pub fn transfer_slots(&self) -> Arc<Semaphore> {
        Arc::clone(&self.transfers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camion_core::MemoryProvider;

    fn session() -> Session {
        Session::new("scratch", Arc::new(MemoryProvider::sample()))
    }

    #[test]
    fn the_folder_asked_for_last_is_the_one_still_wanted() {
        let session = session();

        let first = session.navigating();
        let second = session.navigating();

        assert!(!session.is_current(first));
        assert!(session.is_current(second));
    }

    #[test]
    fn a_navigation_nothing_has_overtaken_is_still_current() {
        let session = session();
        let only = session.navigating();

        assert!(session.is_current(only));
    }
}
