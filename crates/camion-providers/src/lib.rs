//! Every destination Camion can talk to, behind one trait.
//!
//! Adding a service means writing an adapter here and, if it is S3-shaped, often only adding a
//! row to [`S3Preset`]. Nothing above this crate matches on the kind of destination: the UI
//! reads [`camion_core::Capabilities`] instead.

pub mod azure;
pub mod destination;
mod keys;
mod parts;
pub mod ftp;
pub mod s3;
pub mod secret;
pub mod sftp;
pub mod ssh_config;
pub mod trust;
pub mod webdav;

use std::sync::Arc;

use camion_core::{MemoryProvider, Provider, Result};

pub use destination::{
    Azure, Destination, Ftp, S3, S3Preset, SecretShape, Sftp, SshCredential, WebDav,
};
pub use secret::Secret;
pub use trust::{HostKey, HostTrust, Trust};

/// Opens a connection to `destination`.
///
/// The secret is passed in rather than looked up, so this crate never learns where credentials
/// live, and `trust` is asked about unrecognised SSH host keys rather than answering for itself.
pub async fn connect(
    destination: &Destination,
    secret: &Secret,
    trust: Arc<dyn HostTrust>,
) -> Result<Arc<dyn Provider>> {
    match destination {
        Destination::Memory => Ok(Arc::new(MemoryProvider::sample())),
        Destination::Sftp(config) => Ok(Arc::new(
            sftp::SftpProvider::connect(config, secret, trust).await?,
        )),
        Destination::Ftp(config) => Ok(Arc::new(ftp::FtpProvider::connect(config, secret).await?)),
        Destination::S3(config) => Ok(Arc::new(s3::S3Provider::connect(config, secret).await?)),
        Destination::WebDav(config) => Ok(Arc::new(
            webdav::WebDavProvider::connect(config, secret).await?,
        )),
        Destination::Azure(config) => Ok(Arc::new(
            azure::AzureProvider::connect(config, secret).await?,
        )),
    }
}
