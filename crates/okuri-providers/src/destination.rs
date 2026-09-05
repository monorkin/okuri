use serde::{Deserialize, Serialize};

/// Where a connection points, and everything about it that is safe to write down.
///
/// Nothing here is a secret. Passwords, keys, and tokens are held by the secret store and
/// looked up by the connection's id, which is what lets `connections.toml` be a file you can
/// read, diff, and sync without worrying about it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Destination {
    Sftp(Sftp),
    Ftp(Ftp),
    S3(S3),
    WebDav(WebDav),
    Azure(Azure),
    Memory,
}

impl Destination {
    /// The word for this kind of destination, as the connection editor shows it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Sftp(_) => "SFTP",
            Self::Ftp(_) => "FTP",
            Self::S3(storage) => storage.preset.label(),
            Self::WebDav(_) => "WebDAV",
            Self::Azure(_) => "Azure Blob Storage",
            Self::Memory => "Memory",
        }
    }

    /// A one-line summary for the connection list: enough to tell two accounts apart.
    pub fn summary(&self) -> String {
        match self {
            Self::Sftp(sftp) => format!("{}@{}", sftp.username, sftp.host),
            Self::Ftp(ftp) => format!("{}@{}", ftp.username, ftp.host),
            Self::S3(storage) => storage.bucket.clone(),
            Self::WebDav(dav) => dav.url.clone(),
            Self::Azure(azure) => format!("{}/{}", azure.account, azure.container),
            Self::Memory => "in memory".to_owned(),
        }
    }

    /// What connecting needs out of the secret store, which is also what to ask for when the
    /// store has nothing.
    pub fn secret_shape(&self) -> SecretShape {
        match self {
            Self::Memory => SecretShape::None,
            Self::Sftp(sftp) => match sftp.credential {
                SshCredential::Agent => SecretShape::None,
                // A key file may or may not have a passphrase, and only the file knows.
                SshCredential::Key { .. } => SecretShape::None,
                SshCredential::Password => SecretShape::Password,
            },
            // S3 and its lookalikes are the only ones that need two halves.
            Self::S3(_) => SecretShape::KeyPair,
            Self::Ftp(_) | Self::WebDav(_) | Self::Azure(_) => SecretShape::Password,
        }
    }

    pub fn needs_secret(&self) -> bool {
        self.secret_shape() != SecretShape::None
    }

    /// A URL the desktop's own file layer understands, if it understands this kind at all.
    ///
    /// GNOME and KDE can both open `sftp://`, `ftp://`, and `dav://` themselves. Handing one of
    /// those to a file manager instead of a local copy means the file never passes through
    /// Okuri at all: the drop starts immediately however large the file is, and the copy shows
    /// up in the file manager's own progress window rather than nowhere.
    ///
    /// The object stores have no such scheme, so they answer nothing.
    ///
    /// `path` is the whole path on the server. A connection's root is rarely the server's, and
    /// an address built from the part below it would point somewhere else entirely.
    pub fn url_for(&self, path: &str) -> Option<String> {
        let escaped = escape(path);

        match self {
            Self::Sftp(sftp) => Some(format!(
                "sftp://{}@{}{}{escaped}",
                sftp.username,
                sftp.host,
                port_suffix(sftp.port, 22),
            )),
            Self::Ftp(ftp) => Some(format!(
                "ftp://{}@{}{}{escaped}",
                ftp.username,
                ftp.host,
                port_suffix(ftp.port, 21),
            )),
            Self::WebDav(dav) => {
                let (scheme, rest) = dav.url.split_once("://")?;

                // `dav` and `davs` are what GVfs calls the two of them.
                let scheme = match scheme {
                    "https" => "davs",
                    "http" => "dav",
                    _ => return None,
                };

                Some(format!("{scheme}://{}{escaped}", rest.trim_end_matches('/')))
            }
            Self::S3(_) | Self::Azure(_) | Self::Memory => None,
        }
    }
}

/// The path, with the characters a URL cannot carry written the way a URL writes them.
///
/// Separators are left alone: they are the structure of the path, not part of a name.
fn escape(path: &str) -> String {
    const RESERVED: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~')
        .remove(b'/');

    percent_encoding::utf8_percent_encode(path, RESERVED).to_string()
}

fn port_suffix(port: u16, usual: u16) -> String {
    if port == usual {
        String::new()
    } else {
        format!(":{port}")
    }
}

/// What a destination wants from the secret store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretShape {
    None,
    Password,
    /// An identifier and its secret — an access key and the key itself.
    KeyPair,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sftp {
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub username: String,
    #[serde(default)]
    pub credential: SshCredential,
    /// Where the session starts. Empty means the account's home directory.
    #[serde(default)]
    pub home: String,
    /// Whether packets go out as soon as they are written, rather than held back so the
    /// kernel can bundle small ones. On unless switched off: SFTP is a conversation of small
    /// requests and acknowledgements, and every one held back for a bundle that never comes
    /// is a round trip spent waiting.
    #[serde(default = "default_nodelay")]
    pub nodelay: bool,
}

fn default_nodelay() -> bool {
    true
}

/// How to prove who we are to an SSH server, in the order most people actually use.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "kebab-case")]
pub enum SshCredential {
    /// Whatever `ssh-agent` is holding. The default, because it is what already works in a
    /// terminal on the same machine.
    #[default]
    Agent,
    /// A key file, with its passphrase — if it has one — in the secret store.
    Key { path: String },
    Password,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ftp {
    pub host: String,
    #[serde(default = "default_ftp_port")]
    pub port: u16,
    pub username: String,
    /// Explicit FTPS by default. Turning this off sends the password in the clear, so the
    /// connection editor says as much before it lets you.
    #[serde(default = "yes")]
    pub encrypted: bool,
    #[serde(default = "yes")]
    pub passive: bool,
    /// Where the session starts. Empty means wherever the server puts you when you log in,
    /// which is not necessarily the root of anything.
    #[serde(default)]
    pub home: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct S3 {
    pub bucket: String,
    #[serde(default)]
    pub preset: S3Preset,
    #[serde(default)]
    pub region: String,
    /// Overrides the preset's endpoint, for a self-hosted store or a region-specific host.
    #[serde(default)]
    pub endpoint: String,
    /// A prefix to treat as the root, so a connection can point at one folder of a shared
    /// bucket rather than the whole thing.
    #[serde(default)]
    pub root: String,
}

/// The S3-compatible services worth knowing about by name.
///
/// R2 and B2 are not separate protocols — they are S3 with a particular endpoint and
/// addressing style. Keeping them as a table rather than as adapters means the next one is a
/// row here and nothing else.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum S3Preset {
    #[default]
    Aws,
    CloudflareR2,
    BackblazeB2,
    DigitalOceanSpaces,
    Other,
}

impl S3Preset {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Aws => "Amazon S3",
            Self::CloudflareR2 => "Cloudflare R2",
            Self::BackblazeB2 => "Backblaze B2",
            Self::DigitalOceanSpaces => "DigitalOcean Spaces",
            Self::Other => "S3-compatible",
        }
    }

    /// The endpoint template, where `{}` is filled in from the account or region field. AWS
    /// has none because the SDK derives it from the region on its own.
    pub fn endpoint_template(&self) -> Option<&'static str> {
        match self {
            Self::Aws | Self::Other => None,
            Self::CloudflareR2 => Some("https://{}.r2.cloudflarestorage.com"),
            Self::BackblazeB2 => Some("https://s3.{}.backblazeb2.com"),
            Self::DigitalOceanSpaces => Some("https://{}.digitaloceanspaces.com"),
        }
    }

    /// What the region field means for this service, shown as the field's hint. R2 has one
    /// global region and asks for the account id in its place.
    pub fn region_hint(&self) -> &'static str {
        match self {
            Self::Aws => "eu-central-1",
            Self::CloudflareR2 => "your account id",
            Self::BackblazeB2 => "eu-central-003",
            Self::DigitalOceanSpaces => "fra1",
            Self::Other => "us-east-1",
        }
    }

    /// R2 signs against a fixed region regardless of where the bucket lives.
    pub fn signing_region(&self, region: &str) -> String {
        match self {
            Self::CloudflareR2 => "auto".to_owned(),
            _ => region.to_owned(),
        }
    }
}

impl S3 {
    /// The endpoint to talk to: an explicit override wins, then the preset's template, and
    /// otherwise nothing, which leaves the SDK to work it out from the region.
    pub fn resolved_endpoint(&self) -> Option<String> {
        if !self.endpoint.is_empty() {
            Some(self.endpoint.clone())
        } else {
            self.preset
                .endpoint_template()
                .map(|template| template.replace("{}", &self.region))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebDav {
    pub url: String,
    pub username: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Azure {
    pub account: String,
    pub container: String,
    /// Overrides the usual `https://{account}.blob.core.windows.net`, for Azurite or for a
    /// sovereign cloud.
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub root: String,
}

impl Azure {
    pub fn resolved_endpoint(&self) -> String {
        if self.endpoint.is_empty() {
            format!("https://{}.blob.core.windows.net", self.account)
        } else {
            self.endpoint.trim_end_matches('/').to_owned()
        }
    }
}

fn default_ssh_port() -> u16 {
    22
}

fn default_ftp_port() -> u16 {
    21
}

fn yes() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_fill_in_the_endpoint_from_the_region_field() {
        let r2 = S3 {
            bucket: "assets".to_owned(),
            preset: S3Preset::CloudflareR2,
            region: "abc123".to_owned(),
            endpoint: String::new(),
            root: String::new(),
        };

        assert_eq!(
            r2.resolved_endpoint().as_deref(),
            Some("https://abc123.r2.cloudflarestorage.com")
        );
        assert_eq!(r2.preset.signing_region(&r2.region), "auto");
    }

    #[test]
    fn an_explicit_endpoint_wins_over_the_preset() {
        let minio = S3 {
            bucket: "assets".to_owned(),
            preset: S3Preset::BackblazeB2,
            region: "eu-central-003".to_owned(),
            endpoint: "http://localhost:9000".to_owned(),
            root: String::new(),
        };

        assert_eq!(minio.resolved_endpoint().as_deref(), Some("http://localhost:9000"));
    }

    #[test]
    fn amazon_leaves_the_endpoint_to_the_sdk() {
        let aws = S3 {
            bucket: "assets".to_owned(),
            preset: S3Preset::Aws,
            region: "eu-central-1".to_owned(),
            endpoint: String::new(),
            root: String::new(),
        };

        assert_eq!(aws.resolved_endpoint(), None);
    }

    #[test]
    fn a_destination_round_trips_through_the_config_file() {
        let destination = Destination::Sftp(Sftp {
            host: "example.com".to_owned(),
            port: 22,
            username: "stanko".to_owned(),
            credential: SshCredential::Key { path: "~/.ssh/id_ed25519".to_owned() },
            home: "/srv".to_owned(),
            nodelay: true,
        });

        let written = toml::to_string(&destination).unwrap();
        let read_back: Destination = toml::from_str(&written).unwrap();

        assert_eq!(read_back, destination);
    }

    #[test]
    fn ports_and_encryption_have_sensible_defaults() {
        let ftp: Ftp = toml::from_str(
            r##"
                host = "files.example.com"
                username = "anonymous"
            "##,
        )
        .unwrap();

        assert_eq!(ftp.port, 21);
        assert!(ftp.encrypted);
        assert!(ftp.passive);
    }

    #[test]
    fn the_desktop_can_be_handed_a_url_for_the_protocols_it_speaks() {
        let sftp = Destination::Sftp(Sftp {
            host: "example.com".to_owned(),
            port: 22,
            username: "stanko".to_owned(),
            credential: SshCredential::Agent,
            home: "/srv".to_owned(),
            nodelay: true,
        });

        // The whole path, as the server sees it — the connection's root is not the server's.
        assert_eq!(
            sftp.url_for("/srv/logs/last week.txt").as_deref(),
            Some("sftp://stanko@example.com/srv/logs/last%20week.txt")
        );

        let odd_port = Destination::Ftp(Ftp {
            host: "files.example.com".to_owned(),
            port: 2121,
            username: "okuri".to_owned(),
            encrypted: true,
            passive: true,
            home: String::new(),
        });

        assert_eq!(
            odd_port.url_for("/notes.txt").as_deref(),
            Some("ftp://okuri@files.example.com:2121/notes.txt")
        );

        let dav = Destination::WebDav(WebDav {
            url: "https://dav.example.com/remote.php/".to_owned(),
            username: "okuri".to_owned(),
        });

        assert_eq!(
            dav.url_for("/notes.txt").as_deref(),
            Some("davs://dav.example.com/remote.php/notes.txt")
        );
    }

    /// An object store has no scheme the desktop knows, so it says so rather than inventing
    /// one that would fail on the other end.
    #[test]
    fn an_object_store_has_no_url_the_desktop_could_open() {
        let bucket = Destination::S3(S3 {
            bucket: "assets".to_owned(),
            preset: S3Preset::Aws,
            region: "eu-central-1".to_owned(),
            endpoint: String::new(),
            root: String::new(),
        });

        assert_eq!(bucket.url_for("/notes.txt"), None);
        assert_eq!(Destination::Memory.url_for("/notes.txt"), None);
    }

    #[test]
    fn only_the_destinations_that_hold_a_password_need_the_secret_store() {
        let agent = Destination::Sftp(Sftp {
            host: "example.com".to_owned(),
            port: 22,
            username: "stanko".to_owned(),
            credential: SshCredential::Agent,
            home: String::new(),
            nodelay: true,
        });

        assert!(!agent.needs_secret());
        assert!(!Destination::Memory.needs_secret());
    }
}
