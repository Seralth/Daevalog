//! Listing, loading and deleting saved fights, for the history commands.
//!
//! Each runs in the history queue, off the main thread. The list is polled
//! and may be refused when the queue is full; the player's own actions wait.

use tauri::Manager;

use crate::blocking::HISTORY;
use crate::entity::fight_record::{FightRecord, FightSummary};
use crate::share;

use super::AppState;

pub(crate) async fn list_fights(app: tauri::AppHandle) -> Result<Vec<FightSummary>, String> {
    HISTORY.run(move || app.state::<AppState>().fight_history.list_fights()).await
}

pub(crate) async fn load_fight(app: tauri::AppHandle, id: String) -> Result<FightRecord, String> {
    HISTORY.run_waiting(move || {
        app.state::<AppState>().fight_history.load_fight(&id)
    }).await?
}

pub(crate) async fn delete_fight(app: tauri::AppHandle, id: String) -> Result<(), String> {
    if !crate::history::fight_history::is_plain_name(&id) {
        return Err(format!("Invalid fight id: {id:?}"));
    }
    HISTORY.run_waiting(move || {
        let state = app.state::<AppState>();
        share::forget_slice(&state.app_data_dir, &id);
        state.fight_history.delete_fight(&id)
    }).await?
}
