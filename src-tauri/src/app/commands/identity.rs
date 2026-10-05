//! Who the local player is: the name and id the windows send.

use crate::app::local_player::{self, bind_local_actor, bind_local_name};
use crate::app::AppState;

#[tauri::command]
pub(crate) fn set_character_name(state: tauri::State<'_, AppState>, name: String, manual: Option<bool>) {
    local_player::set_character_name(&state, name, manual);
}

/// `manual`: typed in Settings, so it wins over the game's identity.
/// `view`: the window that sent it, for the log.
#[tauri::command]
pub(crate) fn bind_local_actor_id(
    state: tauri::State<'_, AppState>,
    actor_id: i64,
    manual: Option<bool>,
    view: Option<String>,
) {
    bind_local_actor(&state.data_storage, actor_id, manual.unwrap_or(false), &view.unwrap_or_default());
}

#[tauri::command]
pub(crate) fn bind_local_nickname(
    state: tauri::State<'_, AppState>,
    actor_id: i64,
    nickname: String,
    manual: Option<bool>,
    view: Option<String>,
) {
    bind_local_name(&state.data_storage, actor_id, &nickname, manual.unwrap_or(false), &view.unwrap_or_default());
}
