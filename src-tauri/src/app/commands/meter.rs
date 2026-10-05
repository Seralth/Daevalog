//! The live meter: its numbers, the Details behind a row, target modes and reset.

use crate::entity::details_context::{DetailsContext, TargetDetailsResponse};
use crate::entity::dps_data::DpsData;

use crate::app::{meter, AppState};

#[tauri::command]
pub(crate) fn get_app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[tauri::command]
pub(crate) async fn get_dps_snapshot(app: tauri::AppHandle) -> Result<DpsData, String> {
    meter::dps_snapshot(app).await
}

#[tauri::command]
pub(crate) async fn get_skill_details(app: tauri::AppHandle, target_id: i32, actor_ids: Option<Vec<i32>>, summary_only: Option<bool>) -> Result<TargetDetailsResponse, String> {
    meter::skill_details(app, target_id, actor_ids, summary_only.unwrap_or(false)).await
}

/// Skill details behind a meter row, whatever the mode shows.
#[tauri::command]
pub(crate) async fn get_displayed_skill_details(app: tauri::AppHandle, actor_ids: Option<Vec<i32>>, summary_only: Option<bool>) -> Result<TargetDetailsResponse, String> {
    meter::displayed_skill_details(app, actor_ids, summary_only.unwrap_or(false)).await
}

#[tauri::command]
pub(crate) async fn get_details_context(app: tauri::AppHandle) -> Result<DetailsContext, String> {
    meter::details_context(app).await
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
    meter::reset_combat(&state);
}
