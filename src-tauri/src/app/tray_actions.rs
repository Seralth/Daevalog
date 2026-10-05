//! What the tray menu does, and the save that every way of quitting runs.

use tauri::Manager;

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
    save_fights_before_exit(app);
    app.exit(0);
}

/// Save every fight worth keeping before the meter closes, in progress or
/// not. Only the history record: a slice or an upload would hold up the exit.
pub(super) fn save_fights_before_exit(app: &tauri::AppHandle) {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let Some(state) = app.try_state::<AppState>() else { return };
    let records = state.dps_calculator.lock().snapshot_boss_fights_force();
    for record in &records {
        if let Err(e) = state.fight_history.save_fight(record) {
            tracing::warn!("Failed to save {} on exit: {}", record.id, e);
        }
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
