use std::time::Duration;

use async_trait::async_trait;

use crate::error::Result;
use crate::path::RemotePath;

/// Who can read a file besides the account it belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    /// Only someone holding the account's credentials.
    Private,

    /// Anybody who has the address.
    Public,
}

impl Visibility {
    pub fn is_public(&self) -> bool {
        matches!(self, Self::Public)
    }
}

/// Handing a file to somebody who has no account.
///
/// Deliberately not part of [`Provider`](crate::Provider). Most destinations have no notion of
/// this at all: an SFTP server hands out files to people with logins, and inventing an answer
/// for it would mean a trait method every adapter has to decline. A destination that can do
/// this says so by answering [`Provider::sharing`](crate::Provider::sharing), and the interface
/// asks the same question before offering any of it.
#[async_trait]
pub trait Sharing: Send + Sync {
    /// Who can read this file right now.
    async fn visibility(&self, path: &RemotePath) -> Result<Visibility>;

    async fn set_visibility(&self, path: &RemotePath, visibility: Visibility) -> Result<()>;

    /// The address this file has to somebody with no credentials.
    ///
    /// Answered whether or not the file is public, because the address is a property of where
    /// the file is rather than of who may read it — and a private file's address is exactly
    /// what you want to see before deciding to make it public. Whether it currently works is
    /// what [`visibility`](Sharing::visibility) is for.
    fn public_url(&self, path: &RemotePath) -> String;

    /// A link that works for a while, for anybody, without the file being public.
    ///
    /// The address is signed rather than permitted: it carries proof that somebody with an
    /// account asked for it, and stops working when it expires. This is what handing one file
    /// to one person looks like, and unlike [`set_visibility`](Sharing::set_visibility) it is
    /// answered by every store — most of them decide who may read a whole bucket and have
    /// nothing to say about a single file.
    async fn temporary_url(&self, path: &RemotePath, valid_for: Duration) -> Result<String>;
}
