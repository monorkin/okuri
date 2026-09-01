use std::sync::atomic::{AtomicU64, Ordering};

use okuri_core::{Capabilities, Entry, Ownership, RemotePath, Served, Stored};
use tokio::sync::oneshot;

use crate::session::SessionId;
use crate::transfer::{Transfer, TransferId};

/// One try at opening a connection, from the moment it is asked for until it becomes a session
/// or fails.
///
/// Minted by whoever asks and handed back on everything that try produces. Connecting is the
/// one stretch of work with no session to name it by, and it is also the stretch that asks the
/// most questions — a host key, a password, a passphrase. With more than one window open,
/// "somebody is being asked for a password" does not say which window should be asking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Attempt(pub u64);

impl Attempt {
    pub fn next() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);

        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// Whose work an event is about.
///
/// Most events name a session and need nothing else. This is for the two that cannot: something
/// that went wrong while a connection was still being opened, and a question asked in the
/// middle of opening it. Without it, one window's password prompt appears in every window, and
/// one window's failure is reported by all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Concern {
    /// Okuri itself rather than any one connection — a config file that will not parse, a
    /// `known_hosts` that cannot be read.
    Everyone,

    /// A connection being opened, which has no session yet.
    Attempt(Attempt),

    /// An open connection.
    Session(SessionId),
}

impl Concern {
    /// Whether work named by `session` is this concern's own.
    pub fn is_session(&self, session: SessionId) -> bool {
        matches!(self, Self::Session(theirs) if *theirs == session)
    }

    pub fn is_attempt(&self, attempt: Attempt) -> bool {
        matches!(self, Self::Attempt(theirs) if *theirs == attempt)
    }

    pub fn is_everyone(&self) -> bool {
        matches!(self, Self::Everyone)
    }
}

/// Everything the interface learns, on one channel.
///
/// Having a single stream is what keeps the Qt side thin: it translates events into model
/// updates and does nothing else, and the whole application can be watched at one seam.
#[derive(Debug)]
pub enum Event {
    Connecting { attempt: Attempt, connection: String },
    Connected {
        /// Which try this is the end of, so the window that asked is the one that gets the
        /// connection rather than whichever window happens to be listening.
        attempt: Attempt,
        session: SessionId,
        label: String,
        capabilities: Capabilities,
        /// Where this connection's root sits on the server, for naming files to anything
        /// outside Okuri.
        home: String,
    },
    ConnectionFailed { attempt: Attempt, connection: String, reason: String },
    Disconnected { session: SessionId },

    /// A folder's contents, ready to be shown. Carries the path so a listing that arrives after
    /// the user has already moved on can be recognised as stale and dropped.
    Listing { session: SessionId, path: RemotePath, entries: Vec<Entry> },
    Working { session: SessionId, working: bool },

    TransferAdded(Transfer),
    TransferProgress { transfer: TransferId, transferred: u64 },
    TransferFinished { transfer: TransferId, outcome: Outcome },

    /// What is known about who can read a file, and the address it has either way.
    ///
    /// `public` is `None` when the store would not say — reading a file's permissions is itself
    /// a permission, and plenty of accounts can read a file without being allowed to ask who
    /// else can. That is not a failure worth a banner; it is an answer of "cannot tell".
    Shared {
        session: SessionId,
        name: String,
        public: Option<bool>,
        /// Why not, when `public` is `None`.
        why_not: String,
        url: String,
    },

    /// Everything else the destination knows about one file.
    ///
    /// Each part is absent when the destination has no notion of it — an object store has no
    /// group, an SFTP server no storage class. Typed rather than labelled text, so what is on
    /// screen is the interface's wording and what is here can be acted on later.
    Described {
        session: SessionId,
        name: String,
        ownership: Option<Ownership>,
        link_target: Option<String>,
        served: Option<Served>,
        stored: Option<Stored>,
    },

    /// A signed link to a file, good for a while.
    Linked { session: SessionId, name: String, url: String },

    /// Something went wrong that the user should see but that stops nothing else.
    Failed { concern: Concern, message: String },

    /// Something went right that is worth confirming. Saving a credential leaves nothing on
    /// screen to show for it, and silence is indistinguishable from having done nothing.
    Notice { concern: Concern, message: String },

    /// A question that has to be answered before the work in flight can continue.
    Ask(Prompt),
}

impl Event {
    /// Whose news this is.
    ///
    /// The transfer events answer [`Concern::Everyone`] deliberately: the queue is one queue
    /// for the whole application, and a transfer started in one window is still moving when
    /// that window is looking at something else.
    pub fn concern(&self) -> Concern {
        match self {
            Self::Connecting { attempt, .. }
            | Self::Connected { attempt, .. }
            | Self::ConnectionFailed { attempt, .. } => Concern::Attempt(*attempt),

            Self::Disconnected { session }
            | Self::Listing { session, .. }
            | Self::Working { session, .. }
            | Self::Shared { session, .. }
            | Self::Described { session, .. }
            | Self::Linked { session, .. } => Concern::Session(*session),

            Self::Failed { concern, .. } | Self::Notice { concern, .. } => *concern,
            Self::Ask(prompt) => prompt.concern,

            Self::TransferAdded(_)
            | Self::TransferProgress { .. }
            | Self::TransferFinished { .. } => Concern::Everyone,
        }
    }
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

    /// Whose work this is holding up, so exactly one window asks it. Two dialogs over one
    /// question is worse than it sounds: the first answer releases the work and the second
    /// dialog stays on screen asking about something that has already happened.
    pub concern: Concern,

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

    /// A file of that name is already there. Answered by replacing it, keeping both, or
    /// letting the upload go no further.
    Overwrite { name: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Accept,
    Decline,
    /// The third choice, for the questions that have one: [`Question::Overwrite`] offers
    /// keeping both files alongside replacing and cancelling.
    KeepBoth,
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
    pub fn new(concern: Concern, question: Question) -> (Self, oneshot::Receiver<Answer>) {
        let (reply, answer) = oneshot::channel();

        (
            Self { question, concern, reply: std::sync::Mutex::new(Some(reply)) },
            answer,
        )
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
        let (prompt, waiting) = Prompt::new(Concern::Everyone, Question::Passphrase);

        prompt.answer(Answer::Text("correct horse".to_owned()));

        assert_eq!(waiting.await.unwrap(), Answer::Text("correct horse".to_owned()));
    }

    #[tokio::test]
    async fn a_prompt_nobody_answers_counts_as_declining() {
        let (prompt, waiting) = Prompt::new(Concern::Everyone, Question::UnknownHostKey {
            host: "example.com".to_owned(),
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "SHA256:whatever".to_owned(),
        });

        drop(prompt);

        assert_eq!(waiting.await.unwrap(), Answer::Decline);
    }
}
