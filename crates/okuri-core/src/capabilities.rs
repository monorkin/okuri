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

/// Which kinds of thing a destination knows about one file.
///
/// Not the answers — those come one file at a time — but which questions are worth asking, so a
/// panel showing a file can put the rows on screen before the server has replied.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Details {
    pub owning: bool,
    pub linking: bool,
    pub serving: bool,
    pub storing: bool,
}

impl Details {
    pub const fn none() -> Self {
        Self { owning: false, linking: false, serving: false, storing: false }
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

    /// Whether files here can be handed to somebody with no account — made public, and given
    /// an address that works without credentials. The interface offers none of it otherwise.
    ///
    /// Not set by the adapters: the engine fills it in from whether the provider actually
    /// answers [`Provider::sharing`](crate::Provider::sharing), so it cannot claim something
    /// the adapter has not implemented.
    pub sharing: bool,

    /// Whether a file's mode can be changed here. Filled in the same way, from
    /// [`Provider::permitting`](crate::Provider::permitting).
    pub permissions: bool,

    /// Which of the optional traits this destination answers, so the interface can reserve room
    /// for what is coming rather than growing a row at a time as each answer lands.
    ///
    /// Filled in by the engine from the accessors, like the two above.
    pub details: Details,
    /// How many transfers this kind of destination will take at once.
    ///
    /// Stated rather than inferred from the other flags: two dozen parallel uploads is nothing
    /// to an object store and rude to a small FTP server, and nothing else here can tell the
    /// two apart.
    pub transfer_slots: usize,
}

impl Capabilities {
    /// A filesystem-shaped remote: everything is native and nothing is emulated.
    pub const fn filesystem() -> Self {
        Self {
            rename: Support::Native,
            create_folder: Support::Native,
            empty_folders: Support::Native,
            sharing: false,
            permissions: false,
            details: Details::none(),
            transfer_slots: 4,
        }
    }

    /// An object store: a flat keyspace wearing a folder costume.
    pub const fn object_store() -> Self {
        Self {
            rename: Support::Emulated,
            create_folder: Support::Emulated,
            empty_folders: Support::Emulated,
            sharing: false,
            permissions: false,
            details: Details::none(),
            // Object stores answer over HTTP and are built to be asked many things at once.
            // With thousands of small files the time is nearly all round trips, so this is
            // close to a straight multiplier on how long the whole drop takes.
            transfer_slots: 24,
        }
    }

}

impl Default for Capabilities {
    fn default() -> Self {
        Self::filesystem()
    }
}
