use std::fmt;

use crate::error::{Error, Result};

/// A location on a remote, always absolute and always separated by `/`.
///
/// Every provider speaks this and translates at its own edge: SFTP appends it to a home
/// directory, S3 drops the leading slash to make a key prefix, WebDAV percent-encodes it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RemotePath {
    segments: Vec<String>,
}

impl RemotePath {
    pub fn root() -> Self {
        Self::default()
    }

    /// Resolves `input` lexically, honouring `.` and `..`, relative to the root.
    ///
    /// A path that climbs above the root is rejected rather than clamped, so a provider can
    /// never be handed a location outside the tree the user is browsing.
    pub fn parse(input: &str) -> Result<Self> {
        Self::root().resolve(input)
    }

    /// Resolves `input` relative to this path. An input starting with `/` is absolute.
    pub fn resolve(&self, input: &str) -> Result<Self> {
        let mut segments = if input.starts_with('/') {
            Vec::new()
        } else {
            self.segments.clone()
        };

        for segment in input.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    if segments.pop().is_none() {
                        return Err(Error::InvalidPath {
                            input: input.to_owned(),
                            reason: "climbs above the root",
                        });
                    }
                }
                segment => segments.push(segment.to_owned()),
            }
        }

        Ok(Self { segments })
    }

    /// Appends a single name. Anything that would change the shape of the path — a separator,
    /// `.`, `..`, or an empty name — is an error rather than a silent reinterpretation.
    pub fn join(&self, name: &str) -> Result<Self> {
        let reason = match name {
            "" => Some("is empty"),
            "." | ".." => Some("is a relative marker"),
            name if name.contains('/') => Some("contains a separator"),
            _ => None,
        };

        if let Some(reason) = reason {
            Err(Error::InvalidPath { input: name.to_owned(), reason })
        } else {
            let mut segments = self.segments.clone();
            segments.push(name.to_owned());
            Ok(Self { segments })
        }
    }

    pub fn parent(&self) -> Option<Self> {
        if self.is_root() {
            None
        } else {
            let mut segments = self.segments.clone();
            segments.pop();
            Some(Self { segments })
        }
    }

    pub fn name(&self) -> Option<&str> {
        self.segments.last().map(String::as_str)
    }

    /// The name without its final extension, for display and for rename dialogs that want to
    /// preselect the stem.
    pub fn stem(&self) -> Option<&str> {
        self.name().map(|name| match name.rsplit_once('.') {
            Some((stem, _)) if !stem.is_empty() => stem,
            _ => name,
        })
    }

    pub fn extension(&self) -> Option<&str> {
        self.name().and_then(|name| match name.rsplit_once('.') {
            Some((stem, extension)) if !stem.is_empty() => Some(extension),
            _ => None,
        })
    }

    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    pub fn is_root(&self) -> bool {
        self.segments.is_empty()
    }

    pub fn depth(&self) -> usize {
        self.segments.len()
    }

    pub fn starts_with(&self, prefix: &Self) -> bool {
        self.segments.starts_with(&prefix.segments)
    }

    /// The ancestors from the root down to and including this path, which is exactly what a
    /// breadcrumb renders.
    pub fn ancestors(&self) -> Vec<Self> {
        (0..=self.depth())
            .map(|depth| Self { segments: self.segments[..depth].to_vec() })
            .collect()
    }

    /// The path as a key prefix: no leading slash, and a trailing slash on anything below the
    /// root. This is the form object stores want.
    pub fn to_prefix(&self) -> String {
        if self.is_root() {
            String::new()
        } else {
            format!("{}/", self.segments.join("/"))
        }
    }

    /// The path as an object key: no leading slash, no trailing slash.
    pub fn to_key(&self) -> String {
        self.segments.join("/")
    }
}

impl fmt::Display for RemotePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "/{}", self.segments.join("/"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing_normalizes() {
        assert_eq!(RemotePath::parse("/").unwrap().to_string(), "/");
        assert_eq!(RemotePath::parse("").unwrap().to_string(), "/");
        assert_eq!(RemotePath::parse("/var//log/").unwrap().to_string(), "/var/log");
        assert_eq!(RemotePath::parse("/var/./log").unwrap().to_string(), "/var/log");
        assert_eq!(RemotePath::parse("/var/log/../tmp").unwrap().to_string(), "/var/tmp");
    }

    #[test]
    fn parsing_refuses_to_climb_above_the_root() {
        assert!(RemotePath::parse("/..").is_err());
        assert!(RemotePath::parse("/var/../..").is_err());
    }

    #[test]
    fn resolving_is_relative_unless_the_input_is_absolute() {
        let base = RemotePath::parse("/var/log").unwrap();

        assert_eq!(base.resolve("nginx").unwrap().to_string(), "/var/log/nginx");
        assert_eq!(base.resolve("../tmp").unwrap().to_string(), "/var/tmp");
        assert_eq!(base.resolve("/etc").unwrap().to_string(), "/etc");
    }

    #[test]
    fn joining_takes_a_single_name() {
        let base = RemotePath::parse("/var").unwrap();

        assert_eq!(base.join("log").unwrap().to_string(), "/var/log");
        assert!(base.join("").is_err());
        assert!(base.join(".").is_err());
        assert!(base.join("..").is_err());
        assert!(base.join("log/nginx").is_err());
    }

    #[test]
    fn walking_upwards() {
        let path = RemotePath::parse("/var/log/nginx").unwrap();

        assert_eq!(path.parent().unwrap().to_string(), "/var/log");
        assert_eq!(path.name(), Some("nginx"));
        assert_eq!(RemotePath::root().parent(), None);
        assert_eq!(RemotePath::root().name(), None);
    }

    #[test]
    fn names_split_into_a_stem_and_an_extension() {
        let archive = RemotePath::parse("/backups/site.tar.gz").unwrap();
        assert_eq!(archive.stem(), Some("site.tar"));
        assert_eq!(archive.extension(), Some("gz"));

        let dotfile = RemotePath::parse("/home/.bashrc").unwrap();
        assert_eq!(dotfile.stem(), Some(".bashrc"));
        assert_eq!(dotfile.extension(), None);

        let plain = RemotePath::parse("/home/notes").unwrap();
        assert_eq!(plain.stem(), Some("notes"));
        assert_eq!(plain.extension(), None);
    }

    #[test]
    fn ancestors_read_as_a_breadcrumb() {
        let ancestors = RemotePath::parse("/var/log")
            .unwrap()
            .ancestors()
            .iter()
            .map(RemotePath::to_string)
            .collect::<Vec<_>>();

        assert_eq!(ancestors, vec!["/", "/var", "/var/log"]);
    }

    #[test]
    fn object_stores_get_a_key_and_a_prefix() {
        let path = RemotePath::parse("/photos/2026").unwrap();

        assert_eq!(path.to_key(), "photos/2026");
        assert_eq!(path.to_prefix(), "photos/2026/");
        assert_eq!(RemotePath::root().to_key(), "");
        assert_eq!(RemotePath::root().to_prefix(), "");
    }

    #[test]
    fn prefixes_are_compared_by_segment() {
        let nginx = RemotePath::parse("/var/log/nginx").unwrap();

        assert!(nginx.starts_with(&RemotePath::parse("/var/log").unwrap()));
        assert!(nginx.starts_with(&RemotePath::root()));
        assert!(!nginx.starts_with(&RemotePath::parse("/var/lo").unwrap()));
    }
}
