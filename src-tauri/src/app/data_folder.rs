//! The meter's data folder, and which paths lie inside it.

use std::path::{Path, PathBuf};

/// `path` with every link resolved, if it exists and lies inside the data
/// folder (or is the folder itself).
pub(crate) fn inside(path: &Path, data_dir: &Path) -> Option<PathBuf> {
    let root = data_dir.canonicalize().ok()?;
    let path = path.canonicalize().ok()?;
    path.starts_with(&root).then_some(path)
}
