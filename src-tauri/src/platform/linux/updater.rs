//! Updating the Arch package: pacman installs the new package, and pkexec
//! asks for the password through the desktop's own prompt (KDE, GNOME…). The
//! package's upgrade script grants packet capture to the new binary again.

use std::path::Path;

const INSTALLED_BINARY: &str = "/usr/bin/a2tools-dps-meter";

/// Only the meter pacman installed updates itself. A build run from a
/// checkout, or a copy installed some other way, is left alone.
pub fn supported() -> bool {
    std::env::current_exe().is_ok_and(|exe| exe == Path::new(INSTALLED_BINARY))
        && Path::new("/usr/bin/pacman").exists()
        && Path::new("/usr/bin/pkexec").exists()
}

/// The manifest's package for this platform.
pub fn package_url<'a>(_msi_url: &'a str, arch_url: &'a str) -> &'a str {
    arch_url
}

/// Hand the package to pacman once the meter has exited, then start the meter
/// again: the new one, or the old one if the password prompt was cancelled,
/// so a refused update never leaves the player without a meter.
pub fn run_installer(package: &Path, _install_dir: &str) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    const SCRIPT: &str = r#"
        while kill -0 "$2" 2>/dev/null; do sleep 0.2; done
        pkexec /usr/bin/pacman -U --noconfirm "$1"
        rm -f "$1"
        exec /usr/bin/a2tools-dps-meter
    "#;
    tracing::info!("Updating through pacman: {}", package.display());
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(SCRIPT)
        .arg("a2tools-update")
        .arg(package)
        .arg(std::process::id().to_string())
        .stdin(std::process::Stdio::null())
        // Its own process group, so closing a terminal the meter was started
        // from does not take the update down with it.
        .process_group(0)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not start the update: {e}"))
}
