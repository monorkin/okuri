use std::str::FromStr;

use async_trait::async_trait;
use camion_core::{
    ByteRange, ByteStream, Capabilities, Entry, Error, Provider, RemotePath, Result,
};
use futures::StreamExt;
use suppaftp::list::File as Listed;
use suppaftp::tokio::{AsyncRustlsConnector, AsyncRustlsFtpStream};
use suppaftp::types::FileType;
use suppaftp::{FtpError, Mode};
use time::OffsetDateTime;
use tokio::sync::Mutex;

use crate::destination::Ftp as FtpConfig;
use crate::secret::Secret;

/// The oldest protocol Camion speaks, and the one with the fewest promises.
///
/// One control connection, one command at a time — so the stream is held behind a lock and
/// operations queue rather than racing. That is not a limitation of the design, it is what FTP
/// is: interleaving commands on one control channel is how you corrupt a session.
pub struct FtpProvider {
    label: String,
    home: String,
    stream: Mutex<AsyncRustlsFtpStream>,
}

impl FtpProvider {
    fn at(&self, path: &RemotePath) -> String {
        crate::keys::under(&self.home, path)
    }

    pub async fn connect(config: &FtpConfig, secret: &Secret) -> Result<Self> {
        let address = format!("{}:{}", config.host, config.port);
        let mut stream = AsyncRustlsFtpStream::connect(&address)
            .await
            .map_err(|error| Error::caused_by(format!("could not reach {address}"), error))?;

        if config.encrypted {
            stream = stream
                .into_secure(encryption()?, &config.host)
                .await
                .map_err(|error| {
                    Error::caused_by(
                        format!("{} would not start an encrypted session", config.host),
                        error,
                    )
                })?;
        }

        stream.set_mode(match config.passive {
            true => Mode::Passive,
            false => Mode::Active,
        });

        stream
            .login(config.username.as_str(), secret.password().unwrap_or_default())
            .await
            .map_err(|error| Error::Authentication(error.to_string()))?;

        // Everything Camion moves is bytes, and a server that helpfully rewrites line endings
        // in the middle of an upload corrupts it.
        stream
            .transfer_type(FileType::Binary)
            .await
            .map_err(|error| Error::caused_by("the server refused binary transfers", error))?;

        // Logging in rarely puts you at the root of the filesystem, and on a server that is
        // not chrooted an absolute path would leave the account's own directory entirely. So
        // wherever the login lands becomes this connection's root.
        let home = if config.home.is_empty() {
            stream
                .pwd()
                .await
                .map_err(|error| Error::caused_by("could not find the login directory", error))?
        } else {
            config.home.clone()
        };

        Ok(Self {
            label: format!("{}@{}", config.username, config.host),
            home: home.trim_end_matches('/').to_owned(),
            stream: Mutex::new(stream),
        })
    }
}

#[async_trait]
impl Provider for FtpProvider {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn home(&self) -> String {
        self.home.clone()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            // One command channel, one transfer at a time. Handing this connection four
            // uploads at once only queues four whole files behind the same lock — and since a
            // download is read into memory before it is handed on, four of them in RAM.
            transfer_slots: 1,
            ..Capabilities::filesystem()
        }
    }

    async fn list(&self, path: &RemotePath) -> Result<Vec<Entry>> {
        let lines = self
            .stream
            .lock()
            .await
            .list(Some(&self.at(path)))
            .await
            .map_err(|error| translate(error, path))?;

        Ok(lines
            .iter()
            // Servers differ in what they emit, and a line we cannot read is one entry we
            // cannot show — not a reason to fail the whole listing.
            .filter_map(|line| Listed::from_str(line).ok())
            .filter(|listed| listed.name() != "." && listed.name() != "..")
            .map(|listed| describe(&listed))
            .collect())
    }

    async fn stat(&self, path: &RemotePath) -> Result<Entry> {
        if path.is_root() {
            return Ok(Entry::folder("/"));
        }

        let name = path.name().unwrap_or_default().to_owned();
        let parent = path.parent().unwrap_or_else(RemotePath::root);

        self.list(&parent)
            .await?
            .into_iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| Error::NotFound { path: path.clone() })
    }

    async fn read(&self, path: &RemotePath, range: Option<ByteRange>) -> Result<ByteStream> {
        let mut stream = self.stream.lock().await;

        let reader = stream
            .retr_as_stream(self.at(path))
            .await
            .map_err(|error| translate(error, path))?;

        // The data connection has to be read to its end before the control connection is
        // usable again, so the whole file is taken here rather than handed out as a stream
        // that could be dropped half way through and wedge the session.
        let mut bytes = Vec::new();
        let mut reader = reader;
        tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut bytes)
            .await
            .map_err(|error| Error::caused_by("the download was interrupted", error))?;

        stream
            .finalize_retr_stream(reader)
            .await
            .map_err(|error| translate(error, path))?;

        let bytes = match range {
            None => bytes,
            Some(range) => {
                let start = usize::try_from(range.offset).unwrap_or(usize::MAX).min(bytes.len());
                let end = match range.length {
                    Some(length) => start.saturating_add(
                        usize::try_from(length).unwrap_or(usize::MAX),
                    ).min(bytes.len()),
                    None => bytes.len(),
                };

                bytes[start..end].to_vec()
            }
        };

        Ok(ByteStream::once(bytes))
    }

    async fn write(&self, path: &RemotePath, mut body: ByteStream) -> Result<()> {
        let mut stream = self.stream.lock().await;

        let writer = stream
            .put_with_stream(self.at(path))
            .await
            .map_err(|error| translate(error, path))?;

        let mut writer = writer;

        while let Some(chunk) = body.next().await {
            tokio::io::AsyncWriteExt::write_all(&mut writer, &chunk?)
                .await
                .map_err(|error| Error::caused_by("the upload was interrupted", error))?;
        }

        stream
            .finalize_put_stream(writer)
            .await
            .map_err(|error| translate(error, path))
    }

    async fn delete(&self, path: &RemotePath) -> Result<()> {
        let entry = self.stat(path).await?;
        let mut stream = self.stream.lock().await;

        if entry.kind.is_folder() {
            stream.rmdir(self.at(path)).await
        } else {
            stream.rm(self.at(path)).await
        }
        .map_err(|error| translate(error, path))
    }

    async fn create_folder(&self, path: &RemotePath) -> Result<()> {
        self.stream
            .lock()
            .await
            .mkdir(self.at(path))
            .await
            .map_err(|error| translate(error, path))
    }

    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<()> {
        self.stream
            .lock()
            .await
            .rename(self.at(from), self.at(to))
            .await
            .map_err(|error| translate(error, from))
    }

    async fn disconnect(&self) -> Result<()> {
        let _ = self.stream.lock().await.quit().await;

        Ok(())
    }
}

/// Explicit FTPS, verified against whatever the machine already trusts — the same certificates
/// the browser on this desktop uses, rather than a list Camion carries around itself.
fn encryption() -> Result<AsyncRustlsConnector> {
    use rustls_platform_verifier::ConfigVerifierExt as _;

    let config = tokio_rustls::rustls::ClientConfig::with_platform_verifier()
        .map_err(|error| Error::caused_by("could not set up encryption", error))?;

    Ok(tokio_rustls::TlsConnector::from(std::sync::Arc::new(config)).into())
}



fn describe(listed: &Listed) -> Entry {
    let mut entry = match listed.is_directory() {
        true => Entry::folder(listed.name()),
        false => Entry::file(listed.name(), listed.size() as u64),
    };

    entry.modified = listed
        .modified()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|since| OffsetDateTime::from_unix_timestamp(since.as_secs() as i64).ok());

    entry
}

/// FTP says everything in numbers. The ones worth telling apart are "it isn't there" and "you
/// may not"; the rest are passed through with whatever the server said.
fn translate(error: FtpError, path: &RemotePath) -> Error {
    let FtpError::UnexpectedResponse(response) = &error else {
        return Error::caused_by(format!("{path} could not be reached"), error);
    };

    match response.status as u32 {
        550 => Error::NotFound { path: path.clone() },
        532 | 553 => Error::PermissionDenied { path: path.clone() },
        _ => Error::provider(String::from_utf8_lossy(&response.body).trim().to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unix_listing_line_becomes_an_entry() {
        let file = Listed::from_str("-rw-r--r-- 1 camion camion 250000 Aug 26 10:00 harbour.jpg")
            .unwrap();
        let entry = describe(&file);

        assert_eq!(entry.name, "harbour.jpg");
        assert_eq!(entry.size, 250_000);
        assert!(entry.kind.is_file());

        let folder =
            Listed::from_str("drwxr-xr-x 2 camion camion 4096 Aug 26 10:00 reports").unwrap();

        assert!(describe(&folder).kind.is_folder());
    }

    #[test]
    fn a_unix_listing_line_with_a_space_in_the_name_keeps_it() {
        let file =
            Listed::from_str("-rw-r--r-- 1 camion camion 12 Aug 26 10:00 last summer.jpg").unwrap();

        assert_eq!(describe(&file).name, "last summer.jpg");
    }
}
