//! Handing a stream to an HTTP client without reading it first.

use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures::StreamExt;
use http_body::{Body, Frame, SizeHint};
use okuri_core::{ByteStream, Error};

/// A [`ByteStream`] as a request body, of a length said up front.
///
/// What matters is what this does not do. A body collected into memory has been read to the end
/// before the first byte of it reaches the wire, so anything counting the read — which is how
/// the transfer queue knows how far along it is — is counting something that has not happened
/// yet. Handed over like this, the client asks for a chunk when it has room to send one, and
/// the count follows the socket instead of the disk.
///
/// The length has to be known: an object store signs it, and will not take a body of unstated
/// size. A stream that cannot say how long it is still has to be collected.
pub struct Sending {
    /// Behind a lock only because the AWS SDK asks a request body to be `Sync` and a stream
    /// that is merely `Send` is not. One task polls this and nothing else ever touches it, so
    /// the lock is never actually taken: every access here is through `&mut`.
    chunks: Mutex<ByteStream>,
    length: u64,
    /// Whether the stream has said it is over. The client's own wrappers ask again after
    /// that, and a stream asked past its end is allowed to panic — so the end is remembered
    /// here and the stream is left alone once it has been reached.
    finished: bool,
}

impl Sending {
    pub fn new(chunks: ByteStream, length: u64) -> Self {
        Self { chunks: Mutex::new(chunks), length, finished: false }
    }
}

impl Body for Sending {
    type Data = Bytes;
    type Error = Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Error>>> {
        let sending = self.get_mut();

        if sending.finished {
            return Poll::Ready(None);
        }

        let chunks = sending
            .chunks
            .get_mut()
            .expect("a lock nothing ever takes cannot be poisoned");

        let polled = chunks.poll_next_unpin(context);

        if let Poll::Ready(None) = polled {
            sending.finished = true;
        }

        polled.map(|chunk| chunk.map(|chunk| chunk.map(Frame::data)))
    }

    fn is_end_stream(&self) -> bool {
        self.finished
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.length)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    /// Counts the bytes read out of `chunks`, the way the engine's progress counter does.
    fn counted(chunks: &[&'static [u8]], read: Arc<AtomicU64>) -> ByteStream {
        let length = chunks.iter().map(|chunk| chunk.len() as u64).sum();

        let chunks = futures::stream::iter(chunks.to_vec()).map(move |chunk| {
            read.fetch_add(chunk.len() as u64, Ordering::SeqCst);
            Ok(Bytes::from_static(chunk))
        });

        ByteStream::new(chunks, Some(length))
    }

    /// A client is told how much is coming before any of it has been read, which is what lets
    /// it send a `Content-Length` and what stops a store refusing the request outright.
    #[tokio::test]
    async fn the_length_is_known_before_a_byte_has_been_read() {
        let read = Arc::new(AtomicU64::new(0));
        let sending = Sending::new(counted(&[b"ok", b"ur", b"i"], Arc::clone(&read)), 5);

        assert_eq!(sending.size_hint().exact(), Some(5));
        assert_eq!(read.load(Ordering::SeqCst), 0);
    }

    /// The client's wrappers ask for another frame after the last one, and a stream asked past
    /// its end is within its rights to panic. Seen for real: every multipart upload to R2 died
    /// in a worker thread on exactly that.
    #[tokio::test]
    async fn asking_past_the_end_is_answered_with_the_end_again() {
        let past_the_end = futures::stream::unfold(2, |left| async move {
            match left {
                0 => None,
                left => Some((Ok(Bytes::from_static(b"x")), left - 1)),
            }
        });
        let mut sending = Sending::new(ByteStream::new(past_the_end, Some(2)), 2);

        let mut frames = 0;

        while let Some(frame) = std::future::poll_fn(|context| Pin::new(&mut sending).poll_frame(context)).await {
            frame.unwrap();
            frames += 1;
        }

        assert_eq!(frames, 2);
        assert!(sending.is_end_stream());

        let again = std::future::poll_fn(|context| Pin::new(&mut sending).poll_frame(context)).await;
        assert!(again.is_none());
    }

    /// The point of the whole thing: bytes are read when whoever is sending them asks for the
    /// next chunk, so a counter on the read side is counting what has gone out rather than
    /// what has been picked up off the disk.
    #[tokio::test]
    async fn nothing_is_read_until_the_sender_asks_for_it() {
        let read = Arc::new(AtomicU64::new(0));
        let mut sending = Sending::new(counted(&[b"ok", b"ur", b"i"], Arc::clone(&read)), 5);

        // A sender with room for one chunk at a time. After each one it has taken, the count is
        // what it has taken and not a byte more.
        for taken in [2, 4, 5] {
            let frame =
                futures::future::poll_fn(|context| Pin::new(&mut sending).poll_frame(context))
                    .await
                    .unwrap()
                    .unwrap();

            assert!(frame.is_data());
            assert_eq!(read.load(Ordering::SeqCst), taken);
        }

        let end = futures::future::poll_fn(|context| Pin::new(&mut sending).poll_frame(context));

        assert!(end.await.is_none());
    }
}
