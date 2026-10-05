//! Uploads and the share preview, for the share commands.

use crate::share;

use super::AppState;

pub(crate) async fn upload_fight(state: &AppState, fight_id: String) -> Result<share::UploadResult, String> {
    let record = state.fight_history.load_fight(&fight_id)?;
    share::upload(&state.http, &state.app_data_dir, &record).await
}

pub(crate) async fn share_status(state: &AppState) -> Result<std::collections::HashMap<String, share::ShareStatus>, String> {
    let dir = state.app_data_dir.clone();
    tokio::task::spawn_blocking(move || share::share_status(&dir))
        .await
        .map_err(|e| e.to_string())
}

pub(crate) async fn preview_share(state: &AppState, fight_id: String) -> Result<share::PreviewResult, String> {
    let record = state.fight_history.load_fight(&fight_id)?;
    let app_data_dir = state.app_data_dir.clone();
    tokio::task::spawn_blocking(move || {
        let captures = share::find_captures(&app_data_dir);
        let out_dir = app_data_dir.join("share-preview");
        share::preview(&record, &captures, &out_dir)
    })
    .await
    .map_err(|e| format!("preview task failed: {e}"))?
}
