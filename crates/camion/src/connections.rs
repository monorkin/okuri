use std::pin::Pin;

use camion_engine::{Connection, Connections};
use camion_providers::destination::{Ftp, S3Preset, Sftp, SshCredential, WebDav, S3};
use camion_providers::{Azure, Destination};
use cxx_qt::CxxQtType;
use cxx_qt_lib::{
    QByteArray, QHash, QHashPair_i32_QByteArray, QMap, QMapPair_QString_QVariant, QModelIndex,
    QString, QVariant,
};

/// The saved connections, and the editor that adds to them.
#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qvariant.h");
        type QVariant = cxx_qt_lib::QVariant;
        include!("cxx-qt-lib/qmodelindex.h");
        type QModelIndex = cxx_qt_lib::QModelIndex;
        include!("cxx-qt-lib/qhash.h");
        type QHash_i32_QByteArray = cxx_qt_lib::QHash<cxx_qt_lib::QHashPair_i32_QByteArray>;
        include!("cxx-qt-lib/qmap.h");
        type QMap_QString_QVariant = cxx_qt_lib::QMap<cxx_qt_lib::QMapPair_QString_QVariant>;
    }

    unsafe extern "C++Qt" {
        include!(<QtCore/QAbstractListModel>);

        type QAbstractListModel = crate::qt::qobject::QAbstractListModel;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[base = QAbstractListModel]
        #[qproperty(i32, count)]
        type ConnectionList = super::ConnectionListRust;

        #[qinvokable]
        #[cxx_override]
        fn data(self: &ConnectionList, index: &QModelIndex, role: i32) -> QVariant;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &ConnectionList, parent: &QModelIndex) -> i32;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &ConnectionList) -> QHash_i32_QByteArray;

        /// Adds or replaces a connection from the editor's fields, which arrive as a plain
        /// object — `{ name, kind, host, … }` — so the editor can carry only what the chosen
        /// kind actually needs.
        #[qinvokable]
        fn save(self: Pin<&mut ConnectionList>, fields: QMap_QString_QVariant);

        #[qinvokable]
        fn forget(self: Pin<&mut ConnectionList>, id: QString);

        #[qinvokable]
        fn id_at(self: &ConnectionList, row: i32) -> QString;

        /// Everything a saved connection holds, so the editor can show it rather than opening
        /// blank and quietly replacing what was there.
        #[qinvokable]
        fn details(self: &ConnectionList, id: QString) -> QMap_QString_QVariant;

        /// The kinds the editor offers, in the order it offers them.
        #[qinvokable]
        fn kinds(self: &ConnectionList) -> QString;
    }

    unsafe extern "RustQt" {
        #[inherit]
        #[cxx_name = "beginResetModel"]
        unsafe fn begin_reset_model(self: Pin<&mut ConnectionList>);

        #[inherit]
        #[cxx_name = "endResetModel"]
        unsafe fn end_reset_model(self: Pin<&mut ConnectionList>);
    }
}

const USER_ROLE: i32 = 0x0100;

const IDENTIFIER: i32 = USER_ROLE;
const NAME: i32 = USER_ROLE + 1;
const SUMMARY: i32 = USER_ROLE + 2;
const KIND: i32 = USER_ROLE + 3;

pub struct ConnectionListRust {
    count: i32,
    connections: Connections,
}

impl Default for ConnectionListRust {
    fn default() -> Self {
        let connections = crate::store::load();

        Self { count: connections.entries.len() as i32, connections }
    }
}

impl qobject::ConnectionList {
    pub fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let Some(connection) = self.rust().connections.entries.get(index.row() as usize) else {
            return QVariant::default();
        };

        match role {
            IDENTIFIER => QVariant::from(&QString::from(&connection.id)),
            NAME => QVariant::from(&QString::from(&connection.name)),
            SUMMARY => QVariant::from(&QString::from(&connection.destination.summary())),
            KIND => QVariant::from(&QString::from(connection.destination.kind())),
            _ => QVariant::default(),
        }
    }

    pub fn row_count(&self, _parent: &QModelIndex) -> i32 {
        self.rust().connections.entries.len() as i32
    }

    pub fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut names = QHash::<QHashPair_i32_QByteArray>::default();

        names.insert(IDENTIFIER, QByteArray::from("identifier"));
        names.insert(NAME, QByteArray::from("name"));
        names.insert(SUMMARY, QByteArray::from("summary"));
        names.insert(KIND, QByteArray::from("kind"));

        names
    }

    pub fn save(mut self: Pin<&mut Self>, fields: QMap<QMapPair_QString_QVariant>) {
        let fields = Fields::from(&fields);
        let name = fields.text("name");
        let id = fields.text("id");

        let connection = Connection {
            id: match id.is_empty() {
                true => self.rust().connections.unused_id(&name),
                false => id,
            },
            name,
            destination: fields.destination(),
        };

        self.as_mut().rust_mut().connections.put(connection);
        self.persist();
    }

    pub fn forget(mut self: Pin<&mut Self>, id: QString) {
        self.as_mut().rust_mut().connections.remove(&id.to_string());
        self.persist();
    }

    pub fn id_at(&self, row: i32) -> QString {
        QString::from(
            &self
                .rust()
                .connections
                .entries
                .get(row.max(0) as usize)
                .map(|connection| connection.id.clone())
                .unwrap_or_default(),
        )
    }

    pub fn details(&self, id: QString) -> QMap<QMapPair_QString_QVariant> {
        let mut fields = QMap::<QMapPair_QString_QVariant>::default();

        let Some(connection) = self.rust().connections.find(&id.to_string()) else {
            return fields;
        };

        let mut put = |key: &str, value: String| {
            fields.insert(QString::from(key), QVariant::from(&QString::from(&value)));
        };

        put("id", connection.id.clone());
        put("name", connection.name.clone());

        match &connection.destination {
            Destination::Sftp(sftp) => {
                put("kind", "sftp".to_owned());
                put("host", sftp.host.clone());
                put("port", sftp.port.to_string());
                put("username", sftp.username.clone());
                put("home", sftp.home.clone());

                match &sftp.credential {
                    SshCredential::Agent => put("credential", "agent".to_owned()),
                    SshCredential::Password => put("credential", "password".to_owned()),
                    SshCredential::Key { path } => {
                        put("credential", "key".to_owned());
                        put("key", path.clone());
                    }
                }
            }
            Destination::Ftp(ftp) => {
                put("kind", "ftp".to_owned());
                put("host", ftp.host.clone());
                put("port", ftp.port.to_string());
                put("username", ftp.username.clone());
                put("home", ftp.home.clone());
            }
            Destination::S3(storage) => {
                put("kind", match storage.preset {
                    S3Preset::CloudflareR2 => "r2",
                    S3Preset::BackblazeB2 => "b2",
                    _ => "s3",
                }.to_owned());
                put("bucket", storage.bucket.clone());
                put("region", storage.region.clone());
                put("endpoint", storage.endpoint.clone());
                put("root", storage.root.clone());
            }
            Destination::WebDav(dav) => {
                put("kind", "webdav".to_owned());
                put("url", dav.url.clone());
                put("username", dav.username.clone());
            }
            Destination::Azure(azure) => {
                put("kind", "azure".to_owned());
                put("username", azure.account.clone());
                put("bucket", azure.container.clone());
                put("endpoint", azure.endpoint.clone());
                put("root", azure.root.clone());
            }
            Destination::Memory => put("kind", "memory".to_owned()),
        }

        fields
    }

    pub fn kinds(&self) -> QString {
        QString::from("sftp,ftp,s3,r2,b2,webdav,azure")
    }

    fn persist(mut self: Pin<&mut Self>) {
        crate::store::save(&self.rust().connections);

        unsafe { self.as_mut().begin_reset_model() };
        unsafe { self.as_mut().end_reset_model() };

        let count = self.rust().connections.entries.len() as i32;
        self.as_mut().set_count(count);
    }
}

/// The editor's fields, whichever of them the chosen kind asked for.
struct Fields {
    values: QMap<QMapPair_QString_QVariant>,
}

impl From<&QMap<QMapPair_QString_QVariant>> for Fields {
    fn from(values: &QMap<QMapPair_QString_QVariant>) -> Self {
        Self { values: values.clone() }
    }
}

impl Fields {
    fn text(&self, key: &str) -> String {
        self.values
            .get(&QString::from(key))
            .and_then(|value| value.value::<QString>())
            .map(|value| value.to_string())
            .unwrap_or_default()
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
                home: self.text("home"),
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
                home: self.text("home"),
            }),
        }
    }
}
