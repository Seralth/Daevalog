//! Saved fights: the list, loading and deleting.

use crate::entity::fight_record::{FightRecord, FightSummary};

use crate::app::fights;

#[tauri::command]
/// `async` keeps this off the main thread. Sync commands run there, and even
/// with the summary cache a cold call reads every fight file — measured at
/// ~350ms, during which no other IPC and no window painting can proceed. Each
/// window calls this at startup and again every 10s, which is what made opening
/// History feel like it hung.
pub(crate) async fn get_fight_history(app: tauri::AppHandle) -> Result<Vec<FightSummary>, String> {
    fights::list_fights(app).await
}

#[tauri::command]
pub(crate) async fn load_fight(app: tauri::AppHandle, id: String) -> Result<FightRecord, String> {
    fights::load_fight(app, id).await
}

#[tauri::command]
pub(crate) async fn delete_fight(app: tauri::AppHandle, id: String) -> Result<(), String> {
    fights::delete_fight(app, id).await
}
