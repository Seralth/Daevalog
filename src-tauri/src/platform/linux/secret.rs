//! The sign-in token in the desktop keyring (Secret Service over D-Bus).
//! `credentials.dat` holds only `secret-service:<id>`, which names the item.
//!
//! The item's attributes, label and the reference text are a stored format:
//! sign-ins saved by 1.x and the RC must still be found, so they never change.

use std::sync::atomic::{AtomicU64, Ordering};

use secret_service::blocking::{Collection, Item, SecretService};
use secret_service::{EncryptionType, Error, SearchItemsResult};

use crate::platform::UnsealError;

const PREFIX: &str = "secret-service:";
// Attribute keys, and the application value every item of ours carries.
const KEY_APP: &str = "application";
const KEY_PURPOSE: &str = "purpose";
const KEY_ID: &str = "id";
const APP: &str = "a2tools-dps-meter";
const LABEL: &str = "Daevalog DPS Meter account";
const NO_KEYRING: &str = "no desktop keyring is running (GNOME Keyring, KWallet, or KeePassXC with \
Secret Service turned on); start one to stay signed in";

/// Saves made by this process, so two in the same instant still differ.
static SAVES: AtomicU64 = AtomicU64::new(0);

fn connect<'a>() -> Result<SecretService<'a>, Error> {
    SecretService::connect(EncryptionType::Dh)
}

/// Items carrying all the given attributes, unlocked and locked apart.
fn search<'a>(service: &'a SecretService<'a>, pairs: &[(&str, &str)]) -> Option<SearchItemsResult<Item<'a>>> {
    service.search_items(pairs.iter().copied().collect()).ok()
}

/// The item id in a stored reference, if it is one.
fn item_id(sealed: &[u8]) -> Option<&str> {
    match sealed.strip_prefix(PREFIX.as_bytes()).map(std::str::from_utf8) {
        Some(Ok(id)) if !id.is_empty() => Some(id),
        _ => None,
    }
}

/// An id no other save made: the time in ns, this process, and its count.
fn new_id() -> String {
    let nanos = std::time::UNIX_EPOCH.elapsed().map_or(0, |since| since.as_nanos());
    let count = SAVES.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:x}-{:x}-{count:x}", std::process::id())
}

/// The collection new items go in, unlocked. A fresh keyring may have none
/// yet; then we ask for the usual login one, and the keyring shows its own
/// prompt, as it does for unlocking.
fn open_collection<'a>(service: &'a SecretService<'a>) -> Result<Collection<'a>, String> {
    let target = service.get_any_collection().or_else(|e| {
        if !matches!(e, Error::NoResult) {
            return Err(format!("the keyring's collections could not be listed ({e})"));
        }
        service
            .create_collection("Login", "default")
            .map_err(|e| format!("the keyring has no collection and making one failed ({e})"))
    })?;
    let locked = target.is_locked().map_err(|e| format!("the keyring did not say whether it is locked ({e})"))?;
    if locked {
        target.unlock().map_err(|e| format!("the keyring stayed locked ({e})"))?;
    }
    Ok(target)
}

fn store(plaintext: &[u8], entropy: &[u8]) -> Result<String, String> {
    let purpose = std::str::from_utf8(entropy).map_err(|_| "the key purpose is not text".to_string())?;
    let service = connect().map_err(|_| NO_KEYRING.to_string())?;
    let target = open_collection(&service)?;
    let id = new_id();
    let attributes = [(KEY_APP, APP), (KEY_PURPOSE, purpose), (KEY_ID, id.as_str())].into_iter().collect();
    match target.create_item(LABEL, attributes, plaintext, true, "text/plain") {
        Ok(_) => Ok(id),
        Err(e) => Err(format!("the keyring would not store the sign-in ({e})")),
    }
}

/// Whether a Secret Service keyring answers at all.
pub fn available() -> bool {
    connect().map_err(|e| tracing::warn!("No Secret Service keyring answered: {e}")).is_ok()
}

pub fn protect(token: &[u8], entropy: &[u8]) -> Result<Vec<u8>, String> {
    store(token, entropy).map(|id| [PREFIX, &id].concat().into_bytes()).inspect_err(|why| {
        tracing::warn!("Keyring save failed: {why}");
    })
}

pub fn unprotect(sealed: &[u8], entropy: &[u8]) -> Result<Vec<u8>, UnsealError> {
    let (Some(id), Ok(purpose)) = (item_id(sealed), std::str::from_utf8(entropy)) else {
        return Err(UnsealError::Invalid);
    };
    // Past this point nothing is final: the keyring may start later, and a
    // dismissed prompt may be accepted next time.
    let try_later = UnsealError::Unavailable;
    let service = connect().map_err(|_| try_later)?;
    let found = search(&service, &[(KEY_APP, APP), (KEY_PURPOSE, purpose), (KEY_ID, id)]).ok_or(try_later)?;
    if let Some(open) = found.unlocked.first() {
        return open.get_secret().map_err(|_| try_later);
    }
    let shut = found.locked.first().ok_or(try_later)?;
    shut.unlock().map_err(|_| try_later)?;
    shut.get_secret().map_err(|_| try_later)
}

/// Delete the item a reference names, whatever purpose it was saved under.
pub fn forget(sealed: &[u8]) {
    let Some(id) = item_id(sealed) else { return };
    let Ok(service) = connect() else { return };
    let Some(found) = search(&service, &[(KEY_APP, APP), (KEY_ID, id)]) else { return };
    for result in found.unlocked.iter().chain(&found.locked).map(Item::delete) {
        if let Err(e) = result {
            tracing::warn!("Could not delete an old keyring item: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PURPOSE: &[u8] = b"a2tools.account.v1";

    #[test]
    fn references_name_a_keyring_item() {
        assert_eq!(item_id(b"secret-service:abc-1-0"), Some("abc-1-0"));
        assert_eq!(item_id(b"not dpapi output"), None);
    }

    #[test]
    fn new_ids_differ_and_stay_plain() {
        let ids = [new_id(), new_id()];
        assert_ne!(ids[0], ids[1]);
        let plain = |id: &String| !id.is_empty() && id.bytes().all(|c| c == b'-' || c.is_ascii_alphanumeric());
        assert!(ids.iter().all(plain), "{ids:?}");
    }

    #[test]
    fn bad_references_are_invalid_for_good() {
        let refs: [&[u8]; 3] = [b"not a reference", b"secret-service:", b"secret-service:\xff"];
        for sealed in refs {
            assert_eq!(unprotect(sealed, PURPOSE), Err(UnsealError::Invalid));
        }
        assert_eq!(unprotect(b"secret-service:abc", b"\xff"), Err(UnsealError::Invalid));
    }

    #[test]
    fn missing_items_may_come_back() {
        // With no keyring, or one that has no such item: try again later.
        assert_eq!(unprotect(b"secret-service:no-such-item", PURPOSE), Err(UnsealError::Unavailable));
    }

    #[test]
    fn non_text_purpose_is_refused() {
        assert!(protect(b"x", b"\xff").is_err());
    }

    #[test]
    fn forgetting_a_bad_reference_does_nothing() {
        forget(b"not a reference");
        forget(b"secret-service:");
    }

    #[test]
    fn two_saves_give_two_references() {
        if !available() {
            return;
        }
        let refs = [protect(b"first", PURPOSE).unwrap(), protect(b"second", PURPOSE).unwrap()];
        assert_ne!(refs[0], refs[1]);
        assert!(refs[0].starts_with(PREFIX.as_bytes()));
        refs.iter().for_each(|r| forget(r));
    }
}
