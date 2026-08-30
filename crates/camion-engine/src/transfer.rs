use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use camion_core::{ByteStream, RemotePath};

use crate::session::SessionId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TransferId(pub u64);

impl TransferId {
    pub fn next() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);

        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// One end of a transfer.
///
/// Both ends being of the same type is what lets a transfer run between two remotes without a
/// special case — which is the whole point of the side-by-side view that comes later.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Endpoint {
    Local(PathBuf),
    Remote { session: SessionId, path: RemotePath },
}

impl Endpoint {
    pub fn name(&self) -> String {
        match self {
            Self::Local(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Self::Remote { path, .. } => path.name().unwrap_or("/").to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Upload,
    Download,
    Between,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Queued,
    Running,
    Done,
    Failed(String),
    Cancelled,
}

impl State {
    pub fn is_finished(&self) -> bool {
        !matches!(self, Self::Queued | Self::Running)
    }
}

/// A queued or running transfer, as the queue window shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transfer {
    pub id: TransferId,
    pub name: String,
    pub from: Endpoint,
    pub to: Endpoint,
    pub direction: Direction,
    pub state: State,
    pub transferred: u64,
    pub total: Option<u64>,
}

impl Transfer {
    pub fn new(from: Endpoint, to: Endpoint) -> Self {
        let direction = match (&from, &to) {
            (Endpoint::Local(_), Endpoint::Remote { .. }) => Direction::Upload,
            (Endpoint::Remote { .. }, Endpoint::Local(_)) => Direction::Download,
            _ => Direction::Between,
        };

        Self {
            id: TransferId::next(),
            name: to.name(),
            from,
            to,
            direction,
            state: State::Queued,
            transferred: 0,
            total: None,
        }
    }

    /// How far along, between 0 and 1, or nothing when the size is not known up front — which
    /// is a real case for chunked responses and for some FTP servers.
    pub fn fraction(&self) -> Option<f64> {
        match self.total {
            Some(total) if total > 0 => Some((self.transferred as f64 / total as f64).min(1.0)),
            Some(_) => Some(1.0),
            None => None,
        }
    }
}

/// Wraps a stream so that every chunk passing through is counted.
///
/// The progress bar is the only reason the engine cares how many bytes went by, so counting
/// lives here rather than inside any provider.
pub fn counting(
    stream: ByteStream,
    mut report: impl FnMut(u64) + Send + 'static,
) -> ByteStream {
    let size = stream.size();
    let mut transferred = 0;

    let counted = futures::StreamExt::map(stream, move |chunk| {
        if let Ok(bytes) = &chunk {
            transferred += bytes.len() as u64;
            report(transferred);
        }

        chunk
    });

    ByteStream::new(counted, size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn remote(path: &str) -> Endpoint {
        Endpoint::Remote {
            session: SessionId(1),
            path: RemotePath::parse(path).unwrap(),
        }
    }

    #[test]
    fn a_transfer_knows_which_way_it_is_going_and_what_to_call_itself() {
        let upload = Transfer::new(Endpoint::Local("/home/me/notes.txt".into()), remote("/notes.txt"));
        assert_eq!(upload.direction, Direction::Upload);
        assert_eq!(upload.name, "notes.txt");

        let download = Transfer::new(remote("/notes.txt"), Endpoint::Local("/home/me/notes.txt".into()));
        assert_eq!(download.direction, Direction::Download);

        let between = Transfer::new(remote("/a.txt"), remote("/b.txt"));
        assert_eq!(between.direction, Direction::Between);
        assert_eq!(between.name, "b.txt");
    }

    #[test]
    fn progress_is_a_fraction_only_when_the_size_is_known() {
        let mut transfer = Transfer::new(Endpoint::Local("/tmp/a".into()), remote("/a"));

        assert_eq!(transfer.fraction(), None);

        transfer.total = Some(200);
        transfer.transferred = 50;
        assert_eq!(transfer.fraction(), Some(0.25));

        transfer.transferred = 500;
        assert_eq!(transfer.fraction(), Some(1.0));
    }

    #[test]
    fn ids_are_never_reused() {
        let first = TransferId::next();
        let second = TransferId::next();

        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn counting_reports_a_running_total_without_changing_the_bytes() {
        let reported = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&reported);

        let counted = counting(ByteStream::once(&b"camion"[..]), move |total| {
            recorder.lock().unwrap().push(total)
        });

        assert_eq!(counted.size(), Some(6));
        assert_eq!(counted.collect().await.unwrap(), &b"camion"[..]);
        assert_eq!(*reported.lock().unwrap(), vec![6]);
    }
}
