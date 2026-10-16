//! Telling the game apart from other processes by what `/proc/<pid>` shows.
//! No file access here, so it tests anywhere.

use std::borrow::Cow;

const GAME_EXE: &str = "AION2.exe";

/// The first argument in a raw `cmdline`: up to the first NUL (all of it
/// when there is none), as UTF-8 with bad bytes replaced.
pub(crate) fn argv0(cmdline: &[u8]) -> Cow<'_, str> {
    let end = cmdline.iter().position(|&b| b == 0).unwrap_or(cmdline.len());
    String::from_utf8_lossy(&cmdline[..end])
}

/// Whether a process is the game: by `comm` (Wine sets it to the .exe name),
/// else by the file name of argv0 (`cmdline` up to its first NUL). Later
/// arguments do not count, so Proton's wrapper, which only passes the game's
/// path on, is not the game.
pub fn proc_entry_is_game(comm: &str, cmdline: &[u8]) -> bool {
    let argv0 = argv0(cmdline);
    let file_name = match argv0.rfind(['/', '\\']) {
        Some(cut) => &argv0[cut + 1..],
        None => &argv0[..],
    };
    [comm, file_name].iter().any(|name| name.trim().eq_ignore_ascii_case(GAME_EXE))
}

#[cfg(test)]
mod tests {
    use super::proc_entry_is_game;

    const STEAM_PATH: &[u8] = b"Z:\\home\\p\\.steam\\steamapps\\common\\AION2\\Aion2\\Binaries\\Win64\\AION2.exe\0-arg\0";
    const PROTON: &[u8] = b"python3\0/proton\0waitforexitandrun\0Z:\\games\\AION2.exe\0";

    #[test]
    fn game_by_comm_or_first_argument() {
        let game = [
            ("AION2.exe", &b""[..]),
            ("AION2.exe\n", b""),
            ("aion2.EXE", b""),
            ("wine64-preload", STEAM_PATH),
            ("wine64-preload", b"Z:\\x\\Win64\\AION2.exe\0-arg\0"),
            ("x", b"/opt/games/aion2.exe\0"),
            ("x", b"AION2.exe"),
        ];
        let missed: Vec<_> = game.iter().filter(|(comm, cmd)| !proc_entry_is_game(comm, cmd)).collect();
        assert!(missed.is_empty(), "{missed:?}");
    }

    #[test]
    fn proton_wrapper_and_launcher_are_other_processes() {
        let others = [("x", &b""[..]), ("python3", PROTON), ("AION2Launcher", b"AION2Launcher.exe\0"), ("x", b"C:\\AION2.exe.bak\0")];
        let taken: Vec<_> = others.iter().filter(|(comm, cmd)| proc_entry_is_game(comm, cmd)).collect();
        assert!(taken.is_empty(), "{taken:?}");
    }
}
