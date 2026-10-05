//! Capture in the meter's own process, where the OS has no capture helper
//! (Windows, with Npcap). The libpcap code itself is `daevalog_capture::pcap`,
//! shared with the helper.

use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use daevalog_capture::pcap::{self, DeviceInfo, PcapLib, Sink};
use daevalog_capture::{Level, Segment};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use super::captured_payload::CapturedPayload;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Capture log lines go to the meter's log.
pub(crate) fn log_line(level: Level, message: &str) {
    match level {
        Level::Error => error!("{}", message),
        Level::Warn => warn!("{}", message),
        Level::Info => info!("{}", message),
    }
}

fn load() -> Result<PcapLib, String> {
    daevalog_capture::set_logger(log_line);
    PcapLib::load(crate::platform::pcap::LIBRARIES, crate::platform::pcap::MISSING_HELP)
}

pub(crate) fn to_payload(segment: Segment) -> CapturedPayload {
    CapturedPayload {
        src_port: segment.src_port,
        dst_port: segment.dst_port,
        data: segment.data,
        device_name: Some(segment.device),
        captured_at_ms: segment.captured_at_ms,
        src_ip: Some(Ipv4Addr::from(segment.src_ip).to_string()),
        dst_ip: Some(Ipv4Addr::from(segment.dst_ip).to_string()),
        tcp_seq: segment.seq,
        tcp_ack: segment.ack,
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

/// One device's packets, into the dispatcher's channel.
struct ChannelSink {
    label: String,
    sender: mpsc::Sender<CapturedPayload>,
    drops: DropCounter,
}

impl Sink for ChannelSink {
    fn packet(&mut self, segment: Segment) {
        if let Err(mpsc::error::TrySendError::Full(_)) = self.sender.try_send(to_payload(segment)) {
            self.drops.note_drop(&self.label, now_ms());
        }
    }

    fn idle(&mut self) {
        self.drops.report(&self.label, now_ms());
    }
}

/// Manages pcap device handles and captures TCP traffic from network interfaces.
/// Loads the OS's pcap library at runtime (`platform::pcap::LIBRARIES`) — no SDK
/// needed at compile time.
pub struct PcapCapturer {
    running: Arc<AtomicBool>,
    sender: mpsc::Sender<CapturedPayload>,
}

impl PcapCapturer {
    pub fn new(sender: mpsc::Sender<CapturedPayload>) -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            sender,
        }
    }

    pub fn start(&self) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }

        let pcap = match load() {
            Ok(p) => Arc::new(p),
            Err(e) => {
                error!("{}", e);
                self.running.store(false, Ordering::SeqCst);
                return;
            }
        };

        // Only capture on devices that have addresses, and that this OS does not
        // rule out (Linux's catch-all "any" would duplicate every packet).
        let devices = match pcap.capture_devices(crate::platform::pcap::skip_device) {
            Ok(d) => d,
            Err(e) => {
                error!("{}", e);
                self.running.store(false, Ordering::SeqCst);
                return;
            }
        };

        if devices.is_empty() {
            error!("No capture devices found with addresses");
            self.running.store(false, Ordering::SeqCst);
            return;
        }

        pcap::log_devices(&devices);

        pcap::start_filter_watcher(pcap.clone(), self.running.clone());

        let virtual_devices: Vec<_> = devices.iter().filter(|d| d.is_virtual()).cloned().collect();
        let physical_devices: Vec<_> = devices.iter().filter(|d| !d.is_virtual()).cloned().collect();

        // Start virtual/loopback devices first
        for device in &virtual_devices {
            start_capture_thread(
                device.clone(),
                pcap.clone(),
                self.sender.clone(),
                self.running.clone(),
            );
        }

        if virtual_devices.is_empty() {
            // No virtual devices — start physical immediately
            for device in &physical_devices {
                start_capture_thread(
                    device.clone(),
                    pcap.clone(),
                    self.sender.clone(),
                    self.running.clone(),
                );
            }
        } else {
            // Start physical after delay as fallback
            let running = self.running.clone();
            let sender = self.sender.clone();
            let pcap2 = pcap.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(1500));
                if !running.load(Ordering::SeqCst) {
                    return;
                }
                for device in &physical_devices {
                    info!("Starting capture on physical device: {}", device.label());
                    start_capture_thread(
                        device.clone(),
                        pcap2.clone(),
                        sender.clone(),
                        running.clone(),
                    );
                }
            });
        }
    }

    /// Used by diagnostics/packet_dump.rs.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

/// List available capture device labels (for the settings UI dropdown).
pub fn list_device_labels() -> Result<Vec<String>, String> {
    let pcap = load()?;
    let devices = pcap.capture_devices(crate::platform::pcap::skip_device)?;
    Ok(devices.into_iter().map(|d| d.label().to_string()).collect())
}

fn start_capture_thread(
    device: DeviceInfo,
    pcap: Arc<PcapLib>,
    sender: mpsc::Sender<CapturedPayload>,
    running: Arc<AtomicBool>,
) {
    let label = device.label().to_string();

    std::thread::spawn(move || {
        let live = match pcap.open(&device) {
            Ok(live) => live,
            Err(e) => {
                warn!("Failed to open capture on {}: {}", label, e);
                return;
            }
        };
        let mut sink = ChannelSink { label, sender, drops: DropCounter::new() };
        pcap.run(live, &running, i64::MIN, &mut sink);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_are_reported_at_most_once_a_minute() {
        let mut d = DropCounter::new();
        d.note_drop("dev", 1_000);
        assert_eq!(d.dropped, 0, "the first drop is reported at once");
        d.note_drop("dev", 2_000);
        d.note_drop("dev", 30_000);
        d.report("dev", 60_999);
        assert_eq!(d.dropped, 2, "held until a minute has passed");
        d.report("dev", 61_000);
        assert_eq!(d.dropped, 0);
    }

    #[test]
    fn a_segment_becomes_the_payload_the_dispatcher_reads() {
        let p = to_payload(Segment {
            device: "enp5s0".into(),
            captured_at_ms: 1_791_000_000_123,
            src_ip: [193, 202, 112, 99],
            dst_ip: [10, 0, 0, 2],
            src_port: 13328,
            dst_port: 51000,
            seq: 5,
            ack: 6,
            data: b"hi".to_vec(),
        });
        assert_eq!(p.src_ip.as_deref(), Some("193.202.112.99"));
        assert_eq!(p.dst_ip.as_deref(), Some("10.0.0.2"));
        assert_eq!(p.device_name.as_deref(), Some("enp5s0"));
        assert_eq!((p.src_port, p.dst_port, p.tcp_seq, p.tcp_ack), (13328, 51000, 5, 6));
        assert_eq!((p.captured_at_ms, p.data.as_slice()), (1_791_000_000_123, &b"hi"[..]));
    }
}
