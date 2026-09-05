use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use okuri_engine::{Connection, Connections};
use okuri_providers::destination::{Ftp, S3Preset, Sftp, SshCredential, WebDav, S3};
use okuri_providers::{Azure, Destination};

/// The saved connections, and the editor that adds to them.
///
/// One for the process: the picker in every window shows the same list, and a connection saved
/// in one window is there in the other the moment it is saved.
pub struct ConnectionList {
    connections: RefCell<Connections>,
    observers: RefCell<Vec<Rc<dyn Fn() -> bool>>>,
}

thread_local! {
    static LIST: Rc<ConnectionList> = Rc::new(ConnectionList {
        connections: RefCell::new(crate::store::load()),
        observers: RefCell::new(Vec::new()),
    });
}

pub fn list() -> Rc<ConnectionList> {
    LIST.with(Rc::clone)
}

/// The kinds the editor offers, in the order it offers them, with the label each one shows.
pub const KINDS: [(&str, &str); 7] = [
    ("sftp", "SFTP"),
    ("ftp", "FTP"),
    ("s3", "Amazon S3"),
    ("r2", "Cloudflare R2"),
    ("b2", "Backblaze B2"),
    ("webdav", "WebDAV"),
    ("azure", "Azure Blob Storage"),
];

/// How to prove who we are over SSH, in the order the editor offers them.
pub const CREDENTIALS: [(&str, &str); 3] =
    [("agent", "SSH agent"), ("password", "Password"), ("key", "Key file")];

impl ConnectionList {
    /// Calls `observer` whenever the list changes, until it answers `false` — which is how an
    /// observer says the thing it was updating has gone.
    pub fn on_change(&self, observer: impl Fn() -> bool + 'static) {
        self.observers.borrow_mut().push(Rc::new(observer));
    }

    pub fn entries(&self) -> Vec<Connection> {
        self.connections.borrow().entries.clone()
    }

    /// Adds or replaces a connection from the editor's fields, which arrive as plain text under
    /// plain names — `name`, `kind`, `host`, and so on — so the editor can carry only what the
    /// chosen kind actually needs.
    pub fn save(&self, fields: Fields) {
        let name = fields.text("name");
        let id = fields.text("id");

        let existing = self.connections.borrow().find(&id).map(|found| found.destination.clone());

        let connection = Connection {
            id: match id.is_empty() {
                true => self.connections.borrow().unused_id(&name),
                false => id,
            },
            name,
            destination: carried_over(fields.destination(), existing.as_ref()),
        };

        self.connections.borrow_mut().put(connection);
        self.persist();
    }

    pub fn forget(&self, id: &str) {
        self.connections.borrow_mut().remove(id);
        self.persist();
    }

    /// Whether this connection signs in with something worth changing. An SFTP connection
    /// using the agent has nothing stored, so there is nothing to offer.
    pub fn needs_credentials(&self, id: &str) -> bool {
        self.connections
            .borrow()
            .find(id)
            .is_some_and(|connection| connection.destination.needs_secret())
    }

    /// Everything a saved connection holds, so the editor can show it rather than opening
    /// blank and quietly replacing what was there.
    pub fn details(&self, id: &str) -> Option<Fields> {
        let connections = self.connections.borrow();
        let connection = connections.find(id)?;
        let mut fields = Fields::default();

        fields.put("id", connection.id.clone());
        fields.put("name", connection.name.clone());

        match &connection.destination {
            Destination::Sftp(sftp) => {
                fields.put("kind", "sftp".to_owned());
                fields.put("host", sftp.host.clone());
                fields.put("port", sftp.port.to_string());
                fields.put("username", sftp.username.clone());
                fields.put("home", sftp.home.clone());

                match &sftp.credential {
                    SshCredential::Agent => fields.put("credential", "agent".to_owned()),
                    SshCredential::Password => fields.put("credential", "password".to_owned()),
                    SshCredential::Key { path } => {
                        fields.put("credential", "key".to_owned());
                        fields.put("key", path.clone());
                    }
                }
            }
            Destination::Ftp(ftp) => {
                fields.put("kind", "ftp".to_owned());
                fields.put("host", ftp.host.clone());
                fields.put("port", ftp.port.to_string());
                fields.put("username", ftp.username.clone());
                fields.put("home", ftp.home.clone());
            }
            Destination::S3(storage) => {
                fields.put(
                    "kind",
                    match storage.preset {
                        S3Preset::CloudflareR2 => "r2",
                        S3Preset::BackblazeB2 => "b2",
                        _ => "s3",
                    }
                    .to_owned(),
                );
                fields.put("bucket", storage.bucket.clone());
                fields.put("region", storage.region.clone());
                fields.put("endpoint", storage.endpoint.clone());
                fields.put("root", storage.root.clone());
            }
            Destination::WebDav(dav) => {
                fields.put("kind", "webdav".to_owned());
                fields.put("url", dav.url.clone());
                fields.put("username", dav.username.clone());
            }
            Destination::Azure(azure) => {
                fields.put("kind", "azure".to_owned());
                fields.put("username", azure.account.clone());
                fields.put("bucket", azure.container.clone());
                fields.put("endpoint", azure.endpoint.clone());
                fields.put("root", azure.root.clone());
            }
            Destination::Memory => fields.put("kind", "memory".to_owned()),
        }

        Some(fields)
    }

    fn persist(&self) {
        crate::store::save(&self.connections.borrow());

        let observers = self.observers.borrow().clone();
        let alive = observers.into_iter().filter(|observer| observer()).collect::<Vec<_>>();

        *self.observers.borrow_mut() = alive;
    }
}

/// Puts back the settings the editor does not show.
///
/// The editor renders the fields a kind has to be typed in, which is not all of them: FTP also
/// carries whether to use TLS and passive mode, and both file protocols remember which
/// directory they start in. Rebuilding a destination from the form alone answers those with a
/// default — so a plain-FTP connection would quietly become an encrypted one the first time
/// anybody opened its settings and pressed Save.
///
/// Only carried between destinations of the same kind. Changing the kind really is a new
/// destination, and there is nothing to carry.
fn carried_over(edited: Destination, existing: Option<&Destination>) -> Destination {
    match (edited, existing) {
        (Destination::Ftp(edited), Some(Destination::Ftp(existing))) => Destination::Ftp(Ftp {
            encrypted: existing.encrypted,
            passive: existing.passive,
            home: existing.home.clone(),
            ..edited
        }),
        (Destination::Sftp(edited), Some(Destination::Sftp(existing))) => {
            Destination::Sftp(Sftp { home: existing.home.clone(), ..edited })
        }
        (Destination::S3(edited), Some(Destination::S3(existing))) => {
            Destination::S3(S3 { root: existing.root.clone(), ..edited })
        }
        (Destination::Azure(edited), Some(Destination::Azure(existing))) => {
            Destination::Azure(Azure { root: existing.root.clone(), ..edited })
        }
        (edited, _) => edited,
    }
}

/// The editor's fields, whichever of them the chosen kind asked for.
#[derive(Clone, Debug, Default)]
pub struct Fields {
    values: HashMap<String, String>,
}

impl Fields {
    pub fn put(&mut self, key: &str, value: String) {
        self.values.insert(key.to_owned(), value);
    }

    pub fn text(&self, key: &str) -> String {
        self.values.get(key).cloned().unwrap_or_default()
    }

    /// How to prove who we are over SSH. The agent is the default because it is what already
    /// works in a terminal on the same machine; the other two are for when it does not.
    fn credential(&self) -> SshCredential {
        match self.text("credential").as_str() {
            "password" => SshCredential::Password,
            "key" => SshCredential::Key { path: self.text("key") },
            _ => SshCredential::Agent,
        }
    }

    fn port(&self, default: u16) -> u16 {
        match self.text("port").parse::<u16>() {
            Ok(port) if port > 0 => port,
            _ => default,
        }
    }

    /// Turns the fields into a destination.
    ///
    /// R2 and B2 are the same S3 client with a preset filled in, which is exactly why they can
    /// be offered as their own choices without being their own adapters.
    fn destination(&self) -> Destination {
        let storage = |preset: S3Preset| {
            Destination::S3(S3 {
                bucket: self.text("bucket"),
                preset,
                region: self.text("region"),
                endpoint: self.text("endpoint"),
                root: self.text("root"),
            })
        };

        match self.text("kind").as_str() {
            "ftp" => Destination::Ftp(Ftp {
                host: self.text("host"),
                port: self.port(21),
                username: self.text("username"),
                encrypted: true,
                passive: true,
                home: String::new(),
            }),
            "s3" => storage(S3Preset::Aws),
            "r2" => storage(S3Preset::CloudflareR2),
            "b2" => storage(S3Preset::BackblazeB2),
            "webdav" => Destination::WebDav(WebDav {
                url: self.text("url"),
                username: self.text("username"),
            }),
            "azure" => Destination::Azure(Azure {
                account: self.text("username"),
                container: self.text("bucket"),
                endpoint: self.text("endpoint"),
                root: self.text("root"),
            }),
            _ => Destination::Sftp(Sftp {
                host: self.text("host"),
                port: self.port(22),
                username: self.text("username"),
                credential: self.credential(),
                home: String::new(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ftp(encrypted: bool) -> Destination {
        Destination::Ftp(Ftp {
            host: "files.example.com".to_owned(),
            port: 21,
            username: "okuri".to_owned(),
            encrypted,
            passive: false,
            home: "/home/okuri".to_owned(),
        })
    }

    /// The editor shows neither the TLS setting nor the login directory, so saving from it has
    /// to leave both exactly as they were. Turning TLS on behind someone's back means their
    /// connection stops working and the form gives no hint why.
    #[test]
    fn editing_a_connection_leaves_the_settings_the_editor_does_not_show() {
        let Destination::Ftp(saved) = carried_over(ftp(true), Some(&ftp(false))) else {
            panic!("still an FTP connection");
        };

        assert!(!saved.encrypted);
        assert!(!saved.passive);
        assert_eq!(saved.home, "/home/okuri");
        assert_eq!(saved.host, "files.example.com");
    }

    #[test]
    fn changing_the_kind_carries_nothing_over() {
        let changed = carried_over(Destination::Memory, Some(&ftp(false)));

        assert!(matches!(changed, Destination::Memory));
    }

    #[test]
    fn a_new_connection_has_nothing_to_carry_over() {
        let Destination::Ftp(new) = carried_over(ftp(true), None) else {
            panic!("still an FTP connection");
        };

        assert!(new.encrypted);
    }

    /// Every kind the editor offers has to build something, or choosing it from the menu makes
    /// an SFTP connection with the fields of whatever was actually filled in.
    ///
    /// Paired with the label each one shows in the list, which is how anyone would notice that
    /// choosing B2 had quietly made an ordinary S3 connection.
    #[test]
    fn every_kind_the_editor_offers_builds_its_own_destination() {
        for (kind, label) in KINDS {
            let mut fields = Fields::default();
            fields.put("kind", kind.to_owned());

            assert_eq!(fields.destination().kind(), label, "{kind}");
        }
    }

    #[test]
    fn a_port_that_is_not_a_port_falls_back_to_the_usual_one() {
        let mut fields = Fields::default();
        fields.put("kind", "ftp".to_owned());
        fields.put("port", "twenty-one".to_owned());

        let Destination::Ftp(ftp) = fields.destination() else {
            panic!("an FTP connection");
        };

        assert_eq!(ftp.port, 21);
    }
}
