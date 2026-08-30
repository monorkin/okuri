//! Cutting an upload into the pieces an object store will accept.

use camion_core::{ByteStream, Result};
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

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(chunks: &[&'static [u8]]) -> ByteStream {
        let chunks = chunks
            .iter()
            .map(|chunk| Ok(bytes::Bytes::from_static(chunk)))
            .collect::<Vec<_>>();

        ByteStream::new(futures::stream::iter(chunks), None)
    }

    /// Chunks arrive at whatever size the reader hands them over, which has nothing to do with
    /// the size a part has to be.
    #[tokio::test]
    async fn small_chunks_are_gathered_into_a_whole_part() {
        let mut body = stream(&[b"ca", b"mi", b"on", b"!"]);

        assert_eq!(next_part(&mut body, 4).await.unwrap(), Some(b"cami".to_vec()));
        assert_eq!(next_part(&mut body, 4).await.unwrap(), Some(b"on!".to_vec()));
        assert_eq!(next_part(&mut body, 4).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_chunk_larger_than_a_part_is_not_split_further() {
        let mut body = stream(&[b"camion"]);

        assert_eq!(next_part(&mut body, 2).await.unwrap(), Some(b"camion".to_vec()));
        assert_eq!(next_part(&mut body, 2).await.unwrap(), None);
    }

    #[tokio::test]
    async fn an_empty_file_has_no_parts() {
        assert_eq!(next_part(&mut stream(&[]), 4).await.unwrap(), None);
    }
}
