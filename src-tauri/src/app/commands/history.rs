//! Saved fights: the list, loading, saving, deleting and export.

use crate::entity::fight_record::{FightRecord, FightSummary};
use crate::share;

use crate::app::AppState;

#[tauri::command]
/// `async` keeps this off the main thread. Sync commands run there, and even
/// with the summary cache a cold call reads every fight file — measured at
/// ~350ms, during which no other IPC and no window painting can proceed. Each
/// window calls this at startup and again every 10s, which is what made opening
/// History feel like it hung.
pub(crate) async fn get_fight_history(state: tauri::State<'_, AppState>) -> Result<Vec<FightSummary>, String> {
    Ok(state.fight_history.list_fights())
}

#[tauri::command]
pub(crate) fn save_fight(state: tauri::State<'_, AppState>, record: FightRecord) -> Result<(), String> {
    state.fight_history.save_fight(&record)
}

#[tauri::command]
pub(crate) fn load_fight(state: tauri::State<'_, AppState>, id: String) -> Result<FightRecord, String> {
    let mut record = state.fight_history.load_fight(&id)?;

    // Re-resolve supporter status against the roster as it is *now*, rather
    // than trusting the flag written when the fight was saved. Supporter status
    // changes; a fight from last month opened today should show who is a
    // supporter today, and every record saved before this feature existed has
    // no flag at all.
    //
    // Party members are the honest limitation here. `obscure_nickname` masks
    // their names before the record is written, so a name-keyed roster can only
    // ever match the local player, whose name is stored intact. `dbid` is kept
    // on each actor precisely so a dbid-keyed roster resolves everyone — see
    // `crate::supporters::KeyKind`.
    crate::supporters::apply_to_record(&mut record, &state.data_storage.supporters());
    Ok(record)
}

#[tauri::command]
pub(crate) fn delete_fight(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    if !crate::history::fight_history::is_plain_name(&id) {
        return Err(format!("Invalid fight id: {id:?}"));
    }
    share::forget_slice(&state.app_data_dir, &id);
    state.fight_history.delete_fight(&id)
}

#[tauri::command]
pub(crate) fn export_fight_json(state: tauri::State<'_, AppState>, record: FightRecord) -> Result<String, String> {
    state.fight_history.export_fight_json(&record)
}
