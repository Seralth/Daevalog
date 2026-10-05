//! Uploads, the share preview, the game's own records and the logs sent to the developer.

use crate::share;

use crate::app::{game_records, sharing, AppState};

/// Upload a saved fight to a2tools.app as a log, and return its link.
#[tauri::command]
pub(crate) async fn upload_fight(
    state: tauri::State<'_, AppState>,
    fight_id: String,
) -> Result<share::UploadResult, String> {
    sharing::upload_fight(&state, fight_id).await
}

/// Which fights have a slice to upload, and which already have a link.
#[tauri::command]
pub(crate) async fn share_status(
    state: tauri::State<'_, AppState>,
) -> Result<std::collections::HashMap<String, share::ShareStatus>, String> {
    sharing::share_status(&state).await
}

/// Saved fights with a game record, and whether the meter matches it.
/// Async: a new record replays the fight's slice.
#[tauri::command]
pub(crate) async fn game_record_status(
    state: tauri::State<'_, AppState>,
) -> Result<std::collections::HashMap<String, crate::game_record::files::FightStatus>, String> {
    game_records::game_record_status(&state).await
}

/// A saved fight's game records, each beside the meter's numbers.
#[tauri::command]
pub(crate) async fn game_record_details(
    state: tauri::State<'_, AppState>,
    fight_id: String,
) -> Result<Vec<crate::game_record::files::RecordView>, String> {
    game_records::game_record_details(&state, fight_id).await
}

/// Write what sharing this fight *would* upload, without uploading anything.
///
/// The point is auditability: it produces the exact `.a2es` and `.upload.json`
/// an upload would send, so a user can open them — `a2t-inspect` reads the
/// former — instead of taking `docs/PRIVACY.md` on faith. There is no network
/// call anywhere in this path.
///
/// Async because it re-parses a packet capture and replays it to resolve the
/// names the blinder has to remove; on a long capture that is seconds of CPU,
/// and a sync command would hold the main thread (see `get_fight_history`).
#[tauri::command]
pub(crate) async fn preview_share(
    state: tauri::State<'_, AppState>,
    fight_id: String,
) -> Result<share::PreviewResult, String> {
    sharing::preview_share(&state, fight_id).await
}

/// Send the newest packet captures to the developer (Settings, beside packet
/// logging). Returns the report code the player passes on.
#[tauri::command]
pub(crate) async fn send_logs_to_dev(
    state: tauri::State<'_, AppState>,
) -> Result<share::dev_logs::SendResult, String> {
    share::dev_logs::send(&state.http, &state.app_data_dir).await
}
