//! The Linux implementation, for players running AION 2 under Proton. Same
//! modules and signatures as `../win32/`.
//!
//! Ported so far: packet capture (libpcap), finding the game (/proc), the
//! capture-permission check, the account token (the desktop keyring), dialogs
//! (GTK), updates (pacman), and the clock Wine's QueryPerformanceCounter runs
//! on. The rest still comes from `../unsupported/` and does the safe nothing
//! until it is ported here.

pub mod admin;
pub mod clock;
pub mod dialog;
pub mod pcap;
pub mod secret;
pub mod updater;
pub mod window_detector;

#[path = "../unsupported/hotkeys.rs"]
pub mod hotkeys;
#[path = "../unsupported/screen.rs"]
pub mod screen;
#[path = "../unsupported/shell.rs"]
pub mod shell;
#[path = "../unsupported/window.rs"]
pub mod window;
