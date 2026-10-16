//! File modes: the data folder 0700, the files in it 0600.

use std::fs::{OpenOptions, Permissions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// Create the folder (parents as usual) and make it the owner's only. A folder
/// an older version created open is tightened too.
pub fn create_private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    std::fs::set_permissions(path, Permissions::from_mode(0o700))
}

/// Options that create a file only its owner can read or write.
pub fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.mode(0o600);
    options
}

#[cfg(test)]
pub fn symlink(original: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(original, link)
}

/// Where the game's record folders are. Linux: under Proton, inside each Steam library's prefix.
pub const GAME_RECORDS_IN_LOCAL_APPDATA: bool = false;

#[cfg(test)]
mod tests {
    use super::*;

    use crate::platform::files::write_private;
    use std::os::unix::fs::DirBuilderExt;

    fn mode_bits(path: &Path) -> Option<u32> {
        Some(std::fs::metadata(path).ok()?.permissions().mode() & 0o777)
    }

    #[test]
    fn data_folder_is_owner_only() {
        let root = std::env::temp_dir().join(format!("daevalog-modes-{}", std::process::id()));
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("old scratch folder");
        }
        std::fs::DirBuilder::new().mode(0o755).create(&root).expect("scratch folder");
        let (history, settings) = (root.join("history"), root.join("settings.json"));

        // An open folder from before is tightened; a new one starts private.
        for dir in [&root, &history] {
            create_private_dir(dir).expect("private folder");
            assert_eq!(mode_bits(dir), Some(0o700), "{}", dir.display());
        }

        write_private(&settings, "{}").expect("first write");
        assert_eq!(mode_bits(&settings), Some(0o600));
        write_private(&settings, r#"{"a":1}"#).expect("second write");
        assert_eq!(std::fs::read_to_string(&settings).ok().as_deref(), Some(r#"{"a":1}"#));

        std::fs::remove_dir_all(&root).ok();
    }
}
