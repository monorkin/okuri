use std::fmt;

/// What a provider can be asked to do, named the way the UI names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Operation {
    List,
    Stat,
    Read,
    Write,
    Delete,
    CreateFolder,
    Rename,
    SetPermissions,
}

impl fmt::Display for Operation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::List => "listing",
            Self::Stat => "inspecting",
            Self::Read => "downloading",
            Self::Write => "uploading",
            Self::Delete => "deleting",
            Self::CreateFolder => "creating folders",
            Self::Rename => "renaming",
            Self::SetPermissions => "changing permissions",
        };

        formatter.write_str(name)
    }
}

/// How well a provider supports an operation.
///
/// `Emulated` is the interesting one. Renaming on S3 means copying every object under a prefix
/// and deleting the originals — it works, but it costs, and it is not atomic. The UI warns
/// before it, rather than discovering halfway through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    Native,
    Emulated,
    Unsupported,
}

impl Support {
    pub fn is_available(&self) -> bool {
        !matches!(self, Self::Unsupported)
    }

    pub fn needs_warning(&self) -> bool {
        matches!(self, Self::Emulated)
    }
}

/// What a connection can do, as data the UI can read.
///
/// Menu items are enabled from this rather than from a match on the provider kind, so adding a
/// destination never means touching the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub rename: Support,
    pub create_folder: Support,
    pub permissions: Support,
    pub empty_folders: Support,
    pub resume_uploads: bool,
    pub max_upload_size: Option<u64>,
}

impl Capabilities {
    /// A filesystem-shaped remote: everything is native and nothing is emulated.
    pub const fn filesystem() -> Self {
        Self {
            rename: Support::Native,
            create_folder: Support::Native,
            permissions: Support::Native,
            empty_folders: Support::Native,
            resume_uploads: true,
            max_upload_size: None,
        }
    }

    /// An object store: a flat keyspace wearing a folder costume.
    pub const fn object_store() -> Self {
        Self {
            rename: Support::Emulated,
            create_folder: Support::Emulated,
            permissions: Support::Unsupported,
            empty_folders: Support::Emulated,
            resume_uploads: false,
            max_upload_size: None,
        }
    }

    pub fn supports(&self, operation: Operation) -> Support {
        match operation {
            Operation::Rename => self.rename,
            Operation::CreateFolder => self.create_folder,
            Operation::SetPermissions => self.permissions,
            _ => Support::Native,
        }
    }
}

impl Default for Capabilities {
    fn default() -> Self {
        Self::filesystem()
    }
}
