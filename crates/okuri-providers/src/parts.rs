//! Cutting a transfer into the pieces an object store will accept, and keeping several of them
//! moving at once — in both directions.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use okuri_core::{ByteRange, ByteStream, Error, Progress, Result};
use futures::StreamExt;

/// Reads from `body` until there is a whole part's worth of it, or until the file runs out.
///
/// `None` means there is nothing left to send. The last part is allowed to be short; every
/// other one has to reach `size`, which is what both S3 and Azure require, so a part is only
/// handed back once there is enough for one or the bytes have run out.
pub async fn next_part(body: &mut ByteStream, size: usize) -> Result<Option<Vec<u8>>> {
    let mut part = Vec::with_capacity(size);

    while part.len() < size {
        match body.next().await {
            Some(chunk) => part.extend_from_slice(&chunk?),
            None => break,
        }
    }

    if part.is_empty() {
        Ok(None)
    } else {
        Ok(Some(part))
    }
}

/// How much of one connection's memory every multipart transfer on it may hold between them.
///
/// An object store says it will take two dozen transfers at once, and each multipart one holds
/// [`IN_FLIGHT`] parts while they go up. Two dozen is the right number for small files, which is
/// what it was counted for — but at eight megabytes a part it also let a drop of large ones hold
/// three quarters of a gigabyte of buffers per endpoint, and at the part sizes a multi-gigabyte
/// object wants it would have been several times that.
///
/// So the slots stay as they are and the memory is bounded separately. Small files still get all
/// two dozen; large ones queue for room here.
const BUDGET: usize = 128 * 1024 * 1024;

/// What the budget is counted in. Parts are measured in whole megabytes and a semaphore counts
/// in whole numbers, so this keeps the arithmetic in numbers a person can read.
const PER_MEGABYTE: usize = 1024 * 1024;

/// The memory every part in flight on one connection is drawn from.
///
/// One per provider rather than one per transfer, which is the whole point: one transfer on its
/// own should fill the link, and a hundred at once should not fill the machine.
///
/// Deliberately not a [`Capabilities`](okuri_core::Capabilities) flag. This is the adapter's own
/// buffers, not something the interface reads or the engine derives — what a store will *take*
/// is still counted in transfer slots, and this only decides how much is held while it does.
#[derive(Clone)]
pub struct Budget {
    room: std::sync::Arc<tokio::sync::Semaphore>,
    /// The whole budget in megabytes, so a part larger than all of it can be held down to it
    /// rather than waiting for room that cannot exist.
    total: usize,
}

impl Budget {
    pub fn new() -> Self {
        Self::of(BUDGET)
    }

    fn of(bytes: usize) -> Self {
        let total = bytes.div_ceil(PER_MEGABYTE);

        Self { room: std::sync::Arc::new(tokio::sync::Semaphore::new(total)), total }
    }

    /// Room for a part of `bytes`, waited for until there is some.
    ///
    /// Never asks for more than the whole budget, because there is a part larger than it: an
    /// object big enough that staying under the store's limit on how many parts it will take
    /// needs a part size above this. Asking for room that cannot exist would hang such a
    /// transfer for ever; taking everything runs it one part at a time, which is all that can
    /// be done for it.
    async fn hold(&self, bytes: usize) -> tokio::sync::OwnedSemaphorePermit {
        let wanted = bytes.div_ceil(PER_MEGABYTE).clamp(1, self.total);

        std::sync::Arc::clone(&self.room)
            .acquire_many_owned(wanted as u32)
            .await
            .expect("a budget that is never closed")
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self::new()
    }
}

/// `body` as a stream of whole parts, each with the room it is holding.
///
/// Owning the stream rather than borrowing it is what lets a read be put down and picked up
/// again: a part that is half read is kept here between polls, so nothing is lost when the read
/// is left waiting to go and look at a part that has landed.
///
/// The room is taken before the part is read, because reading it is what fills the memory being
/// bounded, and given back by whoever finishes with the part.
fn in_parts(
    body: ByteStream,
    size: usize,
    budget: Budget,
) -> impl futures::Stream<Item = Result<(tokio::sync::OwnedSemaphorePermit, Vec<u8>)>> {
    futures::stream::try_unfold((body, budget), move |(mut body, budget)| async move {
        let room = budget.hold(size).await;

        Ok(next_part(&mut body, size)
            .await?
            .map(|part| ((room, part), (body, budget))))
    })
}

/// How many parts of one file are allowed to be in the air together.
///
/// Sending them one after another leaves the link idle for as long as it takes to read the next
/// part off disk, and the disk idle for as long as the last one takes to upload. Object stores
/// are built to take parts in parallel, and this is the difference between using a connection
/// and filling it.
///
/// Bounded because every part in flight is a part held in memory — for one transfer. What bounds
/// it across all of them is [`Budget`].
pub const IN_FLIGHT: usize = 4;

/// How much of a part is handed over at a time.
///
/// Not about holding less of it: the part is in memory either way, because a store has to be
/// told how long a part is before it will take it. This is about how often the count moves. A
/// part handed over in one piece is a part counted when the request is answered, and a fourteen
/// megabyte file is two parts that go up together and are answered together — a bar that sits at
/// nothing for the whole upload and then jumps to the end.
///
/// A quarter of a megabyte is small enough to move a bar smoothly and large enough that the
/// counting costs nothing next to the sending.
const SLICE: usize = 256 * 1024;

/// One part as a body that counts itself as it goes onto the wire.
///
/// The client asks for the next slice when it has room to send one, so what is counted is what
/// has been handed to the socket rather than what has been read or what has been answered.
///
/// `sent` is this attempt's own tally, which is what makes a retry possible: the shared count
/// has to be told to take back exactly what this attempt managed before the same part goes up
/// again.
fn counting_body(part: Bytes, progress: Option<Progress>, sent: Arc<AtomicU64>) -> ByteStream {
    let length = part.len() as u64;

    let slices = futures::stream::unfold(part, move |mut left| {
        let progress = progress.clone();
        let sent = Arc::clone(&sent);

        async move {
            if left.is_empty() {
                return None;
            }

            let slice = left.split_to(SLICE.min(left.len()));

            sent.fetch_add(slice.len() as u64, Ordering::SeqCst);

            if let Some(progress) = progress {
                progress.add(slice.len() as u64);
            }

            Some((Ok(slice), left))
        }
    });

    // Fused, because an unfolded stream panics if asked again after its end, and the client
    // sending it does ask again.
    ByteStream::new(slices.fuse(), Some(length))
}

/// Whether a part that failed is worth sending again.
///
/// Told apart by what can be ruled out rather than by what can be recognised. A refused
/// credential or a missing bucket will be refused again; everything else that reaches here — a
/// dropped connection, a 500, a store asking to be slowed down — arrives as the same general
/// failure with a sentence inside it. So the rule is to try once more unless the answer was one
/// that cannot change.
fn worth_retrying(error: &Error) -> bool {
    !matches!(
        error,
        Error::Authentication(_)
            | Error::PermissionDenied { .. }
            | Error::NotFound { .. }
            | Error::InvalidPath { .. }
            | Error::Cancelled
    )
}

/// Runs `send` over every part of `body`, keeping [`IN_FLIGHT`] of them going at once.
///
/// Reading and sending happen together, which they have to: a future that nobody polls does not
/// run, so awaiting a read on its own held every part already in the air completely still, and
/// nothing was sent at all until [`IN_FLIGHT`] whole parts had been read.
///
/// Results come back in the order the parts were read, which is the order an object store wants
/// them listed in when the upload is finished. The first failure stops the reading and is
/// returned once the parts already in the air have settled — so nothing is still being written
/// while the caller is cleaning up after it.
///
/// Each part is handed to `send` as a body of stated length that counts itself onto the wire —
/// see [`counting_body`]. Reading a part is not sending it, and neither is being answered for
/// it: a fourteen megabyte file is two parts read in the moment the upload starts and answered
/// within a moment of each other at the end of it, so counting either way leaves the bar at
/// nothing throughout and then jumps it to the end.
///
/// A part that fails is sent once more, unless it failed for a reason that will not change. The
/// bytes are kept for exactly that — the store's own client cannot replay a body it is being
/// handed a piece at a time, so the retry it used to do for us is done here instead, and what
/// the failed attempt had already counted is taken back first.
pub async fn each_part<T, F>(
    mut body: ByteStream,
    size: usize,
    budget: &Budget,
    send: impl Fn(usize, ByteStream) -> F,
) -> Result<Vec<T>>
where
    F: std::future::Future<Output = Result<T>>,
{
    let progress = body.acknowledged_by_writer();
    let mut parts = Box::pin(in_parts(body, size, budget.clone()));
    let mut sending = futures::stream::FuturesOrdered::new();
    let mut sent = Vec::new();
    let mut number = 0;
    let mut reading = true;
    let mut refused = None;

    while reading || !sending.is_empty() {
        tokio::select! {
            // Only while there is somewhere to put what comes back. A part waiting for a slot
            // is a part held in memory, and that is what `IN_FLIGHT` is bounding.
            part = parts.next(), if reading && sending.len() < IN_FLIGHT => match part {
                Some(Ok((room, part))) => {
                    let part = Bytes::from(part);
                    let counting = progress.clone();
                    let sending_one = &send;

                    // The room goes back when the part has gone up, not when it was read: it is
                    // in memory for the whole of that, and for the retry after it.
                    sending.push_back(async move {
                        let went = sent_part(sending_one, number, part, counting).await;
                        drop(room);

                        went
                    });

                    number += 1;
                }
                Some(Err(error)) => {
                    reading = false;
                    refused.get_or_insert(error);
                }
                None => reading = false,
            },
            Some(part) = sending.next() => match part {
                Ok(part) => sent.push(part),
                Err(error) => {
                    reading = false;
                    refused.get_or_insert(error);
                }
            },
        }
    }

    match refused {
        Some(error) => Err(error),
        None => Ok(sent),
    }
}

/// One part sent, and sent once more if the first attempt failed for a reason that might not
/// happen again.
///
/// The bytes are kept rather than handed away because that second attempt needs them: a body
/// being read a slice at a time cannot be replayed by the client sending it, so the one retry
/// the store's own SDK used to do for an in-memory part is done here instead.
async fn sent_part<T, F>(
    send: &impl Fn(usize, ByteStream) -> F,
    number: usize,
    part: Bytes,
    progress: Option<Progress>,
) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    let sent = Arc::new(AtomicU64::new(0));
    let body = counting_body(part.clone(), progress.clone(), Arc::clone(&sent));

    match send(number, body).await {
        Ok(done) => Ok(done),
        Err(error) if !worth_retrying(&error) => Err(error),
        Err(_) => {
            // Whatever reached the socket did not arrive after all, so it is taken back before
            // the same bytes are counted again on their way out.
            if let Some(progress) = &progress {
                progress.rewind(sent.swap(0, Ordering::SeqCst));
            }

            send(number, counting_body(part, progress, sent)).await
        }
    }
}

/// Every piece of an object between `from` and `size`, several requests in the air at once and
/// handed back in order.
///
/// The same shape as an upload in parts, and for the same reason: asking for the next piece only
/// once the last has landed leaves the link idle for a round trip every time. They land in
/// whatever order the store gets to them, and out of order they are not a slow download but a
/// corrupted file, so they are put back the way they were asked for.
///
/// What is held is [`IN_FLIGHT`] pieces of `piece` bytes, which is what an upload of the same
/// object already holds.
pub fn in_pieces<Fetch, Fetching>(
    from: u64,
    size: u64,
    piece: usize,
    budget: Budget,
    fetch: Fetch,
) -> impl futures::Stream<Item = Result<Bytes>> + Send
where
    Fetch: FnMut(ByteRange) -> Fetching + Send + 'static,
    Fetching: std::future::Future<Output = Result<Bytes>> + Send + 'static,
{
    struct Asking<Fetch> {
        fetch: Fetch,
        budget: Budget,
        offset: u64,
        fetching: futures::stream::FuturesOrdered<
            futures::future::BoxFuture<'static, (tokio::sync::OwnedSemaphorePermit, Result<Bytes>)>,
        >,
    }

    let asking = Asking {
        fetch,
        budget,
        offset: from,
        fetching: futures::stream::FuturesOrdered::new(),
    };

    futures::stream::unfold(asking, move |mut state| async move {
        while state.offset < size && state.fetching.len() < IN_FLIGHT {
            let length = (piece as u64).min(size - state.offset);
            let room = state.budget.hold(length as usize).await;
            let asked = (state.fetch)(ByteRange::new(state.offset, length));

            state.offset += length;
            state
                .fetching
                .push_back(Box::pin(async move { (room, asked.await) }));
        }

        // The room goes back as the piece is handed on, not when it landed: until then it is
        // sitting here waiting for the pieces asked for before it.
        let (room, piece) = state.fetching.next().await?;
        drop(room);

        Some((piece, state))
    })
}

/// The object's whole length, out of the `bytes 0-1023/4096` a ranged response answers with.
///
/// Nothing where the store writes `*`, which it is allowed to do and which means only that this
/// has to go on what it was told instead.
pub fn whole_length(content_range: &str) -> Option<u64> {
    content_range
        .rsplit_once('/')
        .and_then(|(_, total)| total.trim().parse().ok())
}

/// What a service will take, so how big a part should be can be worked out without knowing which
/// service it is for.
pub struct Limits {
    /// The smallest part this will use. At least what the service requires of every part but the
    /// last, and usually a little above it so an object just over the threshold still splits
    /// into parts the service accepts.
    pub smallest: usize,
    /// The largest single part the service will take.
    pub largest: usize,
    /// How many parts it will take for one object.
    pub most: usize,
}

/// How many parts a large object is worth splitting into.
///
/// Every part is a request, and a fixed eight megabytes meant a ten gigabyte upload was over a
/// thousand of them. A few hundred is enough to keep the link full and few enough that the
/// requests themselves stop being the cost.
const PARTS_WANTED: u64 = 256;

/// The largest part worth holding in memory, whatever the service would allow.
///
/// Half the budget, so two parts of any size can always be in the air at once. A part size only
/// one of which fits would turn a parallel upload back into a serial one.
const LARGEST_HELD: usize = BUDGET / 2;

/// How big each part of an object of `length` bytes should be.
///
/// Nothing here is a guess about the network. It is two bounds and a preference: the service will
/// not take more than so many parts, the machine will not hold more than so much, and between
/// those, fewer and larger requests beat more and smaller ones.
pub fn part_size(length: u64, limits: &Limits) -> usize {
    // The one bound here that is not a preference. A part size below this is an upload the store
    // refuses when it is nearly finished, which is the worst moment to find out.
    let must = in_whole_megabytes(length.div_ceil(limits.most as u64));
    let wanted = in_whole_megabytes(length.div_ceil(PARTS_WANTED));

    let comfortable = wanted.clamp(
        limits.smallest.min(LARGEST_HELD),
        limits.largest.min(LARGEST_HELD),
    );

    // And the rule beats the preference. A part too large to hold comfortably only makes an
    // upload serial — the budget hands such a transfer everything it has and it runs a part at
    // a time — while too many parts makes it fail outright.
    comfortable.max(must).min(limits.largest)
}

/// Rounded up to whole megabytes, which is what the memory budget counts in.
///
/// Saturating, because an object large enough to overflow this is one no service will take, and
/// it is about to be brought down to a size one will.
fn in_whole_megabytes(bytes: u64) -> usize {
    let rounded = bytes.div_ceil(PER_MEGABYTE as u64).saturating_mul(PER_MEGABYTE as u64);

    usize::try_from(rounded).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    fn stream(chunks: &[&'static [u8]]) -> ByteStream {
        let chunks = chunks
            .iter()
            .map(|chunk| Ok(bytes::Bytes::from_static(chunk)))
            .collect::<Vec<_>>();

        ByteStream::new(futures::stream::iter(chunks), None)
    }

    /// What S3 will take, which is the tighter of the two services on both counts.
    const S3: Limits = Limits {
        smallest: 8 * PER_MEGABYTE,
        largest: 5 * 1024 * PER_MEGABYTE,
        most: 10_000,
    };

    /// A ranged response says how long the whole object is, which is what lets a download told a
    /// stale size still come down whole rather than being cut off at the old one.
    #[test]
    fn a_ranged_response_says_how_long_the_whole_object_is() {
        assert_eq!(whole_length("bytes 0-1023/4096"), Some(4096));

        // Allowed, and it means only that nothing better is on offer than what we were told.
        assert_eq!(whole_length("bytes 0-1023/*"), None);
    }

    /// Every part is a request, and at a fixed eight megabytes a ten gigabyte upload was
    /// thirteen hundred of them.
    #[test]
    fn a_large_object_goes_up_in_parts_worth_a_request() {
        let ten_gigabytes = 10 * 1024 * PER_MEGABYTE as u64;

        assert_eq!(part_size(ten_gigabytes, &S3), 40 * PER_MEGABYTE);
        assert_eq!(ten_gigabytes.div_ceil(part_size(ten_gigabytes, &S3) as u64), PARTS_WANTED);
    }

    /// Splitting a small object finely gains nothing and costs a request each time, so below the
    /// point where it is worth it the part size stays where it was.
    #[test]
    fn a_small_object_keeps_the_smallest_part() {
        assert_eq!(part_size(0, &S3), S3.smallest);
        assert_eq!(part_size(100 * PER_MEGABYTE as u64, &S3), S3.smallest);
    }

    /// A part big enough that only one fits in memory would turn a parallel upload back into a
    /// serial one, so the preference stops short of that however large the object is.
    #[test]
    fn a_part_is_never_larger_than_the_memory_budget_allows() {
        let huge = 100 * 1024 * PER_MEGABYTE as u64;

        assert_eq!(part_size(huge, &S3), LARGEST_HELD);
    }

    /// How many parts a store will take is its rule rather than our preference, so an object
    /// big enough to need parts larger than the budget prefers gets them. It runs a part at a
    /// time instead, which is slow — and an upload refused at ten thousand parts, after all of
    /// them have gone up, is worse than slow.
    #[test]
    fn an_object_too_big_for_the_preferred_part_still_fits_the_stores_part_count() {
        let terabyte = 1024 * 1024 * PER_MEGABYTE as u64;
        let part = part_size(terabyte, &S3);

        assert!(part > LARGEST_HELD, "{part} bytes is not above the budget's preference");
        assert!(terabyte.div_ceil(part as u64) <= S3.most as u64);
    }

    /// Every multipart transfer on one connection draws from the same budget. An object store
    /// offers two dozen transfer slots — the right number for the small files they were counted
    /// for — and without this a drop of large ones held a part for each of them.
    #[tokio::test]
    async fn transfers_on_one_connection_share_a_bound_on_the_parts_they_hold() {
        let budget = Budget::of(2 * PER_MEGABYTE);
        let holding = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));

        let uploads = (0..3).map(|_| {
            let (budget, holding, most) = (budget.clone(), Arc::clone(&holding), Arc::clone(&most));

            async move {
                let sent: Vec<()> = each_part(
                    stream(&[b"aa", b"bb", b"cc", b"dd"]),
                    2,
                    &budget,
                    |_, _| {
                        let (holding, most) = (Arc::clone(&holding), Arc::clone(&most));

                        async move {
                            let now = holding.fetch_add(1, Ordering::SeqCst) + 1;
                            most.fetch_max(now, Ordering::SeqCst);

                            tokio::time::sleep(Duration::from_millis(20)).await;
                            holding.fetch_sub(1, Ordering::SeqCst);

                            Ok(())
                        }
                    },
                )
                .await
                .unwrap();

                assert_eq!(sent.len(), 4);
            }
        });

        futures::future::join_all(uploads).await;

        // Two megabytes of budget is two parts, however many transfers want one.
        assert_eq!(most.load(Ordering::SeqCst), 2);
    }

    /// A megabyte, which is one part of several slices.
    const WHOLE: u64 = 1024 * 1024;

    fn a_megabyte(record: impl Fn(u64) + Send + Sync + 'static) -> ByteStream {
        let chunks = (0..4).map(|_| Ok(Bytes::from(vec![b'a'; 256 * 1024])));

        ByteStream::new(futures::stream::iter(chunks), Some(WHOLE)).counted(record)
    }

    /// The bar follows the socket. Counting a part when the store answers for it left a
    /// fourteen megabyte upload showing nothing at all the whole way and then jumping to the
    /// end — it is two parts that go up together and are answered within a moment of each other.
    #[tokio::test]
    async fn a_part_is_counted_as_it_goes_onto_the_wire_rather_than_when_it_is_answered() {
        let reported = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = Arc::clone(&reported);

        let body = a_megabyte(move |transferred| recorder.lock().unwrap().push(transferred));

        // What the bar was showing each time the sender asked for another slice.
        let shown = Arc::new(std::sync::Mutex::new(Vec::new()));
        let watching = Arc::clone(&reported);
        let noting = Arc::clone(&shown);

        let sent: Vec<()> = each_part(body, WHOLE as usize, &Budget::new(), |_, mut part| {
            let (watching, noting) = (Arc::clone(&watching), Arc::clone(&noting));

            async move {
                loop {
                    let so_far = watching.lock().unwrap().last().copied().unwrap_or(0);
                    noting.lock().unwrap().push(so_far);

                    match part.next().await {
                        Some(slice) => slice?,
                        None => break,
                    };
                }

                Ok(())
            }
        })
        .await
        .unwrap();

        assert_eq!(sent.len(), 1);

        let shown = shown.lock().unwrap();

        assert_eq!(shown.first(), Some(&0), "bytes were counted before any were asked for");
        assert!(
            shown[1..].iter().all(|at| *at > 0),
            "the bar had not moved while the part was still going: {shown:?}"
        );

        assert_eq!(reported.lock().unwrap().last(), Some(&WHOLE));
    }

    /// A part that failed halfway had already counted what reached the socket, and the retry
    /// sends the whole part again from the start. Counting those bytes twice runs the bar past
    /// the end of a file that has not finished.
    #[tokio::test]
    async fn a_part_sent_again_after_a_failure_is_not_counted_twice() {
        let reported = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = Arc::clone(&reported);

        let body = a_megabyte(move |transferred| recorder.lock().unwrap().push(transferred));
        let attempts = Arc::new(AtomicUsize::new(0));
        let counting = Arc::clone(&attempts);

        let sent: Vec<()> = each_part(body, WHOLE as usize, &Budget::new(), move |_, mut part| {
            let counting = Arc::clone(&counting);

            async move {
                let first = counting.fetch_add(1, Ordering::SeqCst) == 0;
                let mut taken = 0u64;

                while let Some(slice) = part.next().await {
                    taken += slice?.len() as u64;

                    // Halfway through the first attempt, the connection goes away.
                    if first && taken * 2 >= WHOLE {
                        return Err(Error::provider("the connection went away"));
                    }
                }

                Ok(())
            }
        })
        .await
        .unwrap();

        assert_eq!(sent.len(), 1);
        assert_eq!(attempts.load(Ordering::SeqCst), 2, "the part was not sent again");

        let reported = reported.lock().unwrap();

        assert_eq!(reported.last(), Some(&WHOLE));
        assert!(
            reported.iter().all(|at| *at <= WHOLE),
            "the bar ran past the end of the file: {reported:?}"
        );
    }

    /// Sending it again is only worth the request when the answer might be different, and a
    /// refused credential will refuse it again.
    #[tokio::test]
    async fn a_part_refused_for_a_reason_that_cannot_change_is_not_sent_again() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let counting = Arc::clone(&attempts);

        let refused: Result<Vec<()>> =
            each_part(stream(&[b"aa"]), 2, &Budget::new(), move |_, _| {
                let counting = Arc::clone(&counting);

                async move {
                    counting.fetch_add(1, Ordering::SeqCst);

                    Err(Error::Authentication("the key was refused".to_owned()))
                }
            })
            .await;

        assert!(refused.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    /// A pattern with a period that shares no factor with a piece, so a piece put back in the
    /// wrong place cannot happen to match.
    fn contents(length: usize) -> Bytes {
        Bytes::from((0..length).map(|index| (index % 251) as u8).collect::<Vec<u8>>())
    }

    /// The store, answering the later requests of every four sooner than the earlier ones — so
    /// anything relying on the order they came back in rather than the order they were asked
    /// for comes back shuffled.
    fn store(
        object: Bytes,
    ) -> impl FnMut(ByteRange) -> futures::future::BoxFuture<'static, Result<Bytes>> {
        let mut asked = 0u64;

        move |range| {
            let start = range.offset as usize;
            let end = start + range.length.expect("a length, since a piece has one") as usize;
            let piece = object.slice(start..end);

            let waiting = Duration::from_millis(20 * (IN_FLIGHT as u64 - asked % IN_FLIGHT as u64));

            asked += 1;

            Box::pin(async move {
                tokio::time::sleep(waiting).await;

                Ok(piece)
            })
        }
    }

    #[tokio::test]
    async fn an_object_asked_for_in_pieces_arrives_whole_and_in_order() {
        let object = contents(16 * 1024);

        let pieces = in_pieces(0, object.len() as u64, 1024, Budget::new(), store(object.clone()));
        let whole = ByteStream::new(pieces, None).collect().await.unwrap();

        assert_eq!(whole, object);
    }

    /// The last piece is whatever is left over, and an object that does not divide evenly is
    /// the usual case rather than the exception.
    #[tokio::test]
    async fn an_object_that_does_not_divide_into_whole_pieces_still_arrives_whole() {
        let object = contents(3 * 1024 + 17);

        let pieces = in_pieces(0, object.len() as u64, 1024, Budget::new(), store(object.clone()));
        let whole = ByteStream::new(pieces, None).collect().await.unwrap();

        assert_eq!(whole, object);
    }

    /// The first piece is in hand before the rest are asked for — it is what said how long the
    /// object is — so the ranges pick up after it rather than fetching it twice.
    #[tokio::test]
    async fn the_pieces_start_where_the_first_one_left_off() {
        let object = contents(8 * 1024);

        let rest = in_pieces(1024, object.len() as u64, 1024, Budget::new(), store(object.clone()));
        let tail = ByteStream::new(rest, None).collect().await.unwrap();

        assert_eq!(tail, object.slice(1024..));
    }

    /// A piece that cannot be fetched has to reach the caller, or a download quietly finishes
    /// with a hole in the middle of it.
    #[tokio::test]
    async fn a_piece_that_fails_fails_the_download() {
        let mut asked = 0;

        let pieces = in_pieces(0, 4 * 1024, 1024, Budget::new(), move |_| {
            asked += 1;

            async move {
                match asked {
                    2 => Err(okuri_core::Error::provider("the store said no")),
                    _ => Ok(Bytes::from_static(&[0; 1024])),
                }
            }
        });

        assert!(ByteStream::new(pieces, None).collect().await.is_err());
    }

    /// Chunks arrive at whatever size the reader hands them over, which has nothing to do with
    /// the size a part has to be.
    #[tokio::test]
    async fn small_chunks_are_gathered_into_a_whole_part() {
        let mut body = stream(&[b"ok", b"ur", b"i", b"!"]);

        assert_eq!(next_part(&mut body, 4).await.unwrap(), Some(b"okur".to_vec()));
        assert_eq!(next_part(&mut body, 4).await.unwrap(), Some(b"i!".to_vec()));
        assert_eq!(next_part(&mut body, 4).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_chunk_larger_than_a_part_is_not_split_further() {
        let mut body = stream(&[b"okuri"]);

        assert_eq!(next_part(&mut body, 2).await.unwrap(), Some(b"okuri".to_vec()));
        assert_eq!(next_part(&mut body, 2).await.unwrap(), None);
    }

    /// A part read to the end, which is what a store sending it would have done to it.
    async fn received(part: ByteStream) -> String {
        String::from_utf8(part.collect().await.unwrap().to_vec()).unwrap()
    }

    /// Parts go up together but must be listed in the order they were read — an object store
    /// reassembles the file from that list, so a shuffled one is a corrupted file.
    #[tokio::test]
    async fn parts_come_back_in_the_order_they_were_read() {
        let body = stream(&[b"aa", b"bb", b"cc", b"dd", b"ee", b"ff", b"gg"]);

        let sent = each_part(body, 2, &Budget::new(), |index, part| async move {
            // The later a part is, the slower it answers — so anything relying on completion
            // order rather than read order comes back backwards.
            tokio::time::sleep(std::time::Duration::from_millis(20 * (8 - index) as u64)).await;

            Ok((index, received(part).await))
        })
        .await
        .unwrap();

        assert_eq!(
            sent,
            vec![
                (0, "aa".to_owned()),
                (1, "bb".to_owned()),
                (2, "cc".to_owned()),
                (3, "dd".to_owned()),
                (4, "ee".to_owned()),
                (5, "ff".to_owned()),
                (6, "gg".to_owned()),
            ]
        );
    }

    #[tokio::test]
    async fn more_than_one_part_is_in_the_air_at_a_time() {
        let sending = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));
        let body = stream(&[b"aa", b"bb", b"cc", b"dd", b"ee", b"ff"]);

        each_part(body, 2, &Budget::new(), |_, _| {
            let (sending, most) = (Arc::clone(&sending), Arc::clone(&most));

            async move {
                let now = sending.fetch_add(1, Ordering::SeqCst) + 1;
                most.fetch_max(now, Ordering::SeqCst);

                tokio::time::sleep(Duration::from_millis(20)).await;
                sending.fetch_sub(1, Ordering::SeqCst);

                Ok(())
            }
        })
        .await
        .unwrap();

        assert!(most.load(Ordering::SeqCst) > 1, "never sent two at once");
        assert!(most.load(Ordering::SeqCst) <= IN_FLIGHT);
    }

    /// Reading the next part must not stop the ones already read from going up. A future that
    /// nobody polls does not run, and awaiting a read on its own meant four whole parts were
    /// read before a single byte of the first one was sent.
    #[tokio::test]
    async fn a_part_goes_up_while_the_next_one_is_still_being_read() {
        let read = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&read);

        // A source that takes a moment over each chunk, the way a disk or another server does.
        let chunks = futures::stream::iter(0..6).then(move |_| {
            let counter = Arc::clone(&counter);

            async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
                counter.fetch_add(1, Ordering::SeqCst);

                Ok(bytes::Bytes::from_static(b"aa"))
            }
        });

        let sent = each_part(ByteStream::new(chunks, Some(12)), 2, &Budget::new(), |_, _| {
            let read = Arc::clone(&read);

            async move { Ok(read.load(Ordering::SeqCst)) }
        })
        .await
        .unwrap();

        assert_eq!(sent[0], 1, "nothing was sent until {} parts had been read", sent[0]);
    }

    /// A part that fails takes the whole upload with it, and does so once the parts already in
    /// the air have settled — the caller aborts the upload next, and aborting one that is still
    /// being written to is how parts get left behind to be paid for.
    #[tokio::test]
    async fn a_failed_part_stops_the_upload_once_the_others_have_settled() {
        let finished = Arc::new(AtomicUsize::new(0));
        let body = stream(&[b"aa", b"bb", b"cc", b"dd"]);

        let sent: Result<Vec<()>> = each_part(body, 2, &Budget::new(), |index, _| {
            let finished = Arc::clone(&finished);

            async move {
                match index {
                    1 => {
                        tokio::time::sleep(Duration::from_millis(10)).await;

                        Err(okuri_core::Error::provider("the part was refused"))
                    }
                    _ => {
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        finished.fetch_add(1, Ordering::SeqCst);

                        Ok(())
                    }
                }
            }
        })
        .await;

        assert!(sent.is_err());
        assert_eq!(finished.load(Ordering::SeqCst), 3, "a part still going up was cut off");
    }

    #[tokio::test]
    async fn an_empty_file_has_no_parts() {
        assert_eq!(next_part(&mut stream(&[]), 4).await.unwrap(), None);
    }
}
