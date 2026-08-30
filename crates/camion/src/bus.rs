use std::sync::{Arc, Mutex, OnceLock};

use camion_engine::{Emitter, Event};

type Listener = Arc<dyn Fn(&Arc<Event>) + Send + Sync>;

/// Where the engine's events arrive and the interface picks them up.
///
/// Each model listens for the events it cares about and ignores the rest, so adding a panel
/// later means adding a listener rather than threading another channel through the application.
///
/// Events are shared rather than copied. A prompt in particular has to stay one question no
/// matter how many listeners see it go by, and whichever listener puts it on screen keeps it
/// alive by holding onto its handle until it has been answered.
fn listeners() -> &'static Mutex<Vec<Listener>> {
    static LISTENERS: OnceLock<Mutex<Vec<Listener>>> = OnceLock::new();

    LISTENERS.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn listen(listener: impl Fn(&Arc<Event>) + Send + Sync + 'static) {
    listeners().lock().unwrap().push(Arc::new(listener));
}

pub fn publish(event: Event) {
    // The list is copied before anything is called, so a listener is free to add another one
    // without deadlocking on the lock it is being called under.
    let listeners = listeners().lock().unwrap().clone();
    let event = Arc::new(event);

    for listener in listeners {
        listener(&event);
    }
}

/// The engine's end of the bus.
pub fn emitter() -> Emitter {
    Arc::new(publish)
}
