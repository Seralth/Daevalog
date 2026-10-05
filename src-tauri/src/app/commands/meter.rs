//! The live meter: its numbers, the Details behind a row, target modes and reset.

use crate::entity::details_context::{DetailsContext, TargetDetailsResponse};
use crate::entity::dps_data::DpsData;

use crate::app::AppState;

#[tauri::command]
pub(crate) fn get_app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[tauri::command]
pub(crate) fn get_dps_snapshot(state: tauri::State<'_, AppState>) -> DpsData {
    state.dps_calculator.lock().get_dps()
}

#[tauri::command]
pub(crate) fn get_skill_details(state: tauri::State<'_, AppState>, target_id: i32, actor_ids: Option<Vec<i32>>) -> TargetDetailsResponse {
    state.dps_calculator.lock().get_target_details(target_id, actor_ids.as_deref())
}

/// Skill details behind a meter row, whatever the mode shows.
#[tauri::command]
pub(crate) fn get_displayed_skill_details(state: tauri::State<'_, AppState>, actor_ids: Option<Vec<i32>>) -> TargetDetailsResponse {
    state.dps_calculator.lock().get_displayed_details(actor_ids.as_deref())
}

#[tauri::command]
pub(crate) fn get_details_context(state: tauri::State<'_, AppState>) -> DetailsContext {
    state.dps_calculator.lock().get_details_context()
}

#[tauri::command]
pub(crate) fn get_ping(state: tauri::State<'_, AppState>) -> Option<i32> {
    state.ping_tracker.current_ping_ms()
}

#[tauri::command]
pub(crate) fn set_target_mode(state: tauri::State<'_, AppState>, mode: String) {
    state.dps_calculator.lock().set_target_selection_mode(&mode);
}

/// ALL mode's "last N minutes" window in ms; 0 = off.
#[tauri::command]
pub(crate) fn set_all_targets_window_ms(state: tauri::State<'_, AppState>, ms: i64) {
    state.dps_calculator.lock().set_all_targets_window_ms(ms);
}

#[tauri::command]
pub(crate) fn reset_combat(state: tauri::State<'_, AppState>) {
    state.dps_calculator.lock().restart_target_selection(true);
    // Don't reset port detector or ping — keep the network connection alive
    // Only clear combat data and re-learn nicknames from future packets
    state.data_storage.reset_nicknames();
    state.data_storage.hide_party_placeholders();
}
