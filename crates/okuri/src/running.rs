//! The one engine, and the vault it signs in with.
//!
//! Held for the process rather than by a window. Windows come and go, and a second engine would
//! mean a second Tokio runtime, a second registry of open connections, and a transfer started in
//! one window that the queue in the other cannot see or cancel.

use std::sync::{Arc, OnceLock};

use okuri_engine::secrets::{EncryptedFile, InMemory, Keyring};
use okuri_engine::{Engine, Vault};

pub fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();

    ENGINE.get_or_init(|| Engine::start(vault(), crate::bus::emitter()))
}

/// The desktop's keyring when one is running, and a passphrase-encrypted file when none is.
///
/// The choice is made once at startup rather than per connection: it is a property of the
/// machine, and a connection that works today should not start asking differently tomorrow
/// because a daemon happened to be slow.
///
/// The file is handed over locked. Opening it needs a passphrase, and the engine asks for that
/// the first time a connection wants a credential — which is the first moment the question
/// means anything, and the first moment there is a window to ask in.
fn vault() -> Arc<Vault> {
    if Keyring::is_available() {
        return Arc::new(Vault::open(Arc::new(Keyring)));
    }

    match EncryptedFile::default_path() {
        Some(path) => Arc::new(Vault::locked(path)),
        None => Arc::new(Vault::open(Arc::new(InMemory::default()))),
    }
}
