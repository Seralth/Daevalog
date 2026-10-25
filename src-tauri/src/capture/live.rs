//! Live capture: through the capture helper where the OS has one (Linux), in
//! the meter's own process elsewhere.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use tokio::sync::mpsc;
use tracing::error;

use super::tcp_chunk::TcpChunk;
use super::helper_process::Supervisor;
use super::in_process::{self, LiveSniffer};
use crate::platform;

static SUPERVISOR: OnceLock<Arc<Supervisor>> = OnceLock::new();

/// Start capturing; payloads go to `sender`. Where a capture helper does the
/// capturing, `notify(false)` says when capture is not available and
/// `notify(true)` when it is again.
pub fn start(sender: mpsc::Sender<TcpChunk>, notify: impl Fn(bool) + Send + Sync + 'static) {
    start_with_helper(None, sender, notify);
}

/// As `start`, with the capture helper at `helper` rather than the one
/// next to the meter's own file. Where the OS has no capture helper, the
/// meter captures in its own process and `helper` is not used.
pub fn start_with_helper(
    helper: Option<PathBuf>,
    sender: mpsc::Sender<TcpChunk>,
    notify: impl Fn(bool) + Send + Sync + 'static,
) {
    let Some(name) = platform::pcap::HELPER else {
        LiveSniffer::new(sender).start();
        return;
    };
    // Next to the meter's own file, never from PATH.
    let path = match helper {
        Some(path) => path,
        None => match std::env::current_exe() {
            Ok(exe) => exe.with_file_name(name),
            Err(e) => {
                error!("Packet capture is off: the meter's own path is unknown ({}). {}", e, platform::pcap::MISSING_HELP);
                notify(false);
                return;
            }
        },
    };
    let _ = SUPERVISOR.set(Supervisor::start(path, sender, notify));
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
        None => in_process::set_filter_port(port),
    }
}

/// The device the player picked; the capture opens it at once when it
/// shows up.
pub fn set_wanted_device(device: Option<String>) {
    *WANTED.lock() = device;
}

pub fn wanted_device() -> Option<String> {
    WANTED.lock().clone()
}

static WANTED: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);

/// The capture devices as the capture last listed them, every few seconds;
/// `None` before the first list. The dispatcher reads it to know whether the
/// picked device is there.
pub fn listed_devices() -> Option<Vec<String>> {
    match platform::pcap::HELPER {
        Some(_) => SUPERVISOR.get()?.listed_devices(),
        None => in_process::listed_devices(),
    }
}

/// The capture devices to offer in Settings. Where a capture helper does the
/// capturing, the running helper is asked.
pub fn list_device_labels() -> Result<Vec<String>, String> {
    match platform::pcap::HELPER {
        Some(_) => match SUPERVISOR.get() {
            Some(supervisor) => supervisor.list_device_labels(),
            None => Err("the capture helper is not running".to_string()),
        },
        None => in_process::list_device_labels(),
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
        None => "in the meter's process".into(),
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn the_device_list_needs_a_running_helper() {
        // No test starts the capture helper.
        assert_eq!(super::list_device_labels(), Err("the capture helper is not running".to_string()));
    }
}
