//! The Windows implementation. Every module here has a twin with the same
//! signatures in `../unsupported/` (and, later, `../linux/`).

pub mod admin;
pub mod clock;
pub mod files;
pub mod hotkeys;
#[cfg(feature = "desktop")]
pub mod opener;
pub mod pcap;
pub mod process;
#[cfg(feature = "desktop")]
pub mod screen;
pub mod secret;
#[cfg(feature = "desktop")]
pub mod window;
pub mod window_detector;
