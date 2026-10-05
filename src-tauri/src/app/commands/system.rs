//! Quitting, links, fetching for the page, its log lines and the icon cache.

use std::time::Duration;

use crate::platform;

use crate::app::tray_actions::save_fights_before_exit;
use crate::app::AppState;

#[tauri::command]
pub(crate) fn quit_app(app: tauri::AppHandle) {
    save_fights_before_exit(&app);
    app.exit(0);
}

#[tauri::command]
pub(crate) fn read_cached_icon(state: tauri::State<'_, AppState>, key: String) -> Option<String> {
    if !crate::history::fight_history::is_plain_name(&key) {
        return None;
    }
    let path = state.app_data_dir.join("icon_cache").join(&key);
    std::fs::read_to_string(&path).ok()
}

#[tauri::command]
pub(crate) fn log_from_ui(message: String) {
    // A problem only the webview can see (an icon the CDN would not serve, say),
    // or a UI debug line, for debug.log. The UI rate-limits them; this keeps
    // each one short.
    let message: String = message.chars().take(300).collect();
    tracing::warn!("UI: {message}");
}

#[tauri::command]
pub(crate) fn write_cached_icon(state: tauri::State<'_, AppState>, key: String, data: String) {
    if !crate::history::fight_history::is_plain_name(&key) {
        return;
    }
    let cache_dir = state.app_data_dir.join("icon_cache");
    let _ = std::fs::create_dir_all(&cache_dir);
    let path = cache_dir.join(&key);
    let _ = std::fs::write(&path, &data);
}

#[tauri::command]
pub(crate) async fn fetch_url(state: tauri::State<'_, AppState>, url: String) -> Result<String, String> {
    state
        .http
        .get(&url)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn open_url(url: String) {
    platform::shell::open_url(&url);
}
