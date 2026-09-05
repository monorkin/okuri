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
use futures::{StreamExt, TryStreamExt};
use okuri_core::{
    ByteRange, ByteStream, Capabilities, Entry, Error, Provider, RemotePath, Result, Serve,
    Served, Serving, Sharing, Stored, Storing, Visibility,
};
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
    /// How much memory every part in flight on this connection shares between them.
    budget: crate::parts::Budget,
}

/// Above this an object is split rather than sent or fetched in one request.
///
/// Comfortably above the five megabytes S3 requires of every part but the last, so a file just
/// over the threshold still splits into parts the service will accept.
const PART_SIZE: usize = 8 * 1024 * 1024;

/// What S3 will take of one object, as the service documents it.
///
/// The smallest is what Okuri will use rather than what S3 requires — the requirement is five
/// megabytes, and starting a little above it is what keeps a file just over the threshold legal.
const LIMITS: crate::parts::Limits = crate::parts::Limits {
    smallest: PART_SIZE,
    largest: 5 * 1024 * 1024 * 1024,
    most: 10_000,
};

/// How many objects are copied at once when a folder is renamed.
///
/// A rename here is a copy of every object under a prefix and then a delete. A copy is work the
/// store does entirely on its own, so asking for the next only once the last has answered spends
/// the whole rename on round trips. Bounded so renaming a folder of ten thousand objects does
/// not become ten thousand requests at once.
const COPIES_AT_ONCE: usize = 4;

/// Above this, an object comes down as several ranges asked for at once.
///
/// One GET runs at whatever a single connection manages, and across a long link that is a
/// fraction of what the store would give: the round trip decides how much can be in the air,
/// not the bandwidth. Below this the extra requests cost more than they save.
const IN_PIECES_ABOVE: u64 = 64 * 1024 * 1024;

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
            .credentials_provider(Credentials::new(id, key, None, None, "okuri"))
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
            budget: crate::parts::Budget::new(),
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

    async fn upload_in_parts(&self, key: &str, serve: &Serve, body: ByteStream) -> Result<()> {
        // Chosen from how big the object is. At a fixed eight megabytes a ten gigabyte upload
        // was thirteen hundred requests; a body that will not say how long it is has to keep the
        // smallest part, since a part size cannot be revised once the first one has gone up.
        let part = match body.size() {
            Some(size) => crate::parts::part_size(size, &LIMITS),
            None => LIMITS.smallest,
        };

        let started = self
            .client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .set_content_type(serve.content_type.clone())
            .set_cache_control(serve.cache_control.clone())
            .set_content_encoding(serve.content_encoding.clone())
            .send()
            .await
            .map_err(|error| failed("could not begin the upload", error))?;

        let Some(upload_id) = started.upload_id() else {
            return Err(Error::provider("the server did not accept a multipart upload"));
        };

        // Several parts at once. A part number says where a part belongs, so they need not
        // arrive in order — and waiting for each one before reading the next leaves the link
        // idle for exactly as long as reading takes.
        let sent = crate::parts::each_part(body, part, &self.budget, |index, bytes| {
            self.upload_part(key, upload_id, index as i32 + 1, bytes)
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

    /// One part, handed over as it goes rather than as a block of memory.
    ///
    /// The bytes are all here already — a part has to be of stated length before the store will
    /// take it — so this is not about holding less. It is so the client asks for the next slice
    /// when it has room to send one, which is what lets the progress bar follow the socket.
    ///
    /// The cost is that the SDK cannot replay this body, so its own one retry does not apply:
    /// it notices the body cannot be cloned and gives up after the first attempt. That retry is
    /// done in [`crate::parts::each_part`] instead, which still has the bytes.
    async fn upload_part(
        &self,
        key: &str,
        upload_id: &str,
        number: i32,
        part: ByteStream,
    ) -> Result<CompletedPart> {
        let length = part.size().expect("a part, which is always of stated length");

        let uploaded = self
            .client
            .upload_part()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .part_number(number)
            .content_length(length as i64)
            .body(AwsStream::from_body_1_x(crate::body::Sending::new(part, length)))
            .send()
            .await
            .map_err(|error| failed(format!("part {number} could not be uploaded"), error))?;

        Ok(CompletedPart::builder()
            .part_number(number)
            .set_e_tag(uploaded.e_tag().map(str::to_owned))
            .build())
    }

    /// How long the object is, asked for with a request that carries no body.
    ///
    /// Nothing when the store will not say. This is only deciding whether an object is worth
    /// splitting into pieces, so a store that refuses the question is not an error here — the
    /// download goes ahead as one request and reports for itself whatever is actually wrong.
    async fn length_of(&self, path: &RemotePath) -> Option<u64> {
        let head = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(self.key(path))
            .send()
            .await
            .ok()?;

        head.content_length().and_then(|length| u64::try_from(length).ok())
    }

    /// A large object, asked for as several ranges at once.
    ///
    /// The first piece is asked for rather than probed for. Its response says what the object is
    /// and how long it really is — everything a HEAD would have said — and hands over a piece of
    /// the object for the same round trip, so nothing is requested and thrown away.
    ///
    /// What the store says about the length beats what the caller was told. A size that has gone
    /// stale would otherwise cut the download off at the length of a file that no longer exists.
    async fn read_in_pieces(&self, path: &RemotePath, size: u64) -> Result<ByteStream> {
        use futures::StreamExt as _;

        // The same size a part of it would go up in, for the same reason: a request is worth
        // making for a piece this size and not for a smaller one.
        let piece = crate::parts::part_size(size, &LIMITS);
        let first = self.piece(path, ByteRange::new(0, (piece as u64).min(size))).await?;

        let size = first.total.unwrap_or(size);
        let serve = first.serve;
        let head = first.bytes;

        let client = self.client.clone();
        let bucket = self.bucket.clone();
        let key = self.key(path);
        let named = path.clone();

        let rest = crate::parts::in_pieces(
            head.len() as u64,
            size,
            piece,
            self.budget.clone(),
            move |range| {
                let asking =
                    piece_of(client.clone(), bucket.clone(), key.clone(), named.clone(), range);

                async move { asking.await.map(|piece| piece.bytes) }
            },
        );

        let chunks = futures::stream::once(async move { Ok(head) }).chain(rest);

        Ok(ByteStream::new(chunks, Some(size)).served_as(serve))
    }

    async fn piece(&self, path: &RemotePath, range: ByteRange) -> Result<Piece> {
        piece_of(
            self.client.clone(),
            self.bucket.clone(),
            self.key(path),
            path.clone(),
            range,
        )
        .await
    }

    /// One object copied to another key.
    async fn copy(&self, key: String, renamed: String) -> Result<()> {
        self.client
            .copy_object()
            .bucket(&self.bucket)
            .copy_source(encoded_source(&self.bucket, &key))
            .key(renamed)
            .send()
            .await
            .map_err(|error| failed(format!("could not copy {key}"), error))?;

        Ok(())
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
        self.read_sized(path, range, None).await
    }

    async fn read_sized(
        &self,
        path: &RemotePath,
        range: Option<ByteRange>,
        size: Option<u64>,
    ) -> Result<ByteStream> {
        // Something large is worth asking for in pieces, several at a time: one GET runs at
        // whatever a single connection manages, and across a long link that is decided by the
        // round trip rather than by the bandwidth.
        //
        // Only when the whole object was asked for. A caller that named a range is resuming or
        // reading a header, and splitting that up again would be answering a different question.
        if range.is_none() {
            // Told, where the transfer was planned from a listing that already said. Asked for
            // with a HEAD otherwise, which is a request with no body — where this used to send
            // a GET, keep its headers, and throw its body away.
            let length = match size {
                Some(size) => Some(size),
                None => self.length_of(path).await,
            };

            if let Some(size) = length
                && size > IN_PIECES_ABOVE
            {
                return self.read_in_pieces(path, size).await;
            }
        }

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

        // What the store says this is, taken from the answer already in hand. Asking again
        // afterwards would be a second round trip for something this response is carrying, and
        // it is what stops a file copied somewhere else arriving as an octet stream.
        let serve = Serve {
            content_type: object.content_type().map(str::to_owned),
            cache_control: object.cache_control().map(str::to_owned),
            content_encoding: object.content_encoding().map(str::to_owned),
        };

        // Taken off the response body as it arrives. Turning it into a reader and back through
        // a `ReaderStream` would copy every chunk the connection handed over into a fresh
        // four-kilobyte buffer, so a megabyte off the wire becomes two hundred and fifty copies
        // and two hundred and fifty chunks for everything downstream to move one at a time.
        let chunks = futures::stream::unfold(object.body, |mut body| async move {
            let chunk = body
                .next()
                .await?
                .map_err(|error| Error::caused_by("the download was interrupted", error));

            Some((chunk, body))
        });

        Ok(ByteStream::new(chunks, size).served_as(serve))
    }

    async fn write(&self, path: &RemotePath, body: ByteStream) -> Result<()> {
        let key = self.key(path);

        // Said at upload time, because it cannot be said later without rewriting the object —
        // and a store told nothing answers `application/octet-stream` to everybody, which is
        // the difference between a browser showing an image and downloading it.
        //
        // What the source said comes first and the name is the fallback. A file copied from
        // another store already knows what it is, and plenty of files worth serving have no
        // extension to guess from.
        let serve = body.serve().or_guessed_from(path.name());

        // A small file is one request; anything bigger is split, so a large upload never has to
        // be held in memory all at once.
        match body.size() {
            Some(size) if size <= PART_SIZE as u64 => {
                // Handed over rather than read first. Collecting it would finish the read
                // before the request had been made, which is what made the progress bar fill
                // up seconds before anything reached the server.
                //
                // The length is stated because the store will not take a body without one, and
                // it is the reason this can be streamed at all: it came with the file.
                //
                // A stream cannot be replayed, so the SDK's one retry does not apply here — it
                // notices the body cannot be cloned and gives up after the first attempt. That
                // is the price of an honest progress bar on a file small enough to have gone up
                // in one request.
                self.client
                    .put_object()
                    .bucket(&self.bucket)
                    .key(key)
                    .set_content_type(serve.content_type.clone())
                    .set_cache_control(serve.cache_control.clone())
                    .set_content_encoding(serve.content_encoding.clone())
                    .content_length(size as i64)
                    .body(AwsStream::from_body_1_x(crate::body::Sending::new(body, size)))
                    .send()
                    .await
                    .map_err(|error| failed("could not upload", error))?;

                Ok(())
            }
            _ => self.upload_in_parts(&key, &serve, body).await,
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

        // Several at once, because a copy is work the store does on its own and the whole rename
        // is otherwise one round trip per object. And every one of them before anything is
        // deleted — this is the only ordering here that matters, since a delete that overtakes a
        // copy takes away the only copy there was.
        let mut copying = Vec::with_capacity(moved.len());

        for key in &moved {
            let renamed = match entry.kind.is_folder() {
                true => rebase(key, &source, &destination),
                false => self.key(to),
            };

            copying.push(self.copy(key.clone(), renamed));
        }

        futures::stream::iter(copying)
            .buffer_unordered(COPIES_AT_ONCE)
            .try_collect::<Vec<()>>()
            .await?;

        self.delete_keys(moved).await
    }
}

/// One range of an object, and what the response said about the object it came from.
struct Piece {
    bytes: Bytes,
    serve: Serve,

    /// How long the whole object is. A ranged response states it even while handing over only
    /// part of the object, which is what makes the first piece worth asking for before anything
    /// else — it is the answer a HEAD would have given, with a piece of the object attached.
    ///
    /// Nothing when the store answers `*`, which it is allowed to do.
    total: Option<u64>,
}

async fn piece_of(
    client: Client,
    bucket: String,
    key: String,
    path: RemotePath,
    range: ByteRange,
) -> Result<Piece> {
    let response = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .range(range.to_header())
        .send()
        .await
        .map_err(|error| missing_or_refused(error, &path))?;

    let serve = Serve {
        content_type: response.content_type().map(str::to_owned),
        cache_control: response.cache_control().map(str::to_owned),
        content_encoding: response.content_encoding().map(str::to_owned),
    };

    let total = response.content_range().and_then(crate::parts::whole_length);

    let bytes = response
        .body
        .collect()
        .await
        .map(|collected| collected.into_bytes())
        .map_err(|error| Error::caused_by("the download was interrupted", error))?;

    let asked = range.length.expect("a length, since every piece is asked for by one");

    // What was asked for is what has to arrive, unless the store says the object ends first —
    // which it says in the same header that gives the length, so a file shorter than it was
    // listed as still comes down whole. Short for any other reason is a hole in the middle of
    // the file, and a file with a hole in it is worse than a download that failed.
    let due = match total {
        Some(total) => asked.min(total.saturating_sub(range.offset)),
        None => asked,
    };

    if bytes.len() as u64 != due {
        return Err(Error::provider(format!(
            "{path} gave back {} bytes where {due} were asked for",
            bytes.len()
        )));
    }

    Ok(Piece { bytes, serve, total })
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
