//! libpcap, as nearly every distribution ships it.

pub const LIBRARY: &str = "libpcap.so.1";

pub const MISSING_HELP: &str = "Install libpcap (e.g. `sudo apt install libpcap0.8`), then let the meter \
capture without root: `sudo setcap cap_net_raw,cap_net_admin=eip <path to the meter>`";

pub fn library_available() -> bool {
    // SAFETY: loading libpcap runs no initialisation we depend on not running.
    unsafe { libloading::Library::new(LIBRARY).is_ok() }
}

/// Skip libpcap's pseudo-devices. "any" sees every packet a second time on top
/// of the real interface it arrived on; the rest carry no IP traffic.
pub fn skip_device(name: &str) -> bool {
    name == "any"
        || ["nflog", "nfqueue", "usbmon", "dbus", "bluetooth"]
            .iter()
            .any(|prefix| name.starts_with(prefix))
}
