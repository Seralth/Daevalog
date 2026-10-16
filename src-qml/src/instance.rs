// SPDX-License-Identifier: AGPL-3.0-only

//! One meter per data folder (`cpp/instance.cpp`): a second start asks the
//! running meter to show its window and quits. Keyed on the folder, so a
//! test run with its own data folder runs beside the real meter.

use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("cxx-qt-lib/qqmlapplicationengine.h");
        type QQmlApplicationEngine = cxx_qt_lib::QQmlApplicationEngine;

        include!("daevalog-qml/cpp/instance.h");

        #[cxx_name = "claimInstance"]
        fn claim_instance(folder: &QString) -> bool;

        #[cxx_name = "listenInstance"]
        fn listen_instance(engine: Pin<&mut QQmlApplicationEngine>);

        // The tests' own.
        #[allow(dead_code)]
        #[cxx_name = "releaseInstance"]
        fn release_instance();

        #[allow(dead_code)]
        #[cxx_name = "waitForShowRequest"]
        fn wait_for_show_request(ms: i32) -> bool;
    }
}

/// Whether this run is one the guard is for: a normal live start. Replays,
/// screenshots and --open views are checks, which run beside the meter.
pub fn guarded(options: &crate::Options) -> bool {
    options.replay.is_none() && options.open.is_none() && options.screenshot.is_none()
}

/// Takes the folder for this meter; false when another meter has it (and
/// was asked to show its window).
pub fn claim(folder: &std::path::Path) -> bool {
    ffi::claim_instance(&QString::from(&*folder.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // The guard is one per process; the tests take turns.
    static TURN: Mutex<()> = Mutex::new(());

    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("daevalog-instance-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_second_start_asks_the_first_to_show_and_stops() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        let dir = folder("second");
        assert!(claim(&dir), "the first start takes the folder");
        // Listening, as the first meter does once Qt runs.
        assert!(!ffi::wait_for_show_request(0));
        // The second start is its own thread: on Windows its request is
        // written only while the first meter reads.
        let second = std::thread::spawn({
            let dir = dir.clone();
            move || claim(&dir)
        });
        assert!(ffi::wait_for_show_request(2000), "the first meter is asked to show");
        assert!(!second.join().unwrap(), "a second start is refused");
        ffi::release_instance();
        assert!(claim(&dir), "free again once the first meter is gone");
        ffi::release_instance();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_lock_left_by_a_crash_is_taken_over() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        let dir = folder("stale");
        std::fs::create_dir_all(&dir).unwrap();
        // A process that has ended: its id names no running process.
        let mut gone = std::process::Command::new("true").spawn().unwrap();
        let pid = gone.id();
        gone.wait().unwrap();
        // QLockFile's own layout: id, program, host.
        std::fs::write(dir.join("instance.lock"), format!("{pid}\ndaevalog-qml\n\n")).unwrap();
        std::os::unix::net::UnixListener::bind(dir.join("instance.socket")).unwrap();
        assert!(claim(&dir), "a crashed meter's lock does not block the next start");
        ffi::release_instance();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn check_runs_skip_the_guard() {
        let run = |list: &[&str]| {
            let list: Vec<String> = list.iter().map(|s| s.to_string()).collect();
            match crate::parse_args(&list) {
                Ok(crate::Command::Run(options)) => guarded(&options),
                other => panic!("{other:?}"),
            }
        };
        assert!(run(&[]));
        assert!(run(&["--locked", "--mode", "enc"]));
        assert!(!run(&["--replay", "p.txt"]));
        assert!(!run(&["--open", "settings/meter"]));
        assert!(!run(&["--screenshot", "m.png"]));
    }
}
