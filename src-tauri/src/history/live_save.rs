//! Saving the live meter's boss fights, the same in every front end: the
//! auto-save writes each fight with the packets behind it every `EVERY`, and
//! quitting writes every fight once more as it stands.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use parking_lot::Mutex;

use crate::combat::data_storage::DataStorage;
use crate::combat::dps_calculator::DpsCalculator;
use crate::entity::fight_record::FightRecord;
use crate::share;

use super::fight_history::FightHistoryManager;

/// How often the auto-save runs.
pub const EVERY: Duration = Duration::from_secs(30);

/// The fights one auto-save wrote.
pub struct Saved {
    pub records: Vec<FightRecord>,
    /// The fights that are over and saved for good: safe to upload.
    pub finished: HashSet<String>,
}

/// One auto-save: writes each boss fight the meter has, with its slice, and
/// marks the ones still going as in progress. `None` before any damage, or
/// when the meter is busy: the next run catches up, and waiting here could
/// stall the caller.
pub fn auto_save(
    app_data_dir: &Path,
    history: &FightHistoryManager,
    storage: &DataStorage,
    meter: &Mutex<DpsCalculator>,
) -> Option<Saved> {
    if storage.damage_generation() == 0 {
        return None;
    }
    // Ticket and snapshot under one hold, so a quit save taken later always
    // carries the later ticket.
    let (ticket, records, finished) = {
        let mut calc = meter.try_lock()?;
        let ticket = history.snapshot_ticket();
        let records = calc.close_boss_fights();
        let finished: HashSet<String> =
            records.iter().filter(|r| calc.fight_finished(r)).map(|r| r.id.clone()).collect();
        (ticket, records, finished)
    };

    for record in &records {
        let _ = history.save_snapshot(record, ticket);
        if let Err(e) = share::store_slice(app_data_dir, record, storage) {
            tracing::debug!("No slice for {}: {}", record.id, e);
        }
    }
    if !records.is_empty() {
        share::trim_old_slices(app_data_dir);
    }
    let going = records.iter().filter(|r| !finished.contains(&r.id)).map(|r| r.id.clone()).collect();
    history.set_in_progress(going);
    Some(Saved { records, finished })
}

/// Save every fight worth keeping before the meter closes, in progress or
/// not. Only the history record: a slice or an upload would hold up the exit.
pub fn save_before_quit(history: &FightHistoryManager, meter: &Mutex<DpsCalculator>) {
    history.set_in_progress(HashSet::new());
    let (ticket, records) = {
        let mut calc = meter.lock();
        (history.snapshot_ticket(), calc.close_boss_fights_now())
    };
    for record in &records {
        if let Err(e) = history.save_snapshot(record, ticket) {
            tracing::warn!("Failed to save {} on exit: {}", record.id, e);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::built::Scratch;
    use crate::capture::ping_tracker::PingTracker;
    use crate::entity::damage_packet::ParsedDamagePacket;
    use crate::i18n::lookup::{NpcLookup, SkillLookup};

    const YOU: i32 = 2259;
    const BOSS: i32 = 800;

    /// A meter with you and one boss (NPC 700).
    fn meter() -> (Arc<DataStorage>, Mutex<DpsCalculator>) {
        let npcs = Arc::new(NpcLookup::new());
        npcs.load_from_json(r#"{"700":{"name":"Boss","isBoss":true}}"#);
        let storage = Arc::new(DataStorage::new());
        storage.set_local_player_id(Some(YOU.into()));
        storage.append_mob(BOSS, 700);
        storage.register_boss(BOSS);
        let skills = Arc::new(SkillLookup::new());
        let calc = DpsCalculator::new(Arc::clone(&storage), skills, npcs, Arc::new(PingTracker::new()));
        (storage, Mutex::new(calc))
    }

    /// Your hits of 500 on the boss, one a second from `from` to `to` ms.
    fn hits(storage: &DataStorage, from: i64, to: i64) {
        for second in 0..=(to - from) / 1_000 {
            let at = from + second * 1_000;
            let mut packet = ParsedDamagePacket::new();
            packet.set_target_id(BOSS);
            packet.set_skill_code(11010000);
            crate::clock::set_override(Some(at));
            packet.set_timestamp(at);
            packet.set_damage(500);
            packet.set_actor_id(YOU);
            storage.append_damage(packet);
        }
    }

    #[test]
    fn the_auto_save_waits_for_damage_and_a_free_meter() {
        let folder = Scratch::new("autosave.wait");
        let history = FightHistoryManager::new(folder.0.clone());
        let (storage, meter) = meter();
        assert!(auto_save(&folder.0, &history, &storage, &meter).is_none(), "no damage yet");
        hits(&storage, 1_000, 10_000);
        let busy = meter.lock();
        assert!(auto_save(&folder.0, &history, &storage, &meter).is_none(), "the meter is busy");
        drop(busy);
        assert!(history.fight_summaries().is_empty());
        crate::clock::set_override(None);
    }

    #[test]
    fn a_fight_is_saved_while_it_goes_on_and_again_on_quitting() {
        let folder = Scratch::new("autosave.quit");
        let history = FightHistoryManager::new(folder.0.clone());
        let (storage, meter) = meter();
        hits(&storage, 1_000, 10_000);
        crate::clock::set_override(Some(22_000));
        let saved = auto_save(&folder.0, &history, &storage, &meter).expect("saved");
        assert_eq!(saved.records.len(), 1);
        let id = saved.records[0].id.clone();
        assert!(saved.finished.is_empty(), "a quiet boss may be in a phase");
        assert_eq!(history.load_fight(&id).unwrap().total_damage, 5_000);
        assert!(history.is_in_progress(&id) && history.fight_summaries()[0].is_live, "History marks it In progress");

        hits(&storage, 30_000, 34_000);
        crate::clock::set_override(Some(35_000));
        save_before_quit(&history, &meter);
        assert_eq!(history.load_fight(&id).unwrap().total_damage, 7_500);
        assert!(!history.fight_summaries()[0].is_live, "nothing is in progress once the meter quits");
        crate::clock::set_override(None);
    }

    #[test]
    fn a_fight_that_is_over_is_finished() {
        let folder = Scratch::new("autosave.over");
        let history = FightHistoryManager::new(folder.0.clone());
        let (storage, meter) = meter();
        hits(&storage, 1_000, 10_000);
        crate::clock::set_override(Some(95_000));
        let saved = auto_save(&folder.0, &history, &storage, &meter).expect("saved");
        assert_eq!(saved.finished, HashSet::from([saved.records[0].id.clone()]));
        assert!(!history.is_in_progress(&saved.records[0].id));
        crate::clock::set_override(Some(125_000));
        let again = auto_save(&folder.0, &history, &storage, &meter).expect("ran");
        assert!(again.records.is_empty(), "saved once");
        crate::clock::set_override(None);
    }

    #[test]
    fn save_and_clear_saves_the_fight_as_finished_and_clears_the_meter() {
        let folder = Scratch::new("autosave.clear");
        let history = FightHistoryManager::new(folder.0.clone());
        let (storage, meter) = meter();
        hits(&storage, 1_000, 30_000);
        crate::clock::set_override(Some(31_000));
        let first = auto_save(&folder.0, &history, &storage, &meter).expect("saved");
        let id = first.records[0].id.clone();
        assert!(history.is_in_progress(&id));

        // A moment later, still fighting.
        hits(&storage, 31_000, 33_000);
        meter.lock().reset_combat();
        let saved = auto_save(&folder.0, &history, &storage, &meter).expect("saved");
        assert_eq!(saved.records.len(), 1);
        assert_eq!(saved.records[0].id, id, "the same fight, not a new one");
        assert_eq!(saved.finished, HashSet::from([id.clone()]));
        assert_eq!(history.load_fight(&id).unwrap().total_damage, 16_500, "every hit up to the clear");
        assert!(!history.is_in_progress(&id), "no longer In progress");
        assert!(storage.copy_targets_brief().is_empty(), "the meter starts afresh");
        crate::clock::set_override(None);
    }
}
