//! Live capture: through the capture helper where the OS has one (Linux), in
//! the meter's own process elsewhere.

use std::sync::{Arc, OnceLock};

use tokio::sync::mpsc;
use tracing::error;

use super::captured_payload::CapturedPayload;
use super::helper_process::Helper;
use super::pcap_capturer::{self, PcapCapturer};
use crate::platform;

static HELPER: OnceLock<Arc<Helper>> = OnceLock::new();

/// Start capturing; payloads go to `sender`.
pub fn start(sender: mpsc::Sender<CapturedPayload>) {
    let Some(name) = platform::pcap::HELPER else {
        PcapCapturer::new(sender).start();
        return;
    };
    // Next to the meter's own file, never from PATH.
    let started = std::env::current_exe()
        .map_err(|e| e.to_string())
        .and_then(|exe| Helper::start(&exe.with_file_name(name), sender));
    match started {
        Ok(helper) => {
            let _ = HELPER.set(helper);
        }
        Err(e) => error!("Packet capture is off: the capture helper did not start ({}). {}", e, platform::pcap::MISSING_HELP),
    }
}

/// Called when the combat port locks (`Some`) or the lock is cleared (`None`).
pub fn set_filter_port(port: Option<u16>) {
    pcap_capturer::set_filter_port(port);
    if let Some(helper) = HELPER.get() {
        helper.set_filter_port(port);
    }
}

/// The capture device labels, for the Settings list.
pub fn list_device_labels() -> Result<Vec<String>, String> {
    match platform::pcap::HELPER {
        Some(_) => HELPER.get().ok_or("the capture helper is not running")?.list_device_labels(),
        None => pcap_capturer::list_device_labels(),
    }
}

/// Whether packets can be captured: the helper has a device open, or this
/// process has the rights to open one.
pub fn can_capture() -> bool {
    match platform::pcap::HELPER {
        Some(_) => HELPER.get().is_some_and(|helper| helper.can_capture()),
        None => platform::admin::is_admin(),
    }
}
