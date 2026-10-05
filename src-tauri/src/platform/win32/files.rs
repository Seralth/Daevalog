//! No Unix modes on Windows: the data folder sits in the user's profile, whose
//! access rules already keep other users out.

use std::fs::OpenOptions;
use std::path::Path;

pub fn create_private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

pub fn private_options() -> OpenOptions {
    OpenOptions::new()
}

/// Needs Developer Mode or an elevated account; tests skip when it fails.
#[cfg(test)]
pub fn symlink(original: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(original, link)
}
