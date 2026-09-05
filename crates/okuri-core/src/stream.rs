use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt};

use crate::error::Result;

/// What a destination should say about a file when it hands it out.
///
/// Travels with the bytes because it describes them: a PNG is a PNG wherever it is put, and
/// gzipped bytes are unreadable to anything not told they are gzipped. A destination with
/// nowhere to keep this ignores it — an SFTP server has no content type — and one that has
/// somewhere uses it instead of guessing from the name.
///
/// Deliberately not [`Served`](crate::Served), which is what a destination *reports*. An ETag
/// is the store's own and a storage class is a decision about where a file now lives; neither
/// is something to copy in from somewhere else.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Serve {
    pub content_type: Option<String>,
    pub cache_control: Option<String>,
    pub content_encoding: Option<String>,
}

impl Serve {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// This, with the type worked out from `name` when nothing said what it was.
    ///
    /// In that order, and not the other way round. The name is a guess from an extension, and a
    /// file that has come from a store that knows what it is holding does not need guessing at
    /// — which matters most for the files that have no extension to guess from.
    pub fn or_guessed_from(&self, name: Option<&str>) -> Self {
        Self {
            content_type: self
                .content_type
                .clone()
                .or_else(|| name.and_then(crate::media_type).map(str::to_owned)),
            ..self.clone()
        }
    }
}

/// How often a running transfer is worth mentioning.
///
/// A progress bar cannot show more than a handful of states a second, and every report costs an
/// event, a heap allocation, and a hop onto the interface thread. Reporting per chunk means
/// millions of them for a large file — enough to hold up the transfer and to bury the window
/// under redraws it gains nothing from.
const REPORT_EVERY: Duration = Duration::from_millis(100);

/// How far along a transfer is, and who to tell.
///
/// Carried by the stream rather than wrapped around it, so a destination that buffers before
/// sending can take it over. A multipart upload reads a whole part into memory long before that
/// part reaches the server: counting the read there filled the bar to the end a second in and
/// then left it sitting while the file actually went up.
///
/// Shared and countable from anywhere, because once a provider has taken it over the counting
/// happens in several places at once — one for each part in flight.
#[derive(Clone)]
pub struct Progress(Arc<Reporting>);

struct Reporting {
    report: Box<dyn Fn(u64) + Send + Sync>,
    /// How much there is altogether, so the report that finishes the transfer can be told apart
    /// from every other one and never be the one the throttle swallows.
    size: Option<u64>,
    /// Whether reading a chunk out of the stream counts as progress. It does until a destination
    /// says otherwise — see [`ByteStream::acknowledged_by_writer`].
    counting_reads: AtomicBool,
    counted: Mutex<Counted>,
}

struct Counted {
    transferred: u64,
    mentioned: Option<Instant>,
}

impl Progress {
    fn new(report: impl Fn(u64) + Send + Sync + 'static, size: Option<u64>) -> Self {
        Self(Arc::new(Reporting {
            report: Box::new(report),
            size,
            counting_reads: AtomicBool::new(true),
            counted: Mutex::new(Counted { transferred: 0, mentioned: None }),
        }))
    }

    /// Counts `bytes` as having arrived where they were going, and says so if it is time to.
    ///
    /// The running total is exact; only how often it is mentioned is throttled. The report that
    /// completes the transfer always goes out, so a bar never stops at 94% on a transfer that
    /// has finished.
    pub fn add(&self, bytes: u64) {
        let mut counted = self.0.counted.lock().expect("a count nothing can poison");

        counted.transferred += bytes;

        let now = Instant::now();
        let due = counted.mentioned.is_none_or(|last| now.duration_since(last) >= REPORT_EVERY);
        let whole = self.0.size.is_some_and(|size| counted.transferred >= size);

        if due || whole {
            counted.mentioned = Some(now);

            // Said with the lock let go. The report hops onto the interface thread, and holding
            // a lock across that would put the window between two parts of an upload.
            let transferred = counted.transferred;
            drop(counted);

            (self.0.report)(transferred);
        }
    }

    /// Takes back bytes that turned out not to have arrived after all.
    ///
    /// A part that failed halfway had already counted whatever reached the socket, and the
    /// retry sends the whole part again from the start. Without this those bytes are counted
    /// twice, and a bar that reads past the end of the file is worse than one that stalls.
    ///
    /// Says nothing on its own. The number only ever goes backwards because something is about
    /// to be sent again, and the next report is a moment away.
    pub fn rewind(&self, bytes: u64) {
        let mut counted = self.0.counted.lock().expect("a count nothing can poison");

        counted.transferred = counted.transferred.saturating_sub(bytes);
    }

    fn read(&self, bytes: u64) {
        if self.0.counting_reads.load(Ordering::SeqCst) {
            self.add(bytes);
        }
    }
}

/// The bytes of one file, moving in one direction.
///
/// A stream carries its own length when the provider knows it, so `write` needs no second
/// argument and the transfer queue can show a percentage without asking twice. It carries how
/// it should be served for the same reason: that is known where the file is read and needed
/// where it is written, and everything in between is only moving bytes. And it carries how far
/// along it is, for the third: only the destination knows when bytes have really landed.
pub struct ByteStream {
    chunks: Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>,
    len: Option<u64>,
    serve: Serve,
    progress: Option<Progress>,
}

impl ByteStream {
    pub fn new(
        chunks: impl Stream<Item = Result<Bytes>> + Send + 'static,
        len: Option<u64>,
    ) -> Self {
        Self { chunks: Box::pin(chunks), len, serve: Serve::default(), progress: None }
    }

    /// Says how the file these bytes make up should be handed out.
    pub fn served_as(mut self, serve: Serve) -> Self {
        self.serve = serve;
        self
    }

    /// What the source said about how to serve this, which is empty when nothing said.
    pub fn serve(&self) -> &Serve {
        &self.serve
    }

    /// Counts what passes through and reports the running total, a few times a second at most.
    ///
    /// Counting the chunks as they are read is right for everything that puts a chunk on the
    /// wire as it gets it, which is most of what happens here. A destination that buffers takes
    /// the count over instead — see [`ByteStream::acknowledged_by_writer`].
    ///
    /// The count lives on the stream, so anything that reads *through* it keeps counting for
    /// free. Anything that puts these bytes into a *different* `ByteStream` has to carry the
    /// count across with them, the same rule [`Serve`] follows — and will need an accessor for
    /// it, since nothing does that today.
    pub fn counted(mut self, report: impl Fn(u64) + Send + Sync + 'static) -> Self {
        self.progress = Some(Progress::new(report, self.len));
        self
    }

    /// Hands the count over to whoever is writing these bytes, and stops counting them as they
    /// are read.
    ///
    /// For a destination that holds a whole part before sending it: reading is not sending, and
    /// the part is in memory for the whole time it takes to go up. Whoever takes this over is
    /// promising to [`add`](Progress::add) the bytes as they reach the wire, and to
    /// [`rewind`](Progress::rewind) any that turned out not to have.
    ///
    /// `None` when nobody is counting, which is every read that is not a transfer.
    pub fn acknowledged_by_writer(&mut self) -> Option<Progress> {
        let progress = self.progress.clone()?;
        progress.0.counting_reads.store(false, Ordering::SeqCst);

        Some(progress)
    }

    pub fn once(bytes: impl Into<Bytes>) -> Self {
        let bytes = bytes.into();
        let len = bytes.len() as u64;

        Self::new(futures::stream::once(async move { Ok(bytes) }), Some(len))
    }

    pub fn empty() -> Self {
        Self::new(futures::stream::empty(), Some(0))
    }

    /// The total number of bytes, when the provider knows it up front. Chunked responses and
    /// some FTP servers do not tell us, so this is honestly optional rather than a guess.
    pub fn size(&self) -> Option<u64> {
        self.len
    }

    /// Every byte, in one piece.
    ///
    /// Read through the stream itself rather than out of it, so that whatever is counting these
    /// bytes still sees them go by.
    pub async fn collect(mut self) -> Result<Bytes> {
        let mut collected = BytesMut::new();

        while let Some(chunk) = self.next().await {
            collected.extend_from_slice(&chunk?);
        }

        Ok(collected.freeze())
    }
}

impl Stream for ByteStream {
    type Item = Result<Bytes>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let stream = self.get_mut();
        let polled = stream.chunks.as_mut().poll_next(context);

        if let Poll::Ready(Some(Ok(chunk))) = &polled
            && let Some(progress) = &stream.progress
        {
            progress.read(chunk.len() as u64);
        }

        polled
    }
}

impl std::fmt::Debug for ByteStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ByteStream").field("len", &self.len).finish()
    }
}

/// A slice of a file, for resuming an interrupted download and for previewing a header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteRange {
    pub offset: u64,
    pub length: Option<u64>,
}

impl ByteRange {
    pub fn from(offset: u64) -> Self {
        Self { offset, length: None }
    }

    pub fn new(offset: u64, length: u64) -> Self {
        Self { offset, length: Some(length) }
    }

    /// The `bytes=0-1023` form that HTTP-shaped providers want.
    ///
    /// A range of no bytes has no last byte to name, so it is written open-ended rather than
    /// as `bytes=0-18446744073709551615`, which is what subtracting one from nothing gives.
    pub fn to_header(&self) -> String {
        match self.length {
            Some(0) | None => format!("bytes={}-", self.offset),
            Some(length) => format!("bytes={}-{}", self.offset, self.offset + length - 1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_render_as_http_headers() {
        assert_eq!(ByteRange::from(512).to_header(), "bytes=512-");
        assert_eq!(ByteRange::new(0, 1024).to_header(), "bytes=0-1023");

        // No bytes has no last byte to name.
        assert_eq!(ByteRange::new(0, 0).to_header(), "bytes=0-");
    }

    #[tokio::test]
    async fn a_stream_knows_its_length_and_collects_back_to_bytes() {
        let stream = ByteStream::once(&b"okuri"[..]);

        assert_eq!(stream.size(), Some(5));
        assert_eq!(stream.collect().await.unwrap(), &b"okuri"[..]);
    }

    fn recording() -> (Arc<Mutex<Vec<u64>>>, impl Fn(u64) + Send + Sync + 'static) {
        let reported = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&reported);

        (reported, move |total| recorder.lock().unwrap().push(total))
    }

    #[tokio::test]
    async fn counting_reports_a_running_total_without_changing_the_bytes() {
        let (reported, record) = recording();
        let counted = ByteStream::once(&b"okuri"[..]).counted(record);

        assert_eq!(counted.size(), Some(5));
        assert_eq!(counted.collect().await.unwrap(), &b"okuri"[..]);
        assert_eq!(*reported.lock().unwrap(), vec![5]);
    }

    /// A stream of many small chunks is one transfer, not one progress bar per chunk. Every
    /// report costs an event and a hop onto the interface thread, and a bar cannot show more
    /// than a few states a second anyway.
    #[tokio::test]
    async fn a_fast_stream_is_not_reported_on_chunk_by_chunk() {
        let (reported, record) = recording();

        let chunks = (0..1000).map(|_| Ok(Bytes::from_static(b"okuri")));
        let counted = ByteStream::new(futures::stream::iter(chunks), Some(5000)).counted(record);

        assert_eq!(counted.collect().await.unwrap().len(), 5000);

        let reported = reported.lock().unwrap();

        assert!(reported.len() < 20, "reported {} times", reported.len());

        // However few reports there were, the last one is the whole file — a bar that stops at
        // 94% on a transfer that finished is worse than one that never moved.
        assert_eq!(reported.last(), Some(&5000));
    }

    /// A destination that holds a part before sending it takes the count over, and reading
    /// stops being what counts. Otherwise the bar reaches the end while the file is still
    /// sitting in memory waiting to go.
    #[tokio::test]
    async fn once_a_writer_has_taken_the_count_over_reading_no_longer_moves_it() {
        let (reported, record) = recording();
        let mut counted = ByteStream::once(&b"okuri"[..]).counted(record);

        let progress = counted.acknowledged_by_writer().expect("something is counting");

        assert_eq!(counted.collect().await.unwrap(), &b"okuri"[..]);
        assert!(reported.lock().unwrap().is_empty(), "the read was counted as sent");

        // And what the writer says did happen is what the bar shows.
        progress.add(5);
        assert_eq!(*reported.lock().unwrap(), vec![5]);
    }

    /// Nobody is counting most reads — a preview, a config file — and taking the count over is
    /// then nothing to do rather than something to guard against.
    #[test]
    fn a_stream_nobody_is_counting_has_no_count_to_hand_over() {
        assert!(ByteStream::once(&b"okuri"[..]).acknowledged_by_writer().is_none());
    }


    /// The name is a guess and what a store said is not, so the guess only fills a gap. A file
    /// copied from another store is the case that matters: it knows what it is, and it is often
    /// exactly the file whose name has no extension to guess from.
    #[test]
    fn what_the_source_said_beats_guessing_from_the_name() {
        let said = Serve {
            content_type: Some("image/png".to_owned()),
            ..Serve::default()
        };

        assert_eq!(
            said.or_guessed_from(Some("harbour.jpg")).content_type.as_deref(),
            Some("image/png")
        );

        assert_eq!(
            said.or_guessed_from(Some("zxw70aa0i2orkjdfulmy8ckt7xox")).content_type.as_deref(),
            Some("image/png")
        );
    }

    #[test]
    fn the_name_fills_the_gap_when_nothing_said() {
        let nothing = Serve::default();

        assert_eq!(
            nothing.or_guessed_from(Some("harbour.jpg")).content_type.as_deref(),
            Some("image/jpeg")
        );

        // Still nothing, so the store applies its own default rather than being told wrongly.
        assert_eq!(nothing.or_guessed_from(Some("LICENSE")).content_type, None);
        assert_eq!(nothing.or_guessed_from(None).content_type, None);
    }

    /// Cache headers and encodings are not guessable from a name, so they pass through as they
    /// are or not at all.
    #[test]
    fn only_the_type_is_ever_guessed() {
        let said = Serve {
            content_type: None,
            cache_control: Some("public, max-age=3600".to_owned()),
            content_encoding: Some("gzip".to_owned()),
        };

        let filled = said.or_guessed_from(Some("harbour.jpg"));

        assert_eq!(filled.cache_control.as_deref(), Some("public, max-age=3600"));
        assert_eq!(filled.content_encoding.as_deref(), Some("gzip"));
    }
}
