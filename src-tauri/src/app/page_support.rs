//! What the pages ask the backend for: the icon cache, fetched URLs and log lines.

use super::AppState;

pub(crate) fn read_cached_icon(state: &AppState, key: String) -> Option<String> {
    if !crate::history::fight_history::is_plain_name(&key) {
        return None;
    }
    let path = state.app_data_dir.join("icon_cache").join(&key);
    std::fs::read_to_string(&path).ok()
}

pub(crate) fn write_cached_icon(state: &AppState, key: String, data: String) {
    if !crate::history::fight_history::is_plain_name(&key) {
        return;
    }
    let cache_dir = state.app_data_dir.join("icon_cache");
    let _ = std::fs::create_dir_all(&cache_dir);
    let path = cache_dir.join(&key);
    let _ = std::fs::write(&path, &data);
}

pub(crate) fn log_from_ui(message: String) {
    // A problem only the webview can see (an icon the CDN would not serve, say),
    // or a UI debug line, for debug.log. The UI rate-limits them; this keeps
    // each one short.
    let message: String = message.chars().take(300).collect();
    tracing::warn!("UI: {message}");
}
