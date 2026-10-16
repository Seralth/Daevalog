//! The capture device the player picked, as a preference rather than a
//! lock, and following the game's connection when it moves: to another
//! device (an Ethernet cable plugged in, a VPN turned off) or to a new
//! connection on the same one (another Wi-Fi). The game reconnects by
//! itself after such a switch; the meter follows within `FOLLOW_MS` and
//! keeps the new connection's first packets while it waits, so its records
//! are read once the lock moves over. Used by the dispatcher.

use std::collections::{HashMap, VecDeque};

use super::tcp_chunk::TcpChunk;

/// How long the picked device, or the locked connection, may carry none of
/// the game's packets before the meter follows the game elsewhere. The game
/// sends its record marker many times a second even while idle, so 12 s of
/// silence is a connection that is gone, not a lag spike; and it is longer
/// than the capture's 10 s port filter (`FILTER_PORT_QUIET_MS`), which must
/// widen first when the new connection uses another server port.
pub const FOLLOW_MS: i64 = 12_000;

/// A waiting connection's packets are kept from this long before the locked
/// one went quiet: the game may open the new connection a moment before the
/// old one ends.
const KEEP_BEFORE_QUIET_MS: i64 = 3_000;

/// At most this much of the waiting connections' packets is kept.
const MAX_WAITING_BYTES: usize = 16 << 20;

/// A connection, by device and ports (server port first).
type FlowId = (Option<String>, u16, u16);

#[derive(Default)]
struct Waiting {
    packets: VecDeque<(i64, TcpChunk)>,
    /// Packets with the game's marker in the current rate window, and when
    /// the last came.
    hits: u32,
    last_hit_ms: i64,
    last_ms: i64,
}

/// What to do with a packet off the locked connection.
#[derive(Debug)]
pub enum Next {
    /// Leave it out (it may be kept for later).
    Drop,
    /// The game moved here: lock this device and server port, and read
    /// these packets first, the oldest first.
    Follow { device: Option<String>, port: u16, packets: Vec<TcpChunk> },
}

pub struct Follow {
    pick: Option<String>,
    /// When the picked device last carried the game's packets, or was picked.
    pick_seen_ms: i64,
    locked: Option<(Option<String>, u16)>,
    /// When the locked connection last carried a packet, or was locked.
    locked_seen_ms: i64,
    waiting: HashMap<FlowId, Waiting>,
    bytes: usize,
    /// The game's rate: this many marked packets within `window_ms`.
    threshold: u32,
    window_ms: i64,
}

pub(super) fn same_device(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.trim().eq_ignore_ascii_case(b.trim()),
        _ => false,
    }
}

impl Follow {
    /// Starting at `now`: a picked device has `FOLLOW_MS` to show the game
    /// before another may be locked.
    pub fn new(now: i64, threshold: u32, window_ms: i64) -> Self {
        Self {
            pick: None,
            pick_seen_ms: now,
            locked: None,
            locked_seen_ms: now,
            waiting: HashMap::new(),
            bytes: 0,
            threshold,
            window_ms,
        }
    }

    /// The picked device and the lock, as they are now.
    pub fn sync(&mut self, now: i64, pick: Option<&str>, locked: Option<(Option<&str>, u16)>) {
        if self.pick.as_deref() != pick {
            self.pick = pick.map(str::to_string);
            self.pick_seen_ms = now;
        }
        let same_lock = match (&self.locked, locked) {
            (None, None) => true,
            (Some((device, port)), Some((d, p))) => *port == p && device.as_deref() == d,
            _ => false,
        };
        if !same_lock {
            self.locked = locked.map(|(d, p)| (d.map(str::to_string), p));
            self.locked_seen_ms = now;
            self.waiting.clear();
            self.bytes = 0;
        }
    }

    pub fn is_pick(&self, device: Option<&str>) -> bool {
        same_device(self.pick.as_deref(), device)
    }

    /// The pick is the locked device and carries a game packet.
    pub fn note_locked_packet(&mut self, now: i64) {
        self.locked_seen_ms = now;
        if self.locked.as_ref().is_some_and(|(d, _)| self.is_pick(d.as_deref())) {
            self.pick_seen_ms = now;
        }
    }

    /// A packet with the game's marker on `device`, before a lock.
    pub fn note_game(&mut self, now: i64, device: Option<&str>) {
        if self.is_pick(device) {
            self.pick_seen_ms = now;
        }
    }

    /// No device is picked, or the picked one is not there (`pick_absent`)
    /// or has carried none of the game's packets for `FOLLOW_MS`.
    pub fn pick_quiet(&self, now: i64, pick_absent: bool) -> bool {
        self.pick.is_none() || pick_absent || now - self.pick_seen_ms >= FOLLOW_MS
    }

    /// Whether the game's connection on `device` may be locked.
    pub fn may_lock(&self, now: i64, device: Option<&str>, pick_absent: bool) -> bool {
        self.is_pick(device) || self.pick_quiet(now, pick_absent)
    }

    /// A packet off the locked connection, while one is locked. `game`: it
    /// carries the game's marker. A connection that carries the game at its
    /// rate is followed at once when it is on the picked device and the lock
    /// is not; otherwise once the locked connection has been quiet for
    /// `FOLLOW_MS` and the pick allows it (`may_lock`).
    pub fn elsewhere(&mut self, now: i64, cap: TcpChunk, game: bool, pick_absent: bool) -> Next {
        let device = cap.device_name.as_deref().map(str::trim).filter(|d| !d.is_empty()).map(str::to_string);
        let id: FlowId = (device.clone(), cap.src_port, cap.dst_port);
        if game && self.is_pick(device.as_deref()) {
            self.pick_seen_ms = now;
        }
        if !game && !self.waiting.contains_key(&id) {
            return Next::Drop;
        }
        if !self.waiting.contains_key(&id) {
            self.waiting.retain(|_, w| now - w.last_ms < FOLLOW_MS);
            self.bytes = self.waiting.values().flat_map(|w| w.packets.iter()).map(|(_, p)| p.data.len()).sum();
        }
        let keep_from = self.locked_seen_ms - KEEP_BEFORE_QUIET_MS;
        let waiting = self.waiting.entry(id.clone()).or_default();
        if game {
            if now - waiting.last_hit_ms > self.window_ms {
                waiting.hits = 0;
            }
            waiting.hits += 1;
            waiting.last_hit_ms = now;
        }
        waiting.last_ms = now;
        while waiting.packets.front().is_some_and(|(at, _)| *at < keep_from) {
            if let Some((_, old)) = waiting.packets.pop_front() {
                self.bytes -= old.data.len();
            }
        }
        if self.bytes + cap.data.len() <= MAX_WAITING_BYTES {
            self.bytes += cap.data.len();
            waiting.packets.push_back((now, cap));
        }
        if waiting.hits < self.threshold {
            return Next::Drop;
        }
        let locked_device = self.locked.as_ref().and_then(|(d, _)| d.clone());
        let back_to_pick = self.is_pick(device.as_deref()) && !self.is_pick(locked_device.as_deref());
        let quiet = now - self.locked_seen_ms >= FOLLOW_MS && self.may_lock(now, device.as_deref(), pick_absent);
        if !back_to_pick && !quiet {
            return Next::Drop;
        }
        let packets = self.waiting.remove(&id).map(|w| w.packets.into_iter().map(|(_, p)| p).collect()).unwrap_or_default();
        self.waiting.clear();
        self.bytes = 0;
        Next::Follow { device, port: id.1, packets }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 12;
    const WINDOW: i64 = 3_000;

    /// One of the game's payloads, server to client: `n`, then the marker.
    fn packet(device: &str, server: u16, client: u16, n: u8) -> TcpChunk {
        crate::built::segment(Some(device), server, client, &[n, 0x0E, 0x00, 0x36], 0)
    }

    /// Feeds the game's packets on `device` from `from` ms, 20 a second,
    /// until it follows; returns when and what.
    fn feed(follow: &mut Follow, device: &str, client: u16, from: i64, absent: bool) -> Option<(i64, Next)> {
        for i in 0..2_000 {
            let now = from + i * 50;
            match follow.elsewhere(now, packet(device, 13328, client, (i % 250) as u8), true, absent) {
                Next::Drop => {}
                next => return Some((now, next)),
            }
        }
        None
    }

    #[test]
    fn a_picked_device_gets_a_head_start_unless_it_is_missing() {
        let mut follow = Follow::new(0, RATE, WINDOW);
        follow.sync(0, Some("vpn0"), None);
        assert!(follow.may_lock(1_000, Some("vpn0"), false));
        assert!(!follow.may_lock(1_000, Some("eth0"), false), "the pick first");
        assert!(follow.may_lock(1_000, Some("eth0"), true), "not there: auto-detect at once");
        assert!(follow.may_lock(FOLLOW_MS, Some("eth0"), false), "no game on the pick for a while");
        // The pick carries the game: others wait again.
        follow.note_game(FOLLOW_MS, Some("VPN0"));
        assert!(!follow.may_lock(FOLLOW_MS + 1_000, Some("eth0"), false));
        // Auto-detect locks anything.
        let mut auto = Follow::new(0, RATE, WINDOW);
        auto.sync(0, None, None);
        assert!(auto.may_lock(0, Some("eth0"), false));
    }

    #[test]
    fn the_game_is_followed_to_a_new_device_with_its_first_packets() {
        // On Wi-Fi; an Ethernet cable goes in and the game reconnects there
        // at 10 s; the Wi-Fi connection's last packet was at 10 s.
        let mut follow = Follow::new(0, RATE, WINDOW);
        follow.sync(0, None, Some((Some("wlan0"), 13328)));
        follow.note_locked_packet(10_000);
        let (at, next) = feed(&mut follow, "eth0", 50001, 10_000, false).expect("follows");
        assert_eq!(at, 10_000 + FOLLOW_MS, "after the Wi-Fi connection was quiet for FOLLOW_MS");
        let Next::Follow { device, port, packets } = next else { panic!() };
        assert_eq!((device.as_deref(), port), (Some("eth0"), 13328));
        assert_eq!(packets.len() as i64, FOLLOW_MS / 50 + 1, "every packet since the new connection began");
        assert_eq!(packets[0].data[0], 0, "the first one first");
    }

    #[test]
    fn a_live_lock_is_not_left_for_a_copy_of_its_traffic() {
        // A ping reducer: the lock is on the loopback relay, and the same
        // game bytes show on the physical device too.
        let mut follow = Follow::new(0, RATE, WINDOW);
        follow.sync(0, None, Some((Some("lo"), 13328)));
        for i in 0..1_000 {
            let now = i * 50;
            follow.note_locked_packet(now);
            assert!(matches!(follow.elsewhere(now, packet("eth0", 13328, 50001, 1), true, false), Next::Drop));
        }
        // Only the last few seconds are kept meanwhile.
        assert!(follow.bytes <= (KEEP_BEFORE_QUIET_MS / 50 + 1) as usize * 4, "{}", follow.bytes);
    }

    #[test]
    fn a_new_connection_on_the_same_device_is_followed() {
        // Wi-Fi to another Wi-Fi: wlan0 stays, the game reconnects from a new
        // address and port, here to another server port.
        let mut follow = Follow::new(0, RATE, WINDOW);
        follow.sync(0, Some("wlan0"), Some((Some("wlan0"), 13328)));
        follow.note_locked_packet(5_000);
        let mut at = None;
        for i in 0..1_000 {
            let now = 5_000 + i * 50;
            if let Next::Follow { device, port, .. } = follow.elsewhere(now, packet("wlan0", 13329, 50002, 1), true, false) {
                assert_eq!((device.as_deref(), port), (Some("wlan0"), 13329));
                at = Some(now);
                break;
            }
        }
        assert_eq!(at, Some(5_000 + FOLLOW_MS));
    }

    #[test]
    fn the_meter_goes_back_to_the_picked_device_when_the_game_does() {
        // vpn0 picked; the VPN was off, so the meter follows eth0.
        let mut follow = Follow::new(0, RATE, WINDOW);
        follow.sync(0, Some("vpn0"), Some((Some("eth0"), 13328)));
        // The VPN is back and the game with it: at once, even while
        // eth0 still carries packets.
        follow.note_locked_packet(30_000);
        let (at, next) = feed(&mut follow, "vpn0", 50003, 30_000, false).expect("goes back");
        assert!(at < 30_000 + WINDOW, "{at}");
        assert!(matches!(next, Next::Follow { ref device, .. } if device.as_deref() == Some("vpn0")));
    }

    #[test]
    fn a_picked_device_that_stopped_carrying_the_game_is_left() {
        // vpn0 picked and locked; the VPN drops and the game reconnects
        // on eth0. The meter follows once vpn0 was quiet for
        // FOLLOW_MS; the pick itself stays.
        let mut follow = Follow::new(0, RATE, WINDOW);
        follow.sync(0, Some("vpn0"), Some((Some("vpn0"), 13328)));
        follow.note_locked_packet(20_000);
        let (at, next) = feed(&mut follow, "eth0", 50004, 20_000, false).expect("follows");
        assert_eq!(at, 20_000 + FOLLOW_MS);
        assert!(matches!(next, Next::Follow { ref device, .. } if device.as_deref() == Some("eth0")));
        follow.sync(at, Some("vpn0"), Some((Some("eth0"), 13328)));
        assert!(follow.is_pick(Some("vpn0")));
    }

    #[test]
    fn traffic_without_the_game_is_never_followed_or_kept() {
        let mut follow = Follow::new(0, RATE, WINDOW);
        follow.sync(0, None, Some((Some("wlan0"), 13328)));
        for i in 0..1_000 {
            let mut p = packet("eth0", 443, 50005, 1);
            p.data = vec![1, 2, 3];
            assert!(matches!(follow.elsewhere(20_000 + i * 50, p, false, false), Next::Drop));
        }
        assert_eq!(follow.bytes, 0);
    }
}
