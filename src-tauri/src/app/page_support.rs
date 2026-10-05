//! What the pages ask the backend for: the icon cache, fetched URLs and log lines.

use super::AppState;

pub(crate) fn read_cached_icon(state: &AppState, key: String) -> Option<String> {
    if !crate::history::fight_history::is_plain_name(&key) {
        return None;
    }
    let path = state.app_data_dir.join("icon_cache").join(&key);
    std::fs::read_to_string(&path).ok()
}
