//! Where the engine's news reaches the interface thread.
//!
//! Everything below the interface runs on the engine's threads, and everything GTK owns may
//! only be touched from the one thread its main loop runs on. This is the one door between
//! them: a listener registered here is called on the interface thread whichever thread the
//! news broke on, and in the order it broke.
//!
//! Three kinds of news come through, because three things change under the window: what the
//! engine did, what the desktop looks like, and how the list is meant to be drawn.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gtk::glib;
use okuri_engine::Event;

type EventListener = Rc<dyn Fn(&Arc<Event>)>;
type Listener = Rc<dyn Fn()>;

#[derive(Default)]
struct Listeners {
    events: Vec<(u64, EventListener)>,
    theme: Vec<(u64, Listener)>,
    view: Vec<(u64, Listener)>,
}

thread_local! {
    static LISTENERS: RefCell<Listeners> = RefCell::new(Listeners::default());
}

static NEXT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy)]
enum Channel {
    Events,
    Theme,
    View,
}

/// What a listener gets back, and what removes it.
///
/// A window holds its subscriptions for as long as it is open and drops them with everything
/// else when it closes. Anything that outlives every window keeps its subscription forever.
#[must_use = "dropping this immediately unsubscribes the listener"]
pub struct Subscription {
    channel: Channel,
    id: u64,
}

impl Subscription {
    /// Keeps the listener for as long as the process runs.
    pub fn forever(self) {
        std::mem::forget(self);
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let id = self.id;

        LISTENERS.with(|listeners| {
            let mut listeners = listeners.borrow_mut();

            match self.channel {
                Channel::Events => listeners.events.retain(|(each, _)| *each != id),
                Channel::Theme => listeners.theme.retain(|(each, _)| *each != id),
                Channel::View => listeners.view.retain(|(each, _)| *each != id),
            }
        });
    }
}

/// Connects the engine, the desktop and the display settings to this thread.
///
/// Called once, at startup, from the thread the main loop will run on. Every hop goes through
/// the default main context, which is the one GTK drives.
pub fn start() {
    crate::bus::listen(|event| {
        let event = Arc::clone(event);

        glib::idle_add_once(move || dispatch_event(&event));
    })
    .forever();

    crate::desktop::on_theme_change(|| {
        glib::idle_add_once(|| dispatch(Channel::Theme));
    });

    crate::view::on_change(|| {
        glib::idle_add_once(|| dispatch(Channel::View));
    });
}

pub fn on_event(listener: impl Fn(&Arc<Event>) + 'static) -> Subscription {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);

    LISTENERS.with(|listeners| listeners.borrow_mut().events.push((id, Rc::new(listener))));

    Subscription { channel: Channel::Events, id }
}

pub fn on_theme_change(listener: impl Fn() + 'static) -> Subscription {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);

    LISTENERS.with(|listeners| listeners.borrow_mut().theme.push((id, Rc::new(listener))));

    Subscription { channel: Channel::Theme, id }
}

pub fn on_view_change(listener: impl Fn() + 'static) -> Subscription {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);

    LISTENERS.with(|listeners| listeners.borrow_mut().view.push((id, Rc::new(listener))));

    Subscription { channel: Channel::View, id }
}

fn dispatch_event(event: &Arc<Event>) {
    // The list is copied before anything is called, so a listener is free to subscribe or
    // unsubscribe without tripping over the borrow it is being called under.
    let listeners = LISTENERS.with(|listeners| listeners.borrow().events.clone());

    for (_, listener) in listeners {
        listener(event);
    }
}

fn dispatch(channel: Channel) {
    let listeners = LISTENERS.with(|listeners| {
        let listeners = listeners.borrow();

        match channel {
            Channel::Theme => listeners.theme.clone(),
            Channel::View => listeners.view.clone(),
            Channel::Events => Vec::new(),
        }
    });

    for (_, listener) in listeners {
        listener();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::Once;

    fn started() {
        static ONCE: Once = Once::new();

        ONCE.call_once(start);
    }

    fn pump() {
        let context = glib::MainContext::default();

        while context.pending() {
            context.iteration(false);
        }
    }

    fn failed(message: &str) -> Event {
        Event::Failed { concern: okuri_engine::Concern::Everyone, message: message.to_owned() }
    }

    /// The engine reports from its own threads, and a listener here is only ever called on
    /// this one — once the main loop has had a turn.
    #[test]
    fn news_from_another_thread_arrives_on_this_one_in_order() {
        started();

        let heard = Rc::new(RefCell::new(Vec::new()));
        let log = Rc::clone(&heard);

        let subscription = on_event(move |event| {
            if let Event::Failed { message, .. } = event.as_ref() {
                log.borrow_mut().push(message.clone());
            }
        });

        std::thread::spawn(|| {
            crate::bus::publish(failed("one"));
            crate::bus::publish(failed("two"));
        })
        .join()
        .unwrap();

        assert!(heard.borrow().is_empty(), "nothing is delivered before the loop turns");

        pump();

        assert_eq!(*heard.borrow(), vec!["one", "two"]);

        drop(subscription);
        crate::bus::publish(failed("three"));
        pump();

        assert_eq!(heard.borrow().len(), 2);
    }

    /// A listener that unsubscribes another while being called must not deadlock on the list
    /// it is being called from.
    #[test]
    fn a_listener_may_unsubscribe_from_inside_a_dispatch() {
        let count = Rc::new(Cell::new(0));
        let held: Rc<RefCell<Option<Subscription>>> = Rc::new(RefCell::new(None));

        let counter = Rc::clone(&count);
        let inner = on_theme_change(move || counter.set(counter.get() + 1));
        *held.borrow_mut() = Some(inner);

        let drop_it = Rc::clone(&held);
        let outer = on_theme_change(move || {
            drop_it.borrow_mut().take();
        });

        dispatch(Channel::Theme);
        dispatch(Channel::Theme);

        // Called once, while it was still subscribed, and never again.
        assert_eq!(count.get(), 1);
        drop(outer);
    }
}
