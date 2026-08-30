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
        }
    }

    pub fn path(&self) -> RemotePath {
        self.path.lock().unwrap().clone()
    }

    pub fn move_to(&self, path: RemotePath) {
        *self.path.lock().unwrap() = path;
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
