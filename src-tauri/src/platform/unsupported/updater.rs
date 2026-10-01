use std::path::Path;

/// There is no installer format for this platform yet.
pub fn run_installer(_package: &Path, _install_dir: &str) -> Result<(), String> {
    Err("automatic updates are not supported on this platform yet".to_string())
}
