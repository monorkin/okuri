use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use async_trait::async_trait;
use bytes::Bytes;
use time::OffsetDateTime;

use crate::capabilities::Capabilities;
use crate::entry::{Entry, Permissions};
use crate::error::{Error, Result};
use crate::path::RemotePath;
use crate::provider::Provider;
use crate::stream::{ByteRange, ByteStream};

/// A filesystem-shaped provider that lives in memory.
///
/// It exists so the conformance suite has something to grade itself against on every
/// `cargo test`, and so the UI has something to browse before a single socket is opened.
pub struct MemoryProvider {
    label: String,
    capabilities: Capabilities,
    contents: Mutex<Contents>,
}

struct Contents {
    folders: BTreeSet<RemotePath>,
    files: BTreeMap<RemotePath, StoredFile>,
}

struct StoredFile {
    bytes: Bytes,
    modified: OffsetDateTime,
    permissions: Permissions,
}

impl MemoryProvider {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            capabilities: Capabilities::filesystem(),
            contents: Mutex::new(Contents {
                folders: BTreeSet::from([RemotePath::root()]),
                files: BTreeMap::new(),
            }),
        }
    }

    /// A small tree to look at while the real providers are being written.
    pub fn sample() -> Self {
        let provider = Self::new("Memory");

        provider.seed_folder("/documents");
        provider.seed_folder("/documents/invoices");
        provider.seed_folder("/photos");
        provider.seed_file("/README.md", b"Okuri\n=====\n".as_slice());
        provider.seed_file("/documents/notes.txt", b"remember the milk".as_slice());
        provider.seed_file("/documents/invoices/2026-08.pdf", &[0u8; 4096]);
        provider.seed_file("/photos/harbour.jpg", &[0u8; 250_000]);

        provider
    }

    pub fn seed_folder(&self, path: &str) {
        let path = RemotePath::parse(path).expect("a valid seed path");
        self.contents.lock().unwrap().folders.insert(path);
    }

    pub fn seed_file(&self, path: &str, bytes: &[u8]) {
        let path = RemotePath::parse(path).expect("a valid seed path");
        let file = StoredFile {
            bytes: Bytes::copy_from_slice(bytes),
            modified: OffsetDateTime::now_utc(),
            permissions: Permissions(0o644),
        };

        self.contents.lock().unwrap().files.insert(path, file);
    }
}

impl Contents {
    fn entry_at(&self, path: &RemotePath) -> Option<Entry> {
        if self.folders.contains(path) {
            Some(Entry::folder(path.name().unwrap_or("/")))
        } else {
            self.files.get(path).map(|file| {
                Entry::file(path.name().unwrap_or_default(), file.bytes.len() as u64)
                    .modified_at(file.modified)
                    .with_permissions(file.permissions)
            })
        }
    }

    fn children_of(&self, path: &RemotePath) -> Vec<Entry> {
        let folders = self.folders.iter().filter(|folder| !folder.is_root());
        let files = self.files.keys();

        folders
            .chain(files)
            .filter(|candidate| candidate.parent().as_ref() == Some(path))
            .filter_map(|child| self.entry_at(child))
            .collect()
    }

    fn require_folder(&self, path: &RemotePath) -> Result<()> {
        if self.folders.contains(path) {
            Ok(())
        } else if self.files.contains_key(path) {
            Err(Error::NotAFolder { path: path.clone() })
        } else {
            Err(Error::NotFound { path: path.clone() })
        }
    }

    fn require_free(&self, path: &RemotePath) -> Result<()> {
        if self.folders.contains(path) || self.files.contains_key(path) {
            Err(Error::AlreadyExists { path: path.clone() })
        } else {
            Ok(())
        }
    }

    fn require_parent_folder(&self, path: &RemotePath) -> Result<()> {
        match path.parent() {
            Some(parent) => self.require_folder(&parent),
            None => Err(Error::AlreadyExists { path: path.clone() }),
        }
    }
}

#[async_trait]
impl Provider for MemoryProvider {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    async fn list(&self, path: &RemotePath) -> Result<Vec<Entry>> {
        let contents = self.contents.lock().unwrap();
        contents.require_folder(path)?;

        Ok(contents.children_of(path))
    }

    async fn stat(&self, path: &RemotePath) -> Result<Entry> {
        let contents = self.contents.lock().unwrap();

        contents
            .entry_at(path)
            .ok_or_else(|| Error::NotFound { path: path.clone() })
    }

    async fn read(&self, path: &RemotePath, range: Option<ByteRange>) -> Result<ByteStream> {
        let contents = self.contents.lock().unwrap();

        let file = contents.files.get(path).ok_or_else(|| {
            if contents.folders.contains(path) {
                Error::IsAFolder { path: path.clone() }
            } else {
                Error::NotFound { path: path.clone() }
            }
        })?;

        let bytes = match range {
            None => file.bytes.clone(),
            Some(range) => {
                let start = range.offset.min(file.bytes.len() as u64) as usize;
                let end = match range.length {
                    Some(length) => (start + length as usize).min(file.bytes.len()),
                    None => file.bytes.len(),
                };

                file.bytes.slice(start..end)
            }
        };

        Ok(ByteStream::once(bytes))
    }

    async fn write(&self, path: &RemotePath, body: ByteStream) -> Result<()> {
        let bytes = body.collect().await?;
        let mut contents = self.contents.lock().unwrap();

        if contents.folders.contains(path) {
            return Err(Error::IsAFolder { path: path.clone() });
        }

        contents.require_parent_folder(path)?;

        let file = StoredFile {
            bytes,
            modified: OffsetDateTime::now_utc(),
            permissions: Permissions(0o644),
        };

        contents.files.insert(path.clone(), file);

        Ok(())
    }

    async fn delete(&self, path: &RemotePath) -> Result<()> {
        let mut contents = self.contents.lock().unwrap();

        if contents.files.remove(path).is_some() {
            Ok(())
        } else if path.is_root() {
            Err(Error::PermissionDenied { path: path.clone() })
        } else if contents.folders.contains(path) {
            if contents.children_of(path).is_empty() {
                contents.folders.remove(path);
                Ok(())
            } else {
                Err(Error::NotEmpty { path: path.clone() })
            }
        } else {
            Err(Error::NotFound { path: path.clone() })
        }
    }

    async fn create_folder(&self, path: &RemotePath) -> Result<()> {
        let mut contents = self.contents.lock().unwrap();

        contents.require_parent_folder(path)?;
        contents.require_free(path)?;
        contents.folders.insert(path.clone());

        Ok(())
    }

    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<()> {
        let mut contents = self.contents.lock().unwrap();

        if contents.entry_at(from).is_none() {
            return Err(Error::NotFound { path: from.clone() });
        }

        contents.require_parent_folder(to)?;
        contents.require_free(to)?;

        if to.starts_with(from) {
            return Err(Error::InvalidPath {
                input: to.to_string(),
                reason: "is inside the folder being moved",
            });
        }

        let moved_folders = contents
            .folders
            .iter()
            .filter(|folder| folder.starts_with(from))
            .cloned()
            .collect::<Vec<_>>();

        for folder in moved_folders {
            contents.folders.remove(&folder);
            contents.folders.insert(rebase(&folder, from, to));
        }

        let moved_files = contents
            .files
            .keys()
            .filter(|file| file.starts_with(from))
            .cloned()
            .collect::<Vec<_>>();

        for file in moved_files {
            let stored = contents.files.remove(&file).expect("a file we just listed");
            contents.files.insert(rebase(&file, from, to), stored);
        }

        Ok(())
    }
}

fn rebase(path: &RemotePath, from: &RemotePath, to: &RemotePath) -> RemotePath {
    path.segments()[from.depth()..]
        .iter()
        .fold(to.clone(), |rebased, segment| {
            rebased.join(segment).expect("a segment from an existing path")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderExt;

    #[tokio::test]
    async fn renaming_a_folder_moves_everything_under_it() {
        let provider = MemoryProvider::sample();
        let from = RemotePath::parse("/documents").unwrap();
        let to = RemotePath::parse("/archive").unwrap();

        provider.rename(&from, &to).await.unwrap();

        assert!(!provider.exists(&from).await.unwrap());
        assert!(provider.exists(&RemotePath::parse("/archive/invoices/2026-08.pdf").unwrap())
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn a_folder_cannot_be_moved_inside_itself() {
        let provider = MemoryProvider::sample();
        let from = RemotePath::parse("/documents").unwrap();
        let to = RemotePath::parse("/documents/invoices/documents").unwrap();

        assert!(provider.rename(&from, &to).await.is_err());
    }

    #[tokio::test]
    async fn sample_listings_are_what_the_ui_will_show() {
        let provider = MemoryProvider::sample();
        let mut entries = provider.list(&RemotePath::root()).await.unwrap();
        crate::entry::Sort::by_name().apply(&mut entries);

        let names = entries.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names, vec!["documents", "photos", "README.md"]);
        assert_eq!(entries[0].kind, crate::entry::EntryKind::Folder);
    }
}
