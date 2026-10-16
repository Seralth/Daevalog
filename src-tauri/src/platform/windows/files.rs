//! The data folder is in %APPDATA%, which Windows already keeps to its
//! user, so its folders and files take the access they inherit.

use std::fs::OpenOptions;
use std::path::Path;

pub fn create_private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

pub fn private_options() -> OpenOptions {
    OpenOptions::new()
}

/// A link to a folder or a file: Windows makes the two apart.
#[cfg(test)]
pub fn symlink(original: &Path, link: &Path) -> std::io::Result<()> {
    if original.is_dir() {
        std::os::windows::fs::symlink_dir(original, link)
    } else {
        std::os::windows::fs::symlink_file(original, link)
    }
}

/// The game keeps its records in %LOCALAPPDATA%.
pub const GAME_RECORDS_IN_LOCAL_APPDATA: bool = true;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::files::write_private;

    #[test]
    fn private_files_are_written_and_replaced() {
        let root = std::env::temp_dir().join(format!("daevalog-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("history");
        create_private_dir(&dir).expect("folder");
        create_private_dir(&dir).expect("an existing folder is fine");
        let file = dir.join("settings.json");
        write_private(&file, "{}").expect("first write");
        write_private(&file, r#"{"a":1}"#).expect("second write");
        assert_eq!(std::fs::read_to_string(&file).ok().as_deref(), Some(r#"{"a":1}"#));
        std::fs::remove_dir_all(&root).ok();
    }
}
