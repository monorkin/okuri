//! The Camion domain.
//!
//! Everything here is about files, folders, and the destinations they live on. There is no
//! HTTP, no SSH, and no Qt: adapters bring those in above, and the UI sits above them.

pub mod capabilities;
pub mod conformance;
pub mod entry;
pub mod error;
pub mod memory;
pub mod path;
pub mod provider;
pub mod stream;

pub use capabilities::{Capabilities, Support};
pub use entry::{Column, Entry, EntryKind, Permissions, Sort};
pub use error::{Error, Result};
pub use memory::MemoryProvider;
pub use path::RemotePath;
pub use provider::{Provider, ProviderExt};
pub use stream::{ByteRange, ByteStream};
