//! Uploads and the share preview, for the share commands.

use tauri::Manager;

use crate::blocking::HISTORY;
use crate::entity::fight_record::FightRecord;
use crate::share;

use super::AppState;

/// A saved fight, read in the history queue.
async fn load_fight(app: &tauri::AppHandle, fight_id: String) -> Result<FightRecord, String> {
    let app = app.clone();
    HISTORY.run_waiting(move || app.state::<AppState>().fight_history.load_fight(&fight_id)).await?
}

pub(crate) async fn upload_fight(app: tauri::AppHandle, fight_id: String) -> Result<share::UploadResult, String> {
    let record = load_fight(&app, fight_id).await?;
    let state = app.state::<AppState>();
    share::upload(&state.http, &state.app_data_dir, &state.settings, &record).await
}

pub(crate) async fn share_status(state: &AppState) -> Result<std::collections::HashMap<String, share::ShareStatus>, String> {
    let dir = state.app_data_dir.clone();
    tokio::task::spawn_blocking(move || share::share_status(&dir))
        .await
        .map_err(|e| e.to_string())
}

pub(crate) async fn preview_share(app: tauri::AppHandle, fight_id: String) -> Result<share::PreviewResult, String> {
    let record = load_fight(&app, fight_id).await?;
    let app_data_dir = app.state::<AppState>().app_data_dir.clone();
    tokio::task::spawn_blocking(move || {
        let captures = share::find_captures(&app_data_dir);
        let out_dir = app_data_dir.join("share-preview");
        share::preview(&record, &captures, &out_dir)
    })
    .await
    .map_err(|e| format!("preview task failed: {e}"))?
}
