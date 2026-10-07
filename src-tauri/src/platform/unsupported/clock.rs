use crate::clock::PerfClock;

/// No performance counter the game is known to stamp pings with; the ping
/// tracker falls back to timing requests.
pub fn perf_clock() -> Option<PerfClock> {
    None
}
