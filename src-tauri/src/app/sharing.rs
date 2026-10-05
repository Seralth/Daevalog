//! Uploads and the share preview, for the share commands.

use crate::share;

use super::AppState;

pub(crate) async fn upload_fight(state: &AppState, fight_id: String) -> Result<share::UploadResult, String> {
    let record = state.fight_history.load_fight(&fight_id)?;
    share::upload(&state.http, &state.app_data_dir, &record).await
}
