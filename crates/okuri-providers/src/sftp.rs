use std::sync::Arc;

use async_trait::async_trait;
use bytes::{Buf, BytesMut};
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

        // A channel's window is how much the server may send before it has to stop and wait to
        // be told the bytes were taken, and everything on the channel shares it. `russh` opens
        // two megabytes, which one download with its reads pipelined fills on its own — leaving
        // the four transfers this connection says it will run at once half a megabyte each.
        //
        // Eight megabytes is enough for all four at full stretch. It is a credit rather than a
        // buffer: nothing is allocated for it, and what arrives is parsed as it lands.
        let settings = client::Config {
            window_size: 8 * 1024 * 1024,
            nodelay: config.nodelay,
            ..client::Config::default()
        };

        let mut connection =
            client::connect(Arc::new(settings), (host.as_str(), port), handler)
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
        self.read_sized(path, range, None).await
    }

    async fn read_sized(
        &self,
        path: &RemotePath,
        range: Option<ByteRange>,
        size: Option<u64>,
    ) -> Result<ByteStream> {
        let file = self
            .sftp
            .open(self.absolute(path))
            .await
            .map_err(|error| translate(error, path))?;

        // Asked for only when nothing has said. SFTP is the destination where this costs: a
        // download planned from a listing was told how long every file is, and asking the
        // server again is a round trip per file before any of them starts moving.
        //
        // Not defaulted to zero when the server will not say: that would read a file the server
        // has as an empty one and call the download a success.
        let size = match size {
            Some(size) => size,
            None => file
                .metadata()
                .await
                .map_err(|error| translate(error, path))?
                .size
                .ok_or_else(|| Error::provider(format!("{path} has no size")))?,
        };

        let (offset, length) = match range {
            None => (0, size),
            Some(range) => {
                let offset = range.offset.min(size);
                (offset, range.length.unwrap_or(u64::MAX).min(size - offset))
            }
        };

        // One handle per read in the air, since a handle can only be waiting on one request at
        // a time — and no more of them than there are blocks to read, so a small file does not
        // open eight files to fetch one packet.
        let blocks = length.div_ceil(READ_BLOCK as u64).min(READS_AHEAD as u64) as usize;
        let more = (1..blocks).map(|_| self.sftp.open(self.absolute(path)));

        let mut handles = vec![file];
        handles.extend(
            futures::future::try_join_all(more)
                .await
                .map_err(|error| translate(error, path))?,
        );

        Ok(ByteStream::new(read_ahead(handles, offset, length), Some(length)))
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

        in_whole_packets(&mut file, body).await?;

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

/// How much one read asks for.
///
/// A little under the client's maximum packet, which is both what OpenSSH allows in a single
/// read — `limits@openssh.com` says 255 KiB — and what is left of a packet on a server that
/// does not offer the extension. Asking for more only splits the request in two.
const READ_BLOCK: usize = 255 * 1024;

/// How many reads are allowed to be in the air together.
///
/// `russh_sftp` asks for one block and waits for the answer, so without this a download runs at
/// one packet per round trip however fast the link is: a quarter of a megabyte every fiftieth
/// of a second across an ocean, which is twelve megabytes a second on a link that could carry
/// ten times that.
///
/// Eight blocks is two megabytes in the air for one transfer, and four transfers at once is
/// what the channel's window is opened to in [`SftpProvider::connect`] — so the server is never
/// holding back an answer waiting to be told there is room for it.
const READS_AHEAD: usize = 8;

/// Every block of the file, several reads in the air at once and handed back in order.
///
/// A handle can only be waiting on one request, so reading ahead means reading through several
/// of them — and their answers come back in whatever order the server gets to them. Putting
/// them back in order is not a nicety: out of order they are a corrupted file.
fn read_ahead<F>(
    handles: Vec<F>,
    offset: u64,
    length: u64,
) -> impl futures::Stream<Item = Result<bytes::Bytes>> + Send
where
    F: tokio::io::AsyncRead + tokio::io::AsyncSeek + Unpin + Send + 'static,
{
    struct Reading<F> {
        idle: Vec<F>,
        reading: futures::stream::FuturesOrdered<
            futures::future::BoxFuture<'static, (F, Result<bytes::Bytes>)>,
        >,
        offset: u64,
        remaining: u64,
    }

    let reading = Reading {
        idle: handles,
        reading: futures::stream::FuturesOrdered::new(),
        offset,
        remaining: length,
    };

    futures::stream::unfold(reading, |mut state| async move {
        while state.remaining > 0 && !state.idle.is_empty() {
            let handle = state.idle.pop().expect("a handle, since one was there to take");
            let taking = state.remaining.min(READ_BLOCK as u64);
            let at = state.offset;

            state.offset += taking;
            state.remaining -= taking;
            state.reading.push_back(Box::pin(read_block(handle, at, taking as usize)));
        }

        let (handle, block) = futures::StreamExt::next(&mut state.reading).await?;
        state.idle.push(handle);

        Some((block, state))
    })
}

/// One block, and the handle back to be used for the next one.
async fn read_block<F>(mut handle: F, offset: u64, length: usize) -> (F, Result<bytes::Bytes>)
where
    F: tokio::io::AsyncRead + tokio::io::AsyncSeek + Unpin + Send + 'static,
{
    let block = at_offset(&mut handle, offset, length).await;

    (handle, block)
}

async fn at_offset<F>(handle: &mut F, offset: u64, length: usize) -> Result<bytes::Bytes>
where
    F: tokio::io::AsyncRead + tokio::io::AsyncSeek + Unpin,
{
    handle
        .seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|error| Error::caused_by("could not seek", error))?;

    // Exactly what was asked for. The block ends where the size says it does, so a short read
    // means the file is not as long as it was listed as — and since that size may have come
    // from a listing taken before the transfer was planned, it is worth saying which of the two
    // went wrong rather than blaming the connection.
    let mut block = vec![0; length];

    handle
        .read_exact(&mut block)
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::UnexpectedEof => Error::provider(
                "the file is shorter than it was listed as, so it changed while it was being read",
            ),
            _ => Error::caused_by("the download was interrupted", error),
        })?;

    Ok(bytes::Bytes::from(block))
}

/// The most a single request can carry, which is `russh_sftp`'s own default packet size.
///
/// Written down rather than asked for, because the client will not say what it allows: a write
/// is capped either at what the server offered under `limits@openssh.com` or at what is left of
/// a packet once its header is subtracted, and neither number is reachable from out here. A
/// buffer this size is always larger than both, which is what makes the first write short
/// enough to answer the question.
const MAX_PACKET: usize = 256 * 1024;

/// Hands `body` over a whole packet at a time.
///
/// Neither what `tokio::io::copy` would do — its buffer is eight kilobytes — nor what the
/// stream hands over. The client sends one packet per write and gives the rest back, and a
/// packet is a little smaller than the quarter of a megabyte the engine reads at a time, so
/// writing each chunk as it arrived would send a full request followed by a scrap of one. Each
/// of those holds one of the eight requests the client keeps in the air: half the pipeline
/// spent on a kilobyte of payload.
///
/// How big a packet is, only the session knows, so the first write is asked rather than told.
/// A buffer of [`MAX_PACKET`] is larger than any packet the client will send, so the write it
/// answers with is short, and how short is the size of every write after it.
async fn in_whole_packets<W>(file: &mut W, body: ByteStream) -> Result<()>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut body = body;
    let mut pending = BytesMut::new();
    let mut packet = None;

    while let Some(chunk) = futures::StreamExt::next(&mut body).await {
        pending.extend_from_slice(&chunk?);

        if packet.is_none() && pending.len() >= MAX_PACKET {
            let sent = file.write(&pending).await.map_err(interrupted)?;

            // Said rather than looped on. A server whose packet is smaller than a write header
            // leaves no room for any payload, and taking that as the packet size would split
            // the file into pieces of nothing for ever rather than failing.
            if sent == 0 {
                return Err(Error::provider("the server will not take a write this small"));
            }

            pending.advance(sent);
            packet = Some(sent);
        }

        if let Some(size) = packet {
            while pending.len() >= size {
                let whole = pending.split_to(size);

                file.write_all(&whole).await.map_err(interrupted)?;
            }
        }
    }

    // Whatever is left over is a packet's worth or less, and it is the end of the file.
    if !pending.is_empty() {
        file.write_all(&pending).await.map_err(interrupted)?;
    }

    Ok(())
}

fn interrupted(error: std::io::Error) -> Error {
    Error::caused_by("the upload was interrupted", error)
}

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

    /// Takes at most a packet's worth per write and remembers how much each one took, which is
    /// exactly what the SFTP client does with a buffer larger than one packet.
    struct Capped {
        packet: usize,
        written: Vec<u8>,
        writes: Vec<usize>,
    }

    impl tokio::io::AsyncWrite for Capped {
        fn poll_write(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            buffer: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            let taken = buffer.len().min(self.packet);

            self.written.extend_from_slice(&buffer[..taken]);
            self.writes.push(taken);

            std::task::Poll::Ready(Ok(taken))
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    fn chunks(count: usize, size: usize) -> ByteStream {
        let chunks = (0..count)
            .map(|index| Ok(bytes::Bytes::from(vec![index as u8; size])))
            .collect::<Vec<_>>();

        ByteStream::new(futures::stream::iter(chunks), Some((count * size) as u64))
    }

    /// What OpenSSH allows: a packet a little short of the quarter megabyte the engine reads at
    /// a time, so a chunk written as it arrives goes out as a full request and a scrap of one.
    const OPENSSH: usize = 255 * 1024;

    #[tokio::test]
    async fn an_upload_goes_out_in_whole_packets_and_a_remainder() {
        let mut file = Capped { packet: OPENSSH, written: Vec::new(), writes: Vec::new() };

        in_whole_packets(&mut file, chunks(4, 256 * 1024)).await.unwrap();

        assert_eq!(file.writes, vec![OPENSSH, OPENSSH, OPENSSH, OPENSSH, 4096]);
    }

    /// Splitting the stream up again is only worth anything if what lands is the same file, in
    /// the same order.
    #[tokio::test]
    async fn every_byte_arrives_once_and_in_order() {
        let mut file = Capped { packet: OPENSSH, written: Vec::new(), writes: Vec::new() };

        in_whole_packets(&mut file, chunks(4, 256 * 1024)).await.unwrap();

        let expected = (0..4u8).flat_map(|index| vec![index; 256 * 1024]).collect::<Vec<_>>();
        assert_eq!(file.written, expected);
    }

    /// Nothing is held back waiting for a packet that will never be filled.
    #[tokio::test]
    async fn a_file_smaller_than_a_packet_is_one_write() {
        let mut file = Capped { packet: OPENSSH, written: Vec::new(), writes: Vec::new() };

        in_whole_packets(&mut file, chunks(4, 10)).await.unwrap();

        assert_eq!(file.writes, vec![40]);
        assert_eq!(file.written.len(), 40);
    }

    /// A pattern with a period that shares no factor with a block, so a block put back in the
    /// wrong place cannot happen to match.
    fn contents(length: usize) -> Vec<u8> {
        (0..length).map(|index| (index % 251) as u8).collect()
    }

    /// A file that answers reads slowly, and the later handles more slowly than the earlier
    /// ones — so the block asked for last is the one that comes back first.
    struct Slow {
        contents: std::io::Cursor<Vec<u8>>,
        delay: std::time::Duration,
        waiting: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
    }

    impl tokio::io::AsyncRead for Slow {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            context: &mut std::task::Context<'_>,
            buffer: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            let file = self.get_mut();
            let delay = file.delay;

            let waiting = file
                .waiting
                .get_or_insert_with(|| Box::pin(tokio::time::sleep(delay)));

            std::task::ready!(std::future::Future::poll(waiting.as_mut(), context));
            file.waiting = None;

            std::pin::Pin::new(&mut file.contents).poll_read(context, buffer)
        }
    }

    impl tokio::io::AsyncSeek for Slow {
        fn start_seek(
            self: std::pin::Pin<&mut Self>,
            position: std::io::SeekFrom,
        ) -> std::io::Result<()> {
            std::pin::Pin::new(&mut self.get_mut().contents).start_seek(position)
        }

        fn poll_complete(
            self: std::pin::Pin<&mut Self>,
            context: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<u64>> {
            std::pin::Pin::new(&mut self.get_mut().contents).poll_complete(context)
        }
    }

    fn slow_handles(bytes: &[u8]) -> Vec<Slow> {
        (0..READS_AHEAD)
            .map(|index| Slow {
                contents: std::io::Cursor::new(bytes.to_vec()),
                delay: std::time::Duration::from_millis(5 * index as u64),
                waiting: None,
            })
            .collect()
    }

    /// Reads go out through several handles at once and come back in whatever order the server
    /// gets to them. Put back in the wrong order they are not a slow download, they are a
    /// corrupted file.
    #[tokio::test]
    async fn every_block_comes_back_in_order_however_fast_it_answered() {
        let bytes = contents(16 * READ_BLOCK);

        let read = read_ahead(slow_handles(&bytes), 0, bytes.len() as u64);
        let whole = ByteStream::new(read, None).collect().await.unwrap();

        assert_eq!(whole.as_ref(), &bytes[..]);
    }

    /// The last block is whatever is left over, and a file that does not divide evenly is the
    /// usual case rather than the exception.
    #[tokio::test]
    async fn a_file_that_does_not_divide_into_whole_blocks_still_arrives_whole() {
        let bytes = contents(3 * READ_BLOCK + 17);

        let read = read_ahead(slow_handles(&bytes), 0, bytes.len() as u64);
        let whole = ByteStream::new(read, None).collect().await.unwrap();

        assert_eq!(whole.as_ref(), &bytes[..]);
    }

    /// The length may have come from a listing taken before the transfer was planned, so it can
    /// be describing a file that has since shrunk. Handing back what there is would be a
    /// truncated file reported as a finished download.
    #[tokio::test]
    async fn a_file_shorter_than_it_was_said_to_be_fails_the_download() {
        let bytes = contents(READ_BLOCK + 100);

        let read = read_ahead(slow_handles(&bytes), 0, bytes.len() as u64 + 1);
        let refused = ByteStream::new(read, None).collect().await.unwrap_err();

        assert_eq!(
            refused.to_string(),
            "the file is shorter than it was listed as, so it changed while it was being read"
        );
    }

    /// Resuming an interrupted download asks for a slice, and reading ahead must not read past
    /// the end of it.
    #[tokio::test]
    async fn a_range_starts_where_it_says_and_stops_where_it_says() {
        let bytes = contents(4 * READ_BLOCK);
        let (offset, length) = (1000, 2 * READ_BLOCK + 3);

        let read = read_ahead(slow_handles(&bytes), offset as u64, length as u64);
        let slice = ByteStream::new(read, None).collect().await.unwrap();

        assert_eq!(slice.as_ref(), &bytes[offset..offset + length]);
    }

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
        // Read rather than set. `HOME` belongs to the whole process, and setting it here raced
        // `ssh_config`, which reads it too: that test expanded one path before this ran and
        // compared it against one expanded after, and failed on the difference.
        let home = std::env::var("HOME").unwrap();

        assert_eq!(expand_home("~/.ssh/id_ed25519"), format!("{home}/.ssh/id_ed25519"));
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
