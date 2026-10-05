//! What the live meter's commands do beyond reading it.

use tauri::Manager;

use crate::blocking::{CALCULATIONS, DPS_TICK};
use crate::entity::details_context::{DetailsContext, TargetDetailsResponse};
use crate::entity::dps_data::DpsData;

use super::AppState;

/// The meter's numbers now. The same work as the tick, so it waits in the
/// tick's queue rather than being refused.
pub(crate) async fn dps_snapshot(app: tauri::AppHandle) -> Result<DpsData, String> {
    DPS_TICK.run_waiting(move || app.state::<AppState>().dps_calculator.lock().get_dps()).await
}

/// Details waits for a place: the Details view asks once per target and adds
/// the answers up, so a refused target would leave the total short.
pub(crate) async fn skill_details(app: tauri::AppHandle, target_id: i32, actor_ids: Option<Vec<i32>>) -> Result<TargetDetailsResponse, String> {
    CALCULATIONS.run_waiting(move || {
        app.state::<AppState>().dps_calculator.lock().get_target_details(target_id, actor_ids.as_deref())
    }).await
}

pub(crate) async fn displayed_skill_details(app: tauri::AppHandle, actor_ids: Option<Vec<i32>>) -> Result<TargetDetailsResponse, String> {
    CALCULATIONS.run_waiting(move || {
        app.state::<AppState>().dps_calculator.lock().get_displayed_details(actor_ids.as_deref())
    }).await
}

/// Polled; a refused call keeps the page's last context.
pub(crate) async fn details_context(app: tauri::AppHandle) -> Result<DetailsContext, String> {
    CALCULATIONS.run(move || app.state::<AppState>().dps_calculator.lock().get_details_context()).await
}

pub(crate) fn reset_combat(state: &AppState) {
    state.dps_calculator.lock().restart_target_selection(true);
    // Don't reset port detector or ping — keep the network connection alive
    // Only clear combat data and re-learn nicknames from future packets
    state.data_storage.reset_nicknames();
    state.data_storage.hide_party_placeholders();
}
