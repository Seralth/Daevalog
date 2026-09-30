//! Keeping the account token off disk in the clear.
//!
//! The token the Device Authorization Grant returns is a bearer credential for
//! this account: anything holding it can read the profile and sync documents
//! until it is revoked. `settings.json` is plain JSON that users open to change
//! their hotkey, so it is the wrong home for it.
//!
//! Windows has DPAPI for exactly this. `CryptProtectData` encrypts with a key
//! derived from the logged-in user, so the file is readable by this user on this
//! machine and nobody else — no key for us to ship, lose, or have extracted from
//! the binary.
//!
//! **What it does not protect against, plainly:** malware already running as
//! this user can call `CryptUnprotectData` exactly as we do. DPAPI stops another
//! account on the machine and stops a copied file being useful elsewhere. It is
//! not a defence against a compromised session, which is why the token is
//! revocable from the website and why it holds narrow scopes.

use std::path::{Path, PathBuf};

/// Where the encrypted token lives, beside the other per-user state.
pub fn token_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("credentials.dat")
}

/// An extra input to the encryption, so a `credentials.dat` lifted from another
/// application's folder cannot be decrypted by ours even under the same user.
const ENTROPY: &[u8] = b"a2tools.account.v1";

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CryptProtectData, CryptUnprotectData,
    };

    fn blob(bytes: &mut [u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_mut_ptr(),
        }
    }

    /// Copy a blob out and hand its buffer back to the OS.
    ///
    /// DPAPI allocates with `LocalAlloc`, so the caller frees with `LocalFree`.
    /// Missing this leaks on every save and load.
    unsafe fn take(out: &CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let slice = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) };
        let owned = slice.to_vec();
        let _ = unsafe { LocalFree(Some(HLOCAL(out.pbData as *mut _))) };
        owned
    }

    pub fn protect(plaintext: &[u8], entropy: &[u8]) -> Option<Vec<u8>> {
        let mut input = plaintext.to_vec();
        let mut extra = entropy.to_vec();
        let mut out = CRYPT_INTEGER_BLOB::default();
        unsafe {
            CryptProtectData(
                &blob(&mut input),
                None,
                Some(&blob(&mut extra)),
                None,
                None,
                0,
                &mut out,
            )
            .ok()?;
            Some(take(&out))
        }
    }

    pub fn unprotect(ciphertext: &[u8], entropy: &[u8]) -> Option<Vec<u8>> {
        let mut input = ciphertext.to_vec();
        let mut extra = entropy.to_vec();
        let mut out = CRYPT_INTEGER_BLOB::default();
        unsafe {
            CryptUnprotectData(
                &blob(&mut input),
                None,
                Some(&blob(&mut extra)),
                None,
                None,
                0,
                &mut out,
            )
            .ok()?;
            Some(take(&out))
        }
    }
}

/// Non-Windows builds exist only so the crate still compiles for tests and the
/// wasm parser. Storing a token in the clear would be worse than not storing
/// one, so this refuses rather than degrading quietly.
#[cfg(not(windows))]
mod imp {
    pub fn protect(_plaintext: &[u8], _entropy: &[u8]) -> Option<Vec<u8>> {
        None
    }
    pub fn unprotect(_ciphertext: &[u8], _entropy: &[u8]) -> Option<Vec<u8>> {
        None
    }
}

/// Encrypt and write the token. Returns false if it could not be stored, in
/// which case the caller must treat the account as not connected rather than
/// keeping a token only in memory and appearing connected until restart.
pub fn save(app_data_dir: &Path, token: &str) -> bool {
    let Some(sealed) = imp::protect(token.as_bytes(), ENTROPY) else {
        tracing::error!("Could not encrypt the account token; refusing to store it");
        return false;
    };
    match std::fs::write(token_path(app_data_dir), &sealed) {
        Ok(()) => true,
        Err(e) => {
            tracing::error!("Could not write the account token: {e}");
            false
        }
    }
}

/// Read the token back, or `None` if there is not a usable one.
///
/// A file that will not decrypt is deleted rather than retried forever: it means
/// the Windows profile changed or the file was copied from elsewhere, and no
/// amount of retrying will fix either.
pub fn load(app_data_dir: &Path) -> Option<String> {
    let path = token_path(app_data_dir);
    let sealed = std::fs::read(&path).ok()?;
    match imp::unprotect(&sealed, ENTROPY).and_then(|b| String::from_utf8(b).ok()) {
        Some(token) if !token.is_empty() => Some(token),
        _ => {
            tracing::warn!(
                "{} could not be decrypted for this user; removing it",
                path.display()
            );
            let _ = std::fs::remove_file(&path);
            None
        }
    }
}

/// Forget the token. Used on sign-out and whenever the server says it is dead.
pub fn clear(app_data_dir: &Path) {
    let path = token_path(app_data_dir);
    if path.exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            tracing::warn!("Could not remove {}: {e}", path.display());
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("a2tools-secret-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_token_round_trips() {
        let dir = temp("roundtrip");
        assert!(save(&dir, "tok_abc123"));
        assert_eq!(load(&dir).as_deref(), Some("tok_abc123"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_file_on_disk_does_not_contain_the_token() {
        // The whole point: `settings.json` is readable, this must not be.
        let dir = temp("opaque");
        assert!(save(&dir, "tok_supersecret"));
        let raw = std::fs::read(token_path(&dir)).unwrap();
        assert!(
            !raw.windows(15).any(|w| w == b"tok_supersecret"),
            "the token is sitting in the file in the clear"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clearing_removes_it() {
        let dir = temp("clear");
        assert!(save(&dir, "tok_x"));
        clear(&dir);
        assert!(load(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_is_discarded_rather_than_retried() {
        let dir = temp("corrupt");
        std::fs::write(token_path(&dir), b"not dpapi output").unwrap();
        assert!(load(&dir).is_none());
        assert!(
            !token_path(&dir).exists(),
            "an undecryptable file should be removed, not left to fail forever"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_token_is_not_an_error() {
        let dir = temp("empty");
        assert!(load(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
