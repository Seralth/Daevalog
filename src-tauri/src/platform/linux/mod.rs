//! The Linux port: Proton's game found in /proc, capture through a helper
//! process, the sign-in kept in the desktop keyring.

/// Always "not admin": capture runs in the helper, which says itself whether
/// it may capture.
pub mod admin {
    pub fn is_admin() -> bool {
        false
    }
}
pub mod clock;
pub mod files;
pub mod pcap;
pub mod process;
pub mod secret;
pub mod window_detector;
