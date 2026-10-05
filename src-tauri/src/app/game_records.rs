//! The checks against the game's own Damage Analyzer records, for the share commands.

use tauri::Manager;

use crate::blocking::HISTORY;

use super::AppState;

/// What the checks of the game's own Damage Analyzer records need. The list
/// of saved fights is read in the history queue.
async fn game_record_checker(app: &tauri::AppHandle) -> Result<crate::game_record::files::Checker, String> {
    let listing = app.clone();
    let fights = HISTORY.run_waiting(move || listing.state::<AppState>().fight_history.list_fights()).await?;
    let state = app.state::<AppState>();
    Ok(crate::game_record::files::Checker {
        app_data_dir: state.app_data_dir.clone(),
        data_dir: state.i18n_data_dir.clone(),
        skills: state.skill_lookup.clone(),
        npcs: state.npc_lookup.clone(),
        fights,
        roots: crate::game_record::files::record_roots(),
        zone: None,
    })
}

pub(crate) async fn game_record_status(app: tauri::AppHandle) -> Result<std::collections::HashMap<String, crate::game_record::files::FightStatus>, String> {
    let checker = game_record_checker(&app).await?;
    tokio::task::spawn_blocking(move || checker.statuses())
        .await
        .map_err(|e| e.to_string())
}

pub(crate) async fn game_record_details(app: tauri::AppHandle, fight_id: String) -> Result<Vec<crate::game_record::files::RecordView>, String> {
    let checker = game_record_checker(&app).await?;
    tokio::task::spawn_blocking(move || checker.views(&fight_id))
        .await
        .map_err(|e| e.to_string())
}
