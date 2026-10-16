//! Thread-scoped subscribers for tests, safe beside other tests' subscribers.
//!
//! tracing caches each callsite's interest for the whole process. While only
//! one dispatcher is registered, a callsite first reached on some other thread
//! asks just that thread's default (none, so "never"), and the line is then
//! dropped on every thread, the tap's too, until the next dispatcher starts.
//! A dispatcher that lives as long as the process and answers "sometimes"
//! keeps the count above one, so each line asks its own thread's subscriber.

use std::sync::OnceLock;

use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::{DefaultGuard, Interest};
use tracing::{Dispatch, Event, Metadata, Subscriber};

static KEEPER: OnceLock<Dispatch> = OnceLock::new();

/// Make `subscriber` this thread's default while the guard lives.
pub(crate) fn set_default(subscriber: impl Subscriber + Send + Sync + 'static) -> DefaultGuard {
    KEEPER.get_or_init(|| Dispatch::new(Keeper));
    tracing::subscriber::set_default(subscriber)
}

/// Run `f` with `subscriber` as this thread's default.
pub(crate) fn with_default<T>(subscriber: impl Subscriber + Send + Sync + 'static, f: impl FnOnce() -> T) -> T {
    let _guard = set_default(subscriber);
    f()
}

/// Hears nothing; never lets a callsite be cached as "never".
struct Keeper;

impl Subscriber for Keeper {
    fn register_callsite(&self, _: &'static Metadata<'static>) -> Interest {
        Interest::sometimes()
    }
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        false
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, _: &Event<'_>) {}
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    use super::*;

    /// Counts the events it is given.
    struct Counter(Arc<AtomicUsize>);

    impl Subscriber for Counter {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, _: &Event<'_>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
    }

    fn shared_line() {
        tracing::info!("trace-tap shared line");
    }

    #[test]
    fn a_line_first_reached_on_a_bare_thread_still_reaches_the_tap() {
        let heard = Arc::new(AtomicUsize::new(0));
        // The other thread reaches the line first, while the tap is set.
        let (tap_ready, bare_done) = (Arc::new(Barrier::new(2)), Arc::new(Barrier::new(2)));
        let bare = {
            let (tap_ready, bare_done) = (tap_ready.clone(), bare_done.clone());
            std::thread::spawn(move || {
                tap_ready.wait();
                shared_line();
                bare_done.wait();
            })
        };
        with_default(Counter(heard.clone()), || {
            tap_ready.wait();
            bare_done.wait();
            shared_line();
        });
        bare.join().unwrap();
        assert_eq!(heard.load(Ordering::Relaxed), 1);
    }
}
