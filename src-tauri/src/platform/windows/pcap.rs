//! Npcap, loaded at run time from its own folder. There is no capture
//! helper: the meter captures in its own process.
//!
//! Npcap puts wpcap.dll in System32\Npcap, which is not on the DLL search
//! path. Npcap's guide says to add that folder with SetDllDirectory before
//! loading it.

use std::sync::Once;

use windows::core::PCWSTR;
use windows::Win32::System::LibraryLoader::SetDllDirectoryW;
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;

use super::{from_wide, wide};

pub const LIBRARIES: &[&str] = &["wpcap.dll"];
/// No helper: the meter captures itself.
pub const HELPER: Option<&str> = None;

pub const MISSING_HELP: &str = "Install Npcap from https://npcap.com, then start the meter again.";

/// Npcap's folder, System32\Npcap; `None` when Windows does not say where
/// System32 is.
fn npcap_folder() -> Option<String> {
    let mut buffer = [0u16; 260];
    // SAFETY: the buffer is writable and its length is passed with it.
    let len = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    if len == 0 || len >= buffer.len() {
        return None;
    }
    Some(format!("{}\\Npcap", from_wide(&buffer[..len])))
}

/// Adds Npcap's folder to the DLL search path, once. Called before any load
/// of wpcap.dll.
pub fn prepare() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let Some(folder) = npcap_folder() else { return };
        let folder = wide(&folder);
        // SAFETY: a NUL-terminated wide string that lives for the call.
        if let Err(e) = unsafe { SetDllDirectoryW(PCWSTR(folder.as_ptr())) } {
            tracing::warn!("Could not add Npcap's folder to the DLL search path: {e}");
        }
    });
}

/// Every device Npcap lists may carry the game's traffic: a VPN adapter or
/// Npcap's loopback adapter (a ping reducer) too.
pub fn skip_device(_name: &str) -> bool {
    false
}

/// Whether wpcap.dll loads. Only a probe: the library is closed again at once.
pub fn library_available() -> bool {
    prepare();
    LIBRARIES.iter().any(|name| {
        // SAFETY: loading wpcap.dll runs no code that touches our state.
        unsafe { libloading::Library::new(name) }.is_ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npcap_folder_is_under_system32() {
        let folder = npcap_folder().expect("System32");
        assert!(folder.to_ascii_lowercase().ends_with("\\system32\\npcap"), "{folder}");
    }

    #[test]
    fn probe_matches_what_is_installed() {
        let installed = npcap_folder().is_some_and(|dir| std::path::Path::new(&dir).join("wpcap.dll").is_file());
        if installed {
            assert!(library_available());
        }
    }

    #[test]
    fn no_device_is_skipped() {
        assert!(!skip_device("\\Device\\NPF_Loopback"));
        assert!(!skip_device("\\Device\\NPF_{00000000-0000-0000-0000-000000000000}"));
    }
}
