//! The bits of key-shaped storage that S3 and Azure share.
//!
//! Both draw folders onto a flat keyspace the same way, so both need the same arithmetic on
//! prefixes and names. Kept together because two copies of it drifted apart once already.

use okuri_core::RemotePath;
use time::OffsetDateTime;

/// Turns a path the interface is holding into one the server understands, by hanging it off the
/// directory this connection calls its root.
///
/// Logging in rarely puts you at the root of the filesystem, and on a server that is not
/// chrooted an absolute path would leave the account's own directory entirely.
pub fn under(home: &str, path: &RemotePath) -> String {
    if path.is_root() {
        match home.is_empty() {
            true => "/".to_owned(),
            false => home.to_owned(),
        }
    } else {
        format!("{home}/{}", path.to_key())
    }
}

/// The part of `key` that comes after `prefix` and before the next separator, which is the name
/// the file list shows.
pub fn last_segment(key: &str, prefix: &str) -> Option<String> {
    let name = key.strip_prefix(prefix)?.trim_end_matches('/');

    if name.is_empty() {
        None
    } else {
        Some(name.to_owned())
    }
}

/// A connection can be pointed at one folder of a shared bucket rather than the whole thing, so
/// the root is stored the way keys are built: no leading slash, one trailing slash.
pub fn normalize_root(root: &str) -> String {
    let root = root.trim_matches('/');

    if root.is_empty() {
        String::new()
    } else {
        format!("{root}/")
    }
}

/// The name a key takes when the folder it is in is renamed.
///
/// Deliberately `strip_prefix` and not `trim_start_matches`: the latter strips the prefix over
/// and over, so moving `photos/` when a key is `photos/photos/harbour.jpg` would produce
/// `harbour.jpg` — and since a rename deletes the originals, the file would be gone.
pub fn rebase(key: &str, from: &str, to: &str) -> String {
    format!("{to}{}", key.strip_prefix(from).unwrap_or(key))
}

/// `Tue, 26 Aug 2026 10:00:00 GMT`, which is RFC 2822 once `GMT` is written the way that format
/// expects an offset to be written.
pub fn parse_http_date(value: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(
        &value.replace("GMT", "+0000"),
        &time::format_description::well_known::Rfc2822,
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_hang_off_the_login_directory() {
        assert_eq!(under("/home/okuri", &RemotePath::root()), "/home/okuri");
        assert_eq!(
            under("/home/okuri", &RemotePath::parse("/reports/q3.txt").unwrap()),
            "/home/okuri/reports/q3.txt"
        );
    }

    #[test]
    fn a_server_that_lands_you_at_the_root_is_addressed_from_there() {
        assert_eq!(under("", &RemotePath::root()), "/");
        assert_eq!(under("", &RemotePath::parse("/reports").unwrap()), "/reports");
    }

    #[test]
    fn names_are_what_is_left_after_the_prefix() {
        assert_eq!(last_segment("photos/2026/", "photos/"), Some("2026".to_owned()));
        assert_eq!(last_segment("photos/a.jpg", "photos/"), Some("a.jpg".to_owned()));
        assert_eq!(last_segment("photos/", "photos/"), None);
        assert_eq!(last_segment("elsewhere/a.jpg", "photos/"), None);
    }

    #[test]
    fn a_root_prefix_is_stored_the_way_keys_are_built() {
        assert_eq!(normalize_root(""), "");
        assert_eq!(normalize_root("/"), "");
        assert_eq!(normalize_root("site"), "site/");
        assert_eq!(normalize_root("/site/assets/"), "site/assets/");
    }

    /// A folder whose name repeats inside its own path is the case that loses files when the
    /// prefix is stripped more than once.
    #[test]
    fn renaming_moves_a_key_without_eating_a_repeated_name() {
        assert_eq!(rebase("photos/a.jpg", "photos/", "album/"), "album/a.jpg");
        assert_eq!(
            rebase("photos/photos/a.jpg", "photos/", "album/"),
            "album/photos/a.jpg"
        );
        assert_eq!(rebase("photos/photos/", "photos/", "album/"), "album/photos/");
    }

    #[test]
    fn a_key_that_does_not_start_where_expected_is_left_alone() {
        assert_eq!(rebase("elsewhere/a.jpg", "photos/", "album/"), "album/elsewhere/a.jpg");
    }

    #[test]
    fn http_dates_are_understood() {
        let at = parse_http_date("Tue, 26 Aug 2026 10:00:00 GMT").unwrap();

        assert_eq!(at.year(), 2026);
        assert_eq!(at.day(), 26);
        assert_eq!(at.hour(), 10);
        assert_eq!(parse_http_date("not a date"), None);
    }
}
