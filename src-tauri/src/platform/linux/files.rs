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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn the_folder_and_its_files_are_private() {
        let dir = std::env::temp_dir().join(format!("a2t-private-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, Permissions::from_mode(0o755)).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().mode() & 0o777;

        // An existing open folder is tightened; a new one is created private.
        create_private_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
        create_private_dir(&dir.join("history")).unwrap();
        assert_eq!(mode(&dir.join("history")), 0o700);

        let file = dir.join("settings.json");
        crate::platform::files::write_private(&file, b"{}").unwrap();
        assert_eq!(mode(&file), 0o600);
        crate::platform::files::write_private(&file, b"{\"a\":1}").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"{\"a\":1}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Where the game's record folders are. Linux: under Proton, inside each Steam library's prefix.
pub const GAME_RECORDS_IN_LOCAL_APPDATA: bool = false;
