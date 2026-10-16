//! The sign-in token in Windows Credential Manager, which encrypts it for
//! the signed-in user. `credentials.dat` holds only `wincred:<id>`, which
//! names the credential, so signing out can delete it for good.

use std::sync::atomic::{AtomicU64, Ordering};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_NOT_FOUND, FILETIME};
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_GENERIC,
};

use super::wide;
use crate::platform::UnsealError;

const PREFIX: &str = "wincred:";
/// Credential names start with this; the purpose and the id follow.
const TARGET: &str = "daevalog-dps-meter";
/// Credential Manager's limit for one secret (CRED_MAX_CREDENTIAL_BLOB_SIZE).
const MAX_SECRET: usize = 5 * 512;

/// Saves made by this process, so two in the same instant still differ.
static SAVES: AtomicU64 = AtomicU64::new(0);

/// The credential's name for a purpose and an id.
fn target(purpose: &str, id: &str) -> Vec<u16> {
    wide(&format!("{TARGET}/{purpose}/{id}"))
}

/// The id in a stored reference, if it is one.
fn item_id(sealed: &[u8]) -> Option<&str> {
    match sealed.strip_prefix(PREFIX.as_bytes()).map(std::str::from_utf8) {
        Some(Ok(id)) if !id.is_empty() && !id.contains('/') => Some(id),
        _ => None,
    }
}

/// An id no other save made: the time in ns, this process, and its count.
fn new_id() -> String {
    let nanos = std::time::UNIX_EPOCH.elapsed().map_or(0, |since| since.as_nanos());
    let count = SAVES.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:x}-{:x}-{count:x}", std::process::id())
}

/// Reads one credential's secret.
fn read(name: &[u16]) -> windows::core::Result<Vec<u8>> {
    let mut found: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: a NUL-terminated name; on success `found` points at a
    // credential that CredFree releases after the copy.
    unsafe {
        CredReadW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None, &mut found)?;
        let credential = &*found;
        let secret = match credential.CredentialBlob.is_null() {
            true => Vec::new(),
            false => std::slice::from_raw_parts(credential.CredentialBlob, credential.CredentialBlobSize as usize).to_vec(),
        };
        CredFree(found as *const _);
        Ok(secret)
    }
}

/// Whether Credential Manager answers: a read of a name no one saved says
/// "not found". Without a logon session (a service) it says otherwise.
pub fn available() -> bool {
    match read(&target("probe", "none")) {
        Ok(_) => true,
        Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => true,
        Err(e) => {
            tracing::warn!("Credential Manager did not answer: {e}");
            false
        }
    }
}

pub fn protect(token: &[u8], entropy: &[u8]) -> Result<Vec<u8>, String> {
    let purpose = std::str::from_utf8(entropy).map_err(|_| "the key purpose is not text".to_string())?;
    if token.len() > MAX_SECRET {
        return Err(format!("the sign-in is longer than Credential Manager holds ({} bytes)", token.len()));
    }
    let id = new_id();
    let mut name = target(purpose, &id);
    let mut user = wide("Daevalog DPS Meter account");
    let mut secret = token.to_vec();
    let credential = CREDENTIALW {
        Flags: CRED_FLAGS(0),
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(name.as_mut_ptr()),
        Comment: PWSTR::null(),
        LastWritten: FILETIME::default(),
        CredentialBlobSize: secret.len() as u32,
        CredentialBlob: secret.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        AttributeCount: 0,
        Attributes: std::ptr::null_mut(),
        TargetAlias: PWSTR::null(),
        UserName: PWSTR(user.as_mut_ptr()),
    };
    // SAFETY: every pointer in `credential` points at a live buffer above.
    match unsafe { CredWriteW(&credential, 0) } {
        Ok(()) => Ok([PREFIX, &id].concat().into_bytes()),
        Err(e) => {
            tracing::warn!("Credential Manager save failed: {e}");
            Err(format!("Credential Manager would not store the sign-in ({e})"))
        }
    }
}

pub fn unprotect(sealed: &[u8], entropy: &[u8]) -> Result<Vec<u8>, UnsealError> {
    let (Some(id), Ok(purpose)) = (item_id(sealed), std::str::from_utf8(entropy)) else {
        return Err(UnsealError::Invalid);
    };
    match read(&target(purpose, id)) {
        Ok(secret) => Ok(secret),
        // Credential Manager is always there: a credential it does not have
        // is gone for good.
        Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => Err(UnsealError::Invalid),
        Err(_) => Err(UnsealError::Unavailable),
    }
}

/// Delete the credential a reference names. The purpose is part of the
/// name, so every purpose the meter uses is tried.
pub fn forget(sealed: &[u8]) {
    let Some(id) = item_id(sealed) else { return };
    for purpose in PURPOSES {
        let name = target(purpose, id);
        // SAFETY: a NUL-terminated name that lives for the call.
        match unsafe { CredDeleteW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None) } {
            Err(e) if e.code() != ERROR_NOT_FOUND.to_hresult() => {
                tracing::warn!("Could not delete an old credential: {e}");
            }
            _ => {}
        }
    }
}

/// The purposes credentials are saved under (`account::secret`), and the
/// tests' own.
const PURPOSES: [&str; 2] = ["a2tools.account.v1", "daevalog.test"];

#[cfg(test)]
mod tests {
    use super::*;

    const PURPOSE: &[u8] = b"daevalog.test";

    #[test]
    fn references_name_a_credential() {
        assert_eq!(item_id(b"wincred:abc-1-0"), Some("abc-1-0"));
        assert_eq!(item_id(b"wincred:"), None);
        assert_eq!(item_id(b"wincred:a/b"), None);
        assert_eq!(item_id(b"secret-service:abc"), None);
    }

    #[test]
    fn bad_references_are_invalid_for_good() {
        let refs: [&[u8]; 3] = [b"not a reference", b"wincred:", b"wincred:\xff"];
        for sealed in refs {
            assert_eq!(unprotect(sealed, PURPOSE), Err(UnsealError::Invalid));
        }
        assert_eq!(unprotect(b"wincred:abc", b"\xff"), Err(UnsealError::Invalid));
    }

    #[test]
    fn a_saved_secret_reads_back_until_forgotten() {
        if !available() {
            return;
        }
        let sealed = protect(b"made-up-token", PURPOSE).expect("saved");
        assert!(sealed.starts_with(PREFIX.as_bytes()));
        assert_eq!(unprotect(&sealed, PURPOSE), Ok(b"made-up-token".to_vec()));
        assert_eq!(unprotect(&sealed, b"another.purpose"), Err(UnsealError::Invalid));
        forget(&sealed);
        assert_eq!(unprotect(&sealed, PURPOSE), Err(UnsealError::Invalid));
    }

    #[test]
    fn oversized_and_non_text_purposes_are_refused() {
        assert!(protect(b"x", b"\xff").is_err());
        assert!(protect(&[b'x'; MAX_SECRET + 1], PURPOSE).is_err());
    }
}
