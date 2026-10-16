//! Round-trip time to the game server, from the client's ping requests and
//! the server's replies that echo the client's send time.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicI32, Ordering};

use parking_lot::Mutex;

use crate::capture::tcp_chunk::TcpChunk;
use crate::capture::varint::varint_at;
use crate::clock::PerfClock;

/// Unreal adds a fixed 16,777,216 s to its counter before it goes on the wire.
const ENGINE_CLOCK_BASE_MS: i64 = 16_777_216_000;
/// KR/TW clients echo .NET wall time: ms since 0001-01-01.
const YEAR_ONE_TO_UNIX_MS: i64 = 62_135_596_800_000;
const LONGEST_ROUND_TRIP_MS: i64 = 9_999;
/// How long a request stays a match for a later reply.
const REQUEST_LIFETIME_MS: i64 = 9_999;
const HISTORY_CAP: usize = 10_000;
/// Two offsets this close are the same clock.
const SAME_CLOCK_MS: u64 = 15;
/// The counter and a paired request may disagree this much.
const COUNTER_SLACK_MS: u32 = 25;
const COUNTER_STRIKES: u8 = 2;
/// The reply frame's opcode bytes, right before the echoed time.
const REPLY_MARK: &[u8] = b"\x03\x36\x00\x00";

pub struct PingTracker {
    perf_clock: Option<PerfClock>,
    /// Last good reading, 0 for none (a reading is never 0).
    shown: AtomicI32,
    book: Mutex<Ledger>,
}

#[derive(Default)]
struct Ledger {
    history: VecDeque<(i64, i32)>,
    /// Capture times of outgoing requests, oldest first.
    sent_at: VecDeque<i64>,
    /// Client clock minus echoed clock, once two replies agree on it.
    learned_offset: Option<i64>,
    /// Every offset the last reply's candidate requests gave.
    prior_offsets: Vec<i64>,
    counter_strikes: u8,
    counter_banned: bool,
}

impl PingTracker {
    pub fn new() -> Self {
        Self::using_clock(None)
    }

    pub fn using_clock(perf_clock: Option<PerfClock>) -> Self {
        PingTracker { perf_clock, shown: AtomicI32::new(0), book: Mutex::new(Ledger::default()) }
    }

    pub fn observe_segment(&self, cap: &TcpChunk, server_port: u16) {
        let mut book = self.book.lock();
        if cap.src_port != server_port && cap.dst_port == server_port {
            if looks_like_ping_request(&cap.data) {
                book.remember_request(cap.captured_at_ms);
            }
        } else {
            echoed_times(&cap.data).into_iter().for_each(|echoed| self.handle_reply(&mut book, cap.captured_at_ms, echoed));
        }
    }

    pub fn current_ping_ms(&self) -> Option<i32> {
        Some(self.shown.load(Ordering::Relaxed)).filter(|&ms| ms != 0)
    }

    pub fn latency_between(&self, start_ms: i64, end_ms: i64) -> Vec<(i64, i32)> {
        let mut picked = Vec::new();
        for &(at, ms) in &self.book.lock().history {
            if start_ms <= at && at <= end_ms {
                picked.push((at, ms));
            }
        }
        picked
    }

    pub fn reset(&self) {
        // The counter's strikes and ban are about this machine, not the
        // connection, so they stay.
        let mut book = self.book.lock();
        let Ledger { history, sent_at, learned_offset, prior_offsets, .. } = &mut *book;
        *learned_offset = None;
        prior_offsets.clear();
        sent_at.clear();
        history.clear();
        self.shown.store(0, Ordering::Relaxed);
    }

    fn handle_reply(&self, book: &mut Ledger, arrival: i64, echoed: i64) {
        let since = |sent: Option<i64>| sent.and_then(|s| arrival.checked_sub(s)).and_then(whole_ping);

        let counter = match self.perf_clock {
            Some(read) if !book.counter_banned => since(sent_by_counter(read(), echoed)),
            _ => None,
        };
        let chosen = book.pair_with_request(arrival, echoed);
        let paired = since(chosen.and_then(|offset| echoed.checked_add(offset)));
        let counter = book.weigh_counter(counter, paired);
        let wall = since(echoed.checked_sub(YEAR_ONE_TO_UNIX_MS));
        let learned = since(book.learned_offset.and_then(|offset| echoed.checked_add(offset)));

        let Some(ping) = [counter, paired, wall, learned].into_iter().flatten().next() else {
            return;
        };
        self.shown.store(ping, Ordering::Relaxed);
        if book.history.len() == HISTORY_CAP {
            book.history.pop_front();
        }
        book.history.push_back((crate::clock::system_ms(), ping));
    }
}

/// Wall time the reply's request left, by the engine counter: the echoed time
/// is the counter at send, and one reading of (counter, wall) maps it over.
fn sent_by_counter((counter_now, wall_now): (i64, i64), echoed: i64) -> Option<i64> {
    let at_send = echoed.checked_sub(ENGINE_CLOCK_BASE_MS).filter(|c| (0..=counter_now).contains(c))?;
    wall_now.checked_sub(counter_now - at_send)
}

impl Ledger {
    fn remember_request(&mut self, at: i64) {
        self.sent_at.retain(|&older| at.saturating_sub(older) <= REQUEST_LIFETIME_MS);
        self.sent_at.push_back(at);
    }

    /// Checks the counter against a paired request; a counter that keeps
    /// disagreeing is wrong on this machine and is not asked again.
    fn weigh_counter(&mut self, counter: Option<i32>, paired: Option<i32>) -> Option<i32> {
        match (counter, paired) {
            (Some(c), Some(p)) if c.abs_diff(p) <= COUNTER_SLACK_MS => {
                self.counter_strikes = 0;
                Some(c)
            }
            (Some(_), Some(_)) => {
                self.counter_strikes = self.counter_strikes.saturating_add(1);
                if self.counter_strikes >= COUNTER_STRIKES && !self.counter_banned {
                    self.counter_banned = true;
                    tracing::warn!("ping: performance counter disagrees with requests, no longer used");
                }
                None
            }
            (c, _) => c,
        }
    }

    /// Picks the request this reply answers and returns its offset, and keeps
    /// the learned offset up to date either way.
    fn pair_with_request(&mut self, arrival: i64, echoed: i64) -> Option<i64> {
        let mut found: Vec<(i64, i64)> = Vec::new();
        for &at in &self.sent_at {
            let waited = arrival.saturating_sub(at);
            if let (true, Some(offset)) = ((0..=REQUEST_LIFETIME_MS).contains(&waited), at.checked_sub(echoed)) {
                found.push((at, offset));
            }
        }
        found.sort_unstable();
        let offsets: Vec<i64> = found.iter().map(|&(_, offset)| offset).collect();
        let close = |a: i64, b: i64| a.abs_diff(b) <= SAME_CLOCK_MS;
        let near_any = |list: &[i64], offset: i64| list.iter().any(|&o| close(o, offset));

        let chosen = if offsets.len() == 1 {
            Some(offsets[0])
        } else {
            let learned: Vec<i64> = self.learned_offset.into_iter().collect();
            let first_near = |list: &[i64]| offsets.iter().copied().find(|&o| near_any(list, o));
            first_near(&learned).or_else(|| first_near(&self.prior_offsets))
        };

        if let Some(offset) = chosen {
            let had = self.learned_offset;
            let keep = had.is_some_and(|l| close(l, offset)) || near_any(&self.prior_offsets, offset);
            self.learned_offset = keep.then_some(offset);
            match (had, keep) {
                (None, true) => tracing::info!("ping: learned clock offset {offset} ms"),
                (Some(_), false) => tracing::info!("ping: clock offset jumped, learning it again"),
                _ => {}
            }
        }
        self.prior_offsets = offsets;
        chosen
    }
}

fn whole_ping(ms: i64) -> Option<i32> {
    i32::try_from(ms).ok().filter(|&v| (1..=LONGEST_ROUND_TRIP_MS).contains(&(v as i64)))
}

/// A ping request inside a relay tunnel: header `05 23 <len>`, then the
/// request's own length byte, which is the relay length plus 3.
fn is_relayed_request(four: &[u8]) -> bool {
    four.starts_with(b"\x05\x23") && matches!(four[2], 0x0C | 0x0D) && four[3] == four[2] + 3
}

/// Frame sizes of a payload that splits cleanly from first byte to last.
fn frame_sizes(data: &[u8]) -> Option<Vec<usize>> {
    let mut sizes = Vec::new();
    let mut rest = data;
    while !rest.is_empty() {
        let head = varint_at(rest, 0);
        if head.length <= 0 {
            return None;
        }
        let size = usize::try_from(head.value as i64 - 3).ok().filter(|&s| s > 0 && s <= rest.len())?;
        sizes.push(size);
        rest = &rest[size..];
    }
    Some(sizes)
}

/// An outgoing payload carries a ping request when it holds a relayed one, or
/// cuts cleanly into frames and one of them has a request's size.
fn looks_like_ping_request(data: &[u8]) -> bool {
    data.windows(4).any(is_relayed_request)
        || frame_sizes(data).is_some_and(|sizes| sizes.contains(&12) || sizes.contains(&13))
}

/// The echoed client times of every reply in an incoming payload, in order.
fn echoed_times(data: &[u8]) -> Vec<i64> {
    let mut times = Vec::new();
    let mut at = 0;
    while let Some(window) = data.get(at..at + 12) {
        let (mark, time) = window.split_at(4);
        if mark == REPLY_MARK {
            times.push(i64::from_le_bytes(time.try_into().unwrap_or_default()));
            at += 12;
        } else {
            at += 1;
        }
    }
    times
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::built::{reply_frame, reply_payload, segment};

    const GAME_PORT: u16 = 13_328;
    const LOCAL_PORT: u16 = 50_000;
    const N: i64 = 1_790_000_000_000;
    /// An echoed time in the engine's own epoch.
    const C: i64 = 17_500_000_000;
    const JUMP: i64 = 2_320;
    /// The .NET echo of wall time N.
    const NET_N: i64 = N + YEAR_ONE_TO_UNIX_MS;

    fn rq_frame() -> Vec<u8> {
        [&b"\x10"[..], &[0xAB; 12]].concat()
    }

    fn beat_frame() -> Vec<u8> {
        [&b"\x0E"[..], &[0xCD; 10]].concat()
    }

    fn engine(counter_ms: i64) -> i64 {
        ENGINE_CLOCK_BASE_MS + counter_ms
    }

    fn clock_at_one_million() -> (i64, i64) {
        (1_000_000, N)
    }

    /// A (counter, wall) reading 800,000,000 ms into the engine's epoch, on a
    /// clock where counter 700,000,000 was wall time 1_790_000_100_000.
    fn clock_late_in_a_session() -> (i64, i64) {
        (800_000_000, 1_790_000_100_000 - 700_000_000 + 800_000_000)
    }

    /// A tracker on the game port, driven step by step.
    struct Rig(PingTracker);

    impl Rig {
        fn plain() -> Self {
            Rig(PingTracker::new())
        }
        fn counted() -> Self {
            Rig(PingTracker::using_clock(Some(clock_at_one_million)))
        }
        fn send(&self, at: i64) -> &Self {
            self.0.observe_segment(&segment(None, LOCAL_PORT, GAME_PORT, &rq_frame(), at), GAME_PORT);
            self
        }
        fn hear(&self, at: i64, echoed: i64) -> &Self {
            self.0.observe_segment(&segment(None, GAME_PORT, LOCAL_PORT, &reply_frame(echoed, at), at), GAME_PORT);
            self
        }
        fn shows(&self, ms: i32) -> &Self {
            assert_eq!(self.0.current_ping_ms(), Some(ms));
            self
        }
        fn blank(&self) -> &Self {
            assert!(self.0.current_ping_ms().is_none(), "expected no ping");
            self
        }
        fn pings(&self) -> Vec<i32> {
            self.0.latency_between(i64::MIN, i64::MAX).into_iter().map(|(_, ms)| ms).collect()
        }
        fn ledger(&self) -> parking_lot::MutexGuard<'_, Ledger> {
            self.0.book.lock()
        }
    }

    /// Two paired replies 10 s apart, so offset N - C is learned.
    fn learned_rig() -> Rig {
        let rig = Rig::plain();
        rig.send(N).hear(N + 64, C).send(N + 10_000).hear(N + 10_064, C + 10_000).shows(64);
        assert_eq!(rig.ledger().learned_offset, Some(N - C));
        rig
    }

    // Section 3 cases.

    #[test]
    fn dotnet_echo_reads_on_its_own() {
        Rig::plain().blank().hear(N + 80, NET_N).shows(80);
    }

    #[test]
    fn engine_echo_pairs_with_request() {
        Rig::plain().send(N).hear(N + 64, C).shows(64);
    }

    #[test]
    fn engine_echo_without_request_is_nothing() {
        Rig::plain().hear(N + 64, C).blank().hear(1_790_000_000_064, C).blank();
    }

    #[test]
    fn zero_round_trip_is_no_reading() {
        Rig::plain().send(N).hear(N, C).blank();
    }

    #[test]
    fn newer_request_pushes_out_old_one() {
        Rig::plain().send(N).send(N + 10_000).hear(N + 9_000, C).blank();
    }

    #[test]
    fn requests_within_window_stay() {
        let rig = Rig::plain();
        rig.send(N).send(N + 9_999);
        assert_eq!(rig.ledger().sent_at, [N, N + 9_999]);
        rig.send(N + 10_000);
        assert_eq!(rig.ledger().sent_at, [N + 9_999, N + 10_000]);
    }

    #[test]
    fn two_candidates_pick_by_last_reply() {
        Rig::plain()
            .send(N)
            .send(N + 5_000)
            .hear(N + 5_100, C)
            .blank()
            .send(N + 10_000)
            .send(N + 15_000)
            .hear(N + 15_100, C + 10_000)
            .shows(5_100);
    }

    #[test]
    fn back_to_back_replies_in_order() {
        let rig = Rig::plain();
        let both = [reply_frame(NET_N - 10, N), reply_frame(NET_N - 20, N)].concat();
        rig.0.observe_segment(&segment(None, GAME_PORT, LOCAL_PORT, &both, N), GAME_PORT);
        rig.shows(20);
        assert_eq!(rig.pings(), [10, 20]);
    }

    #[test]
    fn reply_mark_without_frame_byte_and_short_payloads() {
        let rig = Rig::plain();
        let short = [REPLY_MARK, &[0u8; 7]].concat();
        rig.0.observe_segment(&segment(None, GAME_PORT, LOCAL_PORT, &short, N), GAME_PORT);
        rig.blank();
        let bare = reply_payload(N - 33);
        rig.0.observe_segment(&segment(None, GAME_PORT, LOCAL_PORT, &bare, N), GAME_PORT);
        rig.shows(33);
    }

    #[test]
    fn replies_on_outgoing_side_are_ignored() {
        let rig = Rig::plain();
        rig.0.observe_segment(&segment(None, LOCAL_PORT, GAME_PORT, &reply_frame(NET_N, 0), N + 80), GAME_PORT);
        rig.blank();
    }

    #[test]
    fn requests_on_incoming_side_are_not_kept() {
        let rig = Rig::plain();
        rig.0.observe_segment(&segment(None, GAME_PORT, LOCAL_PORT, &rq_frame(), N), GAME_PORT);
        rig.hear(N + 64, C).blank();
        assert!(rig.ledger().sent_at.is_empty());
    }

    #[test]
    fn equal_ports_read_for_replies_only() {
        let rig = Rig::plain();
        rig.0.observe_segment(&segment(None, GAME_PORT, GAME_PORT, &rq_frame(), N), GAME_PORT);
        assert!(rig.ledger().sent_at.is_empty());
        rig.0.observe_segment(&segment(None, GAME_PORT, GAME_PORT, &reply_frame(NET_N, 0), N + 80), GAME_PORT);
        rig.shows(80);
    }

    #[test]
    fn round_trip_limits() {
        let read: Vec<Option<i32>> = [-5, 0, 1, 9_999, 10_000, i64::MAX].into_iter().map(whole_ping).collect();
        assert_eq!(read, [None, None, Some(1), Some(9_999), None, None]);
    }

    #[test]
    fn extreme_echoed_times_do_not_panic() {
        for rig in [Rig::plain(), Rig::counted()] {
            rig.hear(N + 50, i64::MIN).hear(N + 50, i64::MAX).hear(i64::MIN, i64::MAX).hear(i64::MAX, i64::MIN).blank();
            rig.send(N).hear(N + 50, i64::MIN).hear(N + 50, i64::MAX);
            rig.send(i64::MIN).send(i64::MAX).hear(i64::MAX, i64::MIN).hear(i64::MIN, i64::MIN);
        }
    }

    // Section 4 cases.

    #[test]
    fn engine_echo_learns_offset() {
        Rig::plain()
            .send(N)
            .hear(N + 64, C)
            .shows(64)
            .send(N + 10_000)
            .hear(N + 10_090, C + 10_000)
            .shows(90)
            .hear(N + 20_045, C + 20_000)
            .shows(45);
    }

    #[test]
    fn clock_jump_drops_and_relearns_offset() {
        let rig = Rig::plain();
        rig.send(N).hear(N + 60, C).shows(60).send(N + 10_000).hear(N + 10_060, C + 10_000).shows(60);
        rig.send(N + 20_000 + JUMP).hear(N + 20_070 + JUMP, C + 20_000).shows(70);
        assert_eq!(rig.ledger().learned_offset, None);
        rig.hear(N + 30_055 + JUMP, C + 30_000).shows(70);
        rig.send(N + 40_000 + JUMP).hear(N + 40_080 + JUMP, C + 40_000).shows(80);
        rig.send(N + 50_000 + JUMP).hear(N + 50_080 + JUMP, C + 50_000).shows(80);
        rig.hear(N + 60_045 + JUMP, C + 60_000).shows(45);
    }

    #[test]
    fn request_wins_over_dotnet_echo() {
        Rig::plain().send(N + JUMP).hear(N + JUMP + 65, NET_N).shows(65);
    }

    #[test]
    fn request_detection() {
        let mut big = vec![0x7F; 4096];
        big[0] = 0xFF;
        big[100] = 0x10;
        let cases: [(&str, Vec<u8>, bool); 9] = [
            ("request", rq_frame(), true),
            ("heartbeat then request", [beat_frame(), rq_frame()].concat(), true),
            ("heartbeat", beat_frame(), false),
            ("first frame past the end", big, false),
            ("empty", Vec::new(), false),
            ("varint that does not read", b"\xFF\xFF\xFF\xFF\xFF".to_vec(), false),
            ("frame of size 0", b"\x03".to_vec(), false),
            ("trailing byte", [rq_frame(), b"\x05".to_vec()].concat(), false),
            ("12-byte request", [&b"\x0F"[..], &[0xAB; 11]].concat(), true),
        ];
        for (what, data, want) in cases {
            assert_eq!(looks_like_ping_request(&data), want, "{what}");
        }
    }

    #[test]
    fn relayed_request_detection() {
        let wrapped = b"\x01\x33\x00\x00\x05\x22\x02\x02\x2B\x05\x23\x0C\x0F\xCD\xCD\xCD\xCD\xCD\xCD\xCD\xCD\xCD\xCD\xCD\x05\x25\x01\x01";
        assert!(looks_like_ping_request(wrapped));
        assert!(looks_like_ping_request(b"\x00\x05\x23\x0D\x10\x00"));
        assert!(!looks_like_ping_request(b"\x00\x05\x23\x0D\x0F\x00"));
    }

    #[test]
    fn counter_reads_alone() {
        Rig::counted().hear(1_789_999_990_064, engine(990_000)).shows(64);
    }

    #[test]
    fn counter_leaves_dotnet_echo_alone() {
        Rig::counted().hear(N + 80, NET_N).shows(80);
    }

    #[test]
    fn counter_late_in_a_session() {
        let late_reply_echo = engine(700_000_000);
        Rig(PingTracker::using_clock(Some(clock_late_in_a_session))).hear(1_790_000_100_170, late_reply_echo).shows(170);
    }

    #[test]
    fn disagreeing_counter_is_banned() {
        let w = 1_789_999_990_000;
        let rig = Rig::counted();
        for step in 0..3 {
            let at = w + step * 10_000;
            rig.send(at).hear(at + 70, engine(989_500 + step * 10_000)).shows(70);
        }
        rig.0.reset();
        rig.hear(1_789_999_990_064, engine(990_000)).blank();
    }

    // Section 6 cases.

    #[test]
    fn counter_needs_engine_base() {
        Rig::counted().hear(1_789_999_990_064, ENGINE_CLOCK_BASE_MS - 1).blank();
    }

    #[test]
    fn counter_from_the_future_is_nothing() {
        // Exactly the counter now still reads.
        Rig::counted().hear(1_789_999_990_064, engine(1_000_001)).blank().hear(1_790_000_000_064, engine(1_000_000)).shows(64);
    }

    #[test]
    fn one_agreeing_reply_clears_strikes() {
        let a0 = N - 1_000_000;
        let rig = Rig::counted();
        // Counter 70, request 570: a strike, the request's reading shows.
        rig.send(a0 + 979_500).hear(a0 + 980_070, engine(980_000)).shows(570);
        // Both 60: strikes back to 0.
        rig.send(a0 + 990_000).hear(a0 + 990_060, engine(990_000)).shows(60);
        assert_eq!(rig.ledger().counter_strikes, 0);
        // Counter 570, request 70: one strike again.
        rig.send(a0 + 1_000_000).hear(a0 + 1_000_070, engine(999_500)).shows(70);
        // Still trusted: a lone reply reads from the counter.
        rig.hear(a0 + 990_064, engine(990_000)).shows(64);
    }

    #[test]
    fn history_records_each_reading() {
        let rig = Rig::plain();
        let before = crate::clock::system_ms();
        rig.hear(N + 80, NET_N).hear(N + 90, NET_N);
        let after = crate::clock::system_ms();
        let all = rig.0.latency_between(i64::MIN, i64::MAX);
        assert_eq!(rig.pings(), [80, 90]);
        assert!(all.iter().all(|&(at, _)| before <= at && at <= after));
        // A reply with no reading adds nothing.
        rig.hear(N + 64, C);
        assert_eq!(rig.pings().len(), 2);
    }

    #[test]
    fn history_bounds_are_inclusive() {
        let rig = Rig::plain();
        rig.hear(N + 80, NET_N);
        let only = rig.0.latency_between(i64::MIN, i64::MAX)[0];
        let at = only.0;
        assert_eq!(rig.0.latency_between(at, at), [only]);
        assert!(rig.0.latency_between(at + 1, i64::MAX).is_empty());
        assert!(rig.0.latency_between(i64::MIN, at - 1).is_empty());
    }

    #[test]
    fn history_keeps_newest_ten_thousand() {
        let rig = Rig::plain();
        for i in 0..10_001i64 {
            rig.hear(N + 1 + i % 9_000, NET_N);
        }
        let kept = rig.pings();
        assert_eq!(kept.len(), 10_000);
        assert_eq!((kept[0], kept[9_999]), (2, 1_001));
    }

    #[test]
    fn reset_clears_all_but_counter_ban() {
        let w = 1_789_999_990_000;
        let rig = Rig::counted();
        rig.send(w).hear(w + 70, engine(989_500)).send(w + 10_000).hear(w + 10_070, engine(999_500)).send(w + 15_000);
        assert!(rig.ledger().learned_offset.is_some());
        rig.0.reset();
        rig.blank();
        assert!(rig.pings().is_empty());
        {
            let book = rig.ledger();
            assert!(book.sent_at.is_empty() && book.prior_offsets.is_empty());
            assert_eq!(book.learned_offset, None);
            assert!(book.counter_banned);
        }
        // Without the kept request or offset, an engine echo gives nothing.
        rig.hear(w + 15_064, C).blank();
    }

    #[test]
    fn reset_keeps_strike_count() {
        let w = 1_789_999_990_000;
        let rig = Rig::counted();
        rig.send(w).hear(w + 70, engine(989_500));
        rig.0.reset();
        assert_eq!(rig.ledger().counter_strikes, 1);
        rig.send(w + 10_000).hear(w + 10_070, engine(999_500));
        assert!(rig.ledger().counter_banned);
    }

    #[test]
    fn two_candidates_pick_by_learned_offset() {
        let rig = learned_rig();
        // A lone reply reads the learned offset and empties the last-reply list.
        rig.hear(N + 20_050, C + 20_000).shows(50);
        assert!(rig.ledger().prior_offsets.is_empty());
        rig.send(N + 23_000).send(N + 30_000).hear(N + 30_070, C + 30_000).shows(70);
    }

    #[test]
    fn two_candidates_matching_nothing() {
        let rig = learned_rig();
        // Offsets 7,000 and 2,000 off the learned one: no pick, no reading.
        rig.send(N + 23_000).send(N + 28_000).hear(N + 28_100, C + 30_000).shows(64);
        // With nothing picked, the learned offset stays.
        assert_eq!(rig.ledger().learned_offset, Some(N - C));
        rig.hear(N + 40_055, C + 40_000).shows(55);
    }

    #[test]
    fn learned_offset_follows_drift() {
        let rig = learned_rig();
        // The server's echo runs 10 ms behind: offset grows by 10.
        rig.send(N + 20_000).hear(N + 20_075, C + 19_990).shows(75);
        assert_eq!(rig.ledger().learned_offset, Some(N - C + 10));
        rig.hear(N + 30_050, C + 30_000).shows(40);
    }

    #[test]
    fn single_reply_does_not_learn() {
        let rig = Rig::plain();
        rig.send(N).hear(N + 64, C);
        let book = rig.ledger();
        assert_eq!((book.learned_offset, book.prior_offsets.clone()), (None, vec![N - C]));
    }

    #[test]
    fn new_and_no_clock_match() {
        for tracker in [PingTracker::new(), PingTracker::using_clock(None)] {
            Rig(tracker).blank().hear(1_789_999_990_064, engine(990_000)).blank();
        }
    }

    #[test]
    fn tracker_is_shareable() {
        fn shareable<T: Send + Sync>() {}
        shareable::<PingTracker>();
    }
}
