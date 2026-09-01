//! Cutting an upload into the pieces an object store will accept, and keeping several of them
//! moving at once.

use okuri_core::{ByteStream, Result};
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

/// How many parts of one file are allowed to be in the air together.
///
/// Sending them one after another leaves the link idle for as long as it takes to read the next
/// part off disk, and the disk idle for as long as the last one takes to upload. Object stores
/// are built to take parts in parallel, and this is the difference between using a connection
/// and filling it.
///
/// Bounded because every part in flight is a part held in memory.
pub const IN_FLIGHT: usize = 4;

/// Runs `send` over every part of `body`, keeping [`IN_FLIGHT`] of them going at once.
///
/// Results come back in the order the parts were read, which is the order an object store wants
/// them listed in when the upload is finished. The first failure stops the reading and is
/// returned once the parts already in the air have settled — so nothing is still being written
/// while the caller is cleaning up after it.
pub async fn each_part<T, F>(
    body: &mut ByteStream,
    size: usize,
    mut send: impl FnMut(usize, Vec<u8>) -> F,
) -> Result<Vec<T>>
where
    F: std::future::Future<Output = Result<T>>,
{
    let mut sending = futures::stream::FuturesOrdered::new();
    let mut sent = Vec::new();
    let mut number = 0;
    let mut reading = true;

    loop {
        while reading && sending.len() < IN_FLIGHT {
            match next_part(body, size).await? {
                Some(part) => {
                    sending.push_back(send(number, part));
                    number += 1;
                }
                None => reading = false,
            }
        }

        match sending.next().await {
            Some(part) => sent.push(part?),
            None => return Ok(sent),
        }
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

    /// Parts go up together but must be listed in the order they were read — an object store
    /// reassembles the file from that list, so a shuffled one is a corrupted file.
    #[tokio::test]
    async fn parts_come_back_in_the_order_they_were_read() {
        let mut body = stream(&[b"aa", b"bb", b"cc", b"dd", b"ee", b"ff", b"gg"]);

        let sent = each_part(&mut body, 2, |index, part| async move {
            // The later a part is, the slower it answers — so anything relying on completion
            // order rather than read order comes back backwards.
            tokio::time::sleep(std::time::Duration::from_millis(20 * (8 - index) as u64)).await;

            Ok((index, String::from_utf8(part).unwrap()))
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
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let sending = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));
        let mut body = stream(&[b"aa", b"bb", b"cc", b"dd", b"ee", b"ff"]);

        each_part(&mut body, 2, |_, _| {
            let (sending, most) = (Arc::clone(&sending), Arc::clone(&most));

            async move {
                let now = sending.fetch_add(1, Ordering::SeqCst) + 1;
                most.fetch_max(now, Ordering::SeqCst);

                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                sending.fetch_sub(1, Ordering::SeqCst);

                Ok(())
            }
        })
        .await
        .unwrap();

        assert!(most.load(Ordering::SeqCst) > 1, "never sent two at once");
        assert!(most.load(Ordering::SeqCst) <= IN_FLIGHT);
    }

    /// A part that fails takes the whole upload with it, and does so once the parts already in
    /// the air have settled — the caller aborts the upload next, and aborting one that is still
    /// being written to is how parts get left behind to be paid for.
    #[tokio::test]
    async fn a_failed_part_stops_the_upload() {
        let mut body = stream(&[b"aa", b"bb", b"cc", b"dd"]);

        let sent: Result<Vec<()>> = each_part(&mut body, 2, |index, _| async move {
            match index {
                1 => Err(okuri_core::Error::provider("the part was refused")),
                _ => Ok(()),
            }
        })
        .await;

        assert!(sent.is_err());
    }

    #[tokio::test]
    async fn an_empty_file_has_no_parts() {
        assert_eq!(next_part(&mut stream(&[]), 4).await.unwrap(), None);
    }
}
