//! Where the character files live, and how one is written so a reader never
//! sees half of it.
//!
//! `<data folder>/characters/<server>-<key>.json`, one per character. The
//! key is the first 8 bytes of SHA-256 over `<server>:<name>` (the name as
//! the game sends it), as 16 hex digits: the same character always gets the
//! same file, and no name shows in a file name. A character renamed in game
//! starts a new file. A file is written to `.<file name>.tmp` beside it,
//! then renamed over it; readers skip names that start with a dot.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::file::{CharacterFile, FileError};
use crate::platform::files::{create_private_dir, write_private};

/// The folder in the data folder.
pub const FOLDER: &str = "characters";

/// The file key of the character `name` on `server`.
pub fn key(server: u16, name: &str) -> String {
    let digest = Sha256::digest(format!("{server}:{name}").as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// `<server>-<key>.json`.
pub fn file_name(server: u16, name: &str) -> String {
    format!("{server}-{}.json", key(server, name))
}

/// Is this a character file's name (not a temporary file or anything else)?
fn is_file_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".json") else { return false };
    let Some((server, key)) = stem.split_once('-') else { return false };
    !server.is_empty()
        && server.bytes().all(|b| b.is_ascii_digit())
        && key.len() == 16
        && key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Where the file of the character `name` on `server` lives in `data_dir`.
pub fn path(data_dir: &Path, server: u16, name: &str) -> PathBuf {
    data_dir.join(FOLDER).join(file_name(server, name))
}

/// Writes the character `name`'s file into `data_dir`'s folder, and returns
/// its path.
pub fn save(data_dir: &Path, name: &str, file: &CharacterFile) -> std::io::Result<PathBuf> {
    let path = path(data_dir, file.server, name);
    create_private_dir(&data_dir.join(FOLDER))?;
    let temp = path.with_file_name(format!(".{}.tmp", file_name(file.server, name)));
    let written = write_private(&temp, file.to_json()).and_then(|()| std::fs::rename(&temp, &path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written.map(|()| path)
}

pub fn load(path: &Path) -> Result<CharacterFile, FileError> {
    let text = std::fs::read_to_string(path).map_err(|e| FileError::Io(e.to_string()))?;
    CharacterFile::from_json(&text)
}

/// The character files in `data_dir`'s folder, the most recently written
/// first.
pub fn list(data_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(data_dir.join(FOLDER)) else { return Vec::new() };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_str().is_some_and(is_file_name))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    files.into_iter().map(|(_, path)| path).collect()
}
