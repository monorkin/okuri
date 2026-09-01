//! One suite of behaviours every destination has to agree on.
//!
//! Written once against `dyn Provider` and run against all of them: the in-memory provider on
//! every `cargo test`, and the real backends against local containers. A check the connection
//! genuinely cannot support is skipped by reading [`Capabilities`], never by editing the suite,
//! so "SFTP passes and S3 doesn't" is always a bug and never a footnote.

use bytes::Bytes;

use crate::capabilities::Support;
use crate::entry::EntryKind;
use crate::error::{Error, Result};
use crate::path::RemotePath;
use crate::provider::{Provider, ProviderExt};
use crate::stream::ByteRange;

const CONTENTS: &[u8] = b"Okuri carries files.";

pub struct Conformance<'a> {
    provider: &'a dyn Provider,
    scratch: RemotePath,
}

#[derive(Debug, Default)]
pub struct Report {
    pub passed: Vec<&'static str>,
    pub skipped: Vec<(&'static str, &'static str)>,
    pub failed: Vec<(&'static str, String)>,
}

impl Report {
    pub fn is_conformant(&self) -> bool {
        self.failed.is_empty()
    }

    /// Fails the test with every problem at once, rather than stopping at the first, because
    /// the interesting question when porting an adapter is which behaviours are missing.
    #[track_caller]
    pub fn assert_conformant(&self) {
        if !self.is_conformant() {
            let failures = self
                .failed
                .iter()
                .map(|(check, reason)| format!("  {check}: {reason}"))
                .collect::<Vec<_>>()
                .join("\n");

            panic!(
                "{} of {} conformance checks failed:\n{failures}",
                self.failed.len(),
                self.failed.len() + self.passed.len()
            );
        }
    }
}

impl<'a> Conformance<'a> {
    /// Runs inside `scratch`, which the suite creates and removes. Point it somewhere
    /// disposable: the suite writes, renames, and deletes freely underneath it.
    pub fn new(provider: &'a dyn Provider, scratch: RemotePath) -> Self {
        Self { provider, scratch }
    }

    pub async fn run(&self) -> Report {
        let mut report = Report::default();

        if let Err(error) = self.provider.create_folders(&self.scratch).await {
            report.failed.push(("preparing the scratch folder", error.to_string()));
            return report;
        }

        self.check(&mut report, "writing then reading back", self.written_bytes_come_back()).await;
        self.check(&mut report, "listing what was written", self.listing_shows_written_files()).await;
        self.check(&mut report, "inspecting a file", self.stat_describes_a_file()).await;
        self.check(&mut report, "reading a range", self.ranges_return_a_slice()).await;
        self.check(&mut report, "overwriting", self.writing_twice_replaces_contents()).await;
        self.check(&mut report, "deleting", self.deleting_removes_a_file()).await;
        self.check(&mut report, "missing paths", self.missing_paths_report_not_found()).await;
        self.check(&mut report, "nesting", self.files_nest_inside_folders()).await;

        self.check_supported(
            &mut report,
            "creating folders",
            self.provider.capabilities().create_folder,
            self.folders_can_be_created(),
        )
        .await;

        self.check_supported(
            &mut report,
            "renaming",
            self.provider.capabilities().rename,
            self.renaming_moves_a_file(),
        )
        .await;

        self.check_supported(
            &mut report,
            "empty folders",
            self.provider.capabilities().empty_folders,
            self.an_empty_folder_is_listed(),
        )
        .await;

        if let Err(error) = self.provider.delete_recursively(&self.scratch).await {
            report.failed.push(("clearing the scratch folder", error.to_string()));
        }

        report
    }

    async fn check(
        &self,
        report: &mut Report,
        name: &'static str,
        check: impl Future<Output = Result<()>>,
    ) {
        match check.await {
            Ok(()) => report.passed.push(name),
            Err(error) => report.failed.push((name, error.to_string())),
        }
    }

    async fn check_supported(
        &self,
        report: &mut Report,
        name: &'static str,
        support: Support,
        check: impl Future<Output = Result<()>>,
    ) {
        if support.is_available() {
            self.check(report, name, check).await;
        } else {
            report.skipped.push((name, "the connection does not support it"));
        }
    }

    async fn written_bytes_come_back(&self) -> Result<()> {
        let path = self.scratch.join("hello.txt")?;

        self.provider.write_all(&path, Bytes::from_static(CONTENTS)).await?;
        let read_back = self.provider.read_all(&path).await?;

        expect(read_back == CONTENTS, "the bytes read back differ from the bytes written")?;
        self.provider.delete(&path).await
    }

    async fn listing_shows_written_files(&self) -> Result<()> {
        let path = self.scratch.join("listed.txt")?;
        self.provider.write_all(&path, Bytes::from_static(CONTENTS)).await?;

        let entries = self.provider.list(&self.scratch).await?;
        let listed = entries.iter().find(|entry| entry.name == "listed.txt");

        expect(listed.is_some(), "a written file is missing from its folder's listing")?;
        expect(
            listed.is_some_and(|entry| entry.kind == EntryKind::File),
            "a written file is not listed as a file",
        )?;

        self.provider.delete(&path).await
    }

    async fn stat_describes_a_file(&self) -> Result<()> {
        let path = self.scratch.join("inspected.txt")?;
        self.provider.write_all(&path, Bytes::from_static(CONTENTS)).await?;

        let entry = self.provider.stat(&path).await?;

        expect(entry.name == "inspected.txt", "stat reported the wrong name")?;
        expect(entry.kind == EntryKind::File, "stat reported a file as a folder")?;
        expect(entry.size == CONTENTS.len() as u64, "stat reported the wrong size")?;

        self.provider.delete(&path).await
    }

    async fn ranges_return_a_slice(&self) -> Result<()> {
        let path = self.scratch.join("ranged.txt")?;
        self.provider.write_all(&path, Bytes::from_static(CONTENTS)).await?;

        let tail = self
            .provider
            .read(&path, Some(ByteRange::from(6)))
            .await?
            .collect()
            .await?;
        expect(tail == CONTENTS[6..], "an open-ended range returned the wrong bytes")?;

        let middle = self
            .provider
            .read(&path, Some(ByteRange::new(6, 7)))
            .await?
            .collect()
            .await?;
        expect(middle == CONTENTS[6..13], "a bounded range returned the wrong bytes")?;

        self.provider.delete(&path).await
    }

    async fn writing_twice_replaces_contents(&self) -> Result<()> {
        let path = self.scratch.join("overwritten.txt")?;

        self.provider.write_all(&path, Bytes::from_static(CONTENTS)).await?;
        self.provider.write_all(&path, Bytes::from_static(b"shorter")).await?;

        let read_back = self.provider.read_all(&path).await?;
        expect(read_back == b"shorter"[..], "overwriting left the earlier contents behind")?;

        self.provider.delete(&path).await
    }

    async fn deleting_removes_a_file(&self) -> Result<()> {
        let path = self.scratch.join("deleted.txt")?;

        self.provider.write_all(&path, Bytes::from_static(CONTENTS)).await?;
        self.provider.delete(&path).await?;

        expect(!self.provider.exists(&path).await?, "a deleted file still exists")
    }

    async fn missing_paths_report_not_found(&self) -> Result<()> {
        let path = self.scratch.join("never-written.txt")?;

        match self.provider.stat(&path).await {
            Err(Error::NotFound { .. }) => Ok(()),
            Err(error) => fail(format!("inspecting a missing file reported {error} instead of not found")),
            Ok(_) => fail("inspecting a missing file succeeded"),
        }
    }

    async fn files_nest_inside_folders(&self) -> Result<()> {
        let folder = self.scratch.join("nested")?;
        let path = folder.join("deep.txt")?;

        self.provider.create_folders(&folder).await?;
        self.provider.write_all(&path, Bytes::from_static(CONTENTS)).await?;

        let entries = self.provider.list(&folder).await?;
        expect(entries.len() == 1, "a folder with one file listed something else")?;
        expect(entries[0].name == "deep.txt", "a nested file was listed under the wrong name")?;

        let parents = self.provider.list(&self.scratch).await?;
        expect(
            parents.iter().any(|entry| entry.name == "nested" && entry.kind == EntryKind::Folder),
            "a folder holding a file is missing from its parent's listing",
        )?;

        self.provider.delete_recursively(&folder).await
    }

    async fn folders_can_be_created(&self) -> Result<()> {
        let folder = self.scratch.join("created")?;

        self.provider.create_folder(&folder).await?;

        let entry = self.provider.stat(&folder).await?;
        expect(entry.kind == EntryKind::Folder, "a created folder is not a folder")?;

        self.provider.delete_recursively(&folder).await
    }

    async fn renaming_moves_a_file(&self) -> Result<()> {
        let from = self.scratch.join("before.txt")?;
        let to = self.scratch.join("after.txt")?;

        self.provider.write_all(&from, Bytes::from_static(CONTENTS)).await?;
        self.provider.rename(&from, &to).await?;

        expect(!self.provider.exists(&from).await?, "the original still exists after a rename")?;

        let read_back = self.provider.read_all(&to).await?;
        expect(read_back == CONTENTS, "renaming changed the contents")?;

        self.provider.delete(&to).await
    }

    async fn an_empty_folder_is_listed(&self) -> Result<()> {
        let folder = self.scratch.join("empty")?;

        self.provider.create_folder(&folder).await?;

        let entries = self.provider.list(&self.scratch).await?;
        expect(
            entries.iter().any(|entry| entry.name == "empty"),
            "an empty folder vanished from its parent's listing",
        )?;

        self.provider.delete_recursively(&folder).await
    }
}

fn expect(condition: bool, reason: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        fail(reason)
    }
}

fn fail(reason: impl std::fmt::Display) -> Result<()> {
    Err(Error::provider(reason))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryProvider;

    #[tokio::test]
    async fn the_memory_provider_conforms() {
        let provider = MemoryProvider::sample();
        let scratch = RemotePath::parse("/conformance").unwrap();

        let report = Conformance::new(&provider, scratch).run().await;

        report.assert_conformant();
        assert!(report.skipped.is_empty(), "a filesystem provider should skip nothing");
    }
}
