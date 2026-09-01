use std::path::{Path, PathBuf};

use okuri_providers::Destination;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// One saved destination, as it appears in the connection list.
///
/// The id is what the secret store is keyed by, so renaming a connection keeps its password and
/// deleting one is enough to orphan it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub destination: Destination,
}

impl Connection {
    pub fn new(name: impl Into<String>, destination: Destination) -> Self {
        let name = name.into();

        Self { id: slug(&name), name, destination }
    }

    pub fn summary(&self) -> String {
        format!("{} · {}", self.destination.kind(), self.destination.summary())
    }
}

/// Everything in `connections.toml`.
///
/// The file is meant to be read and edited by hand, so it holds nothing but the plain facts of
/// where a connection points. Secrets are looked up by [`Connection::id`] and live elsewhere.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connections {
    #[serde(default, rename = "connection")]
    pub entries: Vec<Connection>,
}

impl Connections {
    pub fn default_path() -> Option<PathBuf> {
        Some(config_home()?.join("okuri/connections.toml"))
    }

    /// Reads the file, treating "not there yet" as an empty list rather than as a problem.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        match std::fs::read_to_string(path.as_ref()) {
            Ok(contents) => toml::from_str(&contents).map_err(|error| {
                Error::config(format!("{} could not be read: {error}", path.as_ref().display()))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(Error::config(format!(
                "{} could not be opened: {error}",
                path.as_ref().display()
            ))),
        }
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();

        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory).map_err(|error| {
                Error::config(format!("could not create {}: {error}", directory.display()))
            })?;
        }

        let contents = toml::to_string_pretty(self)
            .map_err(|error| Error::config(format!("could not write the connections: {error}")))?;

        std::fs::write(path, contents)
            .map_err(|error| Error::config(format!("could not save {}: {error}", path.display())))
    }

    pub fn find(&self, id: &str) -> Option<&Connection> {
        self.entries.iter().find(|connection| connection.id == id)
    }

    /// Adds a connection, or replaces the one already using its id.
    pub fn put(&mut self, connection: Connection) {
        match self.entries.iter_mut().find(|entry| entry.id == connection.id) {
            Some(existing) => *existing = connection,
            None => self.entries.push(connection),
        }
    }

    pub fn remove(&mut self, id: &str) -> Option<Connection> {
        let position = self.entries.iter().position(|entry| entry.id == id)?;

        Some(self.entries.remove(position))
    }

    /// An id derived from `name` that nothing else is using yet.
    pub fn unused_id(&self, name: &str) -> String {
        let base = slug(name);

        if self.find(&base).is_none() {
            return base;
        }

        let mut suffix = 2;

        loop {
            let candidate = format!("{base}-{suffix}");

            if self.find(&candidate).is_none() {
                return candidate;
            }

            suffix += 1;
        }
    }
}

pub fn config_home() -> Option<PathBuf> {
    directory("XDG_CONFIG_HOME", ".config")
}

pub fn data_home() -> Option<PathBuf> {
    directory("XDG_DATA_HOME", ".local/share")
}

fn directory(variable: &str, fallback: &str) -> Option<PathBuf> {
    match std::env::var_os(variable) {
        Some(value) if !value.is_empty() => Some(PathBuf::from(value)),
        _ => Some(Path::new(&std::env::var_os("HOME")?).join(fallback)),
    }
}

/// A file-name-shaped version of a connection's name, so ids stay readable in the config and in
/// the keyring rather than being opaque numbers.
fn slug(name: &str) -> String {
    let slug = name
        .chars()
        .map(|character| match character.is_ascii_alphanumeric() {
            true => character.to_ascii_lowercase(),
            false => '-',
        })
        .collect::<String>();

    let slug = slug.split('-').filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-");

    if slug.is_empty() {
        "connection".to_owned()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use okuri_providers::destination::{Sftp, SshCredential, S3, S3Preset};

    fn sftp(host: &str) -> Destination {
        Destination::Sftp(Sftp {
            host: host.to_owned(),
            port: 22,
            username: "stanko".to_owned(),
            credential: SshCredential::Agent,
            home: String::new(),
        })
    }

    #[test]
    fn names_become_readable_ids() {
        assert_eq!(Connection::new("Production Web", sftp("example.com")).id, "production-web");
        assert_eq!(Connection::new("  ", sftp("example.com")).id, "connection");
        assert_eq!(Connection::new("R2 — assets!", sftp("example.com")).id, "r2-assets");
    }

    #[test]
    fn a_second_connection_with_the_same_name_gets_its_own_id() {
        let mut connections = Connections::default();
        connections.put(Connection::new("Production", sftp("example.com")));

        assert_eq!(connections.unused_id("Production"), "production-2");
        assert_eq!(connections.unused_id("Staging"), "staging");
    }

    #[test]
    fn connections_round_trip_through_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("okuri/connections.toml");

        let mut connections = Connections::default();
        connections.put(Connection::new("Production", sftp("example.com")));
        connections.put(Connection::new(
            "Assets",
            Destination::S3(S3 {
                bucket: "assets".to_owned(),
                preset: S3Preset::CloudflareR2,
                region: "abc123".to_owned(),
                endpoint: String::new(),
                root: String::new(),
            }),
        ));
        connections.save(&path).unwrap();

        assert_eq!(Connections::load(&path).unwrap(), connections);
    }

    #[test]
    fn the_saved_file_holds_no_secrets_and_can_be_read_by_a_person() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("connections.toml");

        let mut connections = Connections::default();
        connections.put(Connection::new("Production", sftp("example.com")));
        connections.save(&path).unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("example.com"));
        assert!(contents.contains(r#"kind = "sftp""#));
        assert!(!contents.to_lowercase().contains("password"));
    }

    #[test]
    fn a_missing_file_is_an_empty_list_rather_than_an_error() {
        let directory = tempfile::tempdir().unwrap();

        assert_eq!(
            Connections::load(directory.path().join("nothing.toml")).unwrap(),
            Connections::default()
        );
    }

    #[test]
    fn putting_the_same_id_replaces_rather_than_duplicates() {
        let mut connections = Connections::default();
        connections.put(Connection::new("Production", sftp("old.example.com")));
        connections.put(Connection::new("Production", sftp("new.example.com")));

        assert_eq!(connections.entries.len(), 1);
        assert_eq!(
            connections.find("production").unwrap().destination.summary(),
            "stanko@new.example.com"
        );
    }
}
