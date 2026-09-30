use std::collections::VecDeque;

use parking_lot::Mutex;

use crate::capture::captured_payload::CapturedPayload;
use crate::capture::framing;

/// .NET epoch offset: milliseconds between 0001-01-01 and 1970-01-01.
const DOTNET_EPOCH_OFFSET_MS: i64 = 62135596800000;
const MAX_PING_MS: i32 = 9999;
const MIN_PING_RS_BYTES: usize = 12;
const MAX_HISTORY: usize = 10_000;
/// Physical size of the client's ping request frame. Its body is encrypted, but
/// nothing else the client sends is this size: 12 bytes while the timestamp was
/// wall-clock, 13 since it became an arbitrary-epoch clock. Once every ten
/// seconds either way.
const PING_RQ_FRAME_BYTES: std::ops::RangeInclusive<usize> = 12..=13;
/// The same frames as a relay tunnel wraps them: `05 23 <len>` then the frame.
const RELAYED_PING_RQ: [[u8; 4]; 2] = [[0x05, 0x23, 0x0C, 0x0F], [0x05, 0x23, 0x0D, 0x10]];
/// How far a request's clock offset may move between pings and still be the
/// same clock.
const OFFSET_TOLERANCE_MS: i64 = 15;

pub struct PingTracker {
    inner: Mutex<Inner>,
}

struct Inner {
    last_ping: Option<i32>,
    history: Vec<(i64, i32)>,
    /// Capture times of recent ping requests, oldest first.
    requests: VecDeque<i64>,
    /// Local capture clock minus the client's ping clock, once two pings agree
    /// on it.
    clock_offset: Option<i64>,
    /// Offsets the previous response's candidate requests implied.
    prev_offsets: Vec<i64>,
}

impl PingTracker {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                last_ping: None,
                history: Vec::new(),
                requests: VecDeque::new(),
                clock_offset: None,
                prev_offsets: Vec::new(),
            }),
        }
    }

    /// `server_port` is the locked combat port, which tells the two directions
    /// apart.
    pub fn on_packet(&self, cap: &CapturedPayload, server_port: u16) {
        if cap.dst_port == server_port && cap.src_port != server_port {
            if has_ping_rq(&cap.data) {
                let mut inner = self.inner.lock();
                inner.requests.push_back(cap.captured_at_ms);
                while inner
                    .requests
                    .front()
                    .is_some_and(|t| cap.captured_at_ms - t > MAX_PING_MS as i64)
                {
                    inner.requests.pop_front();
                }
            }
            return;
        }
        if cap.data.len() >= MIN_PING_RS_BYTES {
            self.try_ping_rs(&cap.data, cap.captured_at_ms);
        }
    }

    fn try_ping_rs(&self, data: &[u8], arrival_ms: i64) {
        let mut i = 0;
        while i + MIN_PING_RS_BYTES <= data.len() {
            if data[i] == 0x03
                && data[i + 1] == 0x36
                && data[i + 2] == 0x00
                && data[i + 3] == 0x00
            {
                let client_sent_raw = read_i64_le(data, i + 4);
                let mut inner = self.inner.lock();

                // The echoed timestamp used to be wall-clock .NET milliseconds,
                // which the local clock can be subtracted from directly. Newer
                // clients echo a clock with an arbitrary epoch, so fall back to
                // timing the response against the request that caused it.
                let legacy_rtt = arrival_ms
                    .wrapping_sub(client_sent_raw.wrapping_sub(DOTNET_EPOCH_OFFSET_MS));
                let rtt_ms = if is_valid_rtt(legacy_rtt) {
                    Some(legacy_rtt)
                } else {
                    inner.rtt_from_request(client_sent_raw, arrival_ms)
                };

                if let Some(rtt_ms) = rtt_ms {
                    let rtt_ms = rtt_ms as i32;
                    inner.last_ping = Some(rtt_ms);
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64;
                    inner.history.push((now, rtt_ms));
                    if inner.history.len() > MAX_HISTORY {
                        inner.history.remove(0);
                    }
                }
                i += 12;
            } else {
                i += 1;
            }
        }
    }

    pub fn current_ping_ms(&self) -> Option<i32> {
        self.inner.lock().last_ping
    }

    pub fn get_ping_history(&self, start_ms: i64, end_ms: i64) -> Vec<(i64, i32)> {
        self.inner.lock().history.iter()
            .filter(|(ts, _)| *ts >= start_ms && *ts <= end_ms)
            .cloned()
            .collect()
    }

    pub fn reset(&self) {
        let mut inner = self.inner.lock();
        inner.last_ping = None;
        inner.history.clear();
        inner.requests.clear();
        inner.clock_offset = None;
        inner.prev_offsets.clear();
    }
}

impl Inner {
    /// Round trip for a response whose echoed timestamp `client_sent` is on a
    /// clock we cannot read directly.
    ///
    /// Each recent request implies an offset between that clock and ours. The
    /// real one repeats from ping to ping, so an offset is trusted once two
    /// consecutive pings agree on it, and from then on the echoed timestamp
    /// alone is enough.
    fn rtt_from_request(&mut self, client_sent: i64, arrival_ms: i64) -> Option<i64> {
        let offsets: Vec<i64> = self
            .requests
            .iter()
            .filter(|&&t| (0..=MAX_PING_MS as i64).contains(&(arrival_ms - t)))
            .map(|&t| t.wrapping_sub(client_sent))
            .collect();
        let near = |o: i64, others: &[i64]| {
            others
                .iter()
                .copied()
                .find(|x| x.wrapping_sub(o).wrapping_abs() <= OFFSET_TOLERANCE_MS)
        };
        let rtt = |o: i64| arrival_ms.wrapping_sub(client_sent.wrapping_add(o));

        let mut result = None;
        if let Some(known) = self.clock_offset {
            // Follow the request when there is one, so clock drift cannot build up.
            let offset = near(known, &offsets).unwrap_or(known);
            if is_valid_rtt(rtt(offset)) {
                self.clock_offset = Some(offset);
                result = Some(rtt(offset));
            }
        }
        if result.is_none() {
            let confirmed = offsets
                .iter()
                .copied()
                .find(|&o| near(o, &self.prev_offsets).is_some());
            if let Some(offset) = confirmed {
                tracing::info!("Ping clock offset learned: {}ms", offset);
                self.clock_offset = confirmed;
            }
            let lone = if offsets.len() == 1 { Some(offsets[0]) } else { None };
            result = confirmed.or(lone).map(rtt).filter(|&r| is_valid_rtt(r));
        }
        self.prev_offsets = offsets;
        result
    }
}

fn is_valid_rtt(rtt_ms: i64) -> bool {
    (1..=MAX_PING_MS as i64).contains(&rtt_ms)
}

fn has_ping_rq(data: &[u8]) -> bool {
    framing::walk(data).frames.iter().any(|f| PING_RQ_FRAME_BYTES.contains(&f.len()))
        || data.windows(4).any(|w| RELAYED_PING_RQ.iter().any(|rq| w == rq))
}

fn read_i64_le(data: &[u8], offset: usize) -> i64 {
    let mut v: i64 = 0;
    for j in 0..8 {
        v |= (data[offset + j] as i64) << (j * 8);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER: u16 = 13328;
    const CLIENT: u16 = 50000;

    fn cap(src_port: u16, dst_port: u16, at: i64, data: Vec<u8>) -> CapturedPayload {
        CapturedPayload {
            src_port,
            dst_port,
            data,
            device_name: None,
            captured_at_ms: at,
            src_ip: None,
            dst_ip: None,
            tcp_seq: 0,
            tcp_ack: 0,
        }
    }

    fn request(at: i64) -> CapturedPayload {
        let mut data = vec![0x10];
        data.extend_from_slice(&[0xAB; 12]);
        cap(CLIENT, SERVER, at, data)
    }

    fn response(at: i64, client_sent: i64) -> CapturedPayload {
        let mut data = vec![0x18, 0x03, 0x36, 0x00, 0x00];
        data.extend_from_slice(&client_sent.to_le_bytes());
        data.extend_from_slice(&at.to_le_bytes());
        cap(SERVER, CLIENT, at, data)
    }

    #[test]
    fn wall_clock_timestamp_needs_no_request() {
        let t = PingTracker::new();
        let now = 1_790_000_000_000;
        t.on_packet(&response(now + 80, now + DOTNET_EPOCH_OFFSET_MS), SERVER);
        assert_eq!(t.current_ping_ms(), Some(80));
    }

    #[test]
    fn arbitrary_epoch_is_timed_against_the_request() {
        let t = PingTracker::new();
        let now = 1_790_000_000_000;
        // As captured 2026-10-01: a client clock ~203 days old, pings 10s apart.
        let clock = 17_552_452_660;
        t.on_packet(&request(now), SERVER);
        t.on_packet(&response(now + 64, clock), SERVER);
        assert_eq!(t.current_ping_ms(), Some(64));

        t.on_packet(&request(now + 10_000), SERVER);
        t.on_packet(&response(now + 10_090, clock + 10_000), SERVER);
        assert_eq!(t.current_ping_ms(), Some(90));

        // With the offset learned, a response whose request was never seen
        // still resolves.
        t.on_packet(&response(now + 20_045, clock + 20_000), SERVER);
        assert_eq!(t.current_ping_ms(), Some(45));
    }

    #[test]
    fn arbitrary_epoch_without_a_request_reports_nothing() {
        let t = PingTracker::new();
        t.on_packet(&response(1_790_000_000_064, 17_552_452_660), SERVER);
        assert_eq!(t.current_ping_ms(), None);
    }

    #[test]
    fn relay_wrapped_request_is_recognised() {
        let mut data = vec![0x01, 0x33, 0x00, 0x00, 0x05, 0x22, 0x02, 0x02, 0x2B];
        data.extend_from_slice(&RELAYED_PING_RQ[0]);
        data.extend_from_slice(&[0xCD; 11]);
        data.extend_from_slice(&[0x05, 0x25, 0x01, 0x01]);
        assert!(has_ping_rq(&data));
    }
}
