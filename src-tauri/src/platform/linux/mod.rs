//! The Linux implementation, for players running AION 2 under Proton. Same
//! modules and signatures as `../win32/`.
//!
//! Ported so far: packet capture (libpcap), finding the game (/proc), the
//! capture-permission check, and the clock Wine's QueryPerformanceCounter
//! runs on. The rest still comes from `../unsupported/` and does the safe
//! nothing until it is ported here.

pub mod admin;
pub mod clock;
pub mod pcap;
pub mod window_detector;

#[path = "../unsupported/dialog.rs"]
pub mod dialog;
#[path = "../unsupported/hotkeys.rs"]
pub mod hotkeys;
#[path = "../unsupported/screen.rs"]
pub mod screen;
#[path = "../unsupported/secret.rs"]
pub mod secret;
#[path = "../unsupported/shell.rs"]
pub mod shell;
#[path = "../unsupported/updater.rs"]
pub mod updater;
#[path = "../unsupported/window.rs"]
pub mod window;
