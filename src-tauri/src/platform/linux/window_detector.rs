//! Whether the game runs, from /proc. Wayland keeps other clients' windows
//! to themselves, so the process stands in for the window, and focus is not
//! something we can know.

use crate::platform::procfs::{argv0, proc_entry_is_game};

/// Whether `is_aion2_foreground` can tell: not here, so auto-hide is not offered.
pub const SEES_FOREGROUND: bool = false;

/// Always true: with no way to see focus, auto-hide must never hide the meter.
pub fn is_aion2_foreground() -> bool {
    true
}

/// One process: its pid, `comm` and raw `cmdline`. Unreadable files read
/// as empty, so a process that exited halfway is simply not the game.
struct Proc {
    pid: String,
    comm: String,
    cmdline: Vec<u8>,
}

impl Proc {
    fn argv0(&self) -> std::borrow::Cow<'_, str> {
        argv0(&self.cmdline)
    }
}

/// Every process /proc shows; none when /proc cannot be listed.
fn processes() -> Vec<Proc> {
    let mut found = Vec::new();
    let names = std::fs::read_dir("/proc").into_iter().flatten().flatten().map(|e| e.file_name());
    for pid in names.filter_map(|name| name.into_string().ok()) {
        if pid.is_empty() || pid.bytes().any(|b| !b.is_ascii_digit()) {
            continue;
        }
        let read = |file: &str| std::fs::read(format!("/proc/{pid}/{file}")).unwrap_or_default();
        let comm = String::from_utf8_lossy(&read("comm")).into_owned();
        found.push(Proc { cmdline: read("cmdline"), comm, pid });
    }
    found
}

pub fn find_aion2_window() -> bool {
    processes().iter().any(|p| proc_entry_is_game(&p.comm, &p.cmdline))
}

/// "AION2" while the game runs: there is no real title to read.
pub fn find_aion2_window_title() -> Option<String> {
    find_aion2_window().then(|| "AION2".to_string())
}

/// Processes whose name or first argument mentions aion, for the log. At most 8.
pub fn describe_candidates() -> Vec<String> {
    let mentions = |s: &str| s.to_ascii_lowercase().contains("aion");
    processes()
        .into_iter()
        .filter(|p| mentions(&p.comm) || mentions(&p.argv0()))
        .take(8)
        .map(|p| format!("pid {} comm {:?} argv0 {:?}", p.pid, p.comm.trim(), p.argv0()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_answers_agree() {
        assert!(is_aion2_foreground());
        assert!(!SEES_FOREGROUND, "so auto-hide is not offered");
        // The game may run on this machine while tests do, so only check the
        // answers fit each other.
        let title = find_aion2_window_title();
        assert!(matches!(title.as_deref(), None | Some("AION2")));
        assert_eq!(find_aion2_window(), title.is_some());
        assert!(describe_candidates().len() <= 8);
    }

    #[test]
    fn this_test_process_is_listed_but_not_the_game() {
        let own = std::process::id().to_string();
        let me = processes().into_iter().find(|p| p.pid == own).expect("own /proc entry");
        assert!(!me.comm.is_empty() && !me.argv0().is_empty());
        assert!(!proc_entry_is_game(&me.comm, &me.cmdline));
    }
}
