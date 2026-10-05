//! Report a problem: the report info, the bug-report copy of a packet log,
//! and the game's Combat Analysis folder.

use tauri_plugin_opener::OpenerExt;

use crate::app::{links, report, AppState};

/// The report info, the packet logs (newest first), and whether the game's
/// Combat Analysis folder was found.
#[tauri::command]
pub(crate) fn report_info(state: tauri::State<'_, AppState>) -> serde_json::Value {
    serde_json::json!({
        "text": report::report_info(),
        "logs": report::packet_logs(&state.app_data_dir),
        "recordFolder": !crate::game_record::files::record_roots().is_empty(),
    })
}

/// Write the bug-report copy of a packet log (the newest when `name` is
/// none), then open the data folder where it is.
#[tauri::command]
pub(crate) async fn prepare_report_log(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    name: Option<String>,
) -> Result<report::PreparedLog, String> {
    let dir = state.app_data_dir.clone();
    let work_dir = dir.clone();
    let prepared = tauri::async_runtime::spawn_blocking(move || report::prepare_log(&work_dir, name.as_deref()))
        .await
        .map_err(|e| e.to_string())??;
    links::open(&app, &dir, &dir.to_string_lossy());
    Ok(prepared)
}

/// The game's Combat Analysis records, which the player may attach too.
#[tauri::command]
pub(crate) fn open_game_record_folder(app: tauri::AppHandle) {
    let Some(root) = crate::game_record::files::record_roots().into_iter().next() else { return };
    if let Err(e) = app.opener().open_path(root.to_string_lossy(), None::<&str>) {
        tracing::warn!("Could not open the Combat Analysis folder: {e}");
    }
}
