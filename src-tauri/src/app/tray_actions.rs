//! What the tray menu does, and the save that every way of quitting runs.

use std::time::Duration;

use tauri::Manager;

use crate::blocking::HISTORY;

use super::tool_windows::{open_settings_window, request_details_view};
use super::AppState;

/// Tray menu actions. Window creation runs as a task, off the event loop
/// that delivers the menu click (see `open_settings_window`).
pub(crate) fn open_settings_from_tray(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = open_settings_window(app).await {
            tracing::warn!("tray: could not open Settings: {e}");
        }
    });
}

pub(crate) fn open_history_from_tray(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = request_details_view(app, serde_json::json!({ "kind": "history" })).await {
            tracing::warn!("tray: could not open History: {e}");
        }
    });
}

pub(crate) fn quit_from_tray(app: &tauri::AppHandle) {
    quit(app);
}

/// Closing the overlay quits the meter, even with Settings or Details open.
pub(super) fn quit(app: &tauri::AppHandle) {
    save_fights_before_exit(app);
    app.exit(0);
}

/// How long quitting waits for the fights to be saved.
const EXIT_SAVE_WAIT: Duration = Duration::from_secs(5);
/// How long the exit save waits for a history job already running, so the
/// two do not write one fight at once. After that it saves anyway.
const EXIT_SAVE_TURN: Duration = Duration::from_secs(2);

/// Save every fight worth keeping before the meter closes, in progress or
/// not. Only the history record: a slice or an upload would hold up the exit.
/// Waits at most `EXIT_SAVE_WAIT`: a stuck disk cannot keep the meter open.
pub(super) fn save_fights_before_exit(app: &tauri::AppHandle) {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let Some(state) = app.try_state::<AppState>() else { return };
    // Nothing counted after the last snapshot would be saved.
    state.capture_suspended.store(true, std::sync::atomic::Ordering::SeqCst);
    let (saved_tx, saved_rx) = std::sync::mpsc::channel();
    let saving = app.clone();
    let spawned = std::thread::Builder::new().name("exit-save".into()).spawn(move || {
        let _turn = HISTORY.wait_turn(EXIT_SAVE_TURN);
        let state = saving.state::<AppState>();
        let (ticket, records) = {
            let mut calc = state.dps_calculator.lock();
            (state.fight_history.snapshot_ticket(), calc.snapshot_boss_fights_force())
        };
        for record in &records {
            if let Err(e) = state.fight_history.save_snapshot(record, ticket) {
                tracing::warn!("Failed to save {} on exit: {}", record.id, e);
            }
        }
        let _ = saved_tx.send(());
    });
    if let Err(e) = spawned {
        tracing::error!("Could not save fights before exit: {e}");
        return;
    }
    match saved_rx.recv_timeout(EXIT_SAVE_WAIT) {
        Ok(()) => {}
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) =>
            tracing::error!("Saving fights before exit timed out; exiting anyway"),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) =>
            tracing::error!("Saving fights before exit failed"),
    }
}

/// Write the last settings changes before the meter closes. Capped: a stuck
/// disk cannot keep the process alive.
pub(super) fn flush_settings_before_exit(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    if let Err(e) = state.settings.flush() {
        tracing::error!("Could not save settings before exit: {e}");
    }
}
