//! The Okuri domain.
//!
//! Everything here is about files, folders, and the destinations they live on. There is no
//! HTTP, no SSH, and no Qt: adapters bring those in above, and the UI sits above them.

pub mod capabilities;
pub mod conformance;
pub mod entry;
pub mod facets;
pub mod error;
pub mod media;
pub mod memory;
pub mod path;
pub mod permitting;
pub mod provider;
pub mod sharing;
pub mod stream;

pub use capabilities::{Capabilities, Details, Support};
pub use entry::{Access, Column, Entry, EntryKind, Permissions, Sort, Who};
pub use facets::{Linking, Owning, Ownership, Served, Serving, Stored, Storing};
pub use error::{Error, Result};
pub use media::media_type;
pub use memory::MemoryProvider;
pub use path::RemotePath;
pub use permitting::Permitting;
pub use provider::{Provider, ProviderExt};
pub use sharing::{Sharing, Visibility};
pub use stream::{ByteRange, ByteStream, Serve};
