//! Opening a link or folder in the desktop's apps. Tauri's opener starts the
//! app through a fork that exits at once and never waits for that fork, so
//! each link or folder left a zombie until the meter quit (issue #22). The
//! same launchers start here, and the fork is waited for.

use std::ffi::OsStr;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

pub fn open_url(url: &str) -> Result<(), String> {
    open(OsStr::new(url))
}

pub fn open_folder(path: &Path) -> Result<(), String> {
    open(path.as_os_str())
}

/// The first launcher that starts: xdg-open, then gio, gnome-open, kde-open.
fn open(target: &OsStr) -> Result<(), String> {
    let mut last = None;
    for command in open::commands(target) {
        match spawn_detached(command) {
            Ok(_) => return Ok(()),
            Err(e) => last = Some(e),
        }
    }
    Err(last.map_or_else(|| "no launcher".into(), |e| e.to_string()))
}

/// Start `command` in a session of its own, through a fork that exits at
/// once, so the app is not the meter's child and outlives it. Returns that
/// fork's pid, already reaped.
fn spawn_detached(mut command: Command) -> io::Result<u32> {
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: only fork, _exit and setsid run between fork and exec.
    unsafe {
        command.pre_exec(|| {
            match libc::fork() {
                -1 => return Err(io::Error::last_os_error()),
                0 => {}
                _ => libc::_exit(0),
            }
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    // Does not block: spawn returns once the fork has exited and the app's
    // exec is done. Waiting reaps the fork; the app goes to init.
    child.wait()?;
    Ok(child.id())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The state of `pid` from /proc/<pid>/stat, while it is a child of this
    /// process; `None` once it is not.
    fn child_state(pid: u32) -> Option<char> {
        let pid = pid.to_string();
        let listed = std::fs::read_dir("/proc/self/task").unwrap().flatten().any(|task| {
            std::fs::read_to_string(task.path().join("children"))
                .unwrap_or_default()
                .split_whitespace()
                .any(|child| child == pid)
        });
        if !listed {
            return None;
        }
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        // The name is in parentheses and may hold anything; the state follows it.
        Some(stat.rsplit_once(") ").and_then(|(_, rest)| rest.chars().next()).unwrap_or('?'))
    }

    #[test]
    fn an_opened_app_leaves_no_zombie() {
        let pid = spawn_detached(Command::new("true")).unwrap();
        assert_eq!(child_state(pid), None, "the fork in between is still a child (state Z is a zombie)");
    }

    #[test]
    fn opening_does_not_wait_for_the_app() {
        let mut app = Command::new("sleep");
        app.arg("2");
        let started = std::time::Instant::now();
        spawn_detached(app).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(1), "{:?}", started.elapsed());
    }

    #[test]
    fn a_launcher_that_cannot_start_is_an_error() {
        assert!(spawn_detached(Command::new("/nonexistent/daevalog-launcher")).is_err());
    }
}
