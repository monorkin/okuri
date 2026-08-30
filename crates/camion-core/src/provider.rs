use async_trait::async_trait;

use crate::capabilities::Capabilities;
use crate::entry::Entry;
use crate::error::Result;
use crate::path::RemotePath;
use crate::stream::{ByteRange, ByteStream};

/// Everything Camion needs from a destination.
///
/// The verb set is deliberately small: anything a provider cannot do natively it either
/// emulates or declares unsupported in [`Capabilities`], and the UI reads that rather than
/// discovering the answer from a failed call.
#[async_trait]
pub trait Provider: Send + Sync {
    /// A short label for the connection, shown in the window title and the switcher.
    fn label(&self) -> String;

    fn capabilities(&self) -> Capabilities;

    async fn list(&self, path: &RemotePath) -> Result<Vec<Entry>>;

    async fn stat(&self, path: &RemotePath) -> Result<Entry>;

    async fn read(&self, path: &RemotePath, range: Option<ByteRange>) -> Result<ByteStream>;

    async fn write(&self, path: &RemotePath, body: ByteStream) -> Result<()>;

    async fn delete(&self, path: &RemotePath) -> Result<()>;

    async fn create_folder(&self, path: &RemotePath) -> Result<()>;

    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<()>;

    /// The absolute path on the server that this connection's root stands for.
    ///
    /// A connection usually starts somewhere other than `/` — an account's home directory,
    /// often, worked out at connection time rather than written down. Anything that has to name
    /// a file to something outside Camion needs the whole path, not the part below the root.
    /// Destinations with no such notion say nothing.
    fn home(&self) -> String {
        String::new()
    }

    /// Closes the underlying connection. Providers that are stateless leave this alone.
    async fn disconnect(&self) -> Result<()> {
        Ok(())
    }
}

/// Conveniences every provider gets for free. Kept out of [`Provider`] so implementing a new
/// destination stays a matter of the verbs above and nothing else.
#[async_trait]
pub trait ProviderExt: Provider {
    async fn exists(&self, path: &RemotePath) -> Result<bool> {
        match self.stat(path).await {
            Ok(_) => Ok(true),
            Err(crate::Error::NotFound { .. }) => Ok(false),
            Err(error) => Err(error),
        }
    }

    async fn read_all(&self, path: &RemotePath) -> Result<bytes::Bytes> {
        self.read(path, None).await?.collect().await
    }

    async fn write_all(&self, path: &RemotePath, bytes: bytes::Bytes) -> Result<()> {
        self.write(path, ByteStream::once(bytes)).await
    }

    /// Creates every missing folder along `path`, the way `mkdir -p` does.
    async fn create_folders(&self, path: &RemotePath) -> Result<()> {
        for ancestor in path.ancestors() {
            if !ancestor.is_root() && !self.exists(&ancestor).await? {
                self.create_folder(&ancestor).await?;
            }
        }

        Ok(())
    }

    /// Removes a folder and everything under it, depth first.
    async fn delete_recursively(&self, path: &RemotePath) -> Result<()> {
        let entry = self.stat(path).await?;

        if entry.kind.is_folder() {
            for child in self.list(path).await? {
                self.delete_recursively(&path.join(&child.name)?).await?;
            }
        }

        self.delete(path).await
    }

    /// A name that does not collide, by adding ` (2)`, ` (3)`, and so on before the extension.
    /// This is what a drop of an already-present file becomes when the user chooses to keep both.
    async fn unused_name(&self, folder: &RemotePath, name: &str) -> Result<String> {
        let candidate = folder.join(name)?;

        if !self.exists(&candidate).await? {
            return Ok(name.to_owned());
        }

        let stem = candidate.stem().unwrap_or(name).to_owned();
        let extension = candidate.extension().map(str::to_owned);

        let mut suffix = 2;

        loop {
            let name = match &extension {
                Some(extension) => format!("{stem} ({suffix}).{extension}"),
                None => format!("{stem} ({suffix})"),
            };

            if !self.exists(&folder.join(&name)?).await? {
                return Ok(name);
            }

            suffix += 1;
        }
    }
}

impl<T: Provider + ?Sized> ProviderExt for T {}
