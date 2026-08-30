use camion_engine::{Connection, Connections, Event};
use camion_providers::Destination;

/// The saved connections, read from disk every time they are wanted.
///
/// Both the picker and the application itself read through here rather than each keeping a
/// copy, because a connection added in the editor has to be connectable immediately — and two
/// caches of the same file is exactly how that stops being true.
pub fn load() -> Connections {
    let Some(path) = Connections::default_path() else {
        return sample();
    };

    let saved = match Connections::load(path) {
        Ok(saved) => saved,

        // Falling back to the samples here would be the worst of both: the saved connections
        // are still on disk, but the window says they are not — and the next save writes the
        // samples over them. An empty list plus the reason is at least the truth.
        Err(error) => {
            crate::bus::publish(Event::Failed {
                message: format!("your saved connections could not be read: {error}"),
            });

            return Connections::default();
        }
    };

    if saved.entries.is_empty() {
        sample()
    } else {
        saved
    }
}

pub fn save(connections: &Connections) {
    let Some(path) = Connections::default_path() else {
        return;
    };

    if let Err(error) = connections.save(path) {
        crate::bus::publish(Event::Failed {
            message: format!("your connections could not be saved: {error}"),
        });
    }
}

/// A machine that has never been configured still gets something to open, so a first run
/// demonstrates the application rather than showing an empty window and a New button.
fn sample() -> Connections {
    let mut connections = Connections::default();
    connections.put(Connection::new("Sample files", Destination::Memory));

    connections
}
