//! Capture in the meter's own process, where the OS has no capture helper
//! (Windows, with Npcap). The libpcap code itself is `daevalog_capture::pcap`,
//! shared with the helper.

use std::collections::BTreeSet;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use daevalog_capture::pcap::{self, NetDevice, PcapLib};
use daevalog_capture::{Level, Segment};
use parking_lot::Mutex;
use tokio::sync::mpsc::{self, error::TrySendError};
use tracing::{error, info, warn};

use super::tcp_chunk::TcpChunk;
use crate::platform;

/// Physical devices wait this long when a virtual one is read too, so a
/// VPN tunnel's flow reaches the port detector first.
const PHYSICAL_WAIT: Duration = Duration::from_millis(1_500);

/// Milliseconds since 1970 by the wall clock; 0 for a clock set before it.
fn epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

/// The helper crate's log lines, in the meter's log at the same level.
pub(crate) fn log_line(level: Level, message: &str) {
    match level {
        Level::Info => info!("{}", message),
        Level::Warn => warn!("{}", message),
        Level::Error => error!("{}", message),
    }
}

/// Npcap (or libpcap), with the helper crate's log lines sent to ours.
fn open_library() -> Result<PcapLib, String> {
    daevalog_capture::set_logger(log_line);
    platform::pcap::prepare();
    PcapLib::load(platform::pcap::LIBRARIES, platform::pcap::MISSING_HELP)
}

/// A captured segment in the parser's form.
pub(crate) fn to_payload(segment: Segment) -> TcpChunk {
    let Segment { device, captured_at_ms, src_ip, dst_ip, src_port, dst_port, seq, ack, data } = segment;
    TcpChunk {
        src_port,
        dst_port,
        data,
        device_name: Some(device),
        captured_at_ms,
        src_ip: Some(Ipv4Addr::from(src_ip).to_string()),
        dst_ip: Some(Ipv4Addr::from(dst_ip).to_string()),
        tcp_seq: Some(seq),
        tcp_ack: ack,
    }
}


/// Called when the combat port locks (`Some`) or the lock is cleared (`None`).
pub fn set_filter_port(port: Option<u16>) {
    pcap::set_filter_port(port);
}
/// Payloads dropped because the parser fell behind are reported at most this
/// often, with how many there were.
const DROP_WARN_INTERVAL_MS: i64 = 60_000;

/// Counts payloads the channel had no room for, and says so now and then.
pub(crate) struct DropCounter {
    dropped: u64,
    last_warn_ms: i64,
}

impl DropCounter {
    pub(crate) fn new() -> Self {
        Self { dropped: 0, last_warn_ms: i64::MIN / 2 }
    }

    pub(crate) fn note_drop(&mut self, label: &str, now: i64) {
        self.dropped += 1;
        self.report(label, now);
    }

    pub(crate) fn report(&mut self, label: &str, now: i64) {
        if self.dropped > 0 && now - self.last_warn_ms >= DROP_WARN_INTERVAL_MS {
            warn!("Capture on {}: {} packets dropped, the parser is falling behind", label, self.dropped);
            self.dropped = 0;
            self.last_warn_ms = now;
        }
    }
}

/// Where one device's segments go: the parser's channel, never waiting on it.
struct Outlet {
    label: String,
    sender: mpsc::Sender<TcpChunk>,
    drops: DropCounter,
}

impl Outlet {
    fn hand_over(&mut self, segment: Segment, now: i64) {
        // A closed channel means the meter is shutting down: nothing to count.
        if let Err(TrySendError::Full(_)) = self.sender.try_send(to_payload(segment)) {
            self.drops.note_drop(&self.label, now);
        }
    }
}

impl pcap::Sink for Outlet {
    fn packet(&mut self, segment: Segment) {
        self.hand_over(segment, epoch_ms());
    }

    fn idle(&mut self) {
        self.drops.report(&self.label, epoch_ms());
    }
}

/// How often the device list is read again, for devices that came up or
/// back since (a VPN that switched servers, an Ethernet cable).
const DEVICE_POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// The devices read now, by label; and those that would not open, left
/// alone until they leave the list.
static OPEN: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
static FAILED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
static LISTED: Mutex<Option<Vec<String>>> = Mutex::new(None);

/// The devices as last listed; `None` before the first list.
pub fn listed_devices() -> Option<Vec<String>> {
    LISTED.lock().clone()
}

/// Listed labels nobody reads yet and that did not fail to open, in list order.
fn unread_labels(listed: &[String], open: &BTreeSet<String>, failed: &BTreeSet<String>) -> Vec<String> {
    listed.iter().filter(|label| !open.contains(*label) && !failed.contains(*label)).cloned().collect()
}

/// One fresh device list against the books: a failure is forgotten once its
/// label leaves the list, the new labels count as open, and the list is kept
/// for `listed_devices`. Returns the labels to start reading. `None` (the
/// list could not be read) changes nothing.
fn take_list(
    labels: Option<Vec<String>>,
    open: &mut BTreeSet<String>,
    failed: &mut BTreeSet<String>,
    shown: &mut Option<Vec<String>>,
) -> Vec<String> {
    let Some(labels) = labels else {
        return Vec::new();
    };
    failed.retain(|label| labels.contains(label));
    let fresh = unread_labels(&labels, open, failed);
    open.extend(fresh.iter().cloned());
    *shown = Some(labels);
    fresh
}

/// Live capture in this process: one reader thread per device, and a thread
/// that opens devices as they come up.
pub struct LiveSniffer {
    sender: mpsc::Sender<TcpChunk>,
    running: Arc<AtomicBool>,
}

impl LiveSniffer {
    pub fn new(sender: mpsc::Sender<TcpChunk>) -> Self {
        Self { sender, running: Arc::new(AtomicBool::new(false)) }
    }

    /// Open every listed device. Returns at once; the reading happens on
    /// threads that run until `stop`.
    pub fn start(&self) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let lib = match open_library() {
            Ok(lib) => Arc::new(lib),
            Err(e) => return self.give_up(&e),
        };
        let devices = match lib.capture_devices(platform::pcap::skip_device) {
            Ok(devices) => devices,
            Err(e) => return self.give_up(&format!("Could not list capture devices: {}", e)),
        };
        if devices.is_empty() {
            return self.give_up("No capture device with an address was found");
        }

        pcap::log_devices(&devices);
        let labels: Vec<String> = devices.iter().map(|d| d.label().to_string()).collect();
        *LISTED.lock() = Some(labels.clone());
        OPEN.lock().extend(labels);
        pcap::start_filter_watcher(lib.clone(), self.running.clone());
        self.spawn_device_follower(lib.clone());

        let (tunnels, physical): (Vec<NetDevice>, Vec<NetDevice>) =
            devices.into_iter().partition(NetDevice::software_only);
        let delay_physical = !tunnels.is_empty();
        for device in tunnels {
            self.spawn_reader(lib.clone(), device);
        }
        if !delay_physical {
            for device in physical {
                self.spawn_reader(lib.clone(), device);
            }
            return;
        }
        let late = self.clone_handles();
        std::thread::spawn(move || {
            std::thread::sleep(PHYSICAL_WAIT);
            if !late.running.load(Ordering::SeqCst) {
                return;
            }
            for device in physical {
                info!("Starting capture on physical device {}", device.label());
                late.spawn_reader(lib.clone(), device);
            }
        });
    }

    /// Ask every thread of this capture to end. Returns at once.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    fn give_up(&self, why: &str) {
        error!("{}", why);
        self.running.store(false, Ordering::SeqCst);
    }

    fn clone_handles(&self) -> Self {
        Self { sender: self.sender.clone(), running: self.running.clone() }
    }

    /// Read one device until stopped or until it fails. Its label is already
    /// in `OPEN`; it leaves when the thread ends.
    fn spawn_reader(&self, lib: Arc<PcapLib>, device: NetDevice) {
        let running = self.running.clone();
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            let label = device.label().to_string();
            match lib.open(&device, None) {
                Err(e) => {
                    warn!("Could not open capture on {}: {}", label, e);
                    OPEN.lock().remove(&label);
                    FAILED.lock().insert(label);
                }
                Ok(live) => {
                    let mut outlet = Outlet { label: label.clone(), sender, drops: DropCounter::new() };
                    if lib.run(live, &running, i64::MIN, &mut outlet) {
                        warn!("Capture on {} stopped: the device went down or away", label);
                    }
                    OPEN.lock().remove(&label);
                }
            }
        });
    }

    /// Every `DEVICE_POLL`, read the device list and open what came up.
    fn spawn_device_follower(&self, lib: Arc<PcapLib>) {
        let me = self.clone_handles();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(DEVICE_POLL);
                if !me.running.load(Ordering::SeqCst) {
                    break;
                }
                let devices = lib.capture_devices(platform::pcap::skip_device).ok();
                let labels = devices.as_ref().map(|list| list.iter().map(|d| d.label().to_string()).collect());
                let fresh = {
                    let mut open = OPEN.lock();
                    let mut failed = FAILED.lock();
                    let mut shown = LISTED.lock();
                    take_list(labels, &mut open, &mut failed, &mut shown)
                };
                for device in devices.into_iter().flatten() {
                    if fresh.iter().any(|label| label == device.label()) {
                        info!("Capture device {} came up", device.label());
                        me.spawn_reader(lib.clone(), device);
                    }
                }
            }
        });
    }
}

/// The capture device labels, for the Settings list. Opens nothing.
pub fn list_device_labels() -> Result<Vec<String>, String> {
    let devices = open_library()?.capture_devices(platform::pcap::skip_device)?;
    Ok(devices.iter().map(|d| d.label().to_string()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn labels_to_open_skip_open_and_failed() {
        let listed = names(&[r"\Device\NPF_{A}", r"\Device\NPF_{B}", r"\Device\NPF_{C}"]);
        let open = set(&[r"\Device\NPF_{A}"]);
        let failed = set(&[r"\Device\NPF_{C}"]);
        assert_eq!(unread_labels(&listed, &open, &failed), names(&[r"\Device\NPF_{B}"]));

        let all = set(&[r"\Device\NPF_{A}", r"\Device\NPF_{B}", r"\Device\NPF_{C}"]);
        assert!(unread_labels(&listed, &all, &BTreeSet::new()).is_empty());
    }

    #[test]
    fn labels_to_open_keep_list_order() {
        let listed = names(&["wlan", "eth", "tun"]);
        assert_eq!(unread_labels(&listed, &BTreeSet::new(), &BTreeSet::new()), listed);
    }

    #[test]
    fn drop_warnings_at_most_once_a_minute() {
        let mut drops = DropCounter::new();
        drops.note_drop("dev", 1_000);
        assert_eq!(drops.dropped, 0);
        drops.note_drop("dev", 2_000);
        drops.note_drop("dev", 30_000);
        drops.report("dev", 60_999);
        assert_eq!(drops.dropped, 2);
        drops.report("dev", 61_000);
        assert_eq!(drops.dropped, 0);
    }

    fn sample_segment() -> Segment {
        Segment {
            device: "eth0".into(),
            captured_at_ms: 1_791_000_000_123,
            src_ip: [193, 202, 112, 99],
            dst_ip: [10, 0, 0, 2],
            src_port: 13328,
            dst_port: 51000,
            seq: 5,
            ack: 6,
            data: b"hi".to_vec(),
        }
    }

    #[test]
    fn segment_to_payload_fields() {
        let p = to_payload(sample_segment());
        assert_eq!(p.src_ip.as_deref(), Some("193.202.112.99"));
        assert_eq!(p.dst_ip.as_deref(), Some("10.0.0.2"));
        assert_eq!(p.device_name.as_deref(), Some("eth0"));
        assert_eq!((p.src_port, p.dst_port), (13328, 51000));
        assert_eq!(p.tcp_seq, Some(5));
        assert_eq!(p.tcp_ack, 6);
        assert_eq!(p.captured_at_ms, 1_791_000_000_123);
        assert_eq!(p.data, b"hi");
    }

    #[test]
    fn device_list_forgets_failures_once_unlisted() {
        let mut open = BTreeSet::new();
        let mut failed = set(&["vpn"]);
        let mut shown = None;

        // Still listed: the failed device is left alone.
        let fresh = take_list(Some(names(&["eth", "vpn"])), &mut open, &mut failed, &mut shown);
        assert_eq!(fresh, names(&["eth"]));
        assert_eq!(failed, set(&["vpn"]));
        assert_eq!(open, set(&["eth"]));

        // Gone: the failure is forgotten.
        let fresh = take_list(Some(names(&["eth"])), &mut open, &mut failed, &mut shown);
        assert!(fresh.is_empty());
        assert!(failed.is_empty());
        assert_eq!(shown, Some(names(&["eth"])));

        // Back: opened again.
        let fresh = take_list(Some(names(&["eth", "vpn"])), &mut open, &mut failed, &mut shown);
        assert_eq!(fresh, names(&["vpn"]));
        assert_eq!(open, set(&["eth", "vpn"]));
    }

    #[test]
    fn device_list_reopens_a_device_whose_reader_ended() {
        let mut open = set(&["eth", "wifi"]);
        let mut failed = BTreeSet::new();
        let mut shown = Some(names(&["eth", "wifi"]));

        // Both are read: nothing opens twice.
        assert!(take_list(Some(names(&["eth", "wifi"])), &mut open, &mut failed, &mut shown).is_empty());

        // The wifi reader ended and took its label out of the open set.
        open.remove("wifi");
        let fresh = take_list(Some(names(&["eth", "wifi"])), &mut open, &mut failed, &mut shown);
        assert_eq!(fresh, names(&["wifi"]));
        assert!(open.contains("wifi"));
    }

    #[test]
    fn device_list_unreadable_changes_nothing() {
        let mut open = set(&["eth"]);
        let mut failed = set(&["vpn"]);
        let mut shown = Some(names(&["eth", "vpn"]));
        assert!(take_list(None, &mut open, &mut failed, &mut shown).is_empty());
        assert_eq!(open, set(&["eth"]));
        assert_eq!(failed, set(&["vpn"]));
        assert_eq!(shown, Some(names(&["eth", "vpn"])));
    }

    #[test]
    fn outlet_counts_full_channel_not_closed_one() {
        let (sender, receiver) = mpsc::channel(1);
        let mut outlet = Outlet { label: "eth".into(), sender, drops: DropCounter::new() };
        // A first drop warns at once; begin after it so later drops stay counted.
        outlet.drops.note_drop("eth", 0);

        outlet.hand_over(sample_segment(), 1);
        assert_eq!(outlet.drops.dropped, 0);
        outlet.hand_over(sample_segment(), 2);
        assert_eq!(outlet.drops.dropped, 1);

        drop(receiver);
        outlet.hand_over(sample_segment(), 3);
        assert_eq!(outlet.drops.dropped, 1);
    }

    #[test]
    fn helper_log_lines_keep_level_and_text() {
        use tracing::field::{Field, Visit};
        use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

        type Heard = Arc<Mutex<Vec<(tracing::Level, String)>>>;
        struct Ear(Heard);
        struct Text(String);
        impl Visit for Text {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0 = format!("{:?}", value);
                }
            }
        }
        impl<S: tracing::Subscriber> Layer<S> for Ear {
            fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
                let mut text = Text(String::new());
                event.record(&mut text);
                self.0.lock().push((*event.metadata().level(), text.0));
            }
        }

        let heard = Heard::default();
        let subscriber = tracing_subscriber::registry().with(Ear(heard.clone()));
        crate::trace_tap::with_default(subscriber, || {
            log_line(Level::Error, "lib gone");
            log_line(Level::Warn, "no filter on {eth}");
            log_line(Level::Info, "Capture active");
        });
        assert_eq!(
            *heard.lock(),
            vec![
                (tracing::Level::ERROR, "lib gone".to_string()),
                (tracing::Level::WARN, "no filter on {eth}".to_string()),
                (tracing::Level::INFO, "Capture active".to_string()),
            ]
        );
    }
}
