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

// Details read from a reader built from storage, never through the meter's
// mutex, so they neither wait on the tick nor hold it up.

/// Details waits for a place: the Details view asks once per target and adds
/// the answers up, so a refused target would leave the total short.
/// `summary_only` leaves out hit timelines, healing and ping (the hover tooltip).
pub(crate) async fn skill_details(app: tauri::AppHandle, target_id: i32, actor_ids: Option<Vec<i32>>, summary_only: bool) -> Result<TargetDetailsResponse, String> {
    CALCULATIONS.run_waiting(move || {
        let reader = app.state::<AppState>().details.reader();
        if summary_only {
            reader.get_hover_details(target_id, actor_ids.as_deref())
        } else {
            reader.get_target_details(target_id, actor_ids.as_deref())
        }
    }).await
}

pub(crate) async fn displayed_skill_details(app: tauri::AppHandle, actor_ids: Option<Vec<i32>>, summary_only: bool) -> Result<TargetDetailsResponse, String> {
    CALCULATIONS.run_waiting(move || {
        let reader = app.state::<AppState>().details.reader();
        if summary_only {
            reader.get_displayed_hover_details(actor_ids.as_deref())
        } else {
            reader.get_displayed_details(actor_ids.as_deref())
        }
    }).await
}

/// Polled; a refused call keeps the page's last context.
pub(crate) async fn details_context(app: tauri::AppHandle) -> Result<DetailsContext, String> {
    CALCULATIONS.run(move || app.state::<AppState>().details.reader().get_details_context()).await
}

pub(crate) fn reset_combat(state: &AppState) {
    state.dps_calculator.lock().restart_target_selection(true);
    // Don't reset port detector or ping — keep the network connection alive
    // Only clear combat data and re-learn nicknames from future packets
    state.data_storage.reset_nicknames();
    state.data_storage.hide_party_placeholders();
}
