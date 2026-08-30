use async_trait::async_trait;

/// A server's host key, as the prompt shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKey {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    /// The `SHA256:…` form, which is what `ssh` prints and therefore what people compare against.
    pub fingerprint: String,
    /// The key itself, as `ssh-ed25519 AAAAC3…` — the two fields `known_hosts` stores.
    pub public_key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trust {
    /// Already in `known_hosts` and unchanged.
    Known,
    /// The user accepted it just now; remember it.
    Accepted,
    Rejected,
}

impl Trust {
    pub fn is_trusted(&self) -> bool {
        !matches!(self, Self::Rejected)
    }
}

/// Decides whether an SSH host key is the one we expected.
///
/// The adapter asks; something above it answers, by consulting `known_hosts` and, when that has
/// nothing to say, by putting the fingerprint in front of the person at the keyboard.
#[async_trait]
pub trait HostTrust: Send + Sync {
    async fn verify(&self, key: &HostKey) -> Trust;
}

/// Accepts anything. Only for tests against a server we started ourselves.
pub struct TrustEverything;

#[async_trait]
impl HostTrust for TrustEverything {
    async fn verify(&self, _key: &HostKey) -> Trust {
        Trust::Known
    }
}
