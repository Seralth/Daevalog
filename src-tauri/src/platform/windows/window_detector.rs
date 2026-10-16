//! Whether the game runs and is in front: its process (AION2.exe) from the
//! process list, its window from the top-level windows.

use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
};

use super::from_wide;
use crate::platform::procfs::proc_entry_is_game;

/// One process: its id and its .exe file name.
struct Proc {
    pid: u32,
    exe: String,
}

/// Every process the snapshot shows; none when it cannot be taken.
fn processes() -> Vec<Proc> {
    let mut found = Vec::new();
    // SAFETY: the snapshot handle is closed below; `entry` has its size set
    // as Process32FirstW requires.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return found };
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            found.push(Proc { pid: entry.th32ProcessID, exe: from_wide(&entry.szExeFile) });
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    found
}

fn game_pids() -> Vec<u32> {
    processes().into_iter().filter(|p| proc_entry_is_game(&p.exe, b"")).map(|p| p.pid).collect()
}

/// The pid of the process that owns `window`.
fn window_pid(window: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: a valid pointer to a u32; a stale handle gives 0.
    unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    pid
}

/// What the window search looks for and what it found.
struct Search {
    pids: Vec<u32>,
    title: Option<String>,
}

unsafe extern "system" fn visit(window: HWND, state: LPARAM) -> BOOL {
    // SAFETY: `state` is the `Search` that `game_window_title` passes, alive
    // for the whole EnumWindows call.
    let search = unsafe { &mut *(state.0 as *mut Search) };
    // SAFETY: plain queries on a window handle EnumWindows gave.
    let visible = unsafe { IsWindowVisible(window) }.as_bool();
    if visible && search.pids.contains(&window_pid(window)) {
        let mut text = [0u16; 256];
        // SAFETY: the buffer is writable and its length is passed with it.
        let len = unsafe { GetWindowTextW(window, &mut text) }.max(0) as usize;
        let title = from_wide(&text[..len.min(text.len())]);
        if !title.trim().is_empty() {
            search.title = Some(title);
            return BOOL(0);
        }
    }
    BOOL(1)
}

/// The title of a visible window of one of `pids`.
fn game_window_title(pids: Vec<u32>) -> Option<String> {
    let mut search = Search { pids, title: None };
    // SAFETY: the callback reads `search` only while EnumWindows runs. It
    // stops the walk early, which EnumWindows reports as an error.
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize)) };
    search.title
}

/// Whether `is_aion2_foreground` can tell.
pub const SEES_FOREGROUND: bool = true;

/// Whether the window in front belongs to the game.
pub fn is_aion2_foreground() -> bool {
    // SAFETY: no arguments; a null handle when no window is in front.
    let front = unsafe { GetForegroundWindow() };
    if front.is_invalid() {
        return false;
    }
    let pid = window_pid(front);
    pid != 0 && game_pids().contains(&pid)
}

pub fn find_aion2_window() -> bool {
    !game_pids().is_empty()
}

/// The game window's title while the game runs; "AION2" before its window
/// shows.
pub fn find_aion2_window_title() -> Option<String> {
    let pids = game_pids();
    if pids.is_empty() {
        return None;
    }
    game_window_title(pids).or_else(|| Some("AION2".to_string()))
}

/// Processes whose file name mentions aion, for the log. At most 8.
pub fn describe_candidates() -> Vec<String> {
    processes()
        .into_iter()
        .filter(|p| p.exe.to_ascii_lowercase().contains("aion"))
        .take(8)
        .map(|p| format!("pid {} exe {:?}", p.pid, p.exe))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_test_process_is_listed_but_not_the_game() {
        let own = std::process::id();
        let me = processes().into_iter().find(|p| p.pid == own).expect("own process in the snapshot");
        assert!(me.exe.to_ascii_lowercase().ends_with(".exe"), "{}", me.exe);
        assert!(!proc_entry_is_game(&me.exe, b""));
    }

    #[test]
    fn detector_answers_agree() {
        // The game may run on this machine while tests do, so only check the
        // answers fit each other.
        assert_eq!(find_aion2_window(), find_aion2_window_title().is_some());
        assert!(describe_candidates().len() <= 8);
        if !find_aion2_window() {
            assert!(!is_aion2_foreground());
        }
    }
}
