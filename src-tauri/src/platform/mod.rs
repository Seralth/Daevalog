//! Everything that depends on the operating system, and nothing else.
//!
//! **The rule:** `#[cfg(windows)]` / `#[cfg(target_os = ...)]` appear in this
//! module and in `Cargo.toml`, and nowhere else in `src/`. CI enforces it. The
//! rest of the meter calls `platform::...` and never asks which OS it is on.
//!
//! Exactly one OS implementation is compiled in, chosen below. Each lives in its
//! own folder and exposes the **same modules, functions and signatures**:
//!
//! - `win32/` — Windows, the platform the meter ships on.
//! - `unsupported/` — every other target. It compiles, and does the safe
//!   nothing: no capture library, no screenshots, no hotkeys, no stored token.
//!   A Linux port replaces it for `target_os = "linux"` with a `linux/` folder,
//!   leaving `win32/` untouched.
//!
//! OS-neutral helpers that only *support* platform code (the screenshot rect
//! maths and PNG encoder, hotkey-label parsing) live beside this file and
//! re-export the OS half, so callers have one path either way.
//!
//! The folder is `win32`, not `windows`: a local module named `windows` would
//! shadow the `windows` crate the Windows code is written against.

#[cfg(windows)]
#[path = "win32/mod.rs"]
mod os;

#[cfg(not(windows))]
#[path = "unsupported/mod.rs"]
mod os;

pub mod hotkeys;
pub mod screenshot;

pub use os::{admin, clock, dialog, pcap, secret, shell, updater, window, window_detector};
