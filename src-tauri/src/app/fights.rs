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
        let state = app.state::<AppState>();
        let mut record = state.fight_history.load_fight(&id)?;

        // Re-resolve supporter status against the roster as it is *now*, rather
        // than trusting the flag written when the fight was saved. Supporter status
        // changes; a fight from last month opened today should show who is a
        // supporter today, and every record saved before this feature existed has
        // no flag at all.
        //
        // Party members are the honest limitation here. `obscure_nickname` masks
        // their names before the record is written, so a name-keyed roster can only
        // ever match the local player, whose name is stored intact. `dbid` is kept
        // on each actor precisely so a dbid-keyed roster resolves everyone — see
        // `crate::supporters::KeyKind`.
        crate::supporters::apply_to_record(&mut record, &state.data_storage.supporters());
        Ok(record)
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
