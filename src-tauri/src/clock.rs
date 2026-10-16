//! The meter's idea of "now", in Unix milliseconds.
//!
//! Live capture reads the wall clock. A replay pins "now" to each packet's
//! capture time instead, so the same capture parses the same on any machine
//! and any day. The pin is per thread: a replay on one thread never moves the
//! time live capture sees on another.

use std::cell::Cell;
use std::time::{SystemTime, UNIX_EPOCH};

thread_local! {
    static PINNED_NOW: Cell<Option<i64>> = const { Cell::new(None) };
}

/// The pinned time if this thread has one, else the wall clock.
pub fn now_ms() -> i64 {
    PINNED_NOW.get().unwrap_or_else(system_ms)
}

/// Pins "now" on this thread to `ms`, or with `None` goes back to the wall
/// clock.
pub fn set_override(ms: Option<i64>) {
    PINNED_NOW.set(ms);
}

/// The wall clock, whatever the pin says. Whole ms, rounded down.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn system_ms() -> i64 {
    epoch_ms(SystemTime::now())
}

/// wasm32 has no clock to read (`SystemTime::now` panics there). Re-derivation
/// pins every packet, so this 0 never reaches a real fight.
#[cfg(target_arch = "wasm32")]
pub(crate) fn system_ms() -> i64 {
    0
}

/// A clock set before 1970 counts as 0 rather than going negative.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn epoch_ms(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Reads the high-resolution counter and the Unix wall clock back to back, both
/// in ms. Each OS folder hands one out from `platform::clock::perf_clock`; the
/// ping tracker just calls what it is given, which keeps it free of OS code.
pub type PerfClock = fn() -> (i64, i64);

/// Pins this thread's clock for a test; the pin goes when the guard drops.
#[cfg(test)]
pub(crate) struct Clock;

#[cfg(test)]
impl Clock {
    pub(crate) fn at(ms: i64) -> Clock {
        set_override(Some(ms));
        Clock
    }

    pub(crate) fn to(&self, ms: i64) {
        set_override(Some(ms));
    }
}

#[cfg(test)]
impl Drop for Clock {
    fn drop(&mut self) {
        set_override(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    /// Any real wall-clock reading is later than this (September 2020).
    const SANE_WALL: i64 = 1_600_000_000_000;

    /// Pins (or clears), then reads back on the same thread.
    fn after_pin(pin: Option<i64>) -> i64 {
        set_override(pin);
        now_ms()
    }

    #[test]
    fn pinned_time_comes_back_exactly() {
        let cases = [1_700_000_000_000, -40_000, 0, i64::MIN, i64::MAX];
        let read: Vec<i64> = cases.iter().map(|&ms| after_pin(Some(ms))).collect();
        set_override(None);
        assert_eq!(read, cases);
    }

    #[test]
    fn newer_pin_replaces_older() {
        let _ = after_pin(Some(12_000));
        let latest = after_pin(Some(31_000));
        set_override(None);
        assert_eq!(latest, 31_000);
    }

    #[test]
    fn clearing_returns_to_wall_time() {
        let unpinned = after_pin(None);
        let pinned = after_pin(Some(1_700_000_000_000));
        let cleared = after_pin(None);
        let cleared_again = after_pin(None);
        assert!(unpinned > SANE_WALL);
        assert_eq!(pinned, 1_700_000_000_000);
        assert!(cleared > SANE_WALL && cleared_again > SANE_WALL);
    }

    #[test]
    fn unpinned_reading_sits_between_two_system_reads() {
        set_override(None);
        let before = epoch_ms(SystemTime::now());
        let got = now_ms();
        let after = epoch_ms(SystemTime::now());
        assert!(before <= got && got <= after, "{before} <= {got} <= {after}");
    }

    #[test]
    fn epoch_ms_floors_and_never_goes_negative() {
        let t = UNIX_EPOCH + Duration::from_micros(1_999_999);
        assert_eq!(epoch_ms(t), 1_999);
        assert_eq!(epoch_ms(UNIX_EPOCH), 0);
        assert_eq!(epoch_ms(UNIX_EPOCH - Duration::from_secs(3_600)), 0);
    }

    #[test]
    fn other_threads_do_not_see_this_pin() {
        let mine = after_pin(Some(42));
        let theirs = thread::spawn(now_ms).join().expect("reader thread");
        set_override(None);
        assert_eq!(mine, 42);
        assert!(theirs > SANE_WALL, "new thread read {theirs}");
    }

    #[test]
    fn pins_on_two_threads_stay_apart() {
        let (step, wait_step) = mpsc::channel::<()>();
        let (report, reported) = mpsc::channel();
        // Started before the pin below, so an older thread is covered too.
        let other = thread::spawn(move || {
            wait_step.recv().ok();
            let seen = now_ms();
            report.send((seen, after_pin(Some(7)))).ok();
            wait_step.recv().ok();
            set_override(None);
        });
        let mine = after_pin(Some(42));
        step.send(()).ok();
        let (seen, own_pin) = reported.recv().expect("other thread answers");
        // The other thread holds its own pin of 7 while this one reads again.
        let mine_again = now_ms();
        step.send(()).ok();
        other.join().expect("other thread ends");
        let mine_last = now_ms();
        set_override(None);
        assert!(seen > SANE_WALL, "older thread read {seen}");
        assert_eq!(own_pin, 7);
        assert_eq!([mine, mine_again, mine_last], [42; 3]);
    }
}
