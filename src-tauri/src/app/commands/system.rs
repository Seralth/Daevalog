//! Quitting, links, fetching for the page, its log lines and the icon cache.

use crate::platform;

use crate::app::tray_actions::save_fights_before_exit;
use crate::app::{page_support, AppState};

#[tauri::command]
pub(crate) fn quit_app(app: tauri::AppHandle) {
    save_fights_before_exit(&app);
    app.exit(0);
}

#[tauri::command]
pub(crate) fn read_cached_icon(state: tauri::State<'_, AppState>, key: String) -> Option<String> {
    page_support::read_cached_icon(&state, key)
}

#[tauri::command]
pub(crate) fn log_from_ui(message: String) {
    page_support::log_from_ui(message);
}

#[tauri::command]
pub(crate) fn write_cached_icon(state: tauri::State<'_, AppState>, key: String, data: String) {
    page_support::write_cached_icon(&state, key, data);
}

#[tauri::command]
pub(crate) async fn fetch_url(state: tauri::State<'_, AppState>, url: String) -> Result<String, String> {
    page_support::fetch_url(&state, url).await
}

#[tauri::command]
pub(crate) fn open_url(url: String) {
    platform::shell::open_url(&url);
}
