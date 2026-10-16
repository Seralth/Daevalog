//! Whether the meter runs elevated, for the bug report. Npcap captures
//! without it unless it was installed for administrators only.

use std::ffi::c_void;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub fn is_admin() -> bool {
    let mut token = HANDLE::default();
    // SAFETY: the token handle is written by the call and closed below; the
    // buffer is a TOKEN_ELEVATION of the size given.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut written = 0u32;
        let read = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut TOKEN_ELEVATION as *mut c_void),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut written,
        );
        let _ = CloseHandle(token);
        read.is_ok() && elevation.TokenIsElevated != 0
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn asking_does_not_fail() {
        // Either answer is right; the call must not panic.
        let _ = super::is_admin();
    }
}
