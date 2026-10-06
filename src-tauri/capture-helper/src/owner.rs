//! Only the connections of the user who started the helper pass.
//!
//! The helper holds `cap_net_raw`, so any user on the machine could run it.
//! It passes on a packet only when one end of it is a TCP socket that user
//! owns, as `/proc/net/tcp` and `/proc/net/tcp6` list them. Everything else
//! (other users, system services) is dropped and only counted.
//!
//! Capture never waits for the tables. A packet of a connection not known yet
//! is held, and another thread reads the tables and passes on the held packets
//! that are ours. A capture thread that waits is not reading its buffer, and
//! the kernel drops what no longer fits.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

use crate::Segment;

/// One end of a connection: IPv4 address and port.
pub type End = ([u8; 4], u16);

/// A socket's connection as `/proc/net/tcp` lists it: (local, remote).
pub type Flow = (End, End);

/// The socket tables are read at most this often.
pub const REFRESH_MS: i64 = 500;

/// A connection found to be someone else's is not looked up again for this
/// long.
pub const FOREIGN_MS: i64 = 1_000;

/// The most payload held for a read of the tables. Past it, packets of
/// connections not known yet are dropped, and counted.
pub const HELD_MAX_BYTES: usize = 16 << 20;

/// Where the socket tables and the time come from: the system, or a test.
pub trait Source {
    fn now_ms(&mut self) -> i64;
    /// The text of `/proc/net/tcp` and `/proc/net/tcp6`, one after the other.
    fn read(&mut self) -> String;
}

/// The real tables and clock.
pub struct Proc;

impl Source for Proc {
    fn now_ms(&mut self) -> i64 {
        crate::now_ms()
    }

    fn read(&mut self) -> String {
        let mut text = String::new();
        for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
            match std::fs::read_to_string(path) {
                Ok(t) => text.push_str(&t),
                Err(e) => crate::log(crate::Level::Warn, &format!("Cannot read {path}: {e}")),
            }
        }
        text
    }
}

/// The sockets of `uid` in the text of `/proc/net/tcp` or `/proc/net/tcp6`.
/// IPv6 sockets count only with IPv4-mapped addresses (`::ffff:a.b.c.d`):
/// the capture reads IPv4 only.
pub fn parse_sockets(text: &str, uid: u32, out: &mut HashSet<Flow>) {
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 8 || !fields[0].ends_with(':') {
            continue; // the heading
        }
        if fields[7].parse::<u32>().ok() != Some(uid) {
            continue;
        }
        if let (Some(local), Some(remote)) = (parse_end(fields[1]), parse_end(fields[2])) {
            out.insert((local, remote));
        }
    }
}

/// `0100007F:0CEA` (IPv4) or 32 hex digits and a port (IPv6). The kernel
/// prints each 32-bit word of the address as a number in this machine's
/// byte order, so `to_ne_bytes` gives the address bytes back.
fn parse_end(text: &str) -> Option<End> {
    let (address, port) = text.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let word = |i: usize| -> Option<[u8; 4]> {
        Some(u32::from_str_radix(address.get(i * 8..i * 8 + 8)?, 16).ok()?.to_ne_bytes())
    };
    match address.len() {
        8 => Some((word(0)?, port)),
        32 => {
            let head = [word(0)?, word(1)?, word(2)?].concat();
            (head == [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF]).then_some((word(3)?, port))
        }
        _ => None,
    }
}

pub struct Owners {
    uid: u32,
    owned: HashSet<Flow>,
    /// When the last read of the tables started; `i64::MIN` before the first.
    read_at: i64,
    /// Connections (lower end first) found to be someone else's, and until when.
    foreign: HashMap<(End, End), i64>,
    /// Packets of connections not known yet, in the order they came, until a
    /// read of the tables says whose they are.
    held: VecDeque<Segment>,
    held_bytes: usize,
    /// The connections in `held`. Their later packets queue behind them.
    waiting: HashSet<(End, End)>,
    /// Packets dropped as someone else's, for the log.
    pub left_out: u64,
    /// Packets dropped because too much was held already, for the log.
    pub overflow: u64,
}

/// A segment's connection both ways: (source, destination) and back.
fn flows(segment: &Segment) -> (Flow, Flow) {
    let forward = ((segment.src_ip, segment.src_port), (segment.dst_ip, segment.dst_port));
    (forward, (forward.1, forward.0))
}

impl Owners {
    pub fn new(uid: u32) -> Self {
        Self {
            uid,
            owned: HashSet::new(),
            read_at: i64::MIN,
            foreign: HashMap::new(),
            held: VecDeque::new(),
            held_bytes: 0,
            waiting: HashSet::new(),
            left_out: 0,
            overflow: 0,
        }
    }

    /// `segment` back if it belongs to a socket of this user. Loopback traffic
    /// has a socket at both ends; either one being this user's is enough. A
    /// packet the tables cannot judge yet is held for `update`.
    pub fn admit(&mut self, segment: Segment, now: i64) -> Option<Segment> {
        let (forward, back) = flows(&segment);
        if self.is_owned(forward, back) {
            return Some(segment);
        }
        let connection = forward.min(back);
        if self.waiting.contains(&connection) {
            self.hold(connection, segment);
            return None;
        }
        if self.foreign.get(&connection).is_some_and(|&until| now < until) {
            self.left_out += 1;
            return None;
        }
        // A read that started after the packet was captured already shows
        // its socket, if it is this user's: the socket exists before its
        // packets do. An older read may not, so hold the packet for the next
        // read rather than drop a packet that may be ours.
        if self.read_at > segment.captured_at_ms {
            self.mark_foreign(connection, now);
            return None;
        }
        self.hold(connection, segment);
        None
    }

    /// How many packets are held.
    pub fn held(&self) -> usize {
        self.held.len()
    }

    /// When the tables should be read next; `None` while nothing is held.
    pub fn read_due(&self) -> Option<i64> {
        (!self.held.is_empty()).then(|| self.read_at.saturating_add(REFRESH_MS))
    }

    /// Take a read of the tables that started at `read_at`, when the first
    /// `held_before` held packets were already held. Returns the held packets
    /// that are ours, in the order they came.
    pub fn update(&mut self, read_at: i64, held_before: usize, text: &str) -> Vec<Segment> {
        self.read_at = read_at;
        let mut owned = HashSet::new();
        parse_sockets(text, self.uid, &mut owned);
        self.owned = owned;
        self.foreign.retain(|_, until| read_at < *until);

        let held = std::mem::take(&mut self.held);
        self.held_bytes = 0;
        self.waiting.clear();
        let mut ours = Vec::new();
        for (i, segment) in held.into_iter().enumerate() {
            let (forward, back) = flows(&segment);
            let connection = forward.min(back);
            // Held before the read started, or captured before it: the read
            // shows its socket if it is ours.
            let judged = i < held_before || segment.captured_at_ms < read_at;
            if self.is_owned(forward, back) {
                ours.push(segment);
            } else if self.foreign.contains_key(&connection) {
                self.left_out += 1;
            } else if judged && !self.waiting.contains(&connection) {
                self.mark_foreign(connection, read_at);
            } else {
                self.hold(connection, segment);
            }
        }
        ours
    }

    fn is_owned(&self, forward: Flow, back: Flow) -> bool {
        self.owned.contains(&forward) || self.owned.contains(&back)
    }

    fn hold(&mut self, connection: (End, End), segment: Segment) {
        if self.held_bytes + segment.data.len() > HELD_MAX_BYTES {
            self.overflow += 1;
            return;
        }
        self.held_bytes += segment.data.len();
        self.waiting.insert(connection);
        self.held.push_back(segment);
    }

    fn mark_foreign(&mut self, connection: (End, End), now: i64) {
        self.foreign.insert(connection, now + FOREIGN_MS);
        self.left_out += 1;
    }
}

/// `Owners`, shared by the capture threads and the thread that reads the
/// tables.
pub struct Gate {
    owners: Mutex<Owners>,
    /// Wakes the reading thread when a packet is held.
    wake: Condvar,
}

impl Gate {
    pub fn new(uid: u32) -> Self {
        Self { owners: Mutex::new(Owners::new(uid)), wake: Condvar::new() }
    }

    fn lock(&self) -> MutexGuard<'_, Owners> {
        self.owners.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// From a capture thread: `segment` back if it may pass now. Never waits
    /// for the tables.
    pub fn packet(&self, segment: Segment, now: i64) -> Option<Segment> {
        let mut owners = self.lock();
        let was_empty = owners.held() == 0;
        let pass = owners.admit(segment, now);
        if was_empty && owners.held() > 0 {
            self.wake.notify_one();
        }
        pass
    }

    /// Packets dropped so far: someone else's, and held past the limit.
    pub fn counts(&self) -> (u64, u64) {
        let owners = self.lock();
        (owners.left_out, owners.overflow)
    }

    /// Read the tables whenever a held packet needs it, and give `pass` the
    /// held packets that are ours.
    pub fn look_up(&self, source: &mut impl Source, mut pass: impl FnMut(Segment)) -> ! {
        loop {
            self.read_once(source, &mut pass);
        }
    }

    /// Wait until a read is due, read the tables, and pass on what is ours.
    fn read_once(&self, source: &mut impl Source, pass: &mut impl FnMut(Segment)) {
        let mut owners = self.lock();
        loop {
            let now = source.now_ms();
            owners = match owners.read_due() {
                None => self.wake.wait(owners).unwrap_or_else(|e| e.into_inner()),
                Some(at) if now < at => {
                    let wait = Duration::from_millis((at - now) as u64);
                    self.wake.wait_timeout(owners, wait).unwrap_or_else(|e| e.into_inner()).0
                }
                Some(_) => break,
            };
        }
        let held_before = owners.held();
        // Capture goes on while the tables are read.
        drop(owners);
        let read_at = source.now_ms();
        let text = source.read();
        let mut owners = self.lock();
        // Under the lock, so they go before any later packet of theirs.
        for segment in owners.update(read_at, held_before, &text) {
            pass(segment);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n";

    /// A socket line as the kernel prints it on this machine.
    fn line(local: End, remote: End, uid: u32) -> String {
        let hex = |(ip, port): End| format!("{:08X}:{:04X}", u32::from_ne_bytes(ip), port);
        format!(
            "   0: {} {} 01 00000000:00000000 00:00000000 00000000 {uid:>5}        0 4242 1 0000000000000000 20 4 30 10 -1\n",
            hex(local),
            hex(remote)
        )
    }

    fn line6(local: End, remote: End, uid: u32) -> String {
        let hex = |(ip, port): End| {
            let words = [[0u8; 4], [0; 4], [0, 0, 0xFF, 0xFF], ip];
            let address: String = words.iter().map(|w| format!("{:08X}", u32::from_ne_bytes(*w))).collect();
            format!("{address}:{port:04X}")
        };
        format!(
            "   1: {} {} 01 00000000:00000000 00:00000000 00000000 {uid:>5}        0 4243 1 0000000000000000 20 4 30 10 -1\n",
            hex(local),
            hex(remote)
        )
    }

    const ME: End = ([10, 0, 0, 2], 51000);
    const SERVER: End = ([193, 202, 112, 99], 13328);

    #[test]
    fn reads_ipv4_and_mapped_ipv6_sockets_of_one_user() {
        let mut text = HEAD.to_string();
        text += &line(ME, SERVER, 1000);
        text += &line(([10, 0, 0, 2], 51001), SERVER, 1001);
        // Literal kernel text (little-endian): 127.0.0.1:3306 listening, uid 1000.
        text += "   2: 0100007F:0CEA 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 99 1 0 100 0 0 10 0\n";
        text += &line6(([10, 0, 0, 2], 52000), ([1, 2, 3, 4], 443), 1000);
        // A plain IPv6 socket: the capture never sees these.
        text += "   3: 000080FE00000000FF00000001000000:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 98 1 0 100 0 0 10 0\n";
        let mut owned = HashSet::new();
        parse_sockets(&text, 1000, &mut owned);
        let expected: HashSet<Flow> = [
            (ME, SERVER),
            (([127, 0, 0, 1], 3306), ([0, 0, 0, 0], 0)),
            (([10, 0, 0, 2], 52000), ([1, 2, 3, 4], 443)),
        ]
        .into();
        assert_eq!(owned, expected);
    }

    fn segment(src: End, dst: End, at: i64) -> Segment {
        Segment {
            device: "dev".into(),
            captured_at_ms: at,
            src_ip: src.0,
            dst_ip: dst.0,
            src_port: src.1,
            dst_port: dst.1,
            seq: 0,
            ack: 0,
            data: vec![1],
        }
    }

    /// The `n`th packet of a stream, by its sequence number.
    fn nth(src: End, dst: End, at: i64, n: u32) -> Segment {
        Segment { seq: n, ..segment(src, dst, at) }
    }

    fn seqs(segments: &[Segment]) -> Vec<u32> {
        segments.iter().map(|s| s.seq).collect()
    }

    /// Owners that read `text` at `at`.
    fn read_at(at: i64, text: &str) -> Owners {
        let mut owners = Owners::new(1000);
        assert!(owners.update(at, 0, text).is_empty());
        owners
    }

    #[test]
    fn own_connections_pass_both_ways_and_others_do_not() {
        let mut owners = read_at(10_000, &(HEAD.to_string() + &line(ME, SERVER, 1000) + &line(([10, 0, 0, 2], 51001), SERVER, 1001)));
        assert!(owners.admit(segment(SERVER, ME, 9_999), 10_000).is_some(), "from the server");
        assert!(owners.admit(segment(ME, SERVER, 9_999), 10_000).is_some(), "to the server");
        assert!(owners.admit(segment(SERVER, ([10, 0, 0, 2], 51001), 9_999), 10_000).is_none(), "another user's");
        assert!(owners.admit(segment(SERVER, ([10, 0, 0, 3], 51000), 9_999), 10_000).is_none(), "no socket at all");
        assert_eq!(owners.left_out, 2);
        assert_eq!(owners.held(), 0);
    }

    #[test]
    fn loopback_passes_when_either_socket_is_ours() {
        let client = ([127, 0, 0, 1], 40000);
        let service = ([127, 0, 0, 1], 5432);
        // Our client, someone else's service: both directions pass.
        let mut text = HEAD.to_string() + &line(client, service, 1000) + &line(service, client, 0);
        let mut owners = read_at(10_000, &text);
        assert!(owners.admit(segment(client, service, 9_000), 10_000).is_some());
        assert!(owners.admit(segment(service, client, 9_000), 10_000).is_some());
        // Two sockets of someone else's on loopback: neither passes.
        let other = ([127, 0, 0, 1], 40001);
        text += &line(other, service, 0);
        text += &line(service, other, 0);
        assert!(owners.admit(segment(other, service, 10_100), 10_100).is_none());
        assert!(owners.admit(segment(service, other, 10_100), 10_100).is_none());
        assert!(owners.update(10_500, 2, &text).is_empty());
        assert_eq!(owners.left_out, 2);
    }

    #[test]
    fn a_new_connection_is_held_not_waited_for_and_passes_in_order() {
        let known = ([10, 0, 0, 2], 50000);
        let mut text = HEAD.to_string() + &line(known, SERVER, 1000);
        let mut owners = read_at(10_000, &text);
        assert_eq!(owners.read_due(), None);

        // Our connection opens after that read. Its packets are held for the
        // next one, and the call returns at once.
        assert!(owners.admit(nth(SERVER, ME, 10_100, 1), 10_100).is_none());
        assert_eq!(owners.read_due(), Some(10_000 + REFRESH_MS));
        // A known connection is not held up meanwhile.
        assert!(owners.admit(segment(SERVER, known, 10_101), 10_101).is_some());
        assert!(owners.admit(nth(ME, SERVER, 10_150, 2), 10_150).is_none());
        assert_eq!(owners.held(), 2);

        text += &line(ME, SERVER, 1000);
        assert_eq!(seqs(&owners.update(10_500, 2, &text)), [1, 2]);
        assert_eq!(owners.read_due(), None);
        assert!(owners.admit(nth(SERVER, ME, 10_600, 3), 10_600).is_some());
        assert_eq!(owners.left_out, 0);
    }

    #[test]
    fn someone_elses_packet_waits_for_a_newer_read_then_is_left_out() {
        let stranger = ([10, 0, 0, 9], 1234);
        let mut owners = read_at(10_000, HEAD);
        // Captured before the read: that read judges it.
        assert!(owners.admit(segment(stranger, SERVER, 9_990), 10_000).is_none());
        assert_eq!((owners.left_out, owners.held()), (1, 0));
        // Known as someone else's for a while: not held again.
        assert!(owners.admit(segment(SERVER, stranger, 10_200), 10_200).is_none());
        assert_eq!((owners.left_out, owners.held()), (2, 0));

        // Once that has expired, the next read judges it, even one that
        // starts in the same millisecond it was captured.
        let at = 10_000 + FOREIGN_MS;
        assert!(owners.admit(segment(stranger, SERVER, at), at).is_none());
        assert_eq!(owners.held(), 1);
        assert!(owners.update(at, 1, HEAD).is_empty());
        assert_eq!((owners.left_out, owners.held()), (3, 0));
    }

    #[test]
    fn a_packet_captured_after_a_read_started_waits_for_the_next_read() {
        let stranger = ([10, 0, 0, 9], 1234);
        let mut owners = read_at(10_000, HEAD);
        assert!(owners.admit(segment(stranger, SERVER, 10_100), 10_100).is_none());
        // The read starts at 10_500 with one packet held. Our new connection's
        // first packet comes while it runs, after it started.
        assert!(owners.admit(nth(SERVER, ME, 10_600, 1), 10_600).is_none());
        let text = HEAD.to_string() + &line(ME, SERVER, 1000);
        assert!(owners.update(10_500, 1, HEAD).is_empty());
        assert_eq!(owners.left_out, 1, "the stranger");
        assert_eq!(owners.held(), 1, "ours, not judged by an older read");
        assert_eq!(owners.read_due(), Some(10_500 + REFRESH_MS));
        assert!(owners.admit(nth(ME, SERVER, 10_700, 2), 10_700).is_none(), "behind the first");
        assert_eq!(seqs(&owners.update(11_000, 2, &text)), [1, 2]);
    }

    #[test]
    fn a_closed_socket_is_forgotten_by_the_next_read() {
        let mut owners = read_at(10_000, &(HEAD.to_string() + &line(ME, SERVER, 1000)));
        assert!(owners.update(10_500, 0, HEAD).is_empty());
        assert!(owners.admit(segment(SERVER, ME, 10_400), 10_600).is_none());
        assert_eq!(owners.left_out, 1);
    }

    #[test]
    fn too_much_held_is_dropped_and_counted() {
        let mut owners = read_at(10_000, HEAD);
        let big = |port: u16| Segment { data: vec![0; HELD_MAX_BYTES / 2 - 1], ..segment(SERVER, ([10, 0, 0, 2], port), 10_100) };
        for port in [1, 2, 3] {
            assert!(owners.admit(big(port), 10_100).is_none());
        }
        assert_eq!((owners.held(), owners.overflow), (2, 1));
    }

    /// The real clock, and tables that take a second to read.
    struct Slow(String);

    impl Source for Slow {
        fn now_ms(&mut self) -> i64 {
            crate::now_ms()
        }
        fn read(&mut self) -> String {
            std::thread::sleep(Duration::from_secs(1));
            self.0.clone()
        }
    }

    #[test]
    fn capture_does_not_wait_while_the_tables_are_read() {
        let gate = std::sync::Arc::new(Gate::new(1000));
        let (passed, out) = std::sync::mpsc::channel();
        let reader = gate.clone();
        let text = HEAD.to_string() + &line(ME, SERVER, 1000);
        std::thread::spawn(move || reader.look_up(&mut Slow(text), move |s| passed.send(s).unwrap()));

        // The first packet of a connection not known yet starts a read.
        let start = std::time::Instant::now();
        let now = crate::now_ms();
        assert!(gate.packet(nth(SERVER, ME, now, 0), now).is_none());
        std::thread::sleep(Duration::from_millis(100));
        for n in 1..100 {
            let now = crate::now_ms();
            assert!(gate.packet(nth(SERVER, ME, now, n), now).is_none());
        }
        assert!(start.elapsed() < Duration::from_millis(700), "waited {:?} for a read", start.elapsed());

        let got: Vec<u32> = (0..100).map(|_| out.recv_timeout(Duration::from_secs(10)).unwrap().seq).collect();
        assert_eq!(got, (0..100).collect::<Vec<_>>());
        assert_eq!(gate.counts(), (0, 0));
    }
}
