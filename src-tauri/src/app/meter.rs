//! What the live meter's commands do beyond reading it.

use super::AppState;

pub(crate) fn reset_combat(state: &AppState) {
    state.dps_calculator.lock().restart_target_selection(true);
    // Don't reset port detector or ping — keep the network connection alive
    // Only clear combat data and re-learn nicknames from future packets
    state.data_storage.reset_nicknames();
    state.data_storage.hide_party_placeholders();
}
