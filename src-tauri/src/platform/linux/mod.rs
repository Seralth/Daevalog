//! The Linux implementation, for players running AION 2 under Proton. Same
//! modules and signatures as `../win32/`.
//!
//! Ported so far: packet capture (libpcap), finding the game (/proc), the
//! capture-permission check, the account token (the desktop keyring), the
//! folder picker (GTK), screenshots (WebKit's own snapshot), the clock Wine's
//! QueryPerformanceCounter runs on, and the lock hotkey (the desktop's GlobalShortcuts portal). The rest
//! still comes from `../unsupported/` and does the safe nothing until it is
//! ported here.

pub mod admin;
pub mod clock;
mod dialog;
pub mod hotkeys;
pub mod pcap;
pub mod process;
pub mod screen;
pub mod secret;
pub mod window;
pub mod window_detector;

#[path = "../unsupported/shell.rs"]
pub mod shell;
