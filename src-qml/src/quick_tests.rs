// SPDX-License-Identifier: AGPL-3.0-only

//! The QML rules that need no meter, as Qt Quick Test files in `tests/`, run
//! by Qt's qmltestrunner on the offscreen platform.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Qt 6's qmltestrunner, from the folder qmake names for the Qt the build
/// uses (as cxx-qt-build finds it: $QMAKE, then qmake6). A plain
/// `qmltestrunner` on PATH can be Qt 5's.
fn runner() -> Option<PathBuf> {
    let qmake = std::env::var_os("QMAKE").map(PathBuf::from).unwrap_or_else(|| "qmake6".into());
    let out = Command::new(qmake).args(["-query", "QT_INSTALL_BINS"]).output().ok()?;
    let bins = String::from_utf8(out.stdout).ok()?;
    let path = Path::new(bins.trim()).join(format!("qmltestrunner{}", std::env::consts::EXE_SUFFIX));
    path.is_file().then_some(path)
}

/// Runs one test file, which must pass.
fn run(file: &str) {
    let Some(runner) = runner() else {
        eprintln!("Qt 6 qmltestrunner not found: the QML rules are not checked");
        return;
    };
    let test = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join(file);
    let out = Command::new(runner)
        .args(["-platform", "offscreen", "-input"])
        .arg(&test)
        // On Windows Qt otherwise writes to the debugger when no console is attached.
        .env("QT_FORCE_STDERR_LOGGING", "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{:?}\n{text}", out.status);
    assert!(text.contains("0 failed"), "{text}");
}

#[test]
fn tooltips_wait_for_the_pointer_to_rest() {
    run("tst_pointer_rest.qml");
}

#[test]
fn delete_asks_once_more() {
    run("tst_confirm_step.qml");
}

#[test]
fn auto_hide_waits_for_the_game_to_leave_the_front() {
    run("tst_auto_hide.qml");
}

#[test]
fn details_sorts_by_a_head_and_opens_damage_over_time() {
    run("tst_skill_order.qml");
}

#[test]
fn a_recorded_key_press_becomes_a_hotkey_label() {
    run("tst_hotkey_text.qml");
}

#[test]
fn history_shows_thirty_fights_at_a_time_and_folds_its_groups() {
    run("tst_history_pages.qml");
}

#[test]
fn a_number_with_nothing_behind_it_shows_a_dash() {
    run("tst_no_data.qml");
}
