//! Loading and deleting saved fights, for the history commands.

use crate::entity::fight_record::FightRecord;
use crate::share;

use super::AppState;

pub(crate) fn load_fight(state: &AppState, id: String) -> Result<FightRecord, String> {
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
}

pub(crate) fn delete_fight(state: &AppState, id: String) -> Result<(), String> {
    if !crate::history::fight_history::is_plain_name(&id) {
        return Err(format!("Invalid fight id: {id:?}"));
    }
    share::forget_slice(&state.app_data_dir, &id);
    state.fight_history.delete_fight(&id)
}
