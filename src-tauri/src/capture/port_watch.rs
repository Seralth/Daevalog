//! Which captured connection is the game's: its server port and the device
//! it came on. The dispatch thread offers and locks ports here; the Capture
//! page reads the lock, sets the picked device and resets the lock.

use std::collections::HashMap;

use parking_lot::Mutex;
use tracing::info;

use super::live;

/// Off loopback, a port may lock only this long after the first port was
/// offered. With a ping reducer the game's bytes show twice: on the real
/// device towards the relay, and on loopback from the relay to the game. The
/// game reads the loopback copy, which shows a moment later; this wait lets
/// it win.
const OFF_LOOPBACK_WAIT_MS: i64 = 2_500;

#[derive(Default)]
struct Detection {
    pick: Option<String>,
    port: Option<u16>,
    device: Option<String>,
    /// Ports offered since the last lock or reset, with the first device
    /// each came on.
    offered: HashMap<u16, Option<String>>,
    /// When the first of them was offered.
    first_offer_ms: Option<i64>,
    /// When the game's bytes were last taken by the parser.
    parsed_ms: Option<i64>,
}

#[derive(Default)]
pub struct PortWatch {
    state: Mutex<Detection>,
}

/// A device label with its spaces cut, `None` when nothing is left.
fn label(device: Option<&str>) -> Option<String> {
    device.map(str::trim).filter(|d| !d.is_empty()).map(String::from)
}

/// Loopback as each OS names it: `lo` (Linux), `lo0` (macOS), and Npcap's
/// "Adapter for loopback traffic capture" (Windows).
fn loopback_label(device: &str) -> bool {
    let name = device.trim().to_ascii_lowercase();
    matches!(name.as_str(), "lo" | "lo0") || name.contains("loopback")
}

/// Narrows the capture to the locked port and says where the lock is. Called
/// with the detection held, as is every filter change, so the filter always
/// follows the last change to the lock even when the Capture page resets it
/// while the dispatch thread locks.
fn announce_lock(port: u16, device: Option<&str>) {
    live::set_filter_port(Some(port));
    info!("Game port locked: {port} on {}", device.unwrap_or(UNNAMED));
}

const UNNAMED: &str = "an unnamed device";

impl PortWatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// The device the player picked; blank means none. The capture opens it
    /// as soon as it is there. The lock stays as it is.
    pub fn set_preferred_device(&self, device: Option<String>) {
        let pick = label(device.as_deref());
        self.state.lock().pick = pick.clone();
        live::set_wanted_device(pick);
    }

    pub(super) fn picked(&self) -> Option<String> {
        self.state.lock().pick.clone()
    }

    pub fn current_port(&self) -> Option<u16> {
        self.locked().map(|(_, port)| port)
    }

    pub fn current_device(&self) -> Option<String> {
        self.locked().and_then(|(device, _)| device)
    }

    /// The lock as one value: device and server port.
    pub(super) fn locked(&self) -> Option<(Option<String>, u16)> {
        let state = self.state.lock();
        state.port.map(|port| (state.device.clone(), port))
    }

    /// Forget the lock and everything that led to it; the pick stays.
    pub fn reset(&self) {
        let was = {
            let mut state = self.state.lock();
            let pick = state.pick.take();
            let was = state.port;
            *state = Detection { pick, ..Detection::default() };
            live::set_filter_port(None);
            was
        };
        if let Some(port) = was {
            info!("Game port {port} let go: looking for the game's connection again");
        }
    }

    /// A port whose connection carries the game's marker, seen on `device`.
    pub(super) fn offer(&self, port: u16, device: Option<&str>, now: i64) {
        let mut state = self.state.lock();
        state.first_offer_ms.get_or_insert(now);
        let seen = state.offered.entry(port).or_default();
        if seen.is_none() {
            *seen = label(device);
        }
    }

    /// Lock `port` if it was offered and may lock by now: on loopback at
    /// once, elsewhere once `OFF_LOOPBACK_WAIT_MS` passed since the first
    /// offer. True when it locked.
    pub(super) fn try_lock(&self, port: u16, device: Option<&str>, now: i64) -> bool {
        let mut state = self.state.lock();
        if state.port.is_some() {
            return false;
        }
        let Some(first_device) = state.offered.get(&port) else { return false };
        let device = label(device).or_else(|| first_device.clone());
        let waited = state.first_offer_ms.is_some_and(|at| now - at >= OFF_LOOPBACK_WAIT_MS);
        if !waited && !device.as_deref().is_some_and(loopback_label) {
            return false;
        }
        state.offered.clear();
        state.port = Some(port);
        state.device = device;
        announce_lock(port, state.device.as_deref());
        true
    }

    /// The game moved: lock `port` on `device` now, with no wait.
    pub(super) fn move_to(&self, port: u16, device: Option<&str>) {
        let device = label(device);
        let before = {
            let mut state = self.state.lock();
            let before = (state.port, state.device.take());
            state.offered.clear();
            state.first_offer_ms = None;
            state.port = Some(port);
            state.device = device.clone();
            announce_lock(port, device.as_deref());
            before
        };
        let to = device.as_deref().unwrap_or(UNNAMED);
        match before {
            (Some(old), old_device) => {
                info!("Followed the game from {old} on {} to {port} on {to}", old_device.as_deref().unwrap_or(UNNAMED))
            }
            (None, _) => info!("Followed the game to {port} on {to}"),
        }
    }

    /// The parser took the game's bytes, or a new ping reading came.
    pub(super) fn saw_parse(&self, now: i64) {
        self.state.lock().parsed_ms = Some(now);
    }

    pub(super) fn last_parse(&self) -> Option<i64> {
        self.state.lock().parsed_ms
    }

    #[cfg(test)]
    fn offered_device(&self, port: u16) -> Option<Option<String>> {
        self.state.lock().offered.get(&port).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_is_recognised_on_each_platform() {
        for name in ["Adapter for loopback traffic capture", "lo", "lo0", " LO "] {
            assert!(loopback_label(name), "{name}");
        }
        for name in ["lowpan0", "wlan0", "Realtek PCIe GbE Family Controller"] {
            assert!(!loopback_label(name), "{name}");
        }
    }

    #[test]
    fn the_pick_is_trimmed_and_blank_is_none() {
        let ports = PortWatch::new();
        ports.set_preferred_device(Some("  vpn0 ".into()));
        assert_eq!(ports.picked().as_deref(), Some("vpn0"));
        ports.set_preferred_device(Some("   ".into()));
        assert_eq!(ports.picked(), None);
        ports.set_preferred_device(None);
        assert_eq!(ports.picked(), None);
    }

    #[test]
    fn picking_a_device_leaves_the_lock_alone() {
        let ports = PortWatch::new();
        ports.offer(13328, Some("lo"), 0);
        assert!(ports.try_lock(13328, Some("lo"), 0));
        ports.set_preferred_device(Some("wlan0".into()));
        assert_eq!(ports.locked(), Some((Some("lo".into()), 13328)));
    }

    #[test]
    fn an_offered_port_keeps_the_first_device_it_came_with() {
        let ports = PortWatch::new();
        ports.offer(13328, None, 0);
        assert_eq!(ports.offered_device(13328), Some(None));
        ports.offer(13328, Some(" "), 10);
        assert_eq!(ports.offered_device(13328), Some(None));
        ports.offer(13328, Some(" wlan0 "), 20);
        ports.offer(13328, Some("eth0"), 30);
        assert_eq!(ports.offered_device(13328), Some(Some("wlan0".into())));
        assert_eq!(ports.offered_device(443), None);
    }

    #[test]
    fn a_port_never_offered_does_not_lock() {
        let ports = PortWatch::new();
        ports.offer(13328, Some("lo"), 0);
        assert!(!ports.try_lock(13329, Some("lo"), 0));
        assert_eq!(ports.current_port(), None);
    }

    #[test]
    fn off_loopback_waits_from_the_first_offer_of_any_port() {
        let ports = PortWatch::new();
        ports.offer(40000, Some("eth0"), 0);
        ports.offer(13328, Some("eth0"), 2_000);
        assert!(!ports.try_lock(13328, Some("eth0"), 2_499));
        assert!(ports.try_lock(13328, Some("eth0"), 2_500));
        assert_eq!(ports.locked(), Some((Some("eth0".into()), 13328)));
        assert_eq!(ports.offered_device(40000), None, "offers are cleared by a lock");
    }

    #[test]
    fn loopback_locks_at_once() {
        let ports = PortWatch::new();
        ports.offer(13328, Some("eth0"), 0);
        ports.offer(40000, Some("lo"), 1_000);
        assert!(ports.try_lock(40000, Some(" lo "), 1_000));
        assert_eq!(ports.current_device().as_deref(), Some("lo"));
    }

    #[test]
    fn a_blank_device_falls_back_to_the_offered_one() {
        let ports = PortWatch::new();
        ports.offer(13328, Some("lo"), 0);
        assert!(ports.try_lock(13328, Some("  "), 0), "the offer was on loopback");
        assert_eq!(ports.current_device().as_deref(), Some("lo"));
        let ports = PortWatch::new();
        ports.offer(13328, None, 0);
        assert!(!ports.try_lock(13328, None, 100), "no device: not loopback");
        assert!(ports.try_lock(13328, None, 2_500));
        assert_eq!(ports.current_device(), None);
    }

    #[test]
    fn a_second_lock_does_nothing() {
        let ports = PortWatch::new();
        ports.offer(13328, Some("lo"), 0);
        assert!(ports.try_lock(13328, Some("lo"), 0));
        ports.offer(13329, Some("lo"), 10);
        assert!(!ports.try_lock(13329, Some("lo"), 10));
        assert_eq!(ports.current_port(), Some(13328));
    }

    #[test]
    fn moving_the_lock_needs_no_offer_or_wait() {
        let ports = PortWatch::new();
        ports.offer(13328, Some("wlan0"), 0);
        assert!(ports.try_lock(13328, Some("wlan0"), 3_000));
        ports.move_to(13329, Some(" eth0 "));
        assert_eq!(ports.locked(), Some((Some("eth0".into()), 13329)));
    }

    #[test]
    fn a_reset_clears_all_but_the_pick() {
        let ports = PortWatch::new();
        ports.set_preferred_device(Some("vpn0".into()));
        ports.offer(13328, Some("lo"), 0);
        assert!(ports.try_lock(13328, Some("lo"), 0));
        ports.saw_parse(500);
        ports.offer(13329, Some("wlan0"), 600);
        ports.reset();
        assert_eq!((ports.current_port(), ports.current_device(), ports.last_parse()), (None, None, None));
        assert_eq!(ports.offered_device(13329), None);
        assert_eq!(ports.picked().as_deref(), Some("vpn0"));
        // The wait starts from the first offer after the reset.
        ports.offer(13328, Some("wlan0"), 5_000);
        assert!(!ports.try_lock(13328, Some("wlan0"), 7_499));
        assert!(ports.try_lock(13328, Some("wlan0"), 7_500));
        // A reset with no lock is quiet but still allowed.
        ports.reset();
        ports.reset();
        assert_eq!(ports.current_port(), None);
    }
}
