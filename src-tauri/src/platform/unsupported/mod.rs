//! Every target the meter does not support yet. Same modules and signatures as
//! `../win32/`; each does the safe nothing, so the crate compiles and the parts
//! that are OS-neutral can be built and tested here. A real port (`../linux/`)
//! replaces this for its target.

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
