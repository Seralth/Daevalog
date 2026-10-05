//! Live capture: through the capture helper where the OS has one (Linux), in
//! the meter's own process elsewhere.

use std::sync::{Arc, OnceLock};

use tokio::sync::mpsc;
use tracing::error;

use super::captured_payload::CapturedPayload;
use super::helper_process::Supervisor;
use super::pcap_capturer::{self, PcapCapturer};
use crate::platform;

static SUPERVISOR: OnceLock<Arc<Supervisor>> = OnceLock::new();

/// Start capturing; payloads go to `sender`. Where a capture helper does the
/// capturing, `notify(false)` says when capture is not available and
/// `notify(true)` when it is again.
pub fn start(sender: mpsc::Sender<CapturedPayload>, notify: impl Fn(bool) + Send + Sync + 'static) {
    let Some(name) = platform::pcap::HELPER else {
        PcapCapturer::new(sender).start();
        return;
    };
    // Next to the meter's own file, never from PATH.
    match std::env::current_exe() {
        Ok(exe) => {
            let _ = SUPERVISOR.set(Supervisor::start(exe.with_file_name(name), sender, notify));
        }
        Err(e) => {
            error!("Packet capture is off: the meter's own path is unknown ({}). {}", e, platform::pcap::MISSING_HELP);
            notify(false);
        }
    }
}

/// Stop the capture helper, if there is one. The meter is quitting.
pub fn stop() {
    if let Some(supervisor) = SUPERVISOR.get() {
        supervisor.stop();
    }
}

/// Called when the combat port locks (`Some`) or the lock is cleared (`None`).
pub fn set_filter_port(port: Option<u16>) {
    match platform::pcap::HELPER {
        Some(_) => {
            if let Some(supervisor) = SUPERVISOR.get() {
                supervisor.set_filter_port(port);
            }
        }
        None => pcap_capturer::set_filter_port(port),
    }
}

/// The capture device labels, for the Settings list.
pub fn list_device_labels() -> Result<Vec<String>, String> {
    match platform::pcap::HELPER {
        Some(_) => SUPERVISOR.get().ok_or("the capture helper is not running")?.list_device_labels(),
        None => pcap_capturer::list_device_labels(),
    }
}

/// Whether packets can be captured: the helper has a device open, or this
/// process has the rights to open one.
pub fn can_capture() -> bool {
    match platform::pcap::HELPER {
        Some(_) => SUPERVISOR.get().is_some_and(|supervisor| supervisor.can_capture()),
        None => platform::admin::is_admin(),
    }
}

/// The capture state in a few words, for a bug report.
pub fn state() -> String {
    match platform::pcap::HELPER {
        Some(_) => match SUPERVISOR.get() {
            Some(supervisor) => supervisor.state(),
            None => "unavailable (capture did not start)".into(),
        },
        None if platform::admin::is_admin() => "in the meter's process, as administrator".into(),
        None => "unavailable (the meter is not running as administrator)".into(),
    }
}
