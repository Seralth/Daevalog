//! The game traffic of the last hour or so, held in memory, so a saved fight
//! can keep its slice even when packet logging is off. Nothing here is ever
//! written to disk: only the slice cut from it is, for a fight the meter saved.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};

use crate::capture::tcp_chunk::{stream_key, TcpChunk};
use crate::capture::fight_tape::{SliceRecord, SLICE_HEAD_MS, SLICE_LOOKBACK_MS, TAIL_MS};

/// The held segments, oldest at the front, and their bytes added up.
struct Ring {
    size: usize,
    held: VecDeque<SliceRecord>,
}

impl Ring {
    /// Lets the front segment go; false when there is none.
    fn drop_oldest(&mut self) -> bool {
        let Some(gone) = self.held.pop_front() else {
            return false;
        };
        self.size -= gone.bytes.len();
        true
    }

    fn over(&self, cutoff: i64) -> bool {
        self.size > BYTE_CAP || self.held.front().is_some_and(|oldest| oldest.captured_at_ms < cutoff)
    }
}

static RING: Mutex<Ring> = Mutex::new(Ring { size: 0, held: VecDeque::new() });

fn ring() -> MutexGuard<'static, Ring> {
    RING.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// How far back a segment stays: everything a slice reaches for around a
/// fight of up to 35 minutes.
const KEEP_MS: i64 = 35 * 60_000 + SLICE_LOOKBACK_MS + SLICE_HEAD_MS + TAIL_MS;

/// A guard, not a budget: half an hour of play is about 3 MB.
const BYTE_CAP: usize = 96 << 20;

/// Keeps one game segment, stamped with its capture time (or now, when the
/// capture gave none) and named after its connection.
pub fn record(cap: &TcpChunk) {
    let at = cap.capture_time_ms().unwrap_or_else(crate::clock::now_ms);
    record_at(at, stream_key(cap.src_port, cap.dst_port), &cap.data);
}

/// Keeps `data` as the newest segment, then lets the oldest ones go while
/// they are too old for `at` or the ring is over its size. Segments stay in
/// the order they came, so a late stamp never moves one.
pub fn record_at(at: i64, stream: String, data: &[u8]) {
    let mut state = ring();
    state.held.push_back(SliceRecord { captured_at_ms: at, stream, bytes: data.to_vec() });
    state.size += data.len();
    while state.over(at - KEEP_MS) && state.drop_oldest() {}
}

/// A copy of every segment held, oldest first.
pub fn snapshot() -> Vec<SliceRecord> {
    ring().held.iter().cloned().collect()
}

#[cfg(test)]
pub fn clear() {
    let mut state = ring();
    state.held.clear();
    state.size = 0;
}

/// Tests that fill the ring take turns through this.
#[cfg(test)]
pub(super) fn test_turn() -> MutexGuard<'static, ()> {
    static TURN: Mutex<()> = Mutex::new(());
    TURN.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_767_380_434_407;

    fn segment(at_ms: i64, data: &[u8]) -> TcpChunk {
        crate::built::segment(None, 7777, 50000, data, at_ms)
    }

    fn stamps() -> Vec<i64> {
        ring().held.iter().map(|p| p.captured_at_ms).collect()
    }

    #[test]
    fn keep_time_and_size_cap_have_their_set_values() {
        assert_eq!(KEEP_MS, 3_975_000);
        assert_eq!(BYTE_CAP, 100_663_296);
    }

    #[test]
    fn a_segment_past_the_keep_time_is_let_go() {
        let _turn = test_turn();
        clear();
        record(&segment(T0, &[1, 2, 3]));
        record(&segment(T0 + 3_975_001, &[4, 5]));
        let held = snapshot();
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].bytes, [4, 5]);
        assert_eq!(held[0].stream, "Client:50000:7777");
        clear();
    }

    #[test]
    fn a_segment_exactly_at_the_keep_time_stays() {
        let _turn = test_turn();
        clear();
        record_at(T0, "s".into(), &[1]);
        record_at(T0 + 3_975_000, "s".into(), &[2]);
        assert_eq!(stamps(), [T0, T0 + 3_975_000]);
        clear();
    }

    #[test]
    fn a_missing_capture_time_takes_the_clock() {
        let _turn = test_turn();
        clear();
        crate::clock::set_override(Some(1_767_380_434_500));
        record(&segment(T0, &[1]));
        record(&segment(0, &[2]));
        crate::clock::set_override(None);
        assert_eq!(stamps(), [T0, 1_767_380_434_500]);
        assert!(ring().held.iter().all(|p| p.stream == "Client:50000:7777"));
        clear();
    }

    #[test]
    fn segments_keep_their_order_and_leave_from_the_old_end_only() {
        let _turn = test_turn();
        clear();
        record_at(T0 + 10, "s".into(), &[1]);
        record_at(T0, "s".into(), &[2]);
        // The second is too old now, but the first still sits in front of it.
        record_at(T0 + 3_975_005, "s".into(), &[3]);
        assert_eq!(stamps(), [T0 + 10, T0, T0 + 3_975_005]);
        // Once the first is too old as well, both go.
        record_at(T0 + 3_975_011, "s".into(), &[4]);
        assert_eq!(stamps(), [T0 + 3_975_005, T0 + 3_975_011]);
        clear();
    }

    #[test]
    fn the_size_cap_lets_the_oldest_go() {
        let _turn = test_turn();
        clear();
        let big = vec![0u8; 40 * 1024 * 1024];
        for n in 0..3 {
            record_at(T0 + n, "s".into(), &big);
        }
        assert_eq!(stamps(), [T0 + 1, T0 + 2]);
        assert_eq!(ring().size, 80 * 1024 * 1024);
        clear();

        record_at(T0, "s".into(), &[1]);
        record_at(T0 + 1, "s".into(), &vec![0u8; BYTE_CAP + 1]);
        assert!(stamps().is_empty());
        assert_eq!(ring().size, 0);
        clear();
    }

    #[test]
    fn snapshot_leaves_the_ring_alone_and_clear_empties_it() {
        let _turn = test_turn();
        clear();
        record_at(T0, "a".into(), &[1, 2]);
        record_at(T0 + 1, "b".into(), &[3]);
        let first = snapshot();
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].stream, "a");
        assert_eq!(first[1].bytes, [3]);
        assert_eq!(stamps().len(), 2);
        clear();
        assert!(stamps().is_empty());
        assert_eq!(ring().size, 0);
    }
}
