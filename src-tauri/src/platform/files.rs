//! Files in the meter's data folder are the player's alone: on Unix the
//! folders are created 0700 and the files 0600.

use std::io::Write;
use std::path::Path;

#[cfg(test)]
pub use super::os::files::symlink;
pub use super::os::files::{create_private_dir, private_options};

/// `std::fs::write`, creating a new file private.
pub fn write_private(path: impl AsRef<Path>, data: impl AsRef<[u8]>) -> std::io::Result<()> {
    private_options()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?
        .write_all(data.as_ref())
}
