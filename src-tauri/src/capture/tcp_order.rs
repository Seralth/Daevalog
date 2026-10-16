//! Puts one direction of a TCP connection back in order by sequence number.
//!
//! On a bad link the server sends a segment again when an ack goes missing,
//! and libpcap sees both copies. Fed twice, the parser counts every hit in it
//! twice. A segment may also arrive before one sent ahead of it that was lost
//! on the way and comes again later. This hands the parser each byte once, in
//! the order sent.

/// A segment further than this from where the stream stands is taken for a
/// new connection on the same ports, not for a late or early segment.
const RESYNC_BYTES: u32 = 1 << 20;
/// A gap not filled within this long, or with this much waiting behind it, is
/// given up: the segment was lost to the capture, not the network, and will
/// not come again. Ordinary retransmits on a 2-second ping came within 2.4 s.
const GAP_WAIT_MS: i64 = 5_000;
const GAP_WAIT_BYTES: usize = 256 * 1024;

#[derive(Default)]
pub struct TcpOrder {
    /// The sequence number of the next byte the parser has not had.
    next: Option<u32>,
    /// Segments past a gap, as they came.
    held: Vec<(u32, Vec<u8>)>,
    /// When the first held segment came.
    held_since_ms: i64,
}

/// Where `seq` lies from `from`, across the wrap at 2^32.
fn distance(from: u32, seq: u32) -> i64 {
    seq.wrapping_sub(from) as i32 as i64
}

impl TcpOrder {
    pub fn new() -> Self {
        Self::default()
    }

    /// The bytes of this segment the parser has not had yet, with any held
    /// segments they make whole. Empty for a repeat or a segment past a gap.
    pub fn accept(&mut self, seq: u32, data: &[u8], now_ms: i64) -> Vec<u8> {
        if data.is_empty() {
            return Vec::new();
        }
        let Some(next) = self.next else {
            self.next = Some(seq.wrapping_add(data.len() as u32));
            return data.to_vec();
        };
        let at = distance(next, seq);
        if at.unsigned_abs() > RESYNC_BYTES as u64 {
            let mut out = self.give_up_gap();
            out.extend_from_slice(data);
            self.next = Some(seq.wrapping_add(data.len() as u32));
            return out;
        }
        if at > 0 {
            if self.held.is_empty() {
                self.held_since_ms = now_ms;
            }
            self.held.push((seq, data.to_vec()));
            let waiting: usize = self.held.iter().map(|(_, d)| d.len()).sum();
            if now_ms - self.held_since_ms >= GAP_WAIT_MS || waiting >= GAP_WAIT_BYTES {
                return self.give_up_gap();
            }
            return Vec::new();
        }
        let mut out = Vec::new();
        self.take(seq, data, &mut out);
        self.drain_held(&mut out);
        out
    }

    /// Appends the part of a segment past `next`; the segment starts at or
    /// before `next`.
    fn take(&mut self, seq: u32, data: &[u8], out: &mut Vec<u8>) {
        let next = self.next.expect("started");
        let skip = (-distance(next, seq)) as usize;
        if skip < data.len() {
            out.extend_from_slice(&data[skip..]);
            self.next = Some(seq.wrapping_add(data.len() as u32));
        }
    }

    /// Hands on held segments that now follow on.
    fn drain_held(&mut self, out: &mut Vec<u8>) {
        loop {
            let next = self.next.expect("started");
            let Some(i) = self.held.iter().position(|(seq, _)| distance(next, *seq) <= 0) else { break };
            let (seq, data) = self.held.swap_remove(i);
            self.take(seq, &data, out);
        }
    }

    /// Skips the gap to the earliest held segment and hands on what follows.
    fn give_up_gap(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        while let Some(next) = self.next {
            let Some(&(first, _)) = self.held.iter().min_by_key(|(seq, _)| distance(next, *seq)) else { break };
            if distance(next, first) > 0 {
                self.next = Some(first);
            }
            self.drain_held(&mut out);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_repeat_is_dropped() {
        let mut order = TcpOrder::new();
        assert_eq!(order.accept(1000, b"abcd", 0), b"abcd");
        assert_eq!(order.accept(1000, b"abcd", 400), b"");
        assert_eq!(order.accept(1004, b"ef", 500), b"ef");
        // A repeat of something older than the last segment.
        assert_eq!(order.accept(1000, b"abcd", 600), b"");
    }

    #[test]
    fn a_partial_overlap_hands_on_only_the_new_bytes() {
        let mut order = TcpOrder::new();
        assert_eq!(order.accept(1000, b"abcd", 0), b"abcd");
        // Resent with more bytes added: "cd" again, then "efg".
        assert_eq!(order.accept(1002, b"cdefg", 100), b"efg");
        assert_eq!(order.accept(1007, b"h", 200), b"h");
    }

    #[test]
    fn a_segment_past_a_gap_waits_for_the_gap() {
        let mut order = TcpOrder::new();
        assert_eq!(order.accept(1000, b"ab", 0), b"ab");
        // "cd" was lost on the way; "ef" and "gh" come first.
        assert_eq!(order.accept(1004, b"ef", 10), b"");
        assert_eq!(order.accept(1006, b"gh", 20), b"");
        assert_eq!(order.accept(1002, b"cd", 900), b"cdefgh");
        assert_eq!(order.accept(1004, b"ef", 950), b"", "a held segment sent again");
        assert_eq!(order.accept(1008, b"i", 1000), b"i");
    }

    #[test]
    fn a_gap_that_never_fills_is_given_up() {
        let mut order = TcpOrder::new();
        assert_eq!(order.accept(1000, b"ab", 0), b"ab");
        assert_eq!(order.accept(1004, b"ef", 10), b"");
        assert_eq!(order.accept(1008, b"ij", 2_000), b"");
        // The capture lost "cd" and "gh": past the wait, the rest goes on in order.
        assert_eq!(order.accept(1010, b"kl", 10 + GAP_WAIT_MS), b"efijkl");
        assert_eq!(order.accept(1012, b"m", 6_000), b"m");
    }

    #[test]
    fn sequence_numbers_wrap() {
        let mut order = TcpOrder::new();
        let start = u32::MAX - 2;
        assert_eq!(order.accept(start, b"abc", 0), b"abc");
        assert_eq!(order.accept(0, b"def", 10), b"def");
        assert_eq!(order.accept(start, b"abc", 20), b"", "a repeat from before the wrap");
        assert_eq!(order.accept(u32::MAX, b"cdefg", 30), b"g", "an overlap across the wrap");
        // Out of order across the wrap.
        let mut order = TcpOrder::new();
        assert_eq!(order.accept(start - 2, b"xy", 0), b"xy");
        assert_eq!(order.accept(1, b"ef", 10), b"");
        assert_eq!(order.accept(start, b"abcd", 20), b"abcdef");
    }

    #[test]
    fn a_far_jump_starts_over() {
        let mut order = TcpOrder::new();
        assert_eq!(order.accept(1000, b"ab", 0), b"ab");
        assert_eq!(order.accept(1000 + 3 * RESYNC_BYTES, b"cd", 10), b"cd");
        assert_eq!(order.accept(1002 + 3 * RESYNC_BYTES, b"e", 20), b"e");
    }
}
