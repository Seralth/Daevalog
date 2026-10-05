//! Only the connections of the user who started the helper pass.
//!
//! The helper holds `cap_net_raw`, so any user on the machine could run it.
//! It passes on a packet only when one end of it is a TCP socket that user
//! owns, as `/proc/net/tcp` and `/proc/net/tcp6` list them. Everything else
//! (other users, system services) is dropped and only counted.

use std::collections::{HashMap, HashSet};

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

/// Where the socket tables and the time come from: the system, or a test.
pub trait Source {
    fn now_ms(&mut self) -> i64;
    fn sleep_until(&mut self, at_ms: i64);
    /// The text of `/proc/net/tcp` and `/proc/net/tcp6`, one after the other.
    fn read(&mut self) -> String;
}

/// The real tables and clock.
pub struct Proc;

impl Source for Proc {
    fn now_ms(&mut self) -> i64 {
        crate::now_ms()
    }

    fn sleep_until(&mut self, at_ms: i64) {
        let wait = at_ms - crate::now_ms();
        if wait > 0 {
            std::thread::sleep(std::time::Duration::from_millis(wait as u64));
        }
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
    /// Packets dropped as someone else's, for the log.
    pub left_out: u64,
}

impl Owners {
    pub fn new(uid: u32) -> Self {
        Self { uid, owned: HashSet::new(), read_at: i64::MIN, foreign: HashMap::new(), left_out: 0 }
    }

    /// Whether `segment` belongs to a socket of this user. Loopback traffic
    /// has a socket at both ends; either one being this user's is enough.
    pub fn admit(&mut self, segment: &Segment, source: &mut impl Source) -> bool {
        let forward = ((segment.src_ip, segment.src_port), (segment.dst_ip, segment.dst_port));
        let back = (forward.1, forward.0);
        if self.is_owned(forward, back) {
            return true;
        }
        let connection = forward.min(back);
        let now = source.now_ms();
        if self.foreign.get(&connection).is_some_and(|&until| now < until) {
            self.left_out += 1;
            return false;
        }
        // A read that started after the packet was captured already shows
        // its socket, if it is this user's: the socket exists before its
        // packets do. An older read may not, so read again, waiting for the
        // next allowed read rather than drop a packet that may be ours.
        if self.read_at >= segment.captured_at_ms.saturating_add(1) {
            return self.left_out(connection, now);
        }
        let next = self.read_at.saturating_add(REFRESH_MS);
        if now < next {
            source.sleep_until(next);
        }
        self.refresh(source);
        if self.is_owned(forward, back) {
            return true;
        }
        let now = source.now_ms();
        self.left_out(connection, now)
    }

    fn is_owned(&self, forward: Flow, back: Flow) -> bool {
        self.owned.contains(&forward) || self.owned.contains(&back)
    }

    fn left_out(&mut self, connection: (End, End), now: i64) -> bool {
        self.foreign.insert(connection, now + FOREIGN_MS);
        self.left_out += 1;
        false
    }

    /// Read the tables again: closed sockets go, and so do expired entries.
    fn refresh(&mut self, source: &mut impl Source) {
        self.read_at = source.now_ms();
        let mut owned = HashSet::new();
        parse_sockets(&source.read(), self.uid, &mut owned);
        self.owned = owned;
        let now = self.read_at;
        self.foreign.retain(|_, until| now < *until);
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

    /// Fixed tables and a clock that only moves when told to.
    struct Fake {
        now: i64,
        text: String,
        reads: u32,
        slept_until: Option<i64>,
    }

    impl Source for Fake {
        fn now_ms(&mut self) -> i64 {
            self.now
        }
        fn sleep_until(&mut self, at_ms: i64) {
            self.slept_until = Some(at_ms);
            self.now = self.now.max(at_ms);
        }
        fn read(&mut self) -> String {
            self.reads += 1;
            self.text.clone()
        }
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

    fn fake(text: String) -> Fake {
        Fake { now: 10_000, text, reads: 0, slept_until: None }
    }

    #[test]
    fn own_connections_pass_both_ways_and_others_do_not() {
        let mut source = fake(HEAD.to_string() + &line(ME, SERVER, 1000) + &line(([10, 0, 0, 2], 51001), SERVER, 1001));
        let mut owners = Owners::new(1000);
        assert!(owners.admit(&segment(SERVER, ME, 9_999), &mut source), "from the server");
        assert!(owners.admit(&segment(ME, SERVER, 9_999), &mut source), "to the server");
        assert!(!owners.admit(&segment(SERVER, ([10, 0, 0, 2], 51001), 9_999), &mut source), "another user's");
        assert!(!owners.admit(&segment(SERVER, ([10, 0, 0, 3], 51000), 9_999), &mut source), "no socket at all");
        assert_eq!(owners.left_out, 2);
    }

    #[test]
    fn loopback_passes_when_either_socket_is_ours() {
        let client = ([127, 0, 0, 1], 40000);
        let service = ([127, 0, 0, 1], 5432);
        // Our client, someone else's service: both directions pass.
        let mut source = fake(HEAD.to_string() + &line(client, service, 1000) + &line(service, client, 0));
        let mut owners = Owners::new(1000);
        assert!(owners.admit(&segment(client, service, 9_000), &mut source));
        assert!(owners.admit(&segment(service, client, 9_000), &mut source));
        // Two sockets of someone else's on loopback: neither passes.
        let other = ([127, 0, 0, 1], 40001);
        source.text += &line(other, service, 0);
        source.text += &line(service, other, 0);
        source.now += REFRESH_MS;
        assert!(!owners.admit(&segment(other, service, source.now), &mut source));
        assert!(!owners.admit(&segment(service, other, source.now), &mut source));
    }

    #[test]
    fn reads_are_limited_and_a_new_connection_is_found_without_loss() {
        let mut source = fake(HEAD.to_string());
        let mut owners = Owners::new(1000);
        // Someone else's packet: one read, then the answer is kept.
        let stranger = ([10, 0, 0, 9], 1234);
        assert!(!owners.admit(&segment(stranger, SERVER, 9_990), &mut source));
        assert_eq!(source.reads, 1);
        assert!(!owners.admit(&segment(stranger, SERVER, 10_001), &mut source));
        assert!(!owners.admit(&segment(SERVER, stranger, 10_002), &mut source));
        assert_eq!(source.reads, 1, "known as someone else's: not read again");

        // Our connection opens 100 ms after that read. Its first packet waits
        // for the next allowed read instead of being dropped.
        source.now += 100;
        source.text += &line(ME, SERVER, 1000);
        assert!(owners.admit(&segment(SERVER, ME, source.now), &mut source));
        assert_eq!(source.reads, 2);
        assert_eq!(source.slept_until, Some(10_000 + REFRESH_MS));

        // A packet captured before the latest read is judged by that read.
        let late = ([10, 0, 0, 9], 1235);
        assert!(!owners.admit(&segment(late, SERVER, 10_000), &mut source));
        assert_eq!(source.reads, 2);

        // Once the stranger's entry has expired it is looked up again, and a
        // closed socket is forgotten by that read.
        source.now += FOREIGN_MS;
        source.text = HEAD.to_string();
        assert!(!owners.admit(&segment(stranger, SERVER, source.now), &mut source));
        assert_eq!(source.reads, 3);
        assert!(!owners.admit(&segment(SERVER, ME, source.now - 1), &mut source), "closed: no longer ours");
    }
}
