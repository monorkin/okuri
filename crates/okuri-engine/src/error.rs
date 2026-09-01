pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Anything a provider reported. The domain's own errors pass straight through, so the UI
    /// can still tell a missing file from a refused one.
    #[error(transparent)]
    Provider(#[from] okuri_core::Error),

    #[error("that passphrase does not open this file")]
    WrongPassphrase,

    #[error("{0}")]
    Secrets(String),

    #[error("{0}")]
    Config(String),

    #[error("there is no connection called {0}")]
    UnknownConnection(String),

    #[error("that connection is not open")]
    NoSuchSession,

    #[error("{host} presented a different key than the one on file")]
    HostKeyChanged { host: String },

    #[error("{path} could not be read: {reason}")]
    LocalFile { path: String, reason: String },

    #[error("cancelled")]
    Cancelled,
}

impl Error {
    pub fn secrets(message: impl std::fmt::Display) -> Self {
        Self::Secrets(message.to_string())
    }

    pub fn config(message: impl std::fmt::Display) -> Self {
        Self::Config(message.to_string())
    }

    pub fn local_file(path: &std::path::Path, reason: impl std::fmt::Display) -> Self {
        Self::LocalFile { path: path.display().to_string(), reason: reason.to_string() }
    }
}
