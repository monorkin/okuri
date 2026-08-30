//! Where the engine's events arrive and the interface picks them up.
//!
//! Each model listens for the events it cares about and ignores the rest, so adding a panel
//! later means adding a listener rather than threading another channel through the application.
//!
//! Events are shared rather than copied. A prompt in particular has to stay one question no
//! matter how many listeners see it go by, and whichever listener puts it on screen keeps it
//! alive by holding onto its handle until it has been answered.

use std::sync::{Arc, Mutex, OnceLock};

use camion_engine::{Emitter, Event};

type Listener = Arc<dyn Fn(&Arc<Event>) + Send + Sync>;

/// What a listener gets back, and what removes it.
///
/// Every listener today belongs to an object that outlives the window, so most of these are
/// deliberately kept forever. A panel that comes and goes has to hold onto its subscription and
/// drop it, or it keeps being called after the thing it was updating is gone.
#[must_use = "dropping this immediately unsubscribes the listener"]
pub struct Subscription(u64);

impl Subscription {
    /// Keeps the listener for as long as the process runs.
    pub fn forever(self) {
        std::mem::forget(self);
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        listeners().lock().unwrap().retain(|(id, _)| *id != self.0);
    }
}

fn listeners() -> &'static Mutex<Vec<(u64, Listener)>> {
    static LISTENERS: OnceLock<Mutex<Vec<(u64, Listener)>>> = OnceLock::new();

    LISTENERS.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn listen(listener: impl Fn(&Arc<Event>) + Send + Sync + 'static) -> Subscription {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    listeners().lock().unwrap().push((id, Arc::new(listener)));

    Subscription(id)
}

pub fn publish(event: Event) {
    // The list is copied before anything is called, so a listener is free to add another one
    // without deadlocking on the lock it is being called under.
    let listeners = listeners().lock().unwrap().clone();
    let event = Arc::new(event);

    for (_, listener) in listeners {
        listener(&event);
    }
}

/// The engine's end of the bus.
pub fn emitter() -> Emitter {
    Arc::new(publish)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn a_listener_stops_hearing_anything_once_its_subscription_is_dropped() {
        let heard = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&heard);

        let subscription = listen(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        });

        publish(Event::Failed { message: "one".to_owned() });
        assert_eq!(heard.load(Ordering::SeqCst), 1);

        drop(subscription);

        publish(Event::Failed { message: "two".to_owned() });
        assert_eq!(heard.load(Ordering::SeqCst), 1);
    }
}
