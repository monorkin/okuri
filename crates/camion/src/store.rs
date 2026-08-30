use camion_engine::{Connection, Connections};
use camion_providers::Destination;

/// The saved connections, read from disk every time they are wanted.
///
/// Both the picker and the application itself read through here rather than each keeping a
/// copy, because a connection added in the editor has to be connectable immediately — and two
/// caches of the same file is exactly how that stops being true.
pub fn load() -> Connections {
    let saved = Connections::default_path()
        .map(Connections::load)
        .transpose()
        .ok()
        .flatten()
        .unwrap_or_default();

    if saved.entries.is_empty() {
        sample()
    } else {
        saved
    }
}

pub fn save(connections: &Connections) {
    if let Some(path) = Connections::default_path() {
        let _ = connections.save(path);
    }
}

/// A machine that has never been configured still gets something to open, so a first run
/// demonstrates the application rather than showing an empty window and a New button.
fn sample() -> Connections {
    let mut connections = Connections::default();
    connections.put(Connection::new("Sample files", Destination::Memory));

    connections
}
