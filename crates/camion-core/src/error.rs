use std::fmt;

use crate::capabilities::Operation;
use crate::path::RemotePath;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path} does not exist")]
    NotFound { path: RemotePath },

    #[error("{path} already exists")]
    AlreadyExists { path: RemotePath },

    #[error("{path} is a folder")]
    IsAFolder { path: RemotePath },

    #[error("{path} is not a folder")]
    NotAFolder { path: RemotePath },

    #[error("{path} is not empty")]
    NotEmpty { path: RemotePath },

    #[error("not permitted: {path}")]
    PermissionDenied { path: RemotePath },

    #[error("{0} is not supported by this connection")]
    Unsupported(Operation),

    #[error("{input:?} is not a valid path: it {reason}")]
    InvalidPath { input: String, reason: &'static str },

    #[error("could not authenticate: {0}")]
    Authentication(String),

    /// A key file is encrypted and nothing was given to open it. Not a failure on its own —
    /// whoever asked for the connection can ask for the passphrase and try again.
    #[error("{path} needs a passphrase")]
    NeedsPassphrase { path: String },

    #[error("cancelled")]
    Cancelled,

    /// Anything a provider's own libraries raised. The message is what the user sees, so
    /// adapters are expected to say something a person can act on.
    #[error("{message}")]
    Provider {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
}

impl Error {
    pub fn provider(message: impl fmt::Display) -> Self {
        Self::Provider { message: message.to_string(), source: None }
    }

    pub fn caused_by(
        message: impl fmt::Display,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Provider { message: message.to_string(), source: Some(Box::new(source)) }
    }

    /// Whether retrying the same operation could plausibly succeed. The transfer queue uses
    /// this to decide between retrying an item and surfacing it as failed.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Provider { .. })
    }
}
