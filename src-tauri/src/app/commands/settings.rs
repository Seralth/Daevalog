//! Settings: reading and storing them, and the ones that act at once.

use tauri::Emitter;

use crate::app::page_settings;
use crate::app::setting_changes::{self, apply_encounter_timeout, ENCOUNTER_TIMEOUT_KEY};
use crate::app::AppState;

#[tauri::command]
pub(crate) fn get_settings(state: tauri::State<'_, AppState>) -> std::collections::HashMap<String, String> {
    state.settings.get_all()
}

/// The system's clock, which times of day follow until the player picks one
/// in Settings (`dpsMeter.timeFormat`): "24h", "12h", or none when the system
/// does not say.
#[tauri::command]
pub(crate) fn system_time_format() -> Option<&'static str> {
    crate::platform::process::clock_24h().map(|h24| if h24 { "24h" } else { "12h" })
}

/// Store a setting and tell every window about it.
///
/// Settings are edited in their own window, so without this broadcast the meter
/// keeps rendering with whatever it read at startup — toggling something like
/// "Round DPS" would appear to do nothing until the app restarted. Only real
/// changes are emitted (see `Settings::set`), so the originating window's echo
/// stops here rather than bouncing between windows.
#[tauri::command]
pub(crate) fn update_settings(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    key: String,
    value: String,
) {
    // Settings only the backend writes, such as the screenshot folder.
    if key.starts_with("backend.") {
        tracing::warn!("The page may not set {key}");
        return;
    }
    if key == ENCOUNTER_TIMEOUT_KEY {
        apply_encounter_timeout(&state.data_storage, Some(&value));
    }
    if state.settings.set(&key, &value) {
        if key == crate::tray::HIDE_FROM_TASKBAR_KEY {
            crate::tray::apply_taskbar(&app);
        }
        let _ = app.emit("setting-changed", serde_json::json!({ "key": key, "value": value }));
    }
}

/// The page's old copy of the settings, handed over once (`app/page_settings.rs`).
/// Saved to disk before it answers, since the page then deletes its copy.
/// Returns every setting after the move.
#[tauri::command]
pub(crate) fn adopt_page_settings(
    state: tauri::State<'_, AppState>,
    values: std::collections::HashMap<String, String>,
) -> std::collections::HashMap<String, String> {
    let offered = values.len();
    let moved = page_settings::adopt(&state.settings, values);
    tracing::info!("Settings from the page's storage: {offered} found, {} moved to settings.json {moved:?}", moved.len());
    if !moved.is_empty() {
        if let Err(e) = state.settings.flush() {
            tracing::warn!("Could not save the moved settings: {e}");
        }
    }
    state.settings.get_all()
}

#[tauri::command]
pub(crate) fn clear_settings(state: tauri::State<'_, AppState>) {
    state.settings.clear();
}

#[tauri::command]
pub(crate) fn set_language(state: tauri::State<'_, AppState>, language: String) {
    setting_changes::set_language(&state, language);
}

#[tauri::command]
pub(crate) fn set_debug_logging(state: tauri::State<'_, AppState>, enabled: bool) {
    setting_changes::set_debug_logging(&state, enabled);
}

#[tauri::command]
pub(crate) fn set_packet_logging(state: tauri::State<'_, AppState>, enabled: bool) {
    setting_changes::set_packet_logging(&state, enabled);
}
