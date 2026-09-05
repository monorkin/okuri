use std::sync::Arc;

use async_trait::async_trait;
use okuri_core::{
    ByteRange, ByteStream, Capabilities, Entry, Error, Linking, Owning, Ownership, Permissions,
    Permitting, Provider, RemotePath, Result,
};
use russh::client;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::destination::{Sftp, SshCredential};
use crate::secret::Secret;
use crate::ssh_config::SshConfig;
use crate::trust::{HostKey, HostTrust};

/// A remote filesystem over SSH.
///
/// The closest thing to a local disk any of the destinations offer: real folders, real renames,
/// real permissions. Everything in [`Capabilities`] is native.
pub struct SftpProvider {
    label: String,
    home: String,
    sftp: SftpSession,
    connection: client::Handle<HostKeyCheck>,
}

impl SftpProvider {
    pub async fn connect(
        config: &Sftp,
        secret: &Secret,
        trust: Arc<dyn HostTrust>,
    ) -> Result<Self> {
        // What `ssh` would do with this name, which is often not what the name says: an alias
        // for somewhere else, on another port, with a key or an agent of its own.
        let ssh = SshConfig::for_host(&config.host);

        let host = ssh.hostname.clone().unwrap_or_else(|| config.host.clone());
        let port = match config.port {
            22 => ssh.port.unwrap_or(22),
            chosen => chosen,
        };
        let username = match config.username.is_empty() {
            true => ssh.user.clone().unwrap_or_default(),
            false => config.username.clone(),
        };

        let handler = HostKeyCheck {
            trust,
            host: host.clone(),
            port,
        };

        let settings = client::Config { nodelay: config.nodelay, ..client::Config::default() };

        let mut connection = client::connect(
            Arc::new(settings),
            (host.as_str(), port),
            handler,
        )
        .await
        .map_err(|error| Error::caused_by(format!("could not reach {host}"), error))?;

        authenticate(&mut connection, config, &username, &ssh, secret).await?;

        let channel = connection
            .channel_open_session()
            .await
            .map_err(|error| Error::caused_by("could not open an SSH channel", error))?;

        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|error| Error::caused_by("the server refused the SFTP subsystem", error))?;

        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(|error| Error::caused_by("could not start an SFTP session", error))?;

        let home = if config.home.is_empty() {
            sftp.canonicalize(".")
                .await
                .map_err(|error| Error::caused_by("could not find the home directory", error))?
        } else {
            config.home.clone()
        };

        Ok(Self {
            label: format!("{}@{}", config.username, config.host),
            home: home.trim_end_matches('/').to_owned(),
            sftp,
            connection,
        })
    }

    fn absolute(&self, path: &RemotePath) -> String {
        crate::keys::under(&self.home, path)
    }
}

#[async_trait]
impl Owning for SftpProvider {
    /// The name when the server gives one, the number when it does not. A `uid` is still an
    /// answer, and a blank row where the owner should be is not.
    async fn ownership(&self, path: &RemotePath) -> Result<Ownership> {
        let attributes = self
            .sftp
            .metadata(self.absolute(path))
            .await
            .map_err(|error| translate(error, path))?;

        Ok(Ownership {
            user: attributes.user.or_else(|| attributes.uid.map(|uid| uid.to_string())),
            group: attributes.group.or_else(|| attributes.gid.map(|gid| gid.to_string())),
        })
    }
}

#[async_trait]
impl Linking for SftpProvider {
    async fn link_target(&self, path: &RemotePath) -> Result<Option<String>> {
        let absolute = self.absolute(path);

        // Asked of the link itself rather than of what it points at, which is the only way to
        // find out that it is a link at all.
        let attributes = self
            .sftp
            .symlink_metadata(absolute.clone())
            .await
            .map_err(|error| translate(error, path))?;

        if !attributes.is_symlink() {
            return Ok(None);
        }

        Ok(self.sftp.read_link(absolute).await.ok())
    }
}

#[async_trait]
impl Permitting for SftpProvider {
    async fn set_permissions(&self, path: &RemotePath, permissions: Permissions) -> Result<()> {
        // Only the mode is sent. `FileAttributes` also carries the owner and the times, and a
        // default one would ask the server to set those to nothing.
        let attributes = russh_sftp::protocol::FileAttributes {
            permissions: Some(permissions.mode()),
            ..Default::default()
        };

        self.sftp
            .set_metadata(self.absolute(path), attributes)
            .await
            .map_err(|error| translate(error, path))
    }
}

#[async_trait]
impl Provider for SftpProvider {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn home(&self) -> String {
        self.home.clone()
    }

    fn owning(&self) -> Option<&dyn Owning> {
        Some(self)
    }

    fn linking(&self) -> Option<&dyn Linking> {
        Some(self)
    }

    fn permitting(&self) -> Option<&dyn Permitting> {
        Some(self)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::filesystem()
    }

    async fn list(&self, path: &RemotePath) -> Result<Vec<Entry>> {
        let listing = self
            .sftp
            .read_dir(self.absolute(path))
            .await
            .map_err(|error| translate(error, path))?;

        Ok(listing
            .filter(|entry| entry.file_name() != "." && entry.file_name() != "..")
            .map(|entry| describe(entry.file_name(), &entry.metadata()))
            .collect())
    }

    async fn stat(&self, path: &RemotePath) -> Result<Entry> {
        let metadata = self
            .sftp
            .metadata(self.absolute(path))
            .await
            .map_err(|error| translate(error, path))?;

        Ok(describe(path.name().unwrap_or("/").to_owned(), &metadata))
    }

    async fn read(&self, path: &RemotePath, range: Option<ByteRange>) -> Result<ByteStream> {
        let mut file = self
            .sftp
            .open(self.absolute(path))
            .await
            .map_err(|error| translate(error, path))?;

        // Not defaulted to zero: that would read a file the server has as an empty one and
        // call the download a success.
        let size = file
            .metadata()
            .await
            .map_err(|error| translate(error, path))?
            .size
            .ok_or_else(|| Error::provider(format!("{path} has no size")))?;

        let (offset, length) = match range {
            None => (0, size),
            Some(range) => {
                let offset = range.offset.min(size);
                (offset, range.length.unwrap_or(u64::MAX).min(size - offset))
            }
        };

        if offset > 0 {
            file.seek(std::io::SeekFrom::Start(offset))
                .await
                .map_err(|error| Error::caused_by("could not seek", error))?;
        }

        let chunks = futures::StreamExt::map(
            tokio_util::io::ReaderStream::with_capacity(file.take(length), READ_CHUNK),
            |chunk| chunk.map_err(|error| Error::caused_by("the download was interrupted", error)),
        );

        Ok(ByteStream::new(chunks, Some(length)))
    }

    async fn write(&self, path: &RemotePath, body: ByteStream) -> Result<()> {
        let mut file = self
            .sftp
            .open_with_flags(
                self.absolute(path),
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            )
            .await
            .map_err(|error| translate(error, path))?;

        // Written straight from the stream rather than through `tokio::io::copy`, whose own
        // buffer is eight kilobytes: every write would be that size no matter how much had
        // been read. The SFTP client sends up to eight writes without waiting for the replies,
        // so the size of each one is what decides whether a link is filled or a round trip is
        // paid for every eight kilobytes.
        let mut body = body;

        while let Some(chunk) = futures::StreamExt::next(&mut body).await {
            let chunk = chunk?;

            file.write_all(&chunk)
                .await
                .map_err(|error| Error::caused_by("the upload was interrupted", error))?;
        }

        file.shutdown()
            .await
            .map_err(|error| Error::caused_by("the upload could not be finished", error))?;

        Ok(())
    }

    async fn delete(&self, path: &RemotePath) -> Result<()> {
        let entry = self.stat(path).await?;
        let absolute = self.absolute(path);

        if entry.kind.is_folder() {
            self.sftp.remove_dir(absolute).await
        } else {
            self.sftp.remove_file(absolute).await
        }
        .map_err(|error| translate(error, path))
    }

    async fn create_folder(&self, path: &RemotePath) -> Result<()> {
        self.sftp
            .create_dir(self.absolute(path))
            .await
            .map_err(|error| translate(error, path))
    }

    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<()> {
        self.sftp
            .rename(self.absolute(from), self.absolute(to))
            .await
            .map_err(|error| translate(error, from))
    }

    async fn disconnect(&self) -> Result<()> {
        let _ = self.sftp.close().await;
        let _ = self
            .connection
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await;

        Ok(())
    }
}

async fn authenticate(
    connection: &mut client::Handle<HostKeyCheck>,
    config: &Sftp,
    username: &str,
    ssh: &SshConfig,
    secret: &Secret,
) -> Result<()> {
    let accepted = match &config.credential {
        SshCredential::Password => {
            let password = secret
                .password()
                .ok_or_else(|| Error::Authentication("no password was provided".to_owned()))?;

            connection
                .authenticate_password(username, password)
                .await
                .map_err(|error| Error::caused_by("the password was refused", error))?
                .success()
        }
        SshCredential::Key { path } => {
            let key = russh::keys::load_secret_key(expand_home(path), secret.password())
                .map_err(|error| match error {
                    // An encrypted key is not a broken one. Saying which it is lets the
                    // passphrase be asked for rather than the connection simply failing.
                    russh::keys::Error::KeyIsEncrypted => {
                        Error::NeedsPassphrase { path: path.clone() }
                    }
                    error => Error::caused_by(format!("could not read {path}"), error),
                })?;

            connection
                .authenticate_publickey(
                    username,
                    russh::keys::PrivateKeyWithHashAlg::new(
                        Arc::new(key),
                        Some(russh::keys::HashAlg::Sha256),
                    ),
                )
                .await
                .map_err(|error| Error::caused_by("the key was refused", error))?
                .success()
        }
        SshCredential::Agent => {
            let attempt = authenticate_like_ssh(connection, username, ssh).await?;

            if !attempt.accepted {
                return Err(Error::Authentication(format!(
                    "{username} was not accepted by {}: {}",
                    config.host,
                    attempt.describe()
                )));
            }

            true
        }
    };

    if accepted {
        Ok(())
    } else {
        Err(Error::Authentication(format!(
            "{} was not accepted by {}",
            config.username, config.host
        )))
    }
}

/// How much is asked for or handed over at a time.
///
/// Matches the SFTP client's own maximum packet, so a read or a write becomes one request
/// rather than being cut up into several.
const READ_CHUNK: usize = 256 * 1024;

/// Signs in the way `ssh` would: the agent first, then the usual key files.
///
/// Anyone who can reach a server from a terminal expects to reach it from here, and `ssh` does
/// not stop at the agent. Whatever is tried is written down, so that failing says what was
/// attempted rather than guessing at a reason.
async fn authenticate_like_ssh(
    connection: &mut client::Handle<HostKeyCheck>,
    username: &str,
    ssh: &SshConfig,
) -> Result<Attempt> {
    let mut attempt = Attempt::default();

    offer_agent_keys(connection, username, ssh, &mut attempt).await;

    if !attempt.accepted {
        offer_key_files(connection, username, ssh, &mut attempt).await;
    }

    Ok(attempt)
}

/// Every key the agent is holding, which is how anyone with a passphrase-protected key or a
/// hardware token signs in without being asked for anything.
async fn offer_agent_keys(
    connection: &mut client::Handle<HostKeyCheck>,
    username: &str,
    ssh: &SshConfig,
    attempt: &mut Attempt,
) {
    let mut agent = match agent(ssh).await {
        Ok(Some(agent)) => agent,
        Ok(None) => {
            attempt.tried.push(
                "no SSH agent (SSH_AUTH_SOCK is not set, and ~/.ssh/config names none)".to_owned(),
            );

            return;
        }
        Err(error) => {
            attempt.tried.push(format!("the SSH agent ({error})"));

            return;
        }
    };

    let identities = agent.request_identities().await.unwrap_or_default();

    if identities.is_empty() {
        attempt.tried.push("the SSH agent, which is holding no keys".to_owned());
    }

    for identity in identities {
        let (key, name) = named_key(identity);

        let signed = connection
            .authenticate_publickey_with(
                username,
                key,
                Some(russh::keys::HashAlg::Sha256),
                &mut agent,
            )
            .await;

        match signed {
            Ok(result) if result.success() => {
                attempt.accepted = true;

                return;
            }
            Ok(_) => attempt.tried.push(format!("{name} from the agent")),
            Err(error) => attempt.tried.push(format!("{name} from the agent ({error})")),
        }
    }
}

/// An agent holds public keys and certificates alike, and both name themselves by their comment.
fn named_key(identity: russh::keys::agent::AgentIdentity) -> (russh::keys::PublicKey, String) {
    match identity {
        russh::keys::agent::AgentIdentity::PublicKey { key, comment } => (key, comment),
        russh::keys::agent::AgentIdentity::Certificate { certificate, comment } => (
            russh::keys::PublicKey::new(certificate.public_key().clone(), ""),
            comment,
        ),
    }
}

/// The key files `~/.ssh/config` names, and the ones `ssh` would try without being told.
async fn offer_key_files(
    connection: &mut client::Handle<HostKeyCheck>,
    username: &str,
    ssh: &SshConfig,
    attempt: &mut Attempt,
) {
    for path in identities(ssh) {
        let shown = path.display().to_string();

        // A key that needs a passphrase cannot be used this way: there is nowhere to ask for
        // one, so it says which key it was and how to use it.
        let Ok(key) = russh::keys::load_secret_key(&path, None).inspect_err(|error| {
            attempt.tried.push(format!(
                "{shown} ({}) — choose \"Key file\" to be asked for its passphrase",
                short(error)
            ))
        }) else {
            continue;
        };

        let signed = connection
            .authenticate_publickey(
                username,
                russh::keys::PrivateKeyWithHashAlg::new(
                    std::sync::Arc::new(key),
                    Some(russh::keys::HashAlg::Sha256),
                ),
            )
            .await;

        match signed {
            Ok(result) if result.success() => {
                attempt.accepted = true;

                return;
            }
            Ok(_) => attempt.tried.push(shown),
            Err(error) => attempt.tried.push(format!("{shown} ({error})")),
        }
    }
}

/// What signing in tried, so that failing can say so.
#[derive(Default)]
struct Attempt {
    accepted: bool,
    tried: Vec<String>,
}

impl Attempt {
    fn describe(&self) -> String {
        match self.tried.is_empty() {
            true => "nothing to sign in with was found".to_owned(),
            false => format!("tried {}", self.tried.join(", ")),
        }
    }
}

/// The agent, if there is one to reach.
///
/// Not finding `SSH_AUTH_SOCK` and failing to reach the socket it names are different problems
/// with different answers, so they are told apart here rather than reported as the same thing.
async fn agent(
    ssh: &SshConfig,
) -> std::result::Result<Option<russh::keys::agent::client::AgentClient<tokio::net::UnixStream>>, String>
{
    // `IdentityAgent` wins over the session's own, which is the whole point of writing it down:
    // a password manager runs an agent and says so there.
    let socket = match &ssh.identity_agent {
        Some(named) => named.clone().into_os_string(),
        None => match std::env::var_os("SSH_AUTH_SOCK") {
            Some(socket) => socket,
            None => return Ok(None),
        },
    };

    russh::keys::agent::client::AgentClient::connect_uds(&socket)
        .await
        .map(Some)
        .map_err(|error| format!("{} could not be reached: {error}", socket.to_string_lossy()))
}

/// The keys to offer: the ones the config names, and otherwise the ones `ssh` falls back to.
fn identities(ssh: &SshConfig) -> Vec<std::path::PathBuf> {
    if !ssh.identity_files.is_empty() {
        return ssh
            .identity_files
            .iter()
            .filter(|path| path.is_file())
            .cloned()
            .collect();
    }

    default_keys()
}

/// The key files `ssh` reads when nothing else says otherwise, in the order it reads them.
fn default_keys() -> Vec<std::path::PathBuf> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };

    let ssh = std::path::Path::new(&home).join(".ssh");

    ["id_ed25519", "id_ecdsa", "id_ecdsa_sk", "id_ed25519_sk", "id_rsa"]
        .iter()
        .map(|name| ssh.join(name))
        .filter(|path| path.is_file())
        .collect()
}

fn short(error: &impl std::fmt::Display) -> String {
    error.to_string().lines().next().unwrap_or_default().to_owned()
}

struct HostKeyCheck {
    trust: Arc<dyn HostTrust>,
    host: String,
    port: u16,
}

impl client::Handler for HostKeyCheck {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        offered: &russh::keys::PublicKeyOrCertificate,
    ) -> std::result::Result<bool, Self::Error> {
        let key = match offered {
            russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            russh::keys::PublicKeyOrCertificate::Certificate(certificate) => {
                russh::keys::PublicKey::new(certificate.public_key().clone(), "")
            }
        };

        let host_key = HostKey {
            host: self.host.clone(),
            port: self.port,
            algorithm: key.algorithm().to_string(),
            fingerprint: key.fingerprint(Default::default()).to_string(),
            public_key: key
                .to_openssh()
                .unwrap_or_default()
                .split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" "),
        };

        Ok(self.trust.verify(&host_key).await.is_trusted())
    }
}

/// `known_hosts` writes a non-standard port as `[host]:port`, and Okuri has to match that
/// exactly or `ssh` and Okuri will disagree about what has been trusted.
pub fn known_hosts_host(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_owned()
    } else {
        format!("[{host}]:{port}")
    }
}

fn expand_home(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => match std::env::var("HOME") {
            Ok(home) => format!("{home}/{rest}"),
            Err(_) => path.to_owned(),
        },
        None => path.to_owned(),
    }
}

fn describe(name: String, metadata: &russh_sftp::protocol::FileAttributes) -> Entry {
    let mut entry = if metadata.is_dir() {
        Entry::folder(name)
    } else {
        Entry::file(name, metadata.size.unwrap_or_default())
    };

    entry.modified = metadata
        .mtime
        .and_then(|mtime| time::OffsetDateTime::from_unix_timestamp(i64::from(mtime)).ok());
    entry.permissions = metadata.permissions.map(Permissions);

    entry
}

/// Turns an SFTP status into the domain's own vocabulary, so the UI can tell a missing file
/// from a refused one without reading English.
fn translate(error: russh_sftp::client::error::Error, path: &RemotePath) -> Error {
    use russh_sftp::protocol::StatusCode;

    match error {
        russh_sftp::client::error::Error::Status(status) => match status.status_code {
            StatusCode::NoSuchFile => Error::NotFound { path: path.clone() },
            StatusCode::PermissionDenied => Error::PermissionDenied { path: path.clone() },
            _ => Error::provider(status.error_message),
        },
        error => Error::caused_by(format!("{path} could not be reached"), error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ssh` writes a non-standard port in brackets. A key trusted from a terminal has to be
    /// found under the same name here, or Okuri asks again about a host that is already
    /// trusted — and writes a second entry when the answer is yes.
    #[test]
    fn a_non_standard_port_is_written_the_way_known_hosts_writes_it() {
        assert_eq!(known_hosts_host("shire", 22), "shire");
        assert_eq!(known_hosts_host("shire", 2222), "[shire]:2222");
    }

    #[test]
    fn a_leading_tilde_becomes_the_home_directory() {
        // SAFETY: the tests in this module are the only readers of HOME.
        unsafe { std::env::set_var("HOME", "/home/okuri") };

        assert_eq!(expand_home("~/.ssh/id_ed25519"), "/home/okuri/.ssh/id_ed25519");
        assert_eq!(expand_home("/etc/keys/id_ed25519"), "/etc/keys/id_ed25519");

        // Only the leading one: a file really called `~` in a directory is not a home.
        assert_eq!(expand_home("keys/~/id_ed25519"), "keys/~/id_ed25519");
    }

    /// The UI greys out and phrases things from these, so a missing file arriving as a generic
    /// provider failure would read as "something went wrong" for a file that is simply not there.
    #[test]
    fn sftp_statuses_become_the_domains_own_errors() {
        use russh_sftp::protocol::{Status, StatusCode};

        let path = RemotePath::parse("/reports/q3.txt").unwrap();
        let status = |code| {
            russh_sftp::client::error::Error::Status(Status {
                id: 1,
                status_code: code,
                error_message: "no".to_owned(),
                language_tag: String::new(),
            })
        };

        assert!(matches!(
            translate(status(StatusCode::NoSuchFile), &path),
            Error::NotFound { .. }
        ));
        assert!(matches!(
            translate(status(StatusCode::PermissionDenied), &path),
            Error::PermissionDenied { .. }
        ));
        assert!(matches!(
            translate(status(StatusCode::Failure), &path),
            Error::Provider { .. }
        ));
    }
}
