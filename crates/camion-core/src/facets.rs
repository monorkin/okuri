//! The things only some destinations can do.
//!
//! Each is a small trait a provider answers or does not, alongside [`Sharing`](crate::Sharing)
//! and [`Permitting`](crate::Permitting). Kept apart from [`Provider`](crate::Provider) for the
//! same reason as those: an SFTP server has no storage class and an object store has no group,
//! and a trait every adapter has to decline is worse than one it simply does not implement.
//!
//! Typed rather than a bag of labelled strings. A label is display wording and belongs to the
//! interface, and a value that might one day be changed has to be something other than text.

use async_trait::async_trait;

use crate::error::Result;
use crate::path::RemotePath;

/// Who a file belongs to.
///
/// The other two thirds of the Unix triple whose first third is
/// [`Permissions`](crate::Permissions): a file being writable by its group says little until you
/// know which group.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ownership {
    pub user: Option<String>,
    pub group: Option<String>,
}

#[async_trait]
pub trait Owning: Send + Sync {
    async fn ownership(&self, path: &RemotePath) -> Result<Ownership>;
}

/// Where a link points.
#[async_trait]
pub trait Linking: Send + Sync {
    /// The path this file points at, or `None` if it is not a link.
    async fn link_target(&self, path: &RemotePath) -> Result<Option<String>>;
}

/// How a file appears to anything fetching it over HTTP.
///
/// These are the headers a browser acts on: whether it shows the file or downloads it, how long
/// it may keep it, and how it was encoded on the way. Getting the type wrong is the most common
/// invisible mistake in a bucket.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Served {
    pub content_type: Option<String>,
    /// The store's own name for this version of the file, unquoted. Not called a checksum: for
    /// a file uploaded in one request it is the MD5, and for a multipart one it is a digest of
    /// the parts ending in `-<count>`, which no local tool will reproduce.
    pub etag: Option<String>,
    pub cache_control: Option<String>,
    pub content_encoding: Option<String>,
}

#[async_trait]
pub trait Serving: Send + Sync {
    async fn served(&self, path: &RemotePath) -> Result<Served>;
}

/// How the store itself keeps a file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stored {
    /// What it costs and how quickly it can be read — `STANDARD`, `GLACIER`, an Azure tier.
    pub class: Option<String>,
    pub encryption: Option<String>,
    /// Which version this is, where the store keeps more than one.
    pub version: Option<String>,
}

#[async_trait]
pub trait Storing: Send + Sync {
    async fn stored(&self, path: &RemotePath) -> Result<Stored>;
}
