//! Which packet-capture library to load. The API is libpcap's on every OS;
//! on Windows it comes from Npcap.

/// The library `capture::pcap_capturer` loads at runtime (the first of these
/// that loads).
pub const LIBRARIES: &[&str] = &["wpcap.dll"];

/// No capture helper: the meter captures in its own process.
pub const HELPER: Option<&str> = None;

/// What to tell the player when it will not load.
pub const MISSING_HELP: &str = "Is Npcap installed? Download from https://npcap.com";

/// Whether the capture library can be loaded at all.
pub fn library_available() -> bool {
    // SAFETY: loading Npcap runs no initialisation we depend on not running.
    LIBRARIES.iter().any(|name| unsafe { libloading::Library::new(name).is_ok() })
}

/// Devices not worth opening on this OS. None here.
pub fn skip_device(_name: &str) -> bool {
    false
}
