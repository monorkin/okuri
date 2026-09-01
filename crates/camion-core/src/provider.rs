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

    /// Who files here belong to, if anyone.
    fn owning(&self) -> Option<&dyn crate::Owning> {
        None
    }

    /// Where a link points, for destinations that have links.
    fn linking(&self) -> Option<&dyn crate::Linking> {
        None
    }

    /// How files here are served over HTTP, for destinations that are.
    fn serving(&self) -> Option<&dyn crate::Serving> {
        None
    }

    /// How the store keeps a file — its class, its encryption, its version.
    fn storing(&self) -> Option<&dyn crate::Storing> {
        None
    }

    /// How this destination hands files to people with no account, if it can at all.
    ///
    /// Most cannot, and say so by leaving this alone. Kept off the verb list above because a
    /// destination that has no notion of public files should not have to decline a method —
    /// and because the interface has to know whether to offer any of it before it asks.
    fn sharing(&self) -> Option<&dyn crate::Sharing> {
        None
    }

    /// How this destination changes a file's mode, if it keeps one at all.
    fn permitting(&self) -> Option<&dyn crate::Permitting> {
        None
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

}

impl<T: Provider + ?Sized> ProviderExt for T {}
