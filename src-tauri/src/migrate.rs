//! Moving the A2Tools build's data to the Daevalog identifier.
//!
//! The app identifier names the per-user folders: settings, saved fights,
//! slices, the sign-in reference and WebKit's own storage all live in
//! `<data dir>/<identifier>`. The fork was renamed from `com.a2tools.dps-meter`,
//! so on the first start of the renamed build each old folder is moved to the
//! new name. Moved, not copied: one copy of the data, no backup.

use std::path::{Path, PathBuf};

const OLD_IDENTIFIER: &str = "com.a2tools.dps-meter";
/// Must match `identifier` in tauri.conf.json (checked by a test).
const IDENTIFIER: &str = "com.daevalog.dps-meter";

/// Move `<base>/<old>` to `<base>/<new>` when only the old folder exists.
/// Returns whether it moved.
fn move_dir(base: &Path, old: &str, new: &str) -> std::io::Result<bool> {
    let from = base.join(old);
    let to = base.join(new);
    if to.exists() || !from.is_dir() {
        return Ok(false);
    }
    std::fs::rename(&from, &to)?;
    Ok(true)
}

/// Every base folder Tauri puts an identifier folder in: data, local data,
/// config and cache (the same folder twice on some systems).
fn bases() -> Vec<PathBuf> {
    let mut bases: Vec<PathBuf> = [dirs::data_dir(), dirs::data_local_dir(), dirs::config_dir(), dirs::cache_dir()]
        .into_iter()
        .flatten()
        .collect();
    bases.sort();
    bases.dedup();
    bases
}

pub fn from_a2tools() {
    for base in bases() {
        match move_dir(&base, OLD_IDENTIFIER, IDENTIFIER) {
            Ok(true) => tracing::info!(
                "Moved {} to {}",
                base.join(OLD_IDENTIFIER).display(),
                base.join(IDENTIFIER).display()
            ),
            Ok(false) => {}
            Err(e) => tracing::warn!("Could not move {}: {e}", base.join(OLD_IDENTIFIER).display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_old_folder_moves_once_and_never_over_a_new_one() {
        let base = std::env::temp_dir().join(format!("daevalog-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join(OLD_IDENTIFIER).join("history")).unwrap();
        std::fs::write(base.join(OLD_IDENTIFIER).join("settings.json"), b"{}").unwrap();

        assert!(move_dir(&base, OLD_IDENTIFIER, IDENTIFIER).unwrap());
        assert!(!base.join(OLD_IDENTIFIER).exists(), "moved, not copied");
        assert!(base.join(IDENTIFIER).join("settings.json").exists());
        assert!(base.join(IDENTIFIER).join("history").is_dir());

        // Second start: nothing left to move.
        assert!(!move_dir(&base, OLD_IDENTIFIER, IDENTIFIER).unwrap());

        // Both present: the new folder wins and the old one is left alone.
        std::fs::create_dir_all(base.join(OLD_IDENTIFIER)).unwrap();
        assert!(!move_dir(&base, OLD_IDENTIFIER, IDENTIFIER).unwrap());
        assert!(base.join(OLD_IDENTIFIER).exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_identifier_matches_the_tauri_config() {
        let conf: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(conf["identifier"], IDENTIFIER);
    }
}
