use async_trait::async_trait;

use crate::entry::Permissions;
use crate::error::Result;
use crate::path::RemotePath;

/// Changing who may read, write, and run a file.
///
/// Kept off [`Provider`](crate::Provider) for the same reason as
/// [`Sharing`](crate::Sharing): a mode is a Unix idea, and the object stores have nothing to
/// say about it. A destination that keeps modes answers
/// [`Provider::permitting`](crate::Provider::permitting), and the interface asks before it
/// offers anything.
#[async_trait]
pub trait Permitting: Send + Sync {
    async fn set_permissions(&self, path: &RemotePath, permissions: Permissions) -> Result<()>;
}
