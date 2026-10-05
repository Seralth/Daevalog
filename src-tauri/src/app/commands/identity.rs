//! Who the local player is: the name and id the windows send.

use crate::app::local_player::{bind_local_actor, bind_local_name, is_placeholder_id};
use crate::app::AppState;

#[tauri::command]
pub(crate) fn set_character_name(state: tauri::State<'_, AppState>, name: String, manual: Option<bool>) {
    // The game has said who is playing; a name from the window title or the
    // last session is at best the same and at worst another character. A name
    // the player typed (`manual`) is taken anyway: it is their call, and the
    // game's next self record replaces it if it was wrong.
    let ds = &state.data_storage;
    if ds.local_identity_from_self_record() && !manual.unwrap_or(false) {
        return;
    }
    let trimmed = name.trim().to_string();
    ds.set_local_character_name(Some(name));
    // If an actor ID was already bound, put the name on it now so the meter
    // updates. Not permanent: the name follows the local id, not this one.
    if !trimmed.is_empty() {
        if let Some(id) = ds.local_player_id().filter(|&id| !is_placeholder_id(id)) {
            ds.set_local_nickname(id as i32, &trimmed);
        }
    }
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
