use async_trait::async_trait;
use std::time::Duration;

use aws_sdk_s3::config::retry::RetryConfig;
use aws_sdk_s3::config::timeout::TimeoutConfig;
use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::ByteStream as AwsStream;
use aws_sdk_s3::types::{
    CompletedMultipartUpload, CompletedPart, Delete, ObjectCannedAcl, ObjectIdentifier, Permission,
};
use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::Client;
use bytes::Bytes;
use camion_core::{
    media_type, ByteRange, ByteStream, Capabilities, Entry, Error, Provider, RemotePath, Result,
    Served, Serving, Sharing, Stored, Storing, Visibility,
};
use futures::StreamExt;
use time::OffsetDateTime;

use crate::destination::S3 as S3Config;
use crate::keys::{last_segment, normalize_root, rebase};
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
    /// Where the bucket answers, for writing down an address somebody else can open. Kept as
    /// the connection resolved it, because a preset and a self-hosted store disagree about
    /// what the host even is.
    endpoint: String,
}

/// Anything smaller goes up in one request; anything larger is split into parts of this size.
///
/// Comfortably above the five megabytes S3 requires of every part but the last, so a file just
/// over the threshold still splits into parts the service will accept.
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
        let region_for_urls = region.clone();
        let mut builder = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(region))
            .credentials_provider(Credentials::new(id, key, None, None, "camion"))
            // The SDK's defaults are tuned for a server that can afford to wait. Someone
            // looking at a window cannot: a wrong endpoint should say so in seconds rather
            // than retrying quietly for a minute first.
            .retry_config(RetryConfig::standard().with_max_attempts(2))
            // Only the connecting is given a deadline. A deadline on the whole attempt would
            // apply to uploads too, and an eight-megabyte part cannot cross a slow domestic
            // uplink in thirty seconds — every part would time out, retry, and fail. What that
            // deadline was for is a wrong endpoint saying so quickly, which is what a connect
            // timeout is.
            .timeout_config(
                TimeoutConfig::builder()
                    .connect_timeout(Duration::from_secs(5))
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
            endpoint: config
                .resolved_endpoint()
                .unwrap_or_else(|| format!("https://s3.{}.amazonaws.com", region_for_urls)),
            client: Client::from_conf(builder.build()),
        };

        // Building a client talks to nothing, so without this a wrong endpoint or a mistyped
        // key would look like a connection that worked until the first listing came back empty.
        provider.list(&RemotePath::root()).await?;

        Ok(provider)
    }

    /// One `HEAD`, which is where every answer about a single object comes from.
    async fn head(
        &self,
        path: &RemotePath,
    ) -> Result<aws_sdk_s3::operation::head_object::HeadObjectOutput> {
        self.client
            .head_object()
            .bucket(&self.bucket)
            .key(self.key(path))
            .send()
            .await
            .map_err(|error| missing_or_refused(error, path))
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
                .map(|key| {
                    ObjectIdentifier::builder()
                        .key(key)
                        .build()
                        .map_err(|error| Error::caused_by(format!("could not name {key}"), error))
                })
                .collect::<Result<Vec<_>>>()?;

            let delete = Delete::builder()
                .set_objects(Some(objects))
                .build()
                .map_err(|error| Error::caused_by("could not list what to delete", error))?;

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

    async fn upload_in_parts(
        &self,
        key: &str,
        media: Option<&str>,
        mut body: ByteStream,
    ) -> Result<()> {
        let started = self
            .client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .set_content_type(media.map(str::to_owned))
            .send()
            .await
            .map_err(|error| failed("could not begin the upload", error))?;

        let Some(upload_id) = started.upload_id() else {
            return Err(Error::provider("the server did not accept a multipart upload"));
        };

        // Several parts at once. A part number says where a part belongs, so they need not
        // arrive in order — and waiting for each one before reading the next leaves the link
        // idle for exactly as long as reading takes.
        let sent = crate::parts::each_part(&mut body, PART_SIZE, |index, bytes| {
            self.upload_part(key, upload_id, index as i32 + 1, bytes.into())
        })
        .await;

        let parts = match sent {
            Ok(parts) => parts,
            Err(error) => {
                // Leaving the parts behind would quietly cost money for as long as the bucket
                // lives, so a failed upload cleans up after itself.
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
        };

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
impl Serving for S3Provider {
    async fn served(&self, path: &RemotePath) -> Result<Served> {
        let head = self.head(path).await?;

        Ok(Served {
            content_type: head.content_type().map(str::to_owned),
            // The quotes are the protocol's, not part of the value.
            etag: head.e_tag().map(|tag| tag.trim_matches('"').to_owned()),
            cache_control: head.cache_control().map(str::to_owned),
            content_encoding: head.content_encoding().map(str::to_owned),
        })
    }
}

#[async_trait]
impl Storing for S3Provider {
    async fn stored(&self, path: &RemotePath) -> Result<Stored> {
        let head = self.head(path).await?;

        Ok(Stored {
            // A store names the class only when it is not the ordinary one — the protocol says
            // so, and both R2 and MinIO leave it out entirely. Saying nothing would show no row
            // at all for a file that plainly is stored somehow.
            class: Some(
                head.storage_class()
                    .map(|class| class.as_str().to_owned())
                    .unwrap_or_else(|| "STANDARD".to_owned()),
            ),
            encryption: head
                .server_side_encryption()
                .map(|kind| kind.as_str().to_owned()),
            version: head.version_id().map(str::to_owned),
        })
    }
}

#[async_trait]
impl Sharing for S3Provider {
    /// Reads the object's access control list and looks for the grant that means "anyone".
    ///
    /// Buckets can also be opened up by a bucket policy, which this cannot see — a file may
    /// therefore read as private and still be reachable. Reporting what we can actually check
    /// is better than guessing, and the address below is there to be tried either way.
    async fn visibility(&self, path: &RemotePath) -> Result<Visibility> {
        let acl = self
            .client
            .get_object_acl()
            .bucket(&self.bucket)
            .key(self.key(path))
            .send()
            .await
            .map_err(|error| missing_or_refused(error, path))?;

        let public = acl.grants().iter().any(|grant| {
            let everyone = grant
                .grantee()
                .and_then(|grantee| grantee.uri())
                .is_some_and(|uri| uri.ends_with("/groups/global/AllUsers"));

            everyone
                && matches!(
                    grant.permission(),
                    Some(Permission::Read) | Some(Permission::FullControl)
                )
        });

        match public {
            true => Ok(Visibility::Public),
            false => Ok(Visibility::Private),
        }
    }

    async fn set_visibility(&self, path: &RemotePath, visibility: Visibility) -> Result<()> {
        let acl = match visibility {
            Visibility::Public => ObjectCannedAcl::PublicRead,
            Visibility::Private => ObjectCannedAcl::Private,
        };

        self.client
            .put_object_acl()
            .bucket(&self.bucket)
            .key(self.key(path))
            .acl(acl)
            .send()
            .await
            .map_err(|error| refused_to_share(error, path))?;

        Ok(())
    }

    async fn temporary_url(&self, path: &RemotePath, valid_for: Duration) -> Result<String> {
        let signing = PresigningConfig::expires_in(valid_for)
            .map_err(|error| Error::caused_by("that is too long to sign a link for", error))?;

        let signed = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(self.key(path))
            .presigned(signing)
            .await
            .map_err(|error| failed("could not sign a link", error))?;

        Ok(signed.uri().to_owned())
    }

    /// Built rather than asked for: the address of an object is its endpoint, its bucket and
    /// its key, and every one of those is already known here.
    fn public_url(&self, path: &RemotePath) -> String {
        const IN_KEY: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
            .remove(b'-')
            .remove(b'.')
            .remove(b'_')
            .remove(b'~')
            .remove(b'/');

        let key = self.key(path);
        let key = percent_encoding::utf8_percent_encode(&key, IN_KEY);

        format!("{}/{}/{key}", self.endpoint.trim_end_matches('/'), self.bucket)
    }
}

/// A store that will not talk about access control at all, said in words that name the cause.
///
/// Buckets created since 2023 have object ACLs turned off by default, and Cloudflare R2 never
/// had them: on both, the answer is a bucket-wide setting rather than anything about this file.
/// Without this the failure reads as a permissions problem with the file itself.
fn refused_to_share<E: ProvideErrorMetadata>(error: E, path: &RemotePath) -> Error {
    match error.code() {
        Some("AccessControlListNotSupported" | "NotImplemented" | "InvalidRequest") => {
            Error::provider(format!(
                "{path} cannot be shared per file: this store decides who can read a bucket, \
                 not who can read the files in it"
            ))
        }
        _ => failed("could not change who can read this", error),
    }
}

#[async_trait]
impl Provider for S3Provider {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn serving(&self) -> Option<&dyn Serving> {
        Some(self)
    }

    fn storing(&self) -> Option<&dyn Storing> {
        Some(self)
    }

    fn sharing(&self) -> Option<&dyn Sharing> {
        Some(self)
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
            .map_err(|error| missing_or_refused(error, path))?;

        let size = object.content_length().map(|length| length as u64);
        let chunks = tokio_util::io::ReaderStream::new(object.body.into_async_read()).map(|chunk| {
            chunk.map_err(|error| Error::caused_by("the download was interrupted", error))
        });

        Ok(ByteStream::new(chunks, size))
    }

    async fn write(&self, path: &RemotePath, body: ByteStream) -> Result<()> {
        let key = self.key(path);

        // Said at upload time, because it cannot be said later without rewriting the object —
        // and a store told nothing answers `application/octet-stream` to everybody, which is
        // the difference between a browser showing an image and downloading it.
        let media = path.name().and_then(media_type);

        // A small file is one request; anything bigger is split, so a large upload never has to
        // be held in memory all at once.
        match body.size() {
            Some(size) if size <= PART_SIZE as u64 => {
                let bytes = body.collect().await?;

                self.client
                    .put_object()
                    .bucket(&self.bucket)
                    .key(key)
                    .set_content_type(media.map(str::to_owned))
                    .body(AwsStream::from(bytes))
                    .send()
                    .await
                    .map_err(|error| failed("could not upload", error))?;

                Ok(())
            }
            _ => self.upload_in_parts(&key, media, body).await,
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
                true => rebase(key, &source, &destination),
                false => self.key(to),
            };

            self.client
                .copy_object()
                .bucket(&self.bucket)
                .copy_source(encoded_source(&self.bucket, key))
                .key(renamed)
                .send()
                .await
                .map_err(|error| failed(format!("could not copy {key}"), error))?;
        }

        self.delete_keys(moved).await
    }
}

/// The `bucket/key` a copy reads from, with the characters a URL cannot carry escaped. A key
/// is free to contain a space or a `#`; a copy source is a URL and is not.
fn encoded_source(bucket: &str, key: &str) -> String {
    /// Everything but the characters a URL may carry as they are.
    const RESERVED: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~');

    let escaped = key
        .split('/')
        .map(|segment| percent_encoding::utf8_percent_encode(segment, RESERVED).to_string())
        .collect::<Vec<_>>()
        .join("/");

    format!("{bucket}/{escaped}")
}

/// Whether the object is absent, or something else went wrong.
///
/// Reporting a refused key or a dropped connection as "does not exist" is worse than unhelpful:
/// [`Error::NotFound`] is not transient, so the transfer queue will not retry what may only
/// have been a blip.
fn missing_or_refused<E: ProvideErrorMetadata>(error: E, path: &RemotePath) -> Error {
    let absent = matches!(
        error.code(),
        Some("NoSuchKey" | "NoSuchBucket" | "NotFound" | "404")
    );

    if absent {
        Error::NotFound { path: path.clone() }
    } else {
        failed(format!("{path} could not be read"), error)
    }
}

fn timestamp(at: &aws_smithy_types::DateTime) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp(at.secs()).ok()
}

/// What went wrong, in words.
///
/// The SDK's own `Debug` is four lines of nested structs with the sentence buried in the
/// middle, and printing it puts that on screen where a person is meant to read it. Every S3
/// error carries a code and usually a message; between them and the table below there is always
/// something better to say.
fn failed<E: ProvideErrorMetadata>(message: impl std::fmt::Display, error: E) -> Error {
    Error::provider(format!("{message}: {}", explain(&error)))
}

fn explain<E: ProvideErrorMetadata>(error: &E) -> String {
    if let Some(said) = plainly(error.code()) {
        return said.to_owned();
    }

    match (error.message(), error.code()) {
        (Some(message), _) => message.to_owned(),
        (None, Some(code)) => format!("the server said {code}"),
        (None, None) => "the server did not say why".to_owned(),
    }
}

/// The handful of answers worth rewording, because the server's own wording assumes you know
/// how S3 is put together.
fn plainly(code: Option<&str>) -> Option<&'static str> {
    match code? {
        "AccessDenied" => Some("these credentials are not allowed to do that"),
        "InvalidAccessKeyId" => Some("that access key is not one this store knows"),
        "SignatureDoesNotMatch" => Some("the secret key does not match the access key"),
        "NoSuchBucket" => Some("there is no bucket by that name"),
        "NoSuchKey" | "NotFound" => Some("that file is not there"),
        "BucketAlreadyOwnedByYou" => Some("that bucket is already yours"),
        "EntityTooLarge" => Some("that file is larger than this store will take"),
        "RequestTimeTooSkewed" => Some("this machine's clock is too far from the server's"),
        "SlowDown" | "ServiceUnavailable" => Some("the store is busy — try again shortly"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_source_escapes_what_a_url_cannot_carry() {
        assert_eq!(
            encoded_source("assets", "photos/last summer.jpg"),
            "assets/photos/last%20summer.jpg"
        );
        assert_eq!(encoded_source("assets", "a#b/c+d"), "assets/a%23b/c%2Bd");
    }
}
