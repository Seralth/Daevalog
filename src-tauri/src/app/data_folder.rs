//! The meter's data folder, and which paths lie inside it.

use std::path::{Path, PathBuf};

/// `path` with every link resolved, if it exists and lies inside the data
/// folder (or is the folder itself).
pub(crate) fn inside(path: &Path, data_dir: &Path) -> Option<PathBuf> {
    let root = data_dir.canonicalize().ok()?;
    let path = path.canonicalize().ok()?;
    path.starts_with(&root).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_and_climbs_out_of_the_folder_are_outside() {
        let base = std::env::temp_dir().join(format!("a2t-data-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let data = base.join("data");
        let outside = base.join("outside");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(data.join("packets_1.txt"), b"x").unwrap();
        std::fs::write(outside.join("packets_2.txt"), b"x").unwrap();

        let own = data.join("packets_1.txt");
        assert_eq!(inside(&own, &data), Some(own.canonicalize().unwrap()));
        assert_eq!(inside(&outside.join("packets_2.txt"), &data), None);
        assert_eq!(inside(&data.join("..").join("outside").join("packets_2.txt"), &data), None);
        assert_eq!(inside(&data.join("missing.txt"), &data), None);
        // A link inside the folder that points out of it.
        if crate::platform::files::symlink(&outside, &data.join("link")).is_ok() {
            assert_eq!(inside(&data.join("link").join("packets_2.txt"), &data), None);
        }

        let _ = std::fs::remove_dir_all(&base);
    }
}
