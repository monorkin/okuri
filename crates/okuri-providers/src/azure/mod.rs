mod signing;

use async_trait::async_trait;
use okuri_core::{
    ByteRange, ByteStream, Capabilities, Entry, Error, Provider, RemotePath, Result, Serve,
    Served, Serving, Stored, Storing,
};
use futures::{StreamExt, TryStreamExt};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use quick_xml::events::Event as XmlEvent;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_LENGTH, RANGE};
use reqwest::{Client, Method, StatusCode};
use time::OffsetDateTime;

use crate::destination::Azure as AzureConfig;
use crate::keys::{last_segment, normalize_root, parse_http_date, rebase};
use crate::secret::Secret;

/// Azure Blob Storage.
///
/// Another flat keyspace with folders drawn on top, so it behaves like S3 and declares the same
/// capabilities. Requests are signed here rather than by the official SDK, which only accepts
/// token credentials — the account key from the portal is what people actually have.
pub struct AzureProvider {
    label: String,
    container: String,
    root: String,
    requests: Requests,
    /// How much memory every block in flight on this connection shares between them.
    budget: crate::parts::Budget,
}

/// Everything one signed request needs, kept together so it can be handed to work that outlives
/// the call which started it — which every piece of a parallel download is.
///
/// Cheap to clone: the client is a handle to a shared pool and the rest is a few short strings.
#[derive(Clone)]
struct Requests {
    account: String,
    key: String,
    endpoint: String,
    /// The path part of the endpoint, if it has one. The emulator addresses accounts by path
    /// rather than by hostname, and the signature covers the request path as actually sent.
    endpoint_path: String,
    client: Client,
}

/// The API version whose behaviour this adapter was written against.
const VERSION: &str = "2021-12-02";

/// Above this a blob is staged in blocks rather than sent or fetched in one request.
const BLOCK_SIZE: usize = 8 * 1024 * 1024;

/// Above this, a blob comes down as several ranges asked for at once.
///
/// Below it the extra requests cost more than they save.
const IN_PIECES_ABOVE: u64 = 64 * 1024 * 1024;

/// How many blobs are copied at once when a folder is renamed.
///
/// A rename is a copy of every blob under a prefix and then a delete. A copy is work the service
/// does entirely on its own, so asking for the next only once the last has answered spends the
/// whole rename on round trips. Bounded so renaming a folder of ten thousand blobs does not
/// become ten thousand requests at once.
const COPIES_AT_ONCE: usize = 4;

/// What the Blob service will take of one blob under [`VERSION`].
///
/// Four thousand megabytes a block and fifty thousand blocks, both far beyond anything the memory
/// budget would let through — they are written down because the arithmetic that picks a block
/// size needs a ceiling, not because either is ever the binding one here.
const LIMITS: crate::parts::Limits = crate::parts::Limits {
    smallest: BLOCK_SIZE,
    largest: 4000 * 1024 * 1024,
    most: 50_000,
};

const IN_PATH: &AsciiSet = &CONTROLS.add(b' ').add(b'"').add(b'<').add(b'>').add(b'#').add(b'?');

/// A query value may contain anything, and a block id is base64 — which includes `+` and `=`.
const IN_QUERY: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'#')
    .add(b'?')
    .add(b'&')
    .add(b'=')
    .add(b'+')
    .add(b'/');

impl Requests {
    /// Signs and sends one request whose body is already in hand.
    async fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(String, String)],
        headers: HeaderMap,
        body: impl Into<bytes::Bytes>,
    ) -> Result<reqwest::Response> {
        let body = body.into();
        let length = body.len();

        self.dispatch(method, path, query, headers, length, body.into()).await
    }

    /// Signs and sends one request whose body is still being read.
    ///
    /// The length is passed rather than measured, because measuring it would mean reading the
    /// whole file first — and a file read before the request is made is a file the transfer
    /// queue has already counted as sent while none of it has left the machine.
    async fn send_stream(
        &self,
        method: Method,
        path: &str,
        query: &[(String, String)],
        headers: HeaderMap,
        body: ByteStream,
        length: u64,
    ) -> Result<reqwest::Response> {
        self.dispatch(
            method,
            path,
            query,
            headers,
            length as usize,
            reqwest::Body::wrap_stream(body),
        )
        .await
    }

    /// Signs and sends one request.
    ///
    /// Every call goes through here so that nothing can be sent unsigned by accident, and so
    /// the date and version headers are never forgotten.
    async fn dispatch(
        &self,
        method: Method,
        path: &str,
        query: &[(String, String)],
        mut headers: HeaderMap,
        length: usize,
        body: reqwest::Body,
    ) -> Result<reqwest::Response> {
        // Azure wants `Tue, 26 Aug 2026 10:00:00 GMT`, which is RFC 2822 with the offset
        // spelled the way HTTP spells it rather than as `+0000`.
        let now = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc2822)
            .map_err(|error| Error::caused_by("could not stamp the request", error))?
            .replace("+0000", "GMT");

        headers.insert("x-ms-date", header_value(&now)?);
        headers.insert("x-ms-version", header_value(VERSION)?);

        let authorization = signing::authorization(
            &self.account,
            &self.key,
            method.as_str(),
            &format!("{}{path}", self.endpoint_path),
            query,
            &headers,
            length,
        )
        .ok_or_else(|| Error::Authentication("the account key is not valid base64".to_owned()))?;

        headers.insert(AUTHORIZATION, header_value(&authorization)?);

        // Always stated, even when it is zero: the service routes a zero-length PUT by the
        // presence of the header, and a folder marker is exactly that request. A streamed body
        // needs it said for a second reason — without it the request goes up chunked, and the
        // length is what Shared Key signed.
        headers.insert(CONTENT_LENGTH, header_value(&length.to_string())?);

        self.client
            .request(method, format!("{}{path}{}", self.endpoint, query_string(query)))
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(|error| Error::caused_by("the request failed", error))
    }
}

impl AzureProvider {
    pub async fn connect(config: &AzureConfig, secret: &Secret) -> Result<Self> {
        let key = secret
            .password()
            .ok_or_else(|| Error::Authentication("an account key is needed".to_owned()))?;

        let provider = Self {
            label: format!("{}/{}", config.account, config.container),
            container: config.container.clone(),
            root: normalize_root(&config.root),
            requests: Requests {
                account: config.account.clone(),
                key: key.to_owned(),
                endpoint_path: path_of(&config.resolved_endpoint()),
                endpoint: config.resolved_endpoint(),
                client: Client::builder()
                    .build()
                    .map_err(|error| Error::caused_by("could not start an HTTP client", error))?,
            },
            budget: crate::parts::Budget::new(),
        };

        // Nothing has been proven until something is asked for, so a wrong key fails here
        // rather than looking like an empty container.
        provider.list(&RemotePath::root()).await?;

        Ok(provider)
    }

    fn blob_name(&self, path: &RemotePath) -> String {
        format!("{}{}", self.root, path.to_key())
    }

    fn prefix(&self, path: &RemotePath) -> String {
        format!("{}{}", self.root, path.to_prefix())
    }

    /// How long the blob is, asked for with a request that carries no body.
    ///
    /// Nothing when the service will not say. This is only deciding whether a blob is worth
    /// splitting into pieces, so a refusal here is not an error — the download goes ahead as one
    /// request and reports for itself whatever is actually wrong.
    async fn length_of(&self, path: &RemotePath) -> Option<u64> {
        let response = self
            .requests
            .send(Method::HEAD, &self.blob_path(path), &[], HeaderMap::new(), Vec::new())
            .await
            .ok()?;

        if !response.status().is_success() {
            return None;
        }

        response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok())
    }

    /// A large blob, asked for as several ranges at once.
    ///
    /// The first piece is asked for rather than probed for: its response says what the blob is
    /// and how long it really is, and hands over a piece of it for the same round trip. What the
    /// service says about the length beats what the caller was told, so a size that has gone
    /// stale cannot cut the download off at the length of a blob that no longer exists.
    async fn read_in_pieces(&self, path: &RemotePath, size: u64) -> Result<ByteStream> {
        let piece = crate::parts::part_size(size, &LIMITS);
        let blob = self.blob_path(path);

        let first = piece_of(
            self.requests.clone(),
            blob.clone(),
            path.clone(),
            ByteRange::new(0, (piece as u64).min(size)),
        )
        .await?;

        let size = first.total.unwrap_or(size);
        let serve = first.serve;
        let head = first.bytes;

        let requests = self.requests.clone();
        let named = path.clone();

        let rest = crate::parts::in_pieces(
            head.len() as u64,
            size,
            piece,
            self.budget.clone(),
            move |range| {
                let asking = piece_of(requests.clone(), blob.clone(), named.clone(), range);

                async move { asking.await.map(|piece| piece.bytes) }
            },
        );

        let chunks = futures::stream::once(async move { Ok(head) }).chain(rest);

        Ok(ByteStream::new(chunks, Some(size)).served_as(serve))
    }

    /// One blob copied to another name, waited for until the service says it is really there.
    async fn copy(&self, name: String, renamed: String, from: &RemotePath) -> Result<()> {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-ms-copy-source",
            header_value(&format!("{}{}", self.requests.endpoint, self.path_for_name(&name)))?,
        );

        let response = self
            .requests
            .send(Method::PUT, &self.path_for_name(&renamed), &[], headers, Vec::new())
            .await?;

        if !response.status().is_success() {
            return Err(refused(response.status(), from, "copy"));
        }

        // Copy Blob answers `202 Accepted` and goes on copying afterwards. Deleting the source
        // on the strength of that would take the file away mid-copy, so the answer has to say
        // the copy is already done.
        let finished = response
            .headers()
            .get("x-ms-copy-status")
            .and_then(|status| status.to_str().ok())
            .is_none_or(|status| status == "success");

        if finished {
            Ok(())
        } else {
            Err(Error::provider(format!(
                "{name} is still being copied; nothing has been removed"
            )))
        }
    }

    fn container_path(&self) -> String {
        format!("/{}", self.container)
    }

    fn blob_path(&self, path: &RemotePath) -> String {
        self.path_for_name(&self.blob_name(path))
    }

    /// The URL path for a blob named exactly `name`.
    ///
    /// Takes the raw name rather than a [`RemotePath`], because a folder marker is a blob whose
    /// name ends in `/` and a path cannot hold that — routing one through a path silently drops
    /// the slash and addresses a blob that does not exist.
    fn path_for_name(&self, name: &str) -> String {
        let encoded = name
            .split('/')
            .map(|segment| utf8_percent_encode(segment, IN_PATH).to_string())
            .collect::<Vec<_>>()
            .join("/");

        format!("/{}/{encoded}", self.container)
    }

    /// Everything under a prefix, following the markers to the end.
    ///
    /// A container answers at most five thousand blobs at a time and names where to resume.
    /// Stopping at the first answer would make a large folder look complete when it is not —
    /// and deleting one would report success having removed a fraction of it.
    async fn list_blobs(&self, prefix: &str, delimited: bool) -> Result<Listing> {
        let mut listing = Listing::default();
        let mut resume: Option<String> = None;

        loop {
            let mut query = vec![
                ("restype".to_owned(), "container".to_owned()),
                ("comp".to_owned(), "list".to_owned()),
                ("prefix".to_owned(), prefix.to_owned()),
            ];

            if delimited {
                query.push(("delimiter".to_owned(), "/".to_owned()));
            }

            if let Some(marker) = &resume {
                query.push(("marker".to_owned(), marker.clone()));
            }

            let response = self
                .requests
            .send(Method::GET, &self.container_path(), &query, HeaderMap::new(), Vec::new())
                .await?;

            let status = response.status();

            if !status.is_success() {
                return Err(refused(status, &RemotePath::root(), "list the container"));
            }

            let body = response
                .text()
                .await
                .map_err(|error| Error::caused_by("the listing could not be read", error))?;

            let page = parse_listing(&body)?;

            resume = page.next.clone();
            listing.blobs.extend(page.blobs);
            listing.folders.extend(page.folders);

            if resume.is_none() {
                return Ok(listing);
            }
        }
    }

    async fn upload_in_blocks(&self, path: &RemotePath, body: ByteStream) -> Result<()> {
        use base64::Engine as _;

        // Taken before the body is read, because reading it is what consumes it.
        let serve = body.serve().clone();

        // Chosen from how big the blob is, so a large one is not staged eight megabytes at a
        // time. A body that will not say how long it is keeps the smallest block, since the
        // size cannot be revised once the first block has gone up.
        let block_size = match body.size() {
            Some(size) => crate::parts::part_size(size, &LIMITS),
            None => LIMITS.smallest,
        };

        let encoding = base64::engine::general_purpose::STANDARD;
        // Several blocks at once: the block list sent at the end is what puts them in order,
        // so they need not go up in order. Sending them one after another leaves the link idle
        // for as long as reading the next block off disk takes.
        let blocks = crate::parts::each_part(body, block_size, &self.budget, |index, block| {
            // Block ids have to be the same length for every block in one blob, so they are a
            // fixed-width number rather than anything more descriptive.
            let id = encoding.encode(format!("okuri-{index:08}"));

            async {
                let id = id;

                // Handed over as it goes rather than as a block of memory. The bytes are all
                // here already — Shared Key signs the length, so a block has to be of stated
                // size — but sending it a slice at a time is what lets the progress bar follow
                // the socket instead of jumping when the request is answered.
                let length = block.size().expect("a block, which is always of stated length");

                let response = self
                    .requests
                    .send_stream(
                        Method::PUT,
                        &self.blob_path(path),
                        &[
                            ("comp".to_owned(), "block".to_owned()),
                            ("blockid".to_owned(), id.clone()),
                        ],
                        HeaderMap::new(),
                        block,
                        length,
                    )
                    .await?;

                match response.status().is_success() {
                    true => Ok(id),
                    false => Err(refused(response.status(), path, "upload")),
                }
            }
        })
        .await?;

        let list = blocks
            .iter()
            .map(|id| format!("<Latest>{id}</Latest>"))
            .collect::<String>();

        // Set here rather than on the blocks: the block list is what makes the blob, and it is
        // the only request in a multipart upload that carries the blob's own headers.
        let mut headers = HeaderMap::new();
        self.say_what_it_is(path, &serve, &mut headers)?;

        let response = self
            .requests
            .send(
                Method::PUT,
                &self.blob_path(path),
                &[("comp".to_owned(), "blocklist".to_owned())],
                headers,
                format!(
                    r#"<?xml version="1.0" encoding="utf-8"?><BlockList>{list}</BlockList>"#
                )
                .into_bytes(),
            )
            .await?;

        match response.status().is_success() {
            true => Ok(()),
            false => Err(refused(response.status(), path, "finish the upload")),
        }
    }
}

impl AzureProvider {
    /// Says what kind of thing is being uploaded, and how it should be handed out.
    ///
    /// Said at upload time because it cannot be said later without rewriting the blob — and a
    /// store told nothing serves `application/octet-stream` to everybody, which is the
    /// difference between a browser showing an image and downloading it.
    ///
    /// What the source said comes first and the name is the fallback, because a file copied
    /// from another store already knows what it is and may well have no extension to guess by.
    fn say_what_it_is(
        &self,
        path: &RemotePath,
        serve: &Serve,
        headers: &mut HeaderMap,
    ) -> Result<()> {
        let serve = serve.or_guessed_from(path.name());

        let said = [
            ("x-ms-blob-content-type", serve.content_type),
            ("x-ms-blob-cache-control", serve.cache_control),
            ("x-ms-blob-content-encoding", serve.content_encoding),
        ];

        for (header, value) in said {
            if let Some(value) = value {
                headers.insert(header, header_value(&value)?);
            }
        }

        Ok(())
    }

    /// One `HEAD`, which is where every answer about a single blob comes from.
    async fn head(&self, path: &RemotePath) -> Result<HeaderMap> {
        let response = self
            .requests
            .send(Method::HEAD, &self.blob_path(path), &[], HeaderMap::new(), Vec::new())
            .await?;

        match response.status().is_success() {
            true => Ok(response.headers().clone()),
            false => Err(refused(response.status(), path, "read the blob's details")),
        }
    }
}

/// A header, if the blob has one.
fn said(headers: &HeaderMap, header: &str) -> Option<String> {
    headers
        .get(header)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

#[async_trait]
impl Serving for AzureProvider {
    async fn served(&self, path: &RemotePath) -> Result<Served> {
        let headers = self.head(path).await?;

        Ok(Served {
            content_type: said(&headers, "content-type"),
            etag: said(&headers, "etag").map(|tag| tag.trim_matches('"').to_owned()),
            cache_control: said(&headers, "cache-control"),
            content_encoding: said(&headers, "content-encoding"),
        })
    }
}

#[async_trait]
impl Storing for AzureProvider {
    async fn stored(&self, path: &RemotePath) -> Result<Stored> {
        let headers = self.head(path).await?;

        Ok(Stored {
            class: said(&headers, "x-ms-access-tier"),
            encryption: said(&headers, "x-ms-server-encrypted"),
            version: said(&headers, "x-ms-version-id"),
        })
    }
}

#[async_trait]
impl Provider for AzureProvider {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn serving(&self) -> Option<&dyn Serving> {
        Some(self)
    }

    fn storing(&self) -> Option<&dyn Storing> {
        Some(self)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::object_store()
    }

    async fn list(&self, path: &RemotePath) -> Result<Vec<Entry>> {
        let prefix = self.prefix(path);
        let listing = self.list_blobs(&prefix, true).await?;
        let mut entries = Vec::new();

        for folder in &listing.folders {
            if let Some(name) = last_segment(folder, &prefix) {
                entries.push(Entry::folder(name));
            }
        }

        for blob in &listing.blobs {
            if blob.name == prefix {
                continue;
            }

            if let Some(name) = last_segment(&blob.name, &prefix) {
                let mut entry = Entry::file(name, blob.length);
                entry.modified = blob.modified;

                entries.push(entry);
            }
        }

        Ok(entries)
    }

    async fn stat(&self, path: &RemotePath) -> Result<Entry> {
        if path.is_root() {
            return Ok(Entry::folder("/"));
        }

        let response = self
            .requests
            .send(Method::HEAD, &self.blob_path(path), &[], HeaderMap::new(), Vec::new())
            .await?;

        if response.status().is_success() {
            // A HEAD has no body, so the size has to come from the header rather than from
            // what was actually received.
            let length = response
                .headers()
                .get(CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse().ok())
                .unwrap_or_default();

            let mut entry = Entry::file(path.name().unwrap_or_default(), length);

            entry.modified = response
                .headers()
                .get("last-modified")
                .and_then(|value| value.to_str().ok())
                .and_then(parse_http_date);

            return Ok(entry);
        }

        // No blob by that name may still be a folder, which here means only that other blobs
        // share the prefix.
        let listing = self.list_blobs(&self.prefix(path), true).await?;

        if listing.is_empty() {
            Err(Error::NotFound { path: path.clone() })
        } else {
            Ok(Entry::folder(path.name().unwrap_or("/")))
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
        // A large blob comes down as several ranges asked for at once, for the reason S3's does:
        // one GET runs at whatever a single connection manages, which across a long link is
        // decided by the round trip rather than by the bandwidth.
        //
        // Only when the whole blob was asked for. A caller that named a range is resuming or
        // reading a header, and splitting that up again answers a different question.
        if range.is_none() {
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

        let mut headers = HeaderMap::new();

        if let Some(range) = range {
            headers.insert(RANGE, header_value(&range.to_header())?);
        }

        let response = self
            .requests
            .send(Method::GET, &self.blob_path(path), &[], headers, Vec::new())
            .await?;

        if !response.status().is_success() {
            return Err(refused(response.status(), path, "download"));
        }

        let size = response.content_length();
        let serve = what_it_is(&response);

        let chunks = response.bytes_stream().map(|chunk| {
            chunk.map_err(|error| Error::caused_by("the download was interrupted", error))
        });

        Ok(ByteStream::new(chunks, size).served_as(serve))
    }

    async fn write(&self, path: &RemotePath, body: ByteStream) -> Result<()> {
        // Shared Key signs the content length, so a body of unknown size cannot go up in one
        // request. Small and known goes as a single blob; everything else is staged in blocks.
        match body.size() {
            Some(size) if size <= BLOCK_SIZE as u64 => {
                let mut headers = HeaderMap::new();
                headers.insert("x-ms-blob-type", header_value("BlockBlob")?);
                self.say_what_it_is(path, &body.serve().clone(), &mut headers)?;

                // Handed over rather than read first. Collecting it finished the read before
                // the request had even been made, so the progress bar filled up while the file
                // was still sitting in memory waiting to be sent.
                let response = self
                    .requests
                    .send_stream(Method::PUT, &self.blob_path(path), &[], headers, body, size)
                    .await?;

                match response.status().is_success() {
                    true => Ok(()),
                    false => Err(refused(response.status(), path, "upload")),
                }
            }
            _ => self.upload_in_blocks(path, body).await,
        }
    }

    async fn delete(&self, path: &RemotePath) -> Result<()> {
        let entry = self.stat(path).await?;

        let names = if entry.kind.is_folder() {
            self.list_blobs(&self.prefix(path), false)
                .await?
                .blobs
                .into_iter()
                .map(|blob| blob.name)
                .collect()
        } else {
            vec![self.blob_name(path)]
        };

        for name in names {
            let response = self
                .requests
                .send(
                    Method::DELETE,
                    &self.path_for_name(&name),
                    &[],
                    HeaderMap::new(),
                    Vec::new(),
                )
                .await?;

            if !response.status().is_success() && response.status() != StatusCode::NOT_FOUND {
                return Err(refused(response.status(), path, "delete"));
            }
        }

        Ok(())
    }

    async fn create_folder(&self, path: &RemotePath) -> Result<()> {
        if !self.list_blobs(&self.prefix(path), true).await?.is_empty() {
            return Err(Error::AlreadyExists { path: path.clone() });
        }

        let mut headers = HeaderMap::new();
        headers.insert("x-ms-blob-type", header_value("BlockBlob")?);

        // The zero-length blob whose name ends in `/` is how an otherwise empty folder is
        // written down, the same trick every object store client uses.
        let marker = format!("{}/", self.blob_path(path));
        let response = self
            .requests
            .send(Method::PUT, &marker, &[], headers, Vec::new())
            .await?;

        match response.status().is_success() {
            true => Ok(()),
            false => Err(refused(response.status(), path, "create the folder")),
        }
    }

    /// Copies and then deletes, because Azure has no move either.
    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<()> {
        let entry = self.stat(from).await?;

        let moved = if entry.kind.is_folder() {
            self.list_blobs(&self.prefix(from), false)
                .await?
                .blobs
                .into_iter()
                .map(|blob| blob.name)
                .collect()
        } else {
            vec![self.blob_name(from)]
        };

        if moved.is_empty() {
            return Err(Error::NotFound { path: from.clone() });
        }

        let (source, destination) = (self.prefix(from), self.prefix(to));

        // Several at once, because a copy is work the service does on its own and the whole
        // rename is otherwise one round trip per blob. And every one of them before anything is
        // deleted — this is the only ordering here that matters, since a delete that overtakes
        // a copy takes away the only copy there was.
        let mut copying = Vec::with_capacity(moved.len());

        for name in moved {
            let renamed = match entry.kind.is_folder() {
                true => rebase(&name, &source, &destination),
                false => self.blob_name(to),
            };

            copying.push(self.copy(name, renamed, from));
        }

        futures::stream::iter(copying)
            .buffer_unordered(COPIES_AT_ONCE)
            .try_collect::<Vec<()>>()
            .await?;

        self.delete(from).await
    }
}

#[derive(Debug, Default, PartialEq)]
struct Listing {
    blobs: Vec<Blob>,
    folders: Vec<String>,
    /// Where the next page starts, when the container had more to say.
    next: Option<String>,
}

impl Listing {
    fn is_empty(&self) -> bool {
        self.blobs.is_empty() && self.folders.is_empty()
    }
}

#[derive(Debug, Default, PartialEq)]
struct Blob {
    name: String,
    length: u64,
    modified: Option<OffsetDateTime>,
}

/// Reads the XML a container listing comes back as.
fn parse_listing(body: &str) -> Result<Listing> {
    let mut reader = quick_xml::Reader::from_str(body);
    reader.config_mut().check_end_names = true;

    let mut listing = Listing::default();
    let mut blob = Blob::default();
    let mut prefix = String::new();
    let mut inside = String::new();
    let mut in_blob = false;
    let mut in_prefix = false;
    let mut is_a_listing = false;

    loop {
        match reader.read_event() {
            Ok(XmlEvent::Start(element)) => {
                let name = element.name().as_ref().to_ascii_lowercase();

                match name.as_str() {
                    "enumerationresults" => is_a_listing = true,
                    "blob" => {
                        in_blob = true;
                        blob = Blob::default();
                    }
                    "blobprefix" => {
                        in_prefix = true;
                        prefix = String::new();
                    }
                    _ => inside = name,
                }
            }

            Ok(XmlEvent::Text(text)) => {
                let value = text.xml10_content().trim().to_owned();

                if value.is_empty() {
                    continue;
                }

                match inside.as_str() {
                    "nextmarker" => listing.next = Some(value),
                    "name" if in_prefix => prefix = value,
                    "name" if in_blob => blob.name = value,
                    "content-length" if in_blob => blob.length = value.parse().unwrap_or_default(),
                    "last-modified" if in_blob => blob.modified = parse_http_date(&value),
                    _ => {}
                }
            }

            Ok(XmlEvent::End(element)) => {
                match element.name().as_ref().to_ascii_lowercase().as_str() {
                    "blob" => {
                        in_blob = false;
                        listing.blobs.push(std::mem::take(&mut blob));
                    }
                    "blobprefix" => {
                        in_prefix = false;
                        listing.folders.push(std::mem::take(&mut prefix));
                    }
                    _ => {}
                }

                inside.clear();
            }

            Ok(XmlEvent::Eof) if is_a_listing => return Ok(listing),

            Ok(XmlEvent::Eof) => {
                return Err(Error::provider("the server did not answer with a listing"));
            }

            Err(error) => {
                return Err(Error::provider(format!(
                    "the listing could not be read: {error}"
                )));
            }

            _ => {}
        }
    }
}

/// The path a URL carries after its host, which is empty for the usual Azure endpoint and
/// `/account` for the emulator.
fn path_of(endpoint: &str) -> String {
    let after_scheme = endpoint
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(endpoint);

    match after_scheme.find('/') {
        Some(at) => after_scheme[at..].trim_end_matches('/').to_owned(),
        None => String::new(),
    }
}

/// The query as it goes on the wire. Built here rather than by the HTTP client because it has
/// to match, parameter for parameter, what the signature was computed over.
fn query_string(query: &[(String, String)]) -> String {
    if query.is_empty() {
        return String::new();
    }

    let encoded = query
        .iter()
        .map(|(name, value)| {
            format!("{name}={}", utf8_percent_encode(value, IN_QUERY))
        })
        .collect::<Vec<_>>()
        .join("&");

    format!("?{encoded}")
}

fn header_value(value: &str) -> Result<HeaderValue> {
    HeaderValue::from_str(value)
        .map_err(|error| Error::caused_by("a request header could not be built", error))
}

/// What the service says the blob is, taken from the answer already in hand rather than asked
/// for again — which is what stops a file copied to another store arriving as an octet stream.
fn what_it_is(response: &reqwest::Response) -> Serve {
    let said = |header: &str| {
        response
            .headers()
            .get(header)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };

    Serve {
        content_type: said("content-type"),
        cache_control: said("cache-control"),
        content_encoding: said("content-encoding"),
    }
}

/// One range of a blob, and what the response said about the blob it came from.
struct Piece {
    bytes: bytes::Bytes,
    serve: Serve,
    /// How long the whole blob is, which a ranged response states even while handing over only
    /// part of it. Nothing when the service answers `*`, which it is allowed to do.
    total: Option<u64>,
}

async fn piece_of(
    requests: Requests,
    blob: String,
    path: RemotePath,
    range: ByteRange,
) -> Result<Piece> {
    let mut headers = HeaderMap::new();
    headers.insert(RANGE, header_value(&range.to_header())?);

    let response = requests
        .send(Method::GET, &blob, &[], headers, Vec::new())
        .await?;

    if !response.status().is_success() {
        return Err(refused(response.status(), &path, "download"));
    }

    let serve = what_it_is(&response);
    let total = response
        .headers()
        .get("content-range")
        .and_then(|value| value.to_str().ok())
        .and_then(crate::parts::whole_length);

    let bytes = response
        .bytes()
        .await
        .map_err(|error| Error::caused_by("the download was interrupted", error))?;

    let asked = range.length.expect("a length, since every piece is asked for by one");

    // What was asked for is what has to arrive, unless the service says the blob ends first —
    // which it says in the same header that gives the length. Short for any other reason is a
    // hole in the middle of the file, and a file with a hole in it is worse than a download that
    // failed.
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

fn refused(status: StatusCode, path: &RemotePath, doing: &str) -> Error {
    match status {
        StatusCode::NOT_FOUND => Error::NotFound { path: path.clone() },
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            Error::Authentication(format!("the account key was refused ({status})"))
        }
        status => Error::provider(format!("could not {doing}: the server answered {status}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<EnumerationResults ContainerName="okuri">
  <Blobs>
    <BlobPrefix><Name>photos/2026/</Name></BlobPrefix>
    <Blob>
      <Name>photos/harbour.jpg</Name>
      <Properties>
        <Last-Modified>Wed, 26 Aug 2026 10:00:00 GMT</Last-Modified>
        <Content-Length>250000</Content-Length>
      </Properties>
    </Blob>
  </Blobs>
  <NextMarker/>
</EnumerationResults>"##;

    #[test]
    fn a_container_listing_separates_folders_from_blobs() {
        let listing = parse_listing(LISTING).unwrap();

        assert_eq!(listing.folders, vec!["photos/2026/"]);
        assert_eq!(listing.blobs.len(), 1);
        assert_eq!(listing.blobs[0].name, "photos/harbour.jpg");
        assert_eq!(listing.blobs[0].length, 250_000);
        assert!(listing.blobs[0].modified.is_some());
    }

    #[test]
    fn the_emulators_account_in_the_path_is_recognised() {
        assert_eq!(path_of("https://okuri.blob.core.windows.net"), "");
        assert_eq!(path_of("http://127.0.0.1:10000/devstoreaccount1"), "/devstoreaccount1");
        assert_eq!(path_of("http://127.0.0.1:10000/devstoreaccount1/"), "/devstoreaccount1");
    }

    /// A container that answers in pages says where the next one starts, and forgetting to
    /// read that is how a folder looks complete when it is not.
    #[test]
    fn a_listing_says_where_it_left_off() {
        let listing = parse_listing(
            r##"<EnumerationResults><Blobs/><NextMarker>2!go-on</NextMarker></EnumerationResults>"##,
        )
        .unwrap();

        assert_eq!(listing.next.as_deref(), Some("2!go-on"));
        assert_eq!(parse_listing(LISTING).unwrap().next, None);
    }

    #[test]
    fn a_body_that_is_not_a_listing_is_reported() {
        assert!(parse_listing("<html>403 Forbidden</html>").is_err());
        assert!(parse_listing("").is_err());
    }

    #[test]
    fn an_empty_container_is_an_empty_listing_not_a_failure() {
        let listing =
            parse_listing(r#"<EnumerationResults><Blobs/></EnumerationResults>"#).unwrap();

        assert!(listing.is_empty());
    }
}
