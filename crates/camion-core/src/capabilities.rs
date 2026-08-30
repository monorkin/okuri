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
    pub empty_folders: Support,
    /// How many transfers this kind of destination will take at once.
    ///
    /// Stated rather than inferred from the other flags: eight parallel uploads is polite to an
    /// object store and rude to a small FTP server, and nothing else here can tell the two
    /// apart.
    pub transfer_slots: usize,
}

impl Capabilities {
    /// A filesystem-shaped remote: everything is native and nothing is emulated.
    pub const fn filesystem() -> Self {
        Self {
            rename: Support::Native,
            create_folder: Support::Native,
            empty_folders: Support::Native,
            transfer_slots: 4,
        }
    }

    /// An object store: a flat keyspace wearing a folder costume.
    pub const fn object_store() -> Self {
        Self {
            rename: Support::Emulated,
            create_folder: Support::Emulated,
            empty_folders: Support::Emulated,
            transfer_slots: 8,
        }
    }

}

impl Default for Capabilities {
    fn default() -> Self {
        Self::filesystem()
    }
}
