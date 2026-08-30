use base64::Engine as _;
use hmac::{Hmac, Mac};
use reqwest::header::HeaderMap;
use sha2::Sha256;

/// The Shared Key signature Azure Storage expects on every request.
///
/// This is the credential people actually have — the key the portal shows next to the storage
/// account — so it is worth signing requests ourselves rather than being limited to the token
/// credentials the official SDK offers.
///
/// The scheme is unforgiving: the string being signed has a fixed number of lines whether or
/// not the headers exist, `x-ms-` headers are sorted and lowercased, and the resource includes
/// the query in a particular order. Getting any of it wrong gives the same unhelpful 403, which
/// is exactly why it is built here in one place and tested.
pub fn authorization(
    account: &str,
    key: &str,
    method: &str,
    path: &str,
    query: &[(String, String)],
    headers: &HeaderMap,
    content_length: usize,
) -> Option<String> {
    let signature = sign(
        key,
        &string_to_sign(account, method, path, query, headers, content_length),
    )?;

    Some(format!("SharedKey {account}:{signature}"))
}

fn string_to_sign(
    account: &str,
    method: &str,
    path: &str,
    query: &[(String, String)],
    headers: &HeaderMap,
    content_length: usize,
) -> String {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };

    // A zero length is written as nothing at all rather than as "0".
    let length = match content_length {
        0 => String::new(),
        length => length.to_string(),
    };

    [
        method.to_owned(),
        header("content-encoding"),
        header("content-language"),
        length,
        header("content-md5"),
        header("content-type"),
        // The date always travels as `x-ms-date`, so this line is left empty.
        String::new(),
        header("if-modified-since"),
        header("if-match"),
        header("if-none-match"),
        header("if-unmodified-since"),
        header("range"),
        canonical_headers(headers),
        canonical_resource(account, path, query),
    ]
    .join("\n")
}

/// Every `x-ms-` header, lowercased and sorted, one per line.
fn canonical_headers(headers: &HeaderMap) -> String {
    let mut named = headers
        .iter()
        .filter_map(|(name, value)| {
            let name = name.as_str().to_ascii_lowercase();

            match name.starts_with("x-ms-") {
                true => Some((name, value.to_str().ok()?.trim().to_owned())),
                false => None,
            }
        })
        .collect::<Vec<_>>();

    named.sort();

    named
        .iter()
        .map(|(name, value)| format!("{name}:{value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The account name followed by the request path, then each query parameter on its own line,
/// sorted by name.
///
/// The account is prepended unconditionally, even when the path already contains it. Against
/// real Azure the account lives in the hostname and the path is `/container/blob`; against the
/// emulator it is in the path, and the resource genuinely does read `/account/account/...`.
/// That looks like a mistake and is not — it is what the service signs.
fn canonical_resource(account: &str, path: &str, query: &[(String, String)]) -> String {
    let resource = format!("/{account}{path}");

    let mut parameters = query
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
        .collect::<Vec<_>>();

    parameters.sort();

    let mut signed = resource;

    for (name, value) in &parameters {
        signed.push_str(&format!("\n{name}:{value}"));
    }

    signed
}

fn sign(key: &str, message: &str) -> Option<String> {
    let encoding = base64::engine::general_purpose::STANDARD;
    let key = encoding.decode(key).ok()?;

    let mut mac = Hmac::<Sha256>::new_from_slice(&key).ok()?;
    mac.update(message.as_bytes());

    Some(encoding.encode(mac.finalize().into_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderName, HeaderValue};

    /// The well-known key of the storage emulator, which is public and exists for exactly this.
    const KEY: &str = "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();

        for (name, value) in pairs {
            headers.insert(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }

        headers
    }

    #[test]
    fn the_signed_string_has_a_line_for_every_header_present_or_not() {
        let signed = string_to_sign(
            "devstoreaccount1",
            "GET",
            "/photos",
            &[],
            &headers(&[("x-ms-date", "Fri, 29 Aug 2026 12:00:00 GMT"), ("x-ms-version", "2021-12-02")]),
            0,
        );

        let lines = signed.split('\n').collect::<Vec<_>>();

        assert_eq!(lines[0], "GET");
        // Content-Encoding through Range: eleven fixed lines after the verb.
        assert_eq!(&lines[1..12], &["", "", "", "", "", "", "", "", "", "", ""]);
        assert_eq!(lines[12], "x-ms-date:Fri, 29 Aug 2026 12:00:00 GMT");
        assert_eq!(lines[13], "x-ms-version:2021-12-02");
        assert_eq!(lines[14], "/devstoreaccount1/photos");
    }

    #[test]
    fn a_zero_length_body_is_written_as_nothing_rather_than_zero() {
        let empty = string_to_sign("account", "PUT", "/c/b", &[], &HeaderMap::new(), 0);
        let sized = string_to_sign("account", "PUT", "/c/b", &[], &HeaderMap::new(), 42);

        assert_eq!(empty.split('\n').nth(3), Some(""));
        assert_eq!(sized.split('\n').nth(3), Some("42"));
    }

    #[test]
    fn ms_headers_are_lowercased_and_sorted_and_others_are_left_out() {
        let canonical = canonical_headers(&headers(&[
            ("X-Ms-Version", "2021-12-02"),
            ("x-ms-blob-type", "BlockBlob"),
            ("Content-Type", "text/plain"),
        ]));

        assert_eq!(canonical, "x-ms-blob-type:BlockBlob\nx-ms-version:2021-12-02");
    }

    #[test]
    fn the_account_is_always_prepended_even_when_the_path_repeats_it() {
        assert_eq!(
            canonical_resource("camion", "/photos/a.jpg", &[]),
            "/camion/photos/a.jpg"
        );

        // The emulator carries the account in the path, and the service really does sign the
        // doubled form. This was worth a test the moment it looked like a bug.
        assert_eq!(
            canonical_resource("devstoreaccount1", "/devstoreaccount1/photos/a.jpg", &[]),
            "/devstoreaccount1/devstoreaccount1/photos/a.jpg"
        );
    }

    #[test]
    fn query_parameters_are_sorted_and_one_per_line() {
        let canonical = canonical_resource(
            "camion",
            "/photos",
            &[
                ("restype".to_owned(), "container".to_owned()),
                ("comp".to_owned(), "list".to_owned()),
            ],
        );

        assert_eq!(canonical, "/camion/photos\ncomp:list\nrestype:container");
    }

    #[test]
    fn the_same_request_always_signs_the_same_way() {
        let signed = |key: &str| {
            authorization(
                "devstoreaccount1",
                key,
                "GET",
                "/photos",
                &[("comp".to_owned(), "list".to_owned())],
                &headers(&[("x-ms-date", "Fri, 29 Aug 2026 12:00:00 GMT")]),
                0,
            )
        };

        let first = signed(KEY).unwrap();

        assert_eq!(first, signed(KEY).unwrap());
        assert!(first.starts_with("SharedKey devstoreaccount1:"));
        assert_ne!(first, signed("c2Vjb25kIGtleQ==").unwrap());
    }

    #[test]
    fn a_key_that_is_not_base64_is_refused_rather_than_signed_wrongly() {
        assert_eq!(
            authorization("a", "not base64!", "GET", "/a/b", &[], &HeaderMap::new(), 0),
            None
        );
    }
}
