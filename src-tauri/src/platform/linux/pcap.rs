//! libpcap, as nearly every distribution ships it. Capture runs in a helper
//! process that holds the capture permission, so the meter holds none.

/// Which sonames to load, and which devices to skip. Shared with the helper.
pub use daevalog_capture::pcap::linux::{skip_device, LIBRARIES};

/// The capture helper's file name. The meter starts it from its own folder.
pub const HELPER: Option<&str> = Some("daevalog-capture");

pub const MISSING_HELP: &str = "Install libpcap (e.g. `sudo apt install libpcap0.8`), then let the meter's \
capture helper capture without root: `sudo setcap cap_net_raw=ep <the meter's folder>/daevalog-capture`";

pub fn library_available() -> bool {
    // SAFETY: loading libpcap runs no initialisation we depend on not running.
    LIBRARIES.iter().any(|name| unsafe { libloading::Library::new(name).is_ok() })
}
