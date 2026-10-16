//! What a changed setting does in the backend besides being stored, for
//! every front end.

use crate::combat::data_storage::DataStorage;
use crate::combat::dps_calculator::is_party_row_id;

pub const ENCOUNTER_TIMEOUT_KEY: &str = "dpsMeter.encounterTimeoutSec";
pub const CHARACTER_NAME_KEY: &str = "dpsMeter.userName";

/// The encounter timeout setting, in whole seconds; anything else keeps the
/// default.
pub fn apply_encounter_timeout(storage: &DataStorage, value: Option<&str>) {
    storage.set_encounter_timeout_ms(encounter_timeout_ms(value));
}

fn encounter_timeout_ms(value: Option<&str>) -> i64 {
    value
        .and_then(|v| v.trim().parse::<i64>().ok())
        .and_then(|s| s.checked_mul(1000))
        .unwrap_or(crate::combat::data_storage::DEFAULT_ENCOUNTER_TIMEOUT_MS)
}

/// A character name from the page or a saved setting. The game's own self
/// record outranks a remembered name, but not one the player just typed.
pub fn apply_character_name(storage: &DataStorage, name: &str, manual: bool) {
    if !manual && storage.local_identity_from_self_record() {
        return;
    }
    storage.set_local_character_name(Some(name.to_string()));
    let label = name.trim();
    if label.is_empty() {
        return;
    }
    // Party rows have ids of their own; a nickname there would rename a stranger.
    match storage.local_player_id() {
        Some(id) if !is_party_row_id(id) => storage.set_local_nickname(id as i32, label),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remembered_name_gives_way_to_the_games_but_a_typed_one_does_not() {
        let storage = DataStorage::new();
        storage.set_local_identity_from_game(7577, Some("Corvane".into()));
        apply_character_name(&storage, "Other", false);
        assert_eq!(storage.local_character_name().as_deref(), Some("Corvane"));
        apply_character_name(&storage, "Typed", true);
        assert_eq!(storage.local_character_name().as_deref(), Some("Typed"));
    }

    #[test]
    fn the_name_goes_on_your_entity_but_never_on_a_party_row() {
        let storage = DataStorage::new();
        storage.set_local_player_id(Some(6925));
        apply_character_name(&storage, " Corvane ", false);
        assert_eq!(storage.stored_name(6925).as_deref(), Some("Corvane"));
        storage.set_local_player_id(Some(90_000_001));
        apply_character_name(&storage, "Corvane", false);
        assert_eq!(storage.stored_name(90_000_001), None);
    }

    #[test]
    fn a_blank_name_is_kept_but_leaves_the_nickname_alone() {
        let storage = DataStorage::new();
        storage.set_local_player_id(Some(6925));
        apply_character_name(&storage, "  ", false);
        assert_eq!(storage.local_character_name().as_deref(), Some("  "));
        assert_eq!(storage.stored_name(6925), None);
    }

    #[test]
    fn an_encounter_timeout_too_big_to_be_milliseconds_keeps_the_default() {
        let default = crate::combat::data_storage::DEFAULT_ENCOUNTER_TIMEOUT_MS;
        assert_eq!(encounter_timeout_ms(Some("9223372036854775807")), default);
        assert_eq!(encounter_timeout_ms(Some("x")), default);
        assert_eq!(encounter_timeout_ms(Some(" 60 ")), 60_000);
    }
}
