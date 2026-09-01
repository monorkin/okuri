use camion_engine::{Concern, Connections, Event};

/// The saved connections, read from disk every time they are wanted.
///
/// A machine with none gets none. There is an in-memory destination for trying things out, but
/// it belongs in a test rather than in the window of somebody who has just installed this and
/// wants to reach their own server.
///
/// Both the picker and the application itself read through here rather than each keeping a
/// copy, because a connection added in the editor has to be connectable immediately — and two
/// caches of the same file is exactly how that stops being true.
pub fn load() -> Connections {
    let Some(path) = Connections::default_path() else {
        return Connections::default();
    };

    match Connections::load(path) {
        Ok(saved) => saved,

        // An empty list and the reason, rather than an empty list on its own: the connections
        // are still on disk, and a window that shows none without saying why invites somebody
        // to make new ones — whose first save writes over the file that could not be read.
        Err(error) => {
            crate::bus::publish(Event::Failed {
                concern: Concern::Everyone,
                message: format!("your saved connections could not be read: {error}"),
            });

            Connections::default()
        }
    }
}

pub fn save(connections: &Connections) {
    let Some(path) = Connections::default_path() else {
        return;
    };

    if let Err(error) = connections.save(path) {
        crate::bus::publish(Event::Failed {
            concern: Concern::Everyone,
            message: format!("your connections could not be saved: {error}"),
        });
    }
}
