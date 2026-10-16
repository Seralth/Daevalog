//! Everything that depends on the operating system. One OS folder is built in
//! as `os`; each folder offers the same modules with the same signatures, so
//! the rest of the crate never names an OS.

// The module is `os`, not `windows`, which would hide the `windows` crate.
#[cfg(windows)]
#[path = "windows/mod.rs"]
mod os;

#[cfg(target_os = "linux")]
#[path = "linux/mod.rs"]
mod os;

#[cfg(not(any(windows, target_os = "linux")))]
compile_error!("the backend builds for Linux and Windows only");

pub mod files;
pub mod hotkeys;
pub mod procfs;
pub mod screenshot;

pub use os::{admin, clock, pcap, process, secret, window_detector};

/// Why `secret::unprotect` could not give a stored secret back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsealError {
    /// The store did not answer, stayed locked, or does not show the item
    /// now. The same data may unseal later.
    Unavailable,
    /// The data can never be unsealed: not ours, or sealed for another user.
    Invalid,
}
