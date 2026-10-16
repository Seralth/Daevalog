//! The performance counter (QueryPerformanceCounter), which the game's ping
//! counter runs on, so the ping tracker can place it on the wall clock.

use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

use crate::clock::PerfClock;

/// The counter's ticks per second; 0 when it cannot be read. Windows XP and
/// later always have one.
fn frequency() -> i64 {
    let mut ticks = 0i64;
    // SAFETY: a valid pointer to an i64.
    match unsafe { QueryPerformanceFrequency(&mut ticks) } {
        Ok(()) => ticks,
        Err(_) => 0,
    }
}

pub fn perf_clock() -> Option<PerfClock> {
    (frequency() > 0).then_some(counter_then_wall)
}

/// Counter first, wall clock straight after; neither looks at a replay pin.
fn counter_then_wall() -> (i64, i64) {
    let mut ticks = 0i64;
    // SAFETY: a valid pointer to an i64.
    let _ = unsafe { QueryPerformanceCounter(&mut ticks) };
    (ticks_to_ms(ticks, frequency()), crate::clock::system_ms())
}

/// Whole ms, rounded down; i128 so a long uptime does not overflow.
fn ticks_to_ms(ticks: i64, frequency: i64) -> i64 {
    if frequency <= 0 {
        return 0;
    }
    (i128::from(ticks) * 1_000 / i128::from(frequency)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn read() -> (i64, i64) {
        perf_clock().expect("Windows has a performance counter")()
    }

    #[test]
    fn ticks_round_down_to_whole_ms() {
        assert_eq!(ticks_to_ms(10_000_000, 10_000_000), 1_000);
        assert_eq!(ticks_to_ms(9_999, 10_000_000), 0);
        assert_eq!(ticks_to_ms(i64::MAX, 10_000_000), i64::MAX / 10_000);
        assert_eq!(ticks_to_ms(5, 0), 0);
    }

    #[test]
    fn counter_never_steps_back() {
        let (first, _) = read();
        let (second, _) = read();
        assert!(second >= first, "{first} then {second}");
    }

    #[test]
    fn both_clocks_advance_together() {
        let (c0, w0) = read();
        std::thread::sleep(Duration::from_millis(50));
        let (c1, w1) = read();
        let (dc, dw) = (c1 - c0, w1 - w0);
        assert!(dc >= 45, "counter moved {dc} ms");
        assert!((dc - dw).abs() <= 20, "counter {dc} ms, wall {dw} ms");
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
