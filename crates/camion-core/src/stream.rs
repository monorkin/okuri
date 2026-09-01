use std::pin::Pin;
use std::task::{Context, Poll};

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

/// The bytes of one file, moving in one direction.
///
/// A stream carries its own length when the provider knows it, so `write` needs no second
/// argument and the transfer queue can show a percentage without asking twice. It carries how
/// it should be served for the same reason: that is known where the file is read and needed
/// where it is written, and everything in between is only moving bytes.
pub struct ByteStream {
    chunks: Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>,
    len: Option<u64>,
    serve: Serve,
}

impl ByteStream {
    pub fn new(
        chunks: impl Stream<Item = Result<Bytes>> + Send + 'static,
        len: Option<u64>,
    ) -> Self {
        Self { chunks: Box::pin(chunks), len, serve: Serve::default() }
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
