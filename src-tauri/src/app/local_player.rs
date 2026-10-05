//! Binding the local player's id and name from what the windows send.

use crate::combat::data_storage::DataStorage;
use crate::combat::dps_calculator::PARTY_ROW_ID_BASE;

use super::AppState;

/// A party member's row before their entity id is known. Never an entity.
fn is_placeholder_id(actor_id: i64) -> bool {
    actor_id >= PARTY_ROW_ID_BASE as i64
}

/// Another id the game itself named as the local player, which an automatic
/// bind from the UI must not move off. Every window echoes the local id it
/// last saw, and one holding a snapshot from before a zone change sent the
/// old id back 23 s after the zone change: the name left the live entity and
/// a whole boss fight showed no damage for the player (2026-10-04).
fn game_named_local_id(ds: &DataStorage, actor_id: i64) -> Option<i64> {
    if !ds.local_identity_from_self_record() {
        return None;
    }
    let current = ds.local_player_id().filter(|&id| id != actor_id)?;
    let name = ds.local_character_name()?;
    let name = name.trim();
    let named = ds.get_nickname(current as i32);
    (!name.is_empty() && named.as_deref().map(str::trim) == Some(name)).then_some(current)
}

pub(super) fn bind_local_actor(ds: &DataStorage, actor_id: i64, manual: bool, view: &str) {
    if actor_id <= 0 {
        // Clear manual binding — auto-detection will take over
        tracing::info!("bind_local_actor_id: cleared (from the {} window)", view);
        ds.set_local_player_id(None);
        return;
    }
    // The UI bound a party placeholder at startup, which then kept your name
    // for good.
    if is_placeholder_id(actor_id) {
        tracing::info!("bind_local_actor_id: ignored party placeholder {} from the {} window", actor_id, view);
        return;
    }
    if !manual {
        if let Some(current) = game_named_local_id(ds, actor_id) {
            tracing::info!("bind_local_actor_id: ignored {} from the {} window, the game named you {}", actor_id, view, current);
            return;
        }
    }
    let already_bound = ds.local_player_id() == Some(actor_id);
    if !already_bound {
        tracing::info!("bind_local_actor_id: {} (from the {} window)", actor_id, view);
        ds.set_local_player_id(Some(actor_id));
    }
    // Always (re)apply the name if we have a character name, even when the
    // actor_id was already bound — this handles the case where the character
    // name was set AFTER the actor_id binding. Only an id the player chose
    // keeps it for good.
    if let Some(name) = ds.local_character_name() {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            let current = ds.get_nickname(actor_id as i32);
            if manual {
                ds.set_permanent_nickname(actor_id as i32, trimmed);
            } else if current.as_deref() != Some(trimmed) {
                ds.set_local_nickname(actor_id as i32, trimmed);
            }
        }
    }
}

pub(super) fn bind_local_name(ds: &DataStorage, actor_id: i64, nickname: &str, manual: bool, view: &str) {
    if actor_id <= 0 || nickname.trim().is_empty() {
        return;
    }
    if is_placeholder_id(actor_id) {
        tracing::info!("bind_local_nickname: ignored party placeholder {} from the {} window", actor_id, view);
        return;
    }
    if !manual {
        if let Some(current) = game_named_local_id(ds, actor_id) {
            tracing::info!(
                "bind_local_nickname: ignored {} from the {} window, the game named you {}",
                actor_id,
                view,
                current
            );
            return;
        }
    }
    // Always update if the stored nickname differs from the requested one.
    // Previously we skipped if the actor had ANY nickname, which left stale
    // false-positive scan results stuck in place.
    let current = ds.get_nickname(actor_id as i32);
    if ds.local_player_id() == Some(actor_id) && current.as_deref() == Some(nickname) && !manual {
        return;
    }
    // Same as set_character_name: the game's name for the local player wins.
    if ds.local_identity_from_self_record() && ds.local_character_name().as_deref() != Some(nickname.trim()) {
        return;
    }
    tracing::info!("bind_local_nickname: {} -> '{}' (was {:?}, from the {} window)", actor_id, nickname, current, view);
    ds.set_local_player_id(Some(actor_id));
    if manual {
        ds.set_permanent_nickname(actor_id as i32, nickname);
    } else {
        ds.set_local_nickname(actor_id as i32, nickname);
    }
}

pub(crate) fn set_character_name(state: &AppState, name: String, manual: Option<bool>) {
    // The game has said who is playing; a name from the window title or the
    // last session is at best the same and at worst another character. A name
    // the player typed (`manual`) is taken anyway: it is their call, and the
    // game's next self record replaces it if it was wrong.
    let ds = &state.data_storage;
    if ds.local_identity_from_self_record() && !manual.unwrap_or(false) {
        return;
    }
    let trimmed = name.trim().to_string();
    ds.set_local_character_name(Some(name));
    // If an actor ID was already bound, put the name on it now so the meter
    // updates. Not permanent: the name follows the local id, not this one.
    if !trimmed.is_empty() {
        if let Some(id) = ds.local_player_id().filter(|&id| !is_placeholder_id(id)) {
            ds.set_local_nickname(id as i32, &trimmed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_party_placeholder_is_never_bound_as_you() {
        let ds = DataStorage::new();
        ds.set_local_character_name(Some("Seralth".into()));
        bind_local_actor(&ds, 90_000_001, true, "settings");
        bind_local_name(&ds, 90_000_001, "Seralth", true, "settings");
        assert_eq!(ds.local_player_id(), None);
        assert_eq!(ds.get_nickname(90_000_001), None);
    }

    #[test]
    fn an_automatic_bind_does_not_keep_your_name_on_an_old_id() {
        let ds = DataStorage::new();
        ds.set_local_character_name(Some("Seralth".into()));
        bind_local_actor(&ds, 6925, false, "main");
        assert_eq!(ds.get_nickname(6925).as_deref(), Some("Seralth"));
        // The next zone: the game names you on a new id.
        ds.append_nickname_authoritative(7577, "Seralth");
        ds.set_local_identity_from_game(7577, Some("Seralth".into()));
        ds.reset_nicknames();
        assert_eq!(ds.get_nickname(6925), None);
        assert_eq!(ds.find_id_by_nickname("Seralth"), Some(7577));
    }

    #[test]
    fn a_window_cannot_move_you_off_the_id_the_game_named() {
        let ds = DataStorage::new();
        ds.append_nickname_authoritative(7577, "Seralth");
        ds.set_local_identity_from_game(7577, Some("Seralth".into()));
        bind_local_actor(&ds, 6925, false, "details");
        bind_local_name(&ds, 6925, "Seralth", false, "details");
        assert_eq!(ds.local_player_id(), Some(7577));
        assert_eq!(ds.get_nickname(6925), None);
    }

    #[test]
    fn a_cleared_id_unbinds() {
        let ds = DataStorage::new();
        bind_local_actor(&ds, 4321, true, "settings");
        assert_eq!(ds.local_player_id(), Some(4321));
        bind_local_actor(&ds, 0, true, "settings");
        assert_eq!(ds.local_player_id(), None);
    }
}
