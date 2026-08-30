use async_trait::async_trait;
use std::time::Duration;

use aws_sdk_s3::config::retry::RetryConfig;
use aws_sdk_s3::config::timeout::TimeoutConfig;
use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::primitives::ByteStream as AwsStream;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart, Delete, ObjectIdentifier};
use aws_sdk_s3::Client;
use bytes::{Bytes, BytesMut};
use camion_core::{
    ByteRange, ByteStream, Capabilities, Entry, Error, Provider, RemotePath, Result,
};
use futures::StreamExt;
use time::OffsetDateTime;

use crate::destination::S3 as S3Config;
use crate::secret::Secret;

/// Anything that speaks S3: Amazon, Cloudflare R2, Backblaze B2, MinIO, and the rest.
///
/// A flat keyspace wearing a folder costume. Folders are a listing trick — a shared prefix up
/// to the next `/` — so creating one means writing a marker object and renaming one means
/// copying everything underneath and deleting the originals. All of that is declared in
/// [`Capabilities`] rather than discovered when a menu item fails.
pub struct S3Provider {
    label: String,
    bucket: String,
    root: String,
    client: Client,
}

/// Anything smaller goes up in one request; anything larger is split into parts. Five megabytes
/// is the smallest part S3 accepts for all but the last one.
const PART_SIZE: usize = 8 * 1024 * 1024;

/// The zero-byte object whose key ends in `/`, which is how every S3 client agrees to write
/// down a folder that has nothing in it yet.
const FOLDER_MARKER: &str = "/";

impl S3Provider {
    pub async fn connect(config: &S3Config, secret: &Secret) -> Result<Self> {
        let (id, key) = secret.key_pair().ok_or_else(|| {
            Error::Authentication("an access key and secret are needed".to_owned())
        })?;

        let region = config.preset.signing_region(&config.region);
        let mut builder = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(region))
            .credentials_provider(Credentials::new(id, key, None, None, "camion"))
            // The SDK's defaults are tuned for a server that can afford to wait. Someone
            // looking at a window cannot: a wrong endpoint should say so in seconds rather
            // than retrying quietly for a minute first.
            .retry_config(RetryConfig::standard().with_max_attempts(2))
            .timeout_config(
                TimeoutConfig::builder()
                    .connect_timeout(Duration::from_secs(5))
                    .operation_attempt_timeout(Duration::from_secs(30))
                    .build(),
            );

        if let Some(endpoint) = config.resolved_endpoint() {
            // Anything that is not Amazon addresses buckets by path rather than by subdomain,
            // and guessing wrong here is the single most common way an S3 client fails to
            // reach a bucket it can otherwise see.
            builder = builder.endpoint_url(endpoint).force_path_style(true);
        }

        let provider = Self {
            label: format!("{} · {}", config.preset.label(), config.bucket),
            bucket: config.bucket.clone(),
            root: normalize_root(&config.root),
            client: Client::from_conf(builder.build()),
        };

        // Building a client talks to nothing, so without this a wrong endpoint or a mistyped
        // key would look like a connection that worked until the first listing came back empty.
        provider.list(&RemotePath::root()).await?;

        Ok(provider)
    }

    /// The key a path corresponds to, with the connection's root prefix in front of it.
    fn key(&self, path: &RemotePath) -> String {
        format!("{}{}", self.root, path.to_key())
    }

    /// The prefix a folder's contents share.
    fn prefix(&self, path: &RemotePath) -> String {
        format!("{}{}", self.root, path.to_prefix())
    }

    /// Everything under a prefix, following the continuation tokens to the end. Used by rename
    /// and delete, which have to see every key rather than the first page of them.
    async fn keys_under(&self, prefix: &str) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        let mut continuation = None;

        loop {
            let page = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(prefix)
                .set_continuation_token(continuation)
                .send()
                .await
                .map_err(|error| failed("could not list the bucket", error))?;

            keys.extend(
                page.contents()
                    .iter()
                    .filter_map(|object| object.key().map(str::to_owned)),
            );

            continuation = page.next_continuation_token().map(str::to_owned);

            if continuation.is_none() {
                return Ok(keys);
            }
        }
    }

    async fn delete_keys(&self, keys: Vec<String>) -> Result<()> {
        // A thousand at a time is the most the batch endpoint accepts.
        for batch in keys.chunks(1000) {
            let objects = batch
                .iter()
                .filter_map(|key| ObjectIdentifier::builder().key(key).build().ok())
                .collect::<Vec<_>>();

            let Ok(delete) = Delete::builder().set_objects(Some(objects)).build() else {
                continue;
            };

            self.client
                .delete_objects()
                .bucket(&self.bucket)
                .delete(delete)
                .send()
                .await
                .map_err(|error| failed("could not delete", error))?;
        }

        Ok(())
    }

    async fn upload_in_parts(&self, key: &str, mut body: ByteStream) -> Result<()> {
        let started = self
            .client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| failed("could not begin the upload", error))?;

        let Some(upload_id) = started.upload_id() else {
            return Err(Error::provider("the server did not accept a multipart upload"));
        };

        let mut parts = Vec::new();
        let mut buffer = BytesMut::with_capacity(PART_SIZE);
        let mut finished = false;

        while !finished {
            match body.next().await {
                Some(chunk) => buffer.extend_from_slice(&chunk?),
                None => finished = true,
            }

            // The last part is allowed to be small; every other one has to reach the minimum,
            // so a part is only sent once there is enough for one or the bytes have run out.
            if buffer.len() >= PART_SIZE || (finished && !buffer.is_empty()) {
                let number = parts.len() as i32 + 1;
                let part = self.upload_part(key, upload_id, number, buffer.split().freeze()).await;

                match part {
                    Ok(part) => parts.push(part),
                    Err(error) => {
                        // Leaving the parts behind would quietly cost money for as long as the
                        // bucket lives, so a failed upload cleans up after itself.
                        let _ = self
                            .client
                            .abort_multipart_upload()
                            .bucket(&self.bucket)
                            .key(key)
                            .upload_id(upload_id)
                            .send()
                            .await;

                        return Err(error);
                    }
                }
            }
        }

        let completed = CompletedMultipartUpload::builder()
            .set_parts(Some(parts))
            .build();

        self.client
            .complete_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .multipart_upload(completed)
            .send()
            .await
            .map_err(|error| failed("could not finish the upload", error))?;

        Ok(())
    }

    async fn upload_part(
        &self,
        key: &str,
        upload_id: &str,
        number: i32,
        bytes: Bytes,
    ) -> Result<CompletedPart> {
        let uploaded = self
            .client
            .upload_part()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .part_number(number)
            .body(AwsStream::from(bytes))
            .send()
            .await
            .map_err(|error| failed(format!("part {number} could not be uploaded"), error))?;

        Ok(CompletedPart::builder()
            .part_number(number)
            .set_e_tag(uploaded.e_tag().map(str::to_owned))
            .build())
    }

    /// Whether anything exists under this prefix, which is the only sense in which a folder
    /// exists on an object store.
    async fn folder_exists(&self, path: &RemotePath) -> Result<bool> {
        if path.is_root() {
            return Ok(true);
        }

        let page = self
            .client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(self.prefix(path))
            .max_keys(1)
            .send()
            .await
            .map_err(|error| failed("could not list the bucket", error))?;

        Ok(page.key_count().unwrap_or_default() > 0 || !page.common_prefixes().is_empty())
    }
}

#[async_trait]
impl Provider for S3Provider {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::object_store()
    }

    async fn list(&self, path: &RemotePath) -> Result<Vec<Entry>> {
        let prefix = self.prefix(path);
        let mut entries = Vec::new();
        let mut continuation = None;

        loop {
            let page = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(&prefix)
                .delimiter("/")
                .set_continuation_token(continuation)
                .send()
                .await
                .map_err(|error| failed("could not list the bucket", error))?;

            for folder in page.common_prefixes() {
                if let Some(name) = folder.prefix().and_then(|full| last_segment(full, &prefix)) {
                    entries.push(Entry::folder(name));
                }
            }

            for object in page.contents() {
                let Some(key) = object.key() else { continue };

                // The marker that makes an empty folder visible is not itself a file.
                if key == prefix {
                    continue;
                }

                if let Some(name) = last_segment(key, &prefix) {
                    let mut entry = Entry::file(name, object.size().unwrap_or_default() as u64);
                    entry.modified = object.last_modified().and_then(timestamp);

                    entries.push(entry);
                }
            }

            continuation = page.next_continuation_token().map(str::to_owned);

            if continuation.is_none() {
                return Ok(entries);
            }
        }
    }

    async fn stat(&self, path: &RemotePath) -> Result<Entry> {
        if path.is_root() {
            return Ok(Entry::folder("/"));
        }

        let head = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(self.key(path))
            .send()
            .await;

        match head {
            Ok(object) => {
                let mut entry =
                    Entry::file(path.name().unwrap_or_default(), object.content_length().unwrap_or_default() as u64);
                entry.modified = object.last_modified().and_then(timestamp);

                Ok(entry)
            }
            // No object by that name may still mean a folder: on an object store that is only
            // ever a statement about what other keys share a prefix.
            Err(_) if self.folder_exists(path).await? => {
                Ok(Entry::folder(path.name().unwrap_or("/")))
            }
            Err(_) => Err(Error::NotFound { path: path.clone() }),
        }
    }

    async fn read(&self, path: &RemotePath, range: Option<ByteRange>) -> Result<ByteStream> {
        let object = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(self.key(path))
            .set_range(range.map(|range| range.to_header()))
            .send()
            .await
            .map_err(|_| Error::NotFound { path: path.clone() })?;

        let size = object.content_length().map(|length| length as u64);
        let chunks = tokio_util::io::ReaderStream::new(object.body.into_async_read()).map(|chunk| {
            chunk.map_err(|error| Error::caused_by("the download was interrupted", error))
        });

        Ok(ByteStream::new(chunks, size))
    }

    async fn write(&self, path: &RemotePath, body: ByteStream) -> Result<()> {
        let key = self.key(path);

        // A small file is one request; anything bigger is split, so a large upload never has to
        // be held in memory all at once.
        match body.size() {
            Some(size) if (size as usize) <= PART_SIZE => {
                let bytes = body.collect().await?;

                self.client
                    .put_object()
                    .bucket(&self.bucket)
                    .key(key)
                    .body(AwsStream::from(bytes))
                    .send()
                    .await
                    .map_err(|error| failed("could not upload", error))?;

                Ok(())
            }
            _ => self.upload_in_parts(&key, body).await,
        }
    }

    async fn delete(&self, path: &RemotePath) -> Result<()> {
        let entry = self.stat(path).await?;

        if entry.kind.is_folder() {
            let keys = self.keys_under(&self.prefix(path)).await?;

            return self.delete_keys(keys).await;
        }

        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(self.key(path))
            .send()
            .await
            .map_err(|error| failed("could not delete", error))?;

        Ok(())
    }

    async fn create_folder(&self, path: &RemotePath) -> Result<()> {
        if self.folder_exists(path).await? {
            return Err(Error::AlreadyExists { path: path.clone() });
        }

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(format!("{}{FOLDER_MARKER}", self.key(path)))
            .body(AwsStream::from_static(b""))
            .send()
            .await
            .map_err(|error| failed("could not create the folder", error))?;

        Ok(())
    }

    /// Copies and then deletes, because S3 has no move.
    ///
    /// For a folder that means every key underneath, one copy each. It works, and it is why
    /// [`Capabilities`] calls renaming emulated rather than native — the interface warns before
    /// doing this to something large.
    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<()> {
        let entry = self.stat(from).await?;

        let moved = if entry.kind.is_folder() {
            self.keys_under(&self.prefix(from)).await?
        } else {
            vec![self.key(from)]
        };

        if moved.is_empty() {
            return Err(Error::NotFound { path: from.clone() });
        }

        let (source, destination) = (self.prefix(from), self.prefix(to));

        for key in &moved {
            let renamed = match entry.kind.is_folder() {
                true => format!("{destination}{}", key.trim_start_matches(&source)),
                false => self.key(to),
            };

            self.client
                .copy_object()
                .bucket(&self.bucket)
                .copy_source(format!("{}/{key}", self.bucket))
                .key(renamed)
                .send()
                .await
                .map_err(|error| failed(format!("could not copy {key}"), error))?;
        }

        self.delete_keys(moved).await
    }
}

/// The part of `key` that comes after `prefix` and before the next separator, which is the name
/// the file list shows.
fn last_segment(key: &str, prefix: &str) -> Option<String> {
    let name = key.strip_prefix(prefix)?.trim_end_matches('/');

    if name.is_empty() {
        None
    } else {
        Some(name.to_owned())
    }
}

/// A connection can be pointed at one folder of a shared bucket rather than the whole thing, so
/// the root is stored the way keys are built: no leading slash, one trailing slash.
fn normalize_root(root: &str) -> String {
    let root = root.trim_matches('/');

    if root.is_empty() {
        String::new()
    } else {
        format!("{root}/")
    }
}

fn timestamp(at: &aws_smithy_types::DateTime) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp(at.secs()).ok()
}

fn failed<E: std::fmt::Debug>(message: impl std::fmt::Display, error: E) -> Error {
    Error::provider(format!("{message}: {error:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_root_prefix_is_stored_the_way_keys_are_built() {
        assert_eq!(normalize_root(""), "");
        assert_eq!(normalize_root("/"), "");
        assert_eq!(normalize_root("site"), "site/");
        assert_eq!(normalize_root("/site/assets/"), "site/assets/");
    }

    #[test]
    fn names_are_what_is_left_after_the_prefix() {
        assert_eq!(last_segment("photos/2026/", "photos/"), Some("2026".to_owned()));
        assert_eq!(last_segment("photos/a.jpg", "photos/"), Some("a.jpg".to_owned()));
        assert_eq!(last_segment("photos/", "photos/"), None);
        assert_eq!(last_segment("elsewhere/a.jpg", "photos/"), None);
    }
}
