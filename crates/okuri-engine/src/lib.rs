//! What Okuri does when you are not looking at it.
//!
//! Connections, the transfers they carry, where credentials live, and which servers are
//! trusted. Everything here runs off the interface's thread and reports back as [`Event`]s.

pub mod config;
pub mod engine;
pub mod error;
pub mod event;
pub mod known_hosts;
pub mod secrets;
pub mod session;
pub mod transfer;
pub mod trust;

use std::sync::Arc;

pub use config::{Connection, Connections};
pub use engine::{Command, Engine};
pub use error::{Error, Result};
pub use event::{Answer, Attempt, Concern, Event, Outcome, Prompt, Question};
pub use known_hosts::{KnownHosts, Verdict};
pub use secrets::{SecretStore, Vault};
pub use session::SessionId;
pub use transfer::{Transfer, TransferId};

/// How the engine hands an event to whatever is listening.
///
/// The Qt side passes a closure that queues the event onto the interface thread; tests pass one
/// that collects into a vector.
pub type Emitter = Arc<dyn Fn(Event) + Send + Sync>;
