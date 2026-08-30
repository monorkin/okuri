use async_trait::async_trait;
use camion_core::{
    ByteRange, ByteStream, Capabilities, Entry, Error, Provider, RemotePath, Result,
};
use futures::StreamExt;
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, CONTROLS};
use quick_xml::events::Event as XmlEvent;
use reqwest::header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE, RANGE};
use reqwest::{Client, Method, StatusCode};
use time::OffsetDateTime;

use crate::destination::WebDav as WebDavConfig;
use crate::secret::Secret;

/// A remote filesystem over HTTP.
///
/// There is no crate worth depending on here, so this speaks the handful of verbs Camion needs
/// directly: PROPFIND to list, GET and PUT to move bytes, MKCOL and MOVE and DELETE for the
/// rest. It is the smallest of the adapters and the one we most completely own.
pub struct WebDavProvider {
    label: String,
    base: String,
    authorization: Option<HeaderValue>,
    client: Client,
}

/// Everything that has to be escaped in a path segment. `/` is deliberately absent: it
/// separates segments and must survive.
const IN_PATH: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'#')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'%');

const LISTING_REQUEST: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<propfind xmlns="DAV:">
  <prop>
    <resourcetype/>
    <getcontentlength/>
    <getlastmodified/>
  </prop>
</propfind>"#;

impl WebDavProvider {
    pub async fn connect(config: &WebDavConfig, secret: &Secret) -> Result<Self> {
        let client = Client::builder()
            .build()
            .map_err(|error| Error::caused_by("could not start an HTTP client", error))?;

        let authorization = secret.password().map(|password| {
            use base64::Engine as _;

            let encoded = base64::engine::general_purpose::STANDARD
                .encode(format!("{}:{password}", config.username));

            HeaderValue::from_str(&format!("Basic {encoded}"))
                .unwrap_or_else(|_| HeaderValue::from_static(""))
        });

        let provider = Self {
            label: match config.username.is_empty() {
                true => config.url.clone(),
                false => format!("{}@{}", config.username, host_of(&config.url)),
            },
            base: config.url.trim_end_matches('/').to_owned(),
            authorization,
            client,
        };

        // Connecting means nothing over HTTP until something is asked for, so the root is
        // listed now rather than letting a wrong URL or password look like an empty folder.
        provider.list(&RemotePath::root()).await?;

        Ok(provider)
    }

    fn url(&self, path: &RemotePath) -> String {
        let encoded = path
            .segments()
            .iter()
            .map(|segment| utf8_percent_encode(segment, IN_PATH).to_string())
            .collect::<Vec<_>>()
            .join("/");

        format!("{}/{encoded}", self.base)
    }

    fn request(&self, method: Method, url: &str) -> reqwest::RequestBuilder {
        let request = self.client.request(method, url);

        match &self.authorization {
            Some(authorization) => request.header(AUTHORIZATION, authorization),
            None => request,
        }
    }

    async fn propfind(&self, path: &RemotePath, depth: &str) -> Result<Vec<Found>> {
        let response = self
            .request(Method::from_bytes(b"PROPFIND").expect("a valid method"), &self.url(path))
            .header("Depth", depth)
            .header(CONTENT_TYPE, "application/xml; charset=utf-8")
            .body(LISTING_REQUEST)
            .send()
            .await
            .map_err(|error| Error::caused_by("the server could not be reached", error))?;

        let status = response.status();

        if status == StatusCode::NOT_FOUND {
            return Err(Error::NotFound { path: path.clone() });
        }

        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(Error::PermissionDenied { path: path.clone() });
        }

        if !status.is_success() {
            return Err(Error::provider(format!("the server answered {status}")));
        }

        let body = response
            .text()
            .await
            .map_err(|error| Error::caused_by("the listing could not be read", error))?;

        parse_multistatus(&body)
    }
}

#[async_trait]
impl Provider for WebDavProvider {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::filesystem()
    }

    async fn list(&self, path: &RemotePath) -> Result<Vec<Entry>> {
        let here = format!("{}/", self.url(path).trim_end_matches('/'));

        Ok(self
            .propfind(path, "1")
            .await?
            .into_iter()
            // A listing includes the folder itself, which is not one of its own contents.
            .filter(|found| !found.is_same_place_as(&here))
            .filter_map(|found| found.into_entry())
            .collect())
    }

    async fn stat(&self, path: &RemotePath) -> Result<Entry> {
        self.propfind(path, "0")
            .await?
            .into_iter()
            .next()
            .and_then(Found::into_entry)
            .map(|mut entry| {
                entry.name = path.name().unwrap_or("/").to_owned();
                entry
            })
            .ok_or_else(|| Error::NotFound { path: path.clone() })
    }

    async fn read(&self, path: &RemotePath, range: Option<ByteRange>) -> Result<ByteStream> {
        let mut request = self.request(Method::GET, &self.url(path));

        if let Some(range) = range {
            request = request.header(RANGE, range.to_header());
        }

        let response = request
            .send()
            .await
            .map_err(|error| Error::caused_by("the download could not be started", error))?;

        if response.status() == StatusCode::NOT_FOUND {
            return Err(Error::NotFound { path: path.clone() });
        }

        if !response.status().is_success() {
            return Err(Error::provider(format!(
                "the server answered {}",
                response.status()
            )));
        }

        let size = response.content_length();
        let chunks = response.bytes_stream().map(|chunk| {
            chunk.map_err(|error| Error::caused_by("the download was interrupted", error))
        });

        Ok(ByteStream::new(chunks, size))
    }

    async fn write(&self, path: &RemotePath, body: ByteStream) -> Result<()> {
        // Streamed rather than collected: a large file never has to fit in memory.
        let response = self
            .request(Method::PUT, &self.url(path))
            .body(reqwest::Body::wrap_stream(body))
            .send()
            .await
            .map_err(|error| Error::caused_by("the upload failed", error))?;

        expect_success(response.status(), path, "upload")
    }

    async fn delete(&self, path: &RemotePath) -> Result<()> {
        let response = self
            .request(Method::DELETE, &self.url(path))
            .send()
            .await
            .map_err(|error| Error::caused_by("the delete failed", error))?;

        expect_success(response.status(), path, "delete")
    }

    async fn create_folder(&self, path: &RemotePath) -> Result<()> {
        let response = self
            .request(Method::from_bytes(b"MKCOL").expect("a valid method"), &self.url(path))
            .send()
            .await
            .map_err(|error| Error::caused_by("the folder could not be created", error))?;

        if response.status() == StatusCode::METHOD_NOT_ALLOWED {
            return Err(Error::AlreadyExists { path: path.clone() });
        }

        expect_success(response.status(), path, "create the folder")
    }

    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<()> {
        let response = self
            .request(Method::from_bytes(b"MOVE").expect("a valid method"), &self.url(from))
            .header("Destination", self.url(to))
            .header("Overwrite", "F")
            .send()
            .await
            .map_err(|error| Error::caused_by("the rename failed", error))?;

        if response.status() == StatusCode::PRECONDITION_FAILED {
            return Err(Error::AlreadyExists { path: to.clone() });
        }

        expect_success(response.status(), from, "rename")
    }
}

fn expect_success(status: StatusCode, path: &RemotePath, doing: &str) -> Result<()> {
    match status {
        status if status.is_success() => Ok(()),
        // A `409` from MKCOL or PUT means a folder along the way is missing, which is the
        // same problem as a `404` and reads better said that way.
        StatusCode::NOT_FOUND | StatusCode::CONFLICT => Err(Error::NotFound { path: path.clone() }),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            Err(Error::PermissionDenied { path: path.clone() })
        }
        status => Err(Error::provider(format!(
            "could not {doing}: the server answered {status}"
        ))),
    }
}

/// One `<response>` from a multistatus body.
#[derive(Debug, Default, PartialEq)]
struct Found {
    href: String,
    is_collection: bool,
    length: u64,
    modified: Option<OffsetDateTime>,
}

impl Found {
    /// Whether this is the folder that was asked about rather than something inside it.
    /// Servers are inconsistent about trailing slashes and about returning a full URL versus a
    /// bare path, so both ends are compared loosely.
    fn is_same_place_as(&self, url: &str) -> bool {
        let path_of = |text: &str| {
            let without_scheme = text.split_once("://").map(|(_, rest)| rest).unwrap_or(text);
            let path = without_scheme.find('/').map(|at| &without_scheme[at..]).unwrap_or("/");

            path.trim_end_matches('/').to_owned()
        };

        path_of(&self.href) == path_of(url)
    }

    fn into_entry(self) -> Option<Entry> {
        let decoded = percent_decode_str(self.href.trim_end_matches('/'))
            .decode_utf8()
            .ok()?;
        let name = decoded.rsplit('/').next()?.to_owned();

        if name.is_empty() {
            return None;
        }

        let mut entry = match self.is_collection {
            true => Entry::folder(name),
            false => Entry::file(name, self.length),
        };
        entry.modified = self.modified;

        Some(entry)
    }
}

/// Reads a multistatus body.
///
/// Namespace prefixes are whatever the server felt like — `d:`, `D:`, `lp1:` — so elements are
/// matched on their local name. Anything unrecognised is skipped rather than refused: servers
/// return properties we did not ask for, and that is not an error.
fn parse_multistatus(body: &str) -> Result<Vec<Found>> {
    let mut reader = quick_xml::Reader::from_str(body);

    // A body that does not close its own tags is not a listing with a few rows missing, it is a
    // server saying something we do not understand — better to report that than to show a
    // half-read folder as if it were the whole thing.
    reader.config_mut().check_end_names = true;

    let mut found = Vec::new();
    let mut current = Found::default();
    let mut inside = String::new();
    let mut in_response = false;
    let mut is_a_listing = false;

    loop {
        match reader.read_event() {
            Ok(XmlEvent::Start(element)) => {
                let name = local_name(element.name().as_ref());

                match name.as_str() {
                    "multistatus" => is_a_listing = true,
                    "response" => {
                        in_response = true;
                        current = Found::default();
                    }
                    "collection" if in_response => current.is_collection = true,
                    _ => inside = name,
                }
            }

            Ok(XmlEvent::Empty(element)) => {
                if local_name(element.name().as_ref()) == "collection" && in_response {
                    current.is_collection = true;
                }
            }

            Ok(XmlEvent::Text(text)) => {
                let value = text.xml10_content().trim().to_owned();

                if value.is_empty() || !in_response {
                    continue;
                }

                match inside.as_str() {
                    "href" => current.href = value,
                    "getcontentlength" => current.length = value.parse().unwrap_or_default(),
                    "getlastmodified" => current.modified = parse_http_date(&value),
                    _ => {}
                }
            }

            Ok(XmlEvent::End(element)) => {
                if local_name(element.name().as_ref()) == "response" {
                    in_response = false;
                    found.push(std::mem::take(&mut current));
                }

                inside.clear();
            }

            // Something that is not a listing at all — an HTML error page, most often —
            // would otherwise parse to nothing and be shown as an empty folder.
            Ok(XmlEvent::Eof) if is_a_listing => return Ok(found),

            Ok(XmlEvent::Eof) => {
                return Err(Error::provider(
                    "the server did not answer with a WebDAV listing",
                ));
            }

            Err(error) => {
                return Err(Error::provider(format!(
                    "the server's listing could not be read: {error}"
                )));
            }

            _ => {}
        }
    }
}

fn local_name(qualified: &str) -> String {
    qualified.rsplit(':').next().unwrap_or(qualified).to_ascii_lowercase()
}

/// `Tue, 26 Aug 2026 10:00:00 GMT`, which is RFC 2822 once `GMT` is written the way that format
/// expects an offset to be written.
fn parse_http_date(value: &str) -> Option<OffsetDateTime> {
    let normalized = value.replace("GMT", "+0000");

    OffsetDateTime::parse(&normalized, &time::format_description::well_known::Rfc2822).ok()
}

fn host_of(url: &str) -> String {
    url.split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = r##"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/dav/photos/</d:href>
    <d:propstat><d:prop>
      <d:resourcetype><d:collection/></d:resourcetype>
    </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  </d:response>
  <d:response>
    <d:href>/dav/photos/harbour%20view.jpg</d:href>
    <d:propstat><d:prop>
      <d:resourcetype/>
      <d:getcontentlength>250000</d:getcontentlength>
      <d:getlastmodified>Tue, 26 Aug 2026 10:00:00 GMT</d:getlastmodified>
    </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  </d:response>
  <d:response>
    <d:href>/dav/photos/2026/</d:href>
    <d:propstat><d:prop>
      <d:resourcetype><d:collection/></d:resourcetype>
    </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  </d:response>
</d:multistatus>"##;

    #[test]
    fn a_listing_becomes_entries() {
        let found = parse_multistatus(LISTING).unwrap();
        assert_eq!(found.len(), 3);

        let entries = found
            .into_iter()
            .filter(|entry| !entry.is_same_place_as("/dav/photos/"))
            .filter_map(Found::into_entry)
            .collect::<Vec<_>>();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "harbour view.jpg");
        assert_eq!(entries[0].size, 250_000);
        assert!(entries[0].kind.is_file());
        assert_eq!(entries[1].name, "2026");
        assert!(entries[1].kind.is_folder());
    }

    #[test]
    fn the_folder_itself_is_not_one_of_its_contents() {
        let found = parse_multistatus(LISTING).unwrap();

        assert!(found[0].is_same_place_as("/dav/photos/"));
        assert!(found[0].is_same_place_as("/dav/photos"));
        assert!(found[0].is_same_place_as("https://example.com/dav/photos/"));
        assert!(!found[1].is_same_place_as("/dav/photos/"));
    }

    #[test]
    fn whatever_namespace_prefix_the_server_chose_is_fine() {
        let found = parse_multistatus(
            r##"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/notes.txt</D:href>
    <D:propstat><D:prop><lp1:getcontentlength xmlns:lp1="DAV:">12</lp1:getcontentlength></D:prop></D:propstat>
  </D:response>
</D:multistatus>"##,
        )
        .unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].length, 12);
    }

    #[test]
    fn http_dates_are_understood() {
        let at = parse_http_date("Tue, 26 Aug 2026 10:00:00 GMT").unwrap();

        assert_eq!(at.year(), 2026);
        assert_eq!(at.day(), 26);
        assert_eq!(at.hour(), 10);
        assert_eq!(parse_http_date("not a date"), None);
    }

    #[test]
    fn a_body_that_is_not_a_listing_is_reported_rather_than_shown_as_an_empty_folder() {
        assert!(parse_multistatus("<html><body>404 Not Found</body></html>").is_err());
        assert!(parse_multistatus("").is_err());
        assert!(parse_multistatus("<multistatus><response></multistatus>").is_err());
    }

    #[test]
    fn a_listing_with_no_rows_is_an_empty_folder_not_a_failure() {
        assert_eq!(
            parse_multistatus(r#"<multistatus xmlns="DAV:"></multistatus>"#).unwrap(),
            Vec::new()
        );
    }
}
