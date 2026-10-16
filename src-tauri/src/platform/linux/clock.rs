//! The clock Wine's QueryPerformanceCounter runs on, so the ping tracker can
//! place the game's echoed counter on the wall clock.

use crate::clock::PerfClock;

use std::ffi::{c_int, c_long};

/// `CLOCK_MONOTONIC_RAW` from `linux/time.h`. `Instant` reads plain
/// `CLOCK_MONOTONIC`, which NTP slews, so it would drift from Wine's counter.
const MONOTONIC_RAW_ID: c_int = 4;

#[repr(C)]
struct KernelTimespec {
    secs: c_long,
    nanos: c_long,
}

unsafe extern "C" {
    #[link_name = "clock_gettime"]
    fn read_kernel_clock(clock: c_int, out: *mut KernelTimespec) -> c_int;
}

/// Linux always has the raw clock.
pub fn perf_clock() -> Option<PerfClock> {
    Some(counter_then_wall)
}

/// Counter first, wall clock straight after; neither looks at a replay pin.
fn counter_then_wall() -> (i64, i64) {
    let counter = raw_monotonic_ms();
    (counter, crate::clock::system_ms())
}

fn raw_monotonic_ms() -> i64 {
    let mut ts = KernelTimespec { secs: 0, nanos: 0 };
    // SAFETY: `ts` is a live, writable timespec for the whole call.
    let rc = unsafe { read_kernel_clock(MONOTONIC_RAW_ID, &mut ts) };
    // Only a bad clock id or pointer fails, and both are fixed here.
    debug_assert_eq!(rc, 0);
    to_whole_ms(ts.secs as i64, ts.nanos as i64)
}

fn to_whole_ms(secs: i64, nanos: i64) -> i64 {
    secs * 1_000 + nanos / 1_000_000
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn read() -> (i64, i64) {
        perf_clock().expect("Linux has a perf clock")()
    }

    #[test]
    fn nanoseconds_round_down_to_whole_ms() {
        assert_eq!(to_whole_ms(5, 999_999_999), 5_999);
        assert_eq!(to_whole_ms(5, 0), 5_000);
        assert_eq!(to_whole_ms(0, 999_999), 0);
    }

    #[test]
    fn counter_never_steps_back() {
        let (first, _) = read();
        let (second, _) = read();
        assert!(second >= first, "{first} then {second}");
    }

    #[test]
    fn counter_is_since_boot_and_wall_is_since_1970() {
        let (counter, wall) = read();
        assert!(counter >= 0);
        assert!(counter < wall, "{counter} vs {wall}");
        assert!(wall > 1_600_000_000_000);
    }

    #[test]
    fn both_clocks_advance_together() {
        let (c0, w0) = read();
        std::thread::sleep(Duration::from_millis(50));
        let (c1, w1) = read();
        let (dc, dw) = (c1 - c0, w1 - w0);
        assert!(dc >= 50, "counter moved {dc} ms");
        assert!((dc - dw).abs() <= 5, "counter {dc} ms, wall {dw} ms");
    }

    #[test]
    fn replay_pin_is_ignored() {
        crate::clock::set_override(Some(12_345));
        let (counter, wall) = read();
        crate::clock::set_override(None);
        assert_ne!(counter, 12_345);
        assert!(wall > 1_600_000_000_000);
    }
}
