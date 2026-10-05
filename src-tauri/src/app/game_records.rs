//! The checks against the game's own Damage Analyzer records, for the share commands.

use super::AppState;

/// What the checks of the game's own Damage Analyzer records need.
fn game_record_checker(state: &AppState) -> crate::game_record::files::Checker {
    crate::game_record::files::Checker {
        app_data_dir: state.app_data_dir.clone(),
        data_dir: state.i18n_data_dir.clone(),
        skills: state.skill_lookup.clone(),
        npcs: state.npc_lookup.clone(),
        fights: state.fight_history.list_fights(),
        roots: crate::game_record::files::record_roots(),
        zone: None,
    }
}

pub(crate) async fn game_record_status(state: &AppState) -> Result<std::collections::HashMap<String, crate::game_record::files::FightStatus>, String> {
    let checker = game_record_checker(&state);
    tokio::task::spawn_blocking(move || checker.statuses())
        .await
        .map_err(|e| e.to_string())
}

pub(crate) async fn game_record_details(state: &AppState, fight_id: String) -> Result<Vec<crate::game_record::files::RecordView>, String> {
    let checker = game_record_checker(&state);
    tokio::task::spawn_blocking(move || checker.views(&fight_id))
        .await
        .map_err(|e| e.to_string())
}
