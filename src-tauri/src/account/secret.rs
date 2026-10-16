//! The account token at rest. The platform's secret store seals it; one file
//! in the data folder keeps what the store handed back. The token is never
//! written anywhere in the clear.

use std::path::{Path, PathBuf};

use crate::platform::{files, secret as store};
use crate::platform::UnsealError;

/// What `load_stored` found.
#[derive(Debug, Clone, PartialEq)]
pub enum Stored {
    Token(String),
    /// No token on this machine.
    Missing,
    /// A token is stored, but the keyring did not hand it over now: not
    /// running yet, locked, unlock dismissed, or the item not visible. The
    /// file is kept for the next try.
    Locked,
}

fn discard(path: &Path) -> Stored {
    tracing::warn!("{} could not be decrypted for this user; removing it", path.display());
    let _ = std::fs::remove_file(path);
    Stored::Missing
}

/// Passed with every seal and unseal. On Linux it is the keyring item's
/// `purpose`, so a change here would lose every sign-in made before it.
const PURPOSE: &[u8] = b"a2tools.account.v1";

fn sealed_file(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("credentials.dat")
}

/// Seals the token and points the file at it. The sealed copy the file named
/// before is let go only once the new one is written; the folder must exist.
pub(super) fn save(app_data_dir: &Path, token: &str) -> Result<(), String> {
    let path = sealed_file(app_data_dir);
    let sealed = store::protect(token.as_bytes(), PURPOSE)
        .inspect_err(|why| tracing::error!("The account token could not be sealed: {why}"))?;
    let replaced = std::fs::read(&path).ok();
    if let Err(e) = files::write_private(&path, &sealed) {
        tracing::error!("Could not write {}: {e}", path.display());
        return Err(format!("the token file could not be written ({e})"));
    }
    if let Some(old) = replaced {
        store::forget(&old);
    }
    Ok(())
}

/// The stored token, if the store gives it back. A file that can never be
/// unsealed is deleted; one the keyring only holds back for now is kept.
pub fn load_stored(app_data_dir: &Path) -> Stored {
    let path = sealed_file(app_data_dir);
    let Ok(sealed) = std::fs::read(&path) else {
        return Stored::Missing;
    };
    match store::unprotect(&sealed, PURPOSE) {
        Ok(plain) => match String::from_utf8(plain) {
            Ok(token) if !token.is_empty() => Stored::Token(token),
            _ => discard(&path),
        },
        Err(UnsealError::Invalid) => discard(&path),
        Err(UnsealError::Unavailable) => {
            tracing::info!("The keyring did not hand over the account token now; keeping it");
            Stored::Locked
        }
    }
}

#[cfg(test)]
pub fn load(app_data_dir: &Path) -> Option<String> {
    match load_stored(app_data_dir) {
        Stored::Token(token) => Some(token),
        Stored::Missing | Stored::Locked => None,
    }
}

/// Signs out on this machine: the sealed token goes, then the file.
pub fn clear(app_data_dir: &Path) {
    let path = sealed_file(app_data_dir);
    if let Ok(sealed) = std::fs::read(&path) {
        store::forget(&sealed);
    }
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            tracing::warn!("Could not remove {}: {e}", path.display());
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::built::Scratch;

    /// Whether a secret store answers. Without one the keyring tests pass
    /// quietly, unless the run says a store must be there.
    fn store_answers() -> bool {
        let answers = store::available();
        if !answers && std::env::var_os("A2_REQUIRE_KEYRING").is_some() {
            panic!("A2_REQUIRE_KEYRING is set, but no secret store is available");
        }
        answers
    }

    #[test]
    fn a_saved_token_loads_back() {
        if !store_answers() {
            return;
        }
        let dir = Scratch::new("round-trip");
        assert_eq!(save(&dir, "tok_made_up_abc"), Ok(()));
        assert_eq!(load_stored(&dir), Stored::Token("tok_made_up_abc".into()));
        clear(&dir);
    }

    #[test]
    fn the_file_never_holds_the_token() {
        if !store_answers() {
            return;
        }
        let dir = Scratch::new("opaque");
        let token = "tok_made_up_hidden";
        save(&dir, token).unwrap();
        let bytes = std::fs::read(sealed_file(&dir)).unwrap();
        assert!(!bytes.windows(token.len()).any(|w| w == token.as_bytes()));
        clear(&dir);
    }

    #[test]
    fn a_new_token_lets_the_old_one_go() {
        if !store_answers() {
            return;
        }
        let dir = Scratch::new("replace");
        save(&dir, "tok_made_up_a").unwrap();
        let first = std::fs::read(sealed_file(&dir)).unwrap();
        save(&dir, "tok_made_up_b").unwrap();
        assert_ne!(std::fs::read(sealed_file(&dir)).unwrap(), first);
        assert!(store::unprotect(&first, PURPOSE).is_err(), "the first token is still in the store");
        assert_eq!(load(&dir).as_deref(), Some("tok_made_up_b"));
        clear(&dir);
    }

    #[test]
    fn signing_out_removes_token_and_file() {
        if !store_answers() {
            return;
        }
        let dir = Scratch::new("clear");
        save(&dir, "tok_made_up_x").unwrap();
        let sealed = std::fs::read(sealed_file(&dir)).unwrap();
        clear(&dir);
        assert_eq!(load(&dir), None);
        assert!(!sealed_file(&dir).exists());
        assert!(store::unprotect(&sealed, PURPOSE).is_err());
    }

    #[test]
    fn without_a_store_nothing_is_kept() {
        if store::available() {
            return;
        }
        let dir = Scratch::new("no-store");
        assert!(save(&dir, "tok_made_up_y").is_err());
        assert!(!sealed_file(&dir).exists());
    }

    #[test]
    fn an_empty_folder_has_no_token_and_clears_quietly() {
        let dir = Scratch::new("empty");
        assert_eq!(load_stored(&dir), Stored::Missing);
        assert_eq!(load(&dir), None);
        clear(&dir);
        assert!(std::fs::read_dir(&*dir).unwrap().next().is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_file_that_can_never_unseal_goes() {
        let dir = Scratch::new("foreign");
        let file = sealed_file(&dir);
        std::fs::write(&file, "not dpapi output").unwrap();
        assert_eq!(load_stored(&dir), Stored::Missing);
        assert!(!file.exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_token_held_back_by_the_keyring_is_kept() {
        let dir = Scratch::new("held");
        let file = sealed_file(&dir);
        std::fs::write(&file, "secret-service:no-such-item").unwrap();
        assert_eq!(load_stored(&dir), Stored::Locked);
        assert!(file.exists());
    }
}
