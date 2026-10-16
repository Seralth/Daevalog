//! What the tests of target choice, rows, readings and saved fights share.

use std::sync::Arc;

use crate::capture::ping_tracker::PingTracker;
use crate::clock::{now_ms, set_override};
use crate::combat::data_storage::{record, DataStorage, TakenSums};
use crate::entity::damage_packet::ParsedDamagePacket;
use crate::entity::dps_data::DpsData;
use crate::i18n::lookup::{NpcLookup, SkillLookup};

use super::meter_rows::Tallied;
use super::DpsCalculator;

pub(super) const GLADIATOR_SKILL: i32 = 11_010_000;
pub(super) const SORCERER_SKILL: i32 = 15_010_000;
/// In the Gladiator range, but names no class.
pub(super) const CLASSLESS_SKILL: i32 = 11_000_100;

/// Storage that knows you as `me`, when given.
pub(super) fn storage_for(me: Option<i64>) -> Arc<DataStorage> {
    let storage = Arc::new(DataStorage::new());
    if me.is_some() {
        storage.set_local_player_id(me);
    }
    storage
}

/// A calculator on `storage` with the NPC table `npcs` (JSON).
pub(super) fn meter_on(storage: &Arc<DataStorage>, npcs: &str) -> DpsCalculator {
    let table = NpcLookup::new();
    table.load_from_json(npcs);
    let (skills, ping) = (Arc::new(SkillLookup::new()), Arc::new(PingTracker::new()));
    DpsCalculator::new(Arc::clone(storage), skills, Arc::new(table), ping)
}

/// A calculator on `storage` whose NPC table is empty.
pub(super) fn meter(storage: &Arc<DataStorage>) -> DpsCalculator {
    meter_on(storage, "")
}

/// A hit of `damage` from `actor` on `target` at `at`, not yet sent.
pub(super) fn skill_hit(actor: i32, target: i32, at: i64, skill: i32, damage: i64) -> ParsedDamagePacket {
    record(actor, target, skill, damage, at)
}

/// A plain 500 hit with `GLADIATOR_SKILL`, not yet sent.
pub(super) fn hit(actor: i32, target: i32, at: i64) -> ParsedDamagePacket {
    skill_hit(actor, target, at, GLADIATOR_SKILL, 500)
}

/// One hit of `damage` from `actor` on `target` at `at`; the clock moves there.
pub(super) fn land(storage: &DataStorage, actor: i32, target: i32, at: i64, skill: i32, damage: i64) {
    set_override(Some(at));
    storage.append_damage(skill_hit(actor, target, at, skill, damage));
}

/// The meter in `mode`, read once.
pub(super) fn read_in(calc: &mut DpsCalculator, mode: &str) -> DpsData {
    calc.set_target_selection_mode(mode);
    calc.get_dps()
}

/// The meter's reading, checked against the same reading built afresh with
/// nothing kept from earlier reads, as text, so the two match to the byte.
/// Both reads see the same damage generation, so they build the reading the
/// same way; idle targets are not retired in between.
pub(super) fn read_checked(calc: &mut DpsCalculator) -> DpsData {
    calc.last_retire_ms = now_ms();
    let generation = calc.last_damage_gen;
    let kept = calc.get_dps();
    calc.live_copy.clear();
    calc.tallied = Tallied::default();
    (calc.taken_sums, calc.taken_cache, calc.last_damage_gen) = (TakenSums::default(), None, generation);
    let afresh = calc.get_dps();
    let text = |d: &DpsData| serde_json::to_value(d).unwrap().to_string();
    assert_eq!(text(&kept), text(&afresh), "at {}", now_ms());
    kept
}
