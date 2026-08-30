use camion_core::{Capabilities, Entry, RemotePath};
use tokio::sync::oneshot;

use crate::session::SessionId;
use crate::transfer::{Transfer, TransferId};

/// Everything the interface learns, on one channel.
///
/// Having a single stream is what keeps the Qt side thin: it translates events into model
/// updates and does nothing else, and the whole application can be watched at one seam.
#[derive(Debug)]
pub enum Event {
    Connecting { connection: String },
    Connected {
        session: SessionId,
        label: String,
        capabilities: Capabilities,
        /// Where this connection's root sits on the server, for naming files to anything
        /// outside Camion.
        home: String,
    },
    ConnectionFailed { connection: String, reason: String },
    Disconnected { session: SessionId },

    /// A folder's contents, ready to be shown. Carries the path so a listing that arrives after
    /// the user has already moved on can be recognised as stale and dropped.
    Listing { session: SessionId, path: RemotePath, entries: Vec<Entry> },
    Working { session: SessionId, working: bool },

    TransferAdded(Transfer),
    TransferProgress { transfer: TransferId, transferred: u64 },
    TransferFinished { transfer: TransferId, outcome: Outcome },

    /// Something went wrong that the user should see but that stops nothing else.
    Failed { message: String },

    /// A question that has to be answered before the work in flight can continue.
    Ask(Prompt),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Failed(String),
    Cancelled,
}

/// A question the engine is waiting on an answer to.
///
/// Prompts are requests rather than notifications: an unknown host key or a missing passphrase
/// arrives in the middle of connecting, and the connection resumes only once the person at the
/// keyboard has replied. Dropping a prompt without answering declines it, so a closed dialog
/// can never leave a task waiting forever.
#[derive(Debug)]
pub struct Prompt {
    pub question: Question,
    reply: std::sync::Mutex<Option<oneshot::Sender<Answer>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Question {
    /// A server we have never seen. Shows the fingerprint, the way `ssh` does.
    UnknownHostKey { host: String, algorithm: String, fingerprint: String },

    /// A server whose key is not the one on file. Either it was rebuilt, or someone is
    /// listening. Worded so that accepting is clearly the unusual choice.
    ChangedHostKey { host: String, algorithm: String, fingerprint: String },

    /// A password for a connection, asked for when the store has none.
    Password { connection: String },

    /// An access key and its secret, which is what the S3-shaped destinations want. Asked for
    /// together because half of a key pair is no use.
    KeyPair { connection: String },

    /// The passphrase that opens the encrypted secrets file.
    Passphrase,

    /// The passphrase that opens an SSH key file. Asked for only once the key turns out to
    /// need one, so an unencrypted key never prompts.
    KeyPassphrase { path: String },

    /// A file of that name is already there.
    Overwrite { name: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Accept,
    Decline,
    Text(String),
    Pair { id: String, secret: String },
}

impl Answer {
    pub fn is_accepted(&self) -> bool {
        !matches!(self, Self::Decline)
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }

    pub fn pair(&self) -> Option<(&str, &str)> {
        match self {
            Self::Pair { id, secret } => Some((id, secret)),
            _ => None,
        }
    }
}

impl Prompt {
    pub fn new(question: Question) -> (Self, oneshot::Receiver<Answer>) {
        let (reply, answer) = oneshot::channel();

        (Self { question, reply: std::sync::Mutex::new(Some(reply)) }, answer)
    }

    /// Answers the question, releasing whatever was waiting on it. Answering twice is harmless:
    /// the second answer is ignored rather than being an error the dialog has to avoid.
    pub fn answer(&self, answer: Answer) {
        if let Some(reply) = self.reply.lock().unwrap().take() {
            let _ = reply.send(answer);
        }
    }
}

impl Drop for Prompt {
    fn drop(&mut self) {
        if let Some(reply) = self.reply.get_mut().unwrap().take() {
            let _ = reply.send(Answer::Decline);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn answering_releases_whatever_was_waiting() {
        let (prompt, waiting) = Prompt::new(Question::Passphrase);

        prompt.answer(Answer::Text("correct horse".to_owned()));

        assert_eq!(waiting.await.unwrap(), Answer::Text("correct horse".to_owned()));
    }

    #[tokio::test]
    async fn a_prompt_nobody_answers_counts_as_declining() {
        let (prompt, waiting) = Prompt::new(Question::UnknownHostKey {
            host: "example.com".to_owned(),
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "SHA256:whatever".to_owned(),
        });

        drop(prompt);

        assert_eq!(waiting.await.unwrap(), Answer::Decline);
    }
}
