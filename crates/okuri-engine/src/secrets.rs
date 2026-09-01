use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use okuri_providers::Secret;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;

use crate::error::{Error, Result};

/// Where passwords and keys live.
///
/// Connections are looked up by an opaque id, which is all `connections.toml` ever holds. Which
/// store answers is a property of the machine, not of the connection, so a config file can move
/// between a desktop with a keyring and a server without one and still work.
pub trait SecretStore: Send + Sync {
    fn get(&self, id: &str) -> Result<Secret>;
    fn set(&self, id: &str, secret: &Secret) -> Result<()>;
    fn remove(&self, id: &str) -> Result<()>;
}

/// Where secrets live, which may not be open yet.
///
/// The encrypted file needs a passphrase, and at start-up there is nobody to ask: the window is
/// not up, and demanding one before anything needs a secret would greet everyone who has no
/// keyring with a password box they did not ask for. So the choice of store is made once —
/// it is a property of the machine — and the opening of it waits until a connection actually
/// wants a credential, which is a moment the engine can ask about.
pub struct Vault {
    state: Mutex<State>,
}

enum State {
    Open(Arc<dyn SecretStore>),
    Locked(PathBuf),
}

impl Vault {
    pub fn open(store: Arc<dyn SecretStore>) -> Self {
        Self { state: Mutex::new(State::Open(store)) }
    }

    /// An encrypted file that has not been opened yet.
    pub fn locked(path: impl Into<PathBuf>) -> Self {
        Self { state: Mutex::new(State::Locked(path.into())) }
    }

    /// The store, or `None` while a passphrase is still needed.
    pub fn store(&self) -> Option<Arc<dyn SecretStore>> {
        match &*self.state.lock().unwrap() {
            State::Open(store) => Some(Arc::clone(store)),
            State::Locked(_) => None,
        }
    }

    /// Opens the file, or says the passphrase was wrong so it can be asked for again.
    pub fn unlock(&self, passphrase: &str) -> Result<Arc<dyn SecretStore>> {
        let mut state = self.state.lock().unwrap();

        let path = match &*state {
            State::Open(store) => return Ok(Arc::clone(store)),
            State::Locked(path) => path.clone(),
        };

        let store: Arc<dyn SecretStore> = Arc::new(EncryptedFile::open(path, passphrase)?);
        *state = State::Open(Arc::clone(&store));

        Ok(store)
    }
}

/// The desktop's own secret service — GNOME Keyring, KWallet, and anything else that speaks the
/// Secret Service API. The first choice, because it is already unlocked when you log in.
pub struct Keyring;

impl Keyring {
    const SERVICE: &'static str = "okuri";

    /// Whether a secret service is actually running. Checked by trying, because "is the daemon
    /// up" has no better answer on a Linux desktop.
    pub fn is_available() -> bool {
        keyring::Entry::new(Self::SERVICE, "okuri-probe")
            .and_then(|entry| match entry.get_password() {
                Err(keyring::Error::NoEntry) => Ok(()),
                other => other.map(drop),
            })
            .is_ok()
    }

    fn entry(id: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(Self::SERVICE, id)
            .map_err(|error| Error::secrets(format!("the keyring refused the request: {error}")))
    }
}

impl SecretStore for Keyring {
    fn get(&self, id: &str) -> Result<Secret> {
        match Self::entry(id)?.get_password() {
            Ok(encoded) => decode(&encoded),
            Err(keyring::Error::NoEntry) => Ok(Secret::None),
            Err(error) => Err(Error::secrets(format!("could not read from the keyring: {error}"))),
        }
    }

    fn set(&self, id: &str, secret: &Secret) -> Result<()> {
        Self::entry(id)?
            .set_password(&encode(secret)?)
            .map_err(|error| Error::secrets(format!("could not write to the keyring: {error}")))
    }

    fn remove(&self, id: &str) -> Result<()> {
        match Self::entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(Error::secrets(format!("could not clear the keyring: {error}"))),
        }
    }
}

/// One file, encrypted with a passphrase, for machines with no secret service.
///
/// The passphrase is asked for once per session, the first time a connection needs a secret.
/// Deriving the key from the machine instead would be more convenient and would protect nothing:
/// anything running as you could read it just as easily as we can.
pub struct EncryptedFile {
    path: PathBuf,
    /// Held rather than read back each time: it is the salt this store's key was derived from,
    /// so a different one off disk could only ever produce a file we can no longer open.
    salt: Vec<u8>,
    key: chacha20poly1305::Key,
    secrets: Mutex<BTreeMap<String, Secret>>,
}

const MAGIC: &[u8] = b"okuri-secrets-v1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;

impl EncryptedFile {
    pub fn default_path() -> Option<PathBuf> {
        Some(crate::config::data_home()?.join("secrets"))
    }

    /// Opens the store, creating it if this is the first time. A wrong passphrase is reported
    /// as such rather than as a corrupt file, because that is nearly always what it is.
    pub fn open(path: impl Into<PathBuf>, passphrase: &str) -> Result<Self> {
        let path = path.into();

        let (salt, key, secrets) = match std::fs::read(&path) {
            Ok(contents) => {
                let (salt, sealed) = split(&contents)?;
                let key = derive(passphrase, &salt)?;
                let secrets = unseal(&key, sealed)?;

                (salt, key, secrets)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut salt = [0u8; SALT_LEN];
                rand::thread_rng().fill_bytes(&mut salt);

                // Deriving a key is deliberately slow, so it happens once here rather than
                // once per branch and again on the way out.
                let key = derive(passphrase, &salt)?;

                (salt.to_vec(), key, BTreeMap::new())
            }
            Err(error) => {
                return Err(Error::secrets(format!("could not read {}: {error}", path.display())));
            }
        };

        let store = Self { path, salt, key, secrets: Mutex::new(secrets) };

        store.save()?;

        Ok(store)
    }

    fn save(&self) -> Result<()> {
        let plaintext = serde_json::to_vec(&*self.secrets.lock().unwrap())
            .map_err(|error| Error::secrets(format!("could not encode the secrets: {error}")))?;

        let mut nonce = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce);

        let sealed = XChaCha20Poly1305::new(&self.key)
            .encrypt(XNonce::from_slice(&nonce), plaintext.as_slice())
            .map_err(|_| Error::secrets("could not encrypt the secrets"))?;

        let mut contents = Vec::with_capacity(MAGIC.len() + SALT_LEN + NONCE_LEN + sealed.len());
        contents.extend_from_slice(MAGIC);
        contents.extend_from_slice(&self.salt);
        contents.extend_from_slice(&nonce);
        contents.extend_from_slice(&sealed);

        write_privately(&self.path, &contents)
    }
}

impl SecretStore for EncryptedFile {
    fn get(&self, id: &str) -> Result<Secret> {
        Ok(self.secrets.lock().unwrap().get(id).cloned().unwrap_or_default())
    }

    fn set(&self, id: &str, secret: &Secret) -> Result<()> {
        self.secrets.lock().unwrap().insert(id.to_owned(), secret.clone());
        self.save()
    }

    fn remove(&self, id: &str) -> Result<()> {
        self.secrets.lock().unwrap().remove(id);
        self.save()
    }
}

/// Remembers secrets for as long as the process runs and no longer. Used by tests, and by a
/// session where the person declined to save anything.
#[derive(Default)]
pub struct InMemory {
    secrets: Mutex<BTreeMap<String, Secret>>,
}

impl SecretStore for InMemory {
    fn get(&self, id: &str) -> Result<Secret> {
        Ok(self.secrets.lock().unwrap().get(id).cloned().unwrap_or_default())
    }

    fn set(&self, id: &str, secret: &Secret) -> Result<()> {
        self.secrets.lock().unwrap().insert(id.to_owned(), secret.clone());
        Ok(())
    }

    fn remove(&self, id: &str) -> Result<()> {
        self.secrets.lock().unwrap().remove(id);
        Ok(())
    }
}

fn split(contents: &[u8]) -> Result<(Vec<u8>, &[u8])> {
    let body = contents
        .strip_prefix(MAGIC)
        .ok_or_else(|| Error::secrets("this is not an Okuri secrets file"))?;

    if body.len() < SALT_LEN + NONCE_LEN {
        return Err(Error::secrets("the secrets file is truncated"));
    }

    Ok((body[..SALT_LEN].to_vec(), &body[SALT_LEN..]))
}

fn unseal(key: &chacha20poly1305::Key, sealed: &[u8]) -> Result<BTreeMap<String, Secret>> {
    let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);

    let plaintext = XChaCha20Poly1305::new(key)
        .decrypt(XNonce::from_slice(nonce), ciphertext)
        .map_err(|_| Error::WrongPassphrase)?;

    serde_json::from_slice(&plaintext)
        .map_err(|error| Error::secrets(format!("the secrets file is damaged: {error}")))
}

fn derive(passphrase: &str, salt: &[u8]) -> Result<chacha20poly1305::Key> {
    let mut key = [0u8; 32];

    argon2::Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|error| Error::secrets(format!("could not derive a key: {error}")))?;

    Ok(chacha20poly1305::Key::from(key))
}

/// Writes with an owner-only mode from the start, rather than creating the file and tightening
/// it afterwards.
fn write_privately(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;

    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)
            .map_err(|error| Error::secrets(format!("could not create {}: {error}", directory.display())))?;
    }

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);

    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);

    options
        .open(path)
        .and_then(|mut file| file.write_all(contents))
        .map_err(|error| Error::secrets(format!("could not write {}: {error}", path.display())))
}

fn encode(secret: &Secret) -> Result<String> {
    serde_json::to_string(secret)
        .map_err(|error| Error::secrets(format!("could not encode a secret: {error}")))
}

fn decode(encoded: &str) -> Result<Secret> {
    serde_json::from_str(encoded)
        .map_err(|error| Error::secrets(format!("could not read a stored secret: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_survive_a_close_and_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("secrets");

        let store = EncryptedFile::open(&path, "correct horse").unwrap();
        store.set("live", &Secret::Password("hunter2".to_owned())).unwrap();
        store
            .set("bucket", &Secret::KeyPair { id: "AKIA".to_owned(), secret: "shh".to_owned() })
            .unwrap();
        drop(store);

        let reopened = EncryptedFile::open(&path, "correct horse").unwrap();

        assert_eq!(reopened.get("live").unwrap(), Secret::Password("hunter2".to_owned()));
        assert_eq!(
            reopened.get("bucket").unwrap(),
            Secret::KeyPair { id: "AKIA".to_owned(), secret: "shh".to_owned() }
        );
    }

    #[test]
    fn the_wrong_passphrase_says_so() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("secrets");

        EncryptedFile::open(&path, "correct horse")
            .unwrap()
            .set("live", &Secret::Password("hunter2".to_owned()))
            .unwrap();

        assert!(matches!(
            EncryptedFile::open(&path, "battery staple"),
            Err(Error::WrongPassphrase)
        ));
    }

    #[test]
    fn nothing_readable_is_left_on_disk() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("secrets");

        EncryptedFile::open(&path, "correct horse")
            .unwrap()
            .set("live", &Secret::Password("hunter2".to_owned()))
            .unwrap();

        let contents = std::fs::read(&path).unwrap();
        assert!(!contents.windows(7).any(|window| window == b"hunter2"));
        assert!(!contents.windows(4).any(|window| window == b"live"));
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("secrets");

        EncryptedFile::open(&path, "correct horse").unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn an_unknown_id_is_simply_absent() {
        let directory = tempfile::tempdir().unwrap();
        let store = EncryptedFile::open(directory.path().join("secrets"), "correct horse").unwrap();

        assert_eq!(store.get("never-saved").unwrap(), Secret::None);
    }

    #[test]
    fn removing_forgets_the_secret() {
        let store = InMemory::default();

        store.set("live", &Secret::Password("hunter2".to_owned())).unwrap();
        store.remove("live").unwrap();

        assert_eq!(store.get("live").unwrap(), Secret::None);
    }
}
