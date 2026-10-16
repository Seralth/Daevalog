//! libpcap, as nearly every distribution ships it. Capture runs in a helper
//! process that holds the capture permission, so the meter holds none.

/// Which sonames to load, and which devices to skip. Shared with the helper.
pub use daevalog_capture::pcap::linux::{skip_device, LIBRARIES};
/// The capture helper's file name. The meter starts it from its own folder.
pub const HELPER: Option<&str> = Some("daevalog-capture");

pub const MISSING_HELP: &str = "Install libpcap (e.g. `sudo apt install libpcap0.8t64`, or `libpcap0.8` on older systems), then let the meter's \
capture helper capture without root: `sudo setcap cap_net_raw=ep <the meter's folder>/daevalog-capture`";

/// Nothing to set up: libpcap is on the usual library path.
pub fn prepare() {}

/// Whether libpcap can be loaded under one of its usual names. Only a probe:
/// the library is closed again at once.
pub fn library_available() -> bool {
    LIBRARIES.iter().any(|soname| {
        // SAFETY: loading libpcap runs no init code that touches our state.
        unsafe { libloading::Library::new(soname) }.is_ok()
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn libpcap_probe_matches_what_is_installed() {
        let found = super::library_available();
        let dirs = ["/usr/lib", "/usr/lib64", "/lib/x86_64-linux-gnu", "/usr/lib/x86_64-linux-gnu"];
        if dirs.iter().any(|dir| std::path::Path::new(dir).join("libpcap.so.1").exists()) {
            assert!(found);
        }
    }

    #[test]
    fn the_hint_names_the_package_before_and_after_the_t64_rename() {
        assert!(super::MISSING_HELP.contains("`sudo apt install libpcap0.8t64`, or `libpcap0.8` on older systems"));
    }
}
