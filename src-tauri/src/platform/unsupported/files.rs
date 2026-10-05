//! No file modes here: folders and files take the system's defaults.

use std::fs::OpenOptions;
use std::path::Path;

pub fn create_private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

pub fn private_options() -> OpenOptions {
    OpenOptions::new()
}

#[cfg(test)]
pub fn symlink(_original: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}
