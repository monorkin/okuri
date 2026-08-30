use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt};

use crate::error::Result;

/// The bytes of one file, moving in one direction.
///
/// A stream carries its own length when the provider knows it, so `write` needs no second
/// argument and the transfer queue can show a percentage without asking twice.
pub struct ByteStream {
    chunks: Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>,
    len: Option<u64>,
}

impl ByteStream {
    pub fn new(
        chunks: impl Stream<Item = Result<Bytes>> + Send + 'static,
        len: Option<u64>,
    ) -> Self {
        Self { chunks: Box::pin(chunks), len }
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

    pub async fn collect(mut self) -> Result<Bytes> {
        let mut collected = BytesMut::new();

        while let Some(chunk) = self.chunks.next().await {
            collected.extend_from_slice(&chunk?);
        }

        Ok(collected.freeze())
    }
}

impl Stream for ByteStream {
    type Item = Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.chunks.as_mut().poll_next(context)
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
        let stream = ByteStream::once(&b"camion"[..]);

        assert_eq!(stream.size(), Some(6));
        assert_eq!(stream.collect().await.unwrap(), &b"camion"[..]);
    }
}
