use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::combat::data_storage::DataStorage;
use crate::combat::ping_tracker::PingTracker;
use crate::entity::dps_data::DpsData;
use crate::i18n::lookup::{NpcLookup, SkillLookup};

mod details;
mod fights;
mod meter_rows;
mod rows;
mod target;

pub use rows::PARTY_ROW_ID_BASE;
pub use target::TargetSelectionMode;

pub struct DpsCalculator {
    data_storage: Arc<DataStorage>,
    skill_lookup: Arc<SkillLookup>,
    npc_lookup: Arc<NpcLookup>,
    ping_tracker: Arc<PingTracker>,
    current_target: i32,
    last_dps_snapshot: Option<DpsData>,
    last_damage_gen: i64,
    target_selection_mode: TargetSelectionMode,
    last_known_local_id: Option<i64>,
    all_targets_window_ms: i64,
    nickname_job_cache: HashMap<String, String>,
    /// Fights saved for good, as (target, segment start) -> last hit at the
    /// time. A segment that goes on after that is saved again.
    saved_fights: HashMap<(i32, i64), i64>,
    /// The targets behind the rows on screen, and their battle time: what a
    /// row's skill details cover.
    displayed_targets: Vec<i32>,
    displayed_battle_time: i64,
    /// The above, as details readers see them.
    view: Arc<RwLock<DetailsView>>,
    /// When idle targets were last retired. See `retire_idle_targets`.
    last_retire_ms: i64,
}

/// What Details needs of the meter beyond storage: the mode and the targets
/// behind the rows on screen. The meter publishes it after each change.
#[derive(Debug, Clone)]
struct DetailsView {
    mode: TargetSelectionMode,
    displayed_targets: Vec<i32>,
    displayed_battle_time: i64,
}

/// Builds details readers: calculators that share the meter's storage,
/// lookups and view but not its mutex, so Details never waits on the meter.
#[derive(Clone)]
pub struct DetailsSource {
    data_storage: Arc<DataStorage>,
    skill_lookup: Arc<SkillLookup>,
    npc_lookup: Arc<NpcLookup>,
    ping_tracker: Arc<PingTracker>,
    view: Arc<RwLock<DetailsView>>,
}

impl DetailsSource {
    /// A calculator for the Details calls only, as the meter stands now.
    pub fn reader(&self) -> DpsCalculator {
        let mut reader = DpsCalculator::new(self.data_storage.clone(), self.skill_lookup.clone(),
            self.npc_lookup.clone(), self.ping_tracker.clone());
        let view = self.view.read().clone();
        reader.target_selection_mode = view.mode;
        reader.displayed_targets = view.displayed_targets;
        reader.displayed_battle_time = view.displayed_battle_time;
        reader
    }
}

impl DpsCalculator {
    pub fn new(
        data_storage: Arc<DataStorage>,
        skill_lookup: Arc<SkillLookup>,
        npc_lookup: Arc<NpcLookup>,
        ping_tracker: Arc<PingTracker>,
    ) -> Self {
        Self {
            data_storage,
            skill_lookup,
            npc_lookup,
            ping_tracker,
            current_target: 0,
            last_dps_snapshot: None,
            last_damage_gen: -1,
            target_selection_mode: TargetSelectionMode::BossTargets,
            last_known_local_id: None,
            all_targets_window_ms: 0,
            nickname_job_cache: HashMap::new(),
            saved_fights: HashMap::new(),
            displayed_targets: Vec::new(),
            displayed_battle_time: 0,
            view: Arc::new(RwLock::new(DetailsView {
                mode: TargetSelectionMode::BossTargets,
                displayed_targets: Vec::new(),
                displayed_battle_time: 0,
            })),
            last_retire_ms: i64::MIN,
        }
    }

    pub fn details_source(&self) -> DetailsSource {
        DetailsSource {
            data_storage: self.data_storage.clone(),
            skill_lookup: self.skill_lookup.clone(),
            npc_lookup: self.npc_lookup.clone(),
            ping_tracker: self.ping_tracker.clone(),
            view: self.view.clone(),
        }
    }

    pub fn get_dps(&mut self) -> DpsData {
        let dps = self.compute_dps();
        // After the rows: a mode just switched to has picked its targets.
        self.retire_idle_targets();
        self.publish_view();
        dps
    }

    fn publish_view(&self) {
        let mut view = self.view.write();
        view.mode = self.target_selection_mode;
        if view.displayed_targets != self.displayed_targets {
            view.displayed_targets.clone_from(&self.displayed_targets);
        }
        view.displayed_battle_time = self.displayed_battle_time;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use super::fights::fight_dungeon;
    use super::meter_rows::active_time;
    use crate::entity::details_context::TargetDetailsResponse;
    use crate::entity::fight_record::FightRecord;
    use crate::entity::summon_resolver;
    use crate::entity::damage_packet::ParsedDamagePacket;
    use crate::entity::special_damage::SpecialDamage;

    fn hit(actor: i32, target: i32, at: i64) -> ParsedDamagePacket {
        let mut p = ParsedDamagePacket::new();
        p.set_actor_id(actor);
        p.set_target_id(target);
        p.set_skill_code(11010000);
        p.set_damage(500);
        p.set_timestamp(at);
        p
    }

    fn meter(storage: &Arc<DataStorage>) -> DpsCalculator {
        DpsCalculator::new(storage.clone(), Arc::new(SkillLookup::new()),
            Arc::new(NpcLookup::new()), Arc::new(PingTracker::new()))
    }

    const BOSS: i32 = 700;
    const DUMMY: i32 = 701;

    /// A meter whose NPC table knows one boss and one training dummy.
    fn meter_with_npcs(storage: &Arc<DataStorage>) -> DpsCalculator {
        let npcs = NpcLookup::new();
        npcs.load_from_json(r#"{"700":{"name":"Boss","isBoss":true},"701":{"name":"Training Scarecrow"}}"#);
        DpsCalculator::new(storage.clone(), Arc::new(SkillLookup::new()),
            Arc::new(npcs), Arc::new(PingTracker::new()))
    }

    fn spawn(storage: &DataStorage, id: i32, code: i32) {
        storage.append_mob(id, code);
        if code == BOSS {
            storage.register_boss(id);
        } else if code == DUMMY {
            storage.register_training_dummy(id);
        }
    }

    /// One hit a second from `actor` on `target`, `from` to `to` inclusive.
    fn hits(storage: &DataStorage, actor: i32, target: i32, from: i64, to: i64) {
        let mut at = from;
        while at <= to {
            crate::clock::set_override(Some(at));
            storage.append_damage(hit(actor, target, at));
            at += 1_000;
        }
    }

    fn snapshot_at(calc: &mut DpsCalculator, now: i64) -> Vec<FightRecord> {
        crate::clock::set_override(Some(now));
        calc.snapshot_boss_fights()
    }

    fn ids(records: &[FightRecord]) -> Vec<String> {
        let mut ids: Vec<String> = records.iter().map(|r| r.id.clone()).collect();
        ids.sort();
        ids
    }

    #[test]
    fn a_second_run_on_the_same_dummy_is_saved_too() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 36734, DUMMY);
        let mut calc = meter_with_npcs(&s);
        hits(&s, 2259, 36734, 1_000, 11_000);
        let saved = snapshot_at(&mut calc, 50_000);
        assert_eq!(ids(&saved), vec!["auto_36734_1000"]);
        assert!(calc.fight_finished(&saved[0]));
        assert!(snapshot_at(&mut calc, 60_000).is_empty(), "saved once");

        hits(&s, 2259, 36734, 70_000, 80_000);
        let saved = snapshot_at(&mut calc, 120_000);
        assert_eq!(ids(&saved), vec!["auto_36734_70000"]);
        crate::clock::set_override(None);
    }

    #[test]
    fn a_run_cut_off_by_the_idle_restart_is_saved() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 36734, DUMMY);
        let mut calc = meter_with_npcs(&s);
        hits(&s, 2259, 36734, 1_000, 11_000);
        // Back on the same dummy after 34 s, before any auto-save ran.
        hits(&s, 2259, 36734, 45_000, 50_000);
        let saved = snapshot_at(&mut calc, 51_000);
        assert_eq!(ids(&saved), vec!["auto_36734_1000"]);
        assert_eq!(saved[0].duration_ms, 10_000);
        assert!(calc.fight_finished(&saved[0]));
        crate::clock::set_override(None);
    }

    #[test]
    fn a_pause_in_a_boss_fight_does_not_end_it() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        hits(&s, 2259, 800, 1_000, 10_000);
        let saved = snapshot_at(&mut calc, 22_000);
        assert_eq!(ids(&saved), vec!["auto_800_1000"]);
        assert!(!calc.fight_finished(&saved[0]), "12 s of quiet may be a phase");

        hits(&s, 2259, 800, 30_000, 60_000);
        let saved = snapshot_at(&mut calc, 61_000);
        assert_eq!(ids(&saved), vec!["auto_800_1000"], "the same fight, saved again");
        assert_eq!(saved[0].duration_ms, 59_000);
        assert!(!calc.fight_finished(&saved[0]));

        let saved = snapshot_at(&mut calc, 95_000);
        assert_eq!(saved[0].duration_ms, 59_000);
        assert!(calc.fight_finished(&saved[0]));
        assert!(snapshot_at(&mut calc, 125_000).is_empty());
        crate::clock::set_override(None);
    }

    #[test]
    fn a_fight_takes_its_dungeon_from_the_boss_when_the_table_names_it() {
        let npcs = NpcLookup::new();
        npcs.load_from_json(r#"{
            "2310218": {"name": "Divine Auldor", "isBoss": true, "dungeonId": 600011},
            "2310206": {"name": "Guardian Captain Raur", "isBoss": true, "dungeonId": 600011},
            "2300475": {"name": "Gargaum", "isBoss": true},
            "2701090": {"name": "Mutated Bargott", "isBoss": true},
            "2310219": {"name": "Auldor Sanctum Gatekeeper"}
        }"#);
        // After a teleport, before the roster names the instance again.
        assert_eq!(fight_dungeon(&npcs, 2310218, 0), 600011);
        // A stale id gives way to the boss's own instance.
        assert_eq!(fight_dungeon(&npcs, 2310206, 600072), 600011);
        // A boss the table does not place keeps the stamped id.
        assert_eq!(fight_dungeon(&npcs, 2300475, 610073), 610073);
        assert_eq!(fight_dungeon(&npcs, 2701090, 0), 0);
        // Trash keeps the stamped id.
        assert_eq!(fight_dungeon(&npcs, 2310219, 600011), 600011);
    }

    fn dungeon_of(records: &[FightRecord], id: &str) -> i32 {
        records.iter().find(|r| r.id == id).map(|r| r.dungeon_id).expect(id)
    }

    #[test]
    fn a_fight_outside_after_leaving_an_instance_has_no_dungeon() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        spawn(&s, 900, BOSS);
        let mut calc = meter_with_npcs(&s);
        s.set_current_dungeon(600002);
        hits(&s, 2259, 800, 1_000, 8_000);
        crate::clock::set_override(Some(12_000));
        assert!(s.note_zone_change(), "the load out of the instance");
        assert_eq!(s.current_dungeon_id(), 600002, "a load alone does not say where to");
        s.note_map_load(1010);
        assert_eq!(s.current_dungeon_id(), 0, "World_L_A is open world");
        let mut saved = snapshot_at(&mut calc, 12_000);
        hits(&s, 2259, 900, 20_000, 30_000);
        crate::clock::set_override(Some(31_000));
        saved.extend(calc.snapshot_boss_fights_force());
        assert_eq!(dungeon_of(&saved, "auto_800_1000"), 600002);
        assert_eq!(dungeon_of(&saved, "auto_900_20000"), 0);
        crate::clock::set_override(None);
    }

    #[test]
    fn a_fight_before_the_first_roster_still_gets_its_dungeon() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        hits(&s, 2259, 800, 1_000, 3_000);
        s.set_current_dungeon(600002);
        hits(&s, 2259, 800, 4_000, 8_000);
        crate::clock::set_override(Some(9_000));
        assert_eq!(dungeon_of(&calc.snapshot_boss_fights_force(), "auto_800_1000"), 600002);
        crate::clock::set_override(None);
    }

    #[test]
    fn a_teleport_during_a_fight_in_an_instance_keeps_the_dungeon() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        s.set_current_dungeon(600002);
        hits(&s, 2259, 800, 1_000, 5_000);
        crate::clock::set_override(Some(5_500));
        assert!(!s.note_zone_change(), "mid-fight: no combat reset");
        s.note_map_load(600002);
        assert_eq!(s.current_dungeon_id(), 600002, "the load names the instance again");
        hits(&s, 2259, 800, 6_000, 9_000);
        crate::clock::set_override(Some(10_000));
        assert_eq!(dungeon_of(&calc.snapshot_boss_fights_force(), "auto_800_1000"), 600002);
        crate::clock::set_override(None);
    }

    #[test]
    fn a_boss_after_a_teleport_inside_an_instance_keeps_the_dungeon() {
        // taengu's 600011 capture: a teleport to the last boss room, the boss
        // killed before the roster names the instance again.
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        s.set_current_dungeon(600011);
        crate::clock::set_override(Some(1_000));
        s.note_zone_change();
        s.note_map_load(600011);
        hits(&s, 2259, 800, 2_000, 30_000);
        crate::clock::set_override(Some(31_000));
        assert_eq!(dungeon_of(&calc.snapshot_boss_fights_force(), "auto_800_2000"), 600011);
        crate::clock::set_override(None);
    }

    #[test]
    fn world_layers_and_the_abyss_are_open_world_and_seals_are_not() {
        assert!(crate::combat::data_storage::is_open_world_map(1010), "World_L_A");
        assert!(crate::combat::data_storage::is_open_world_map(101021), "a layer of World_L_A");
        assert!(crate::combat::data_storage::is_open_world_map(20), "Chaotic Lower Reshanta");
        assert!(!crate::combat::data_storage::is_open_world_map(310051), "Seal_Verteron_051");
        assert!(!crate::combat::data_storage::is_open_world_map(600021), "Fire_Temple_Easy");
        assert!(!crate::combat::data_storage::is_open_world_map(999_999_999), "unknown map");

        let s = DataStorage::new();
        s.set_current_dungeon(600021);
        s.note_map_load(310051);
        assert_eq!(s.current_dungeon_id(), 310051, "a seal is its own dungeon");
        s.note_map_load(101021);
        assert_eq!(s.current_dungeon_id(), 0, "a world layer ends it");
        s.note_map_load(600021);
        s.set_current_dungeon(600021);
        s.note_map_load(20);
        assert_eq!(s.current_dungeon_id(), 0, "so does the Abyss");
    }

    #[test]
    fn a_fight_in_a_sealed_dungeon_is_filed_under_it() {
        // 2026-10-04 capture: boss 2701096, fought in Seal_Verteron_051 with
        // no party roster, was saved as open world.
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        s.note_map_load(1010);
        s.note_map_load(310051);
        assert_eq!(s.current_dungeon_id(), 310051);
        s.set_current_dungeon(600021);
        assert_eq!(s.current_dungeon_id(), 310051, "the party's queue is not the seal");
        hits(&s, 2259, 800, 1_000, 8_000);
        crate::clock::set_override(Some(12_000));
        s.note_map_load(1010);
        assert_eq!(s.current_dungeon_id(), 0);
        assert!(s.note_zone_change());
        assert_eq!(dungeon_of(&snapshot_at(&mut calc, 12_000), "auto_800_1000"), 310051);
        s.note_map_load(600021);
        assert_eq!(s.current_dungeon_id(), 600021, "the queue, at the load into it");
        crate::clock::set_override(None);
    }

    #[test]
    fn a_queued_dungeon_waits_for_the_load_into_it() {
        // 2026-10-04 capture: the roster named Krao Cave (600002) 46 seconds
        // before the load into it, while the party was still in World_L_A.
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        s.note_map_load(1010);
        s.set_current_dungeon(600002);
        assert_eq!(s.current_dungeon_id(), 0, "queued, still in the open world");
        hits(&s, 2259, 800, 1_000, 8_000);
        s.note_map_load(1010);
        assert_eq!(s.current_dungeon_id(), 0, "a load inside the open world");
        s.note_map_load(600002);
        assert_eq!(s.current_dungeon_id(), 600002, "the load into it");
        crate::clock::set_override(Some(9_000));
        assert_eq!(dungeon_of(&calc.snapshot_boss_fights_force(), "auto_800_1000"), 0);
        crate::clock::set_override(None);
    }

    #[test]
    fn an_open_world_fight_ended_by_the_load_into_an_instance_has_no_dungeon() {
        // A field boss killed as the queue pops: the load into the instance
        // ends the fight, after the instance's id is known.
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        s.note_map_load(1010);
        hits(&s, 2259, 800, 1_000, 8_000);
        s.set_current_dungeon(600002);
        crate::clock::set_override(Some(20_000));
        s.note_map_load(600002);
        assert!(s.note_zone_change(), "the load ends the fight");
        assert_eq!(dungeon_of(&snapshot_at(&mut calc, 20_000), "auto_800_1000"), 0);
        crate::clock::set_override(None);
    }

    #[test]
    fn an_encounter_ends_after_the_timeout_unless_a_live_boss_holds_it() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, 1);
        spawn(&s, 810, 1);
        hits(&s, 2259, 800, 1_000, 5_000);
        hits(&s, 2259, 810, 30_000, 33_000);
        let e = s.current_encounter().unwrap();
        assert_eq!((e.start, e.targets.len()), (30_000, 1), "25 s quiet ends it");

        let mut calc = meter_with_npcs(&s);
        calc.set_target_selection_mode("encounter");
        let shown = calc.get_dps();
        assert_eq!(shown.battle_time, 3_000);
        assert_eq!(shown.map.values().map(|r| r.amount).sum::<f64>(), 4.0 * 500.0);

        // A live boss holds it through a long pause; a dead one does not.
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 900, BOSS);
        spawn(&s, 910, 1);
        hits(&s, 2259, 900, 1_000, 3_000);
        hits(&s, 2259, 910, 40_000, 41_000);
        assert_eq!(s.current_encounter().unwrap().start, 1_000);
        s.mark_entity_dead(900);
        hits(&s, 2259, 910, 80_000, 81_000);
        assert_eq!(s.current_encounter().unwrap().start, 80_000);

        // A quiet span shorter than the timeout keeps one encounter; a custom
        // timeout is honoured.
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        s.set_encounter_timeout_ms(30_000);
        spawn(&s, 800, 1);
        hits(&s, 2259, 800, 1_000, 2_000);
        hits(&s, 2259, 800, 25_000, 26_000);
        assert_eq!(s.current_encounter().unwrap().start, 1_000);
        crate::clock::set_override(None);
    }

    #[test]
    fn enc_shows_nothing_until_the_meter_knows_you_then_only_your_fights() {
        use crate::combat::data_storage::PartyMember;
        let s = Arc::new(DataStorage::new());
        s.append_nickname_authoritative(3000, "A");
        s.set_party_roster(vec![("A".into(), PartyMember { slot: 1, ..Default::default() })], true);
        let mut calc = meter(&s);
        calc.set_target_selection_mode("encounter");
        // Strangers fight nearby from 1 s; you and your party member from 5 s.
        hits(&s, 9000, 900, 1_000, 10_000);
        hits(&s, 2259, 901, 5_000, 12_000);
        hits(&s, 3000, 901, 6_000, 7_000);
        crate::clock::set_override(Some(12_500));
        // Party members stand on the meter as placeholders, with no damage.
        let dealt = |d: &DpsData| { let mut r: Vec<i32> = d.map.iter().filter(|(_, r)| r.amount > 0.0).map(|(&k, _)| k).collect(); r.sort(); r };
        let shown = calc.get_dps();
        assert!(dealt(&shown).is_empty(), "nobody's fights before the meter knows you");
        assert_eq!(shown.battle_time, 0);

        s.set_local_player_id(Some(2259));
        assert!(dealt(&calc.get_dps()).is_empty(), "nor until your next hit sorts them out");
        hits(&s, 2259, 901, 14_000, 14_000);
        let shown = calc.get_dps();
        assert_eq!(dealt(&shown), vec![2259, 3000], "you and your party, no stranger");
        assert_eq!(shown.map[&2259].amount, 9.0 * 500.0, "your hits from before you were known count");
        assert_eq!(calc.displayed_targets, vec![901]);
        assert_eq!(shown.battle_time, 9_000, "from your first hit");
        crate::clock::set_override(None);
    }

    #[test]
    fn a_zone_load_or_a_reset_ends_the_encounter() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, 1);
        hits(&s, 2259, 800, 1_000, 3_000);
        assert!(s.current_encounter().is_some());
        crate::clock::set_override(Some(10_000));
        assert!(s.note_zone_change());
        assert!(s.current_encounter().is_none());

        hits(&s, 2259, 800, 20_000, 21_000);
        let mut calc = meter_with_npcs(&s);
        calc.restart_target_selection(true);
        assert!(s.current_encounter().is_none());
        crate::clock::set_override(None);
    }

    #[test]
    fn encounter_rows_carry_encdps_own_dps_and_last_n() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, 1);
        hits(&s, 2259, 800, 1_000, 70_000);
        // Someone else joins for the last three seconds.
        hits(&s, 3000, 800, 68_000, 70_000);
        crate::clock::set_override(Some(70_000));
        let mut calc = meter_with_npcs(&s);
        calc.set_target_selection_mode("encounter");
        let shown = calc.get_dps();
        assert_eq!(shown.battle_time, 69_000);
        let me = &shown.map[&2259];
        let other = &shown.map[&3000];
        let close = |a: f64, b: f64| (a - b).abs() < 0.01;
        assert!(close(me.dps, 70.0 * 500.0 / 69.0), "ENCDPS over the encounter");
        assert!(close(other.dps, 3.0 * 500.0 / 69.0));
        assert!(close(other.active_dps, 3.0 * 500.0 / 2.0), "own DPS over its own 2 s");
        assert!(close(me.last10_dps, 11.0 * 500.0 / 10.0), "60..70 s");
        assert!(close(me.last60_dps, 61.0 * 500.0 / 60.0), "10..70 s");
        crate::clock::set_override(None);
    }

    #[test]
    fn a_boss_pull_keeps_the_trash_of_the_open_encounter() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, 1);
        spawn(&s, 900, BOSS);
        hits(&s, 2259, 800, 1_000, 10_000);
        // The pull comes 10 s later, inside the encounter, and clears the trash.
        hits(&s, 2259, 900, 20_000, 29_000);
        crate::clock::set_override(Some(29_000));
        let mut calc = meter_with_npcs(&s);
        calc.set_target_selection_mode("encounter");
        let shown = calc.get_dps();
        let me = &shown.map[&2259];
        assert_eq!(shown.battle_time, 28_000);
        assert_eq!(me.amount, 20.0 * 500.0, "the trash before the pull still counts");
        assert!((me.last30_dps - 20.0 * 500.0 / 30.0).abs() < 0.01);
        // Trash hit again after the pull: one row of both parts.
        hits(&s, 2259, 800, 30_000, 31_000);
        assert_eq!(calc.get_dps().map[&2259].amount, 22.0 * 500.0);
        assert_eq!(calc.get_displayed_details(None).total_target_damage, 22 * 500, "details too");
        // The other modes still start clean at the pull.
        calc.set_target_selection_mode("mostDamage");
        assert_eq!(calc.get_dps().map[&2259].amount, 10.0 * 500.0);

        // A pull after the encounter ended carries nothing over.
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, 1);
        spawn(&s, 900, BOSS);
        hits(&s, 2259, 800, 1_000, 10_000);
        hits(&s, 2259, 900, 40_000, 49_000);
        let mut calc = meter_with_npcs(&s);
        calc.set_target_selection_mode("encounter");
        let shown = calc.get_dps();
        assert_eq!((shown.battle_time, shown.map[&2259].amount), (9_000, 10.0 * 500.0));
        crate::clock::set_override(None);
    }

    #[test]
    fn encounter_rows_count_hits_crits_and_the_biggest_hit() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, 1);
        // Before the encounter: left out once 20 s pass without combat.
        hits(&s, 2259, 800, 1_000, 2_000);
        crate::clock::set_override(Some(30_000));
        let mut crit = hit(2259, 800, 30_000);
        crit.set_damage(2_000);
        crit.set_specials(vec![SpecialDamage::Critical]);
        s.append_damage(crit);
        hits(&s, 2259, 800, 31_000, 33_000);
        let mut tick = hit(2259, 800, 33_500);
        tick.set_dot(true);
        tick.set_damage(5_000);
        crate::clock::set_override(Some(33_500));
        s.append_damage(tick);
        let mut calc = meter_with_npcs(&s);
        calc.set_target_selection_mode("encounter");
        let me = &calc.get_dps().map[&2259];
        assert_eq!((me.hits, me.crit_hits, me.max_hit), (4, 1, 2_000), "a tick is not a hit");

        // Without a window the whole fight counts.
        calc.set_target_selection_mode("lastHitByMe");
        let me = &calc.get_dps().map[&2259];
        assert_eq!((me.hits, me.crit_hits, me.max_hit), (6, 1, 2_000));
        crate::clock::set_override(None);
    }

    #[test]
    fn row_details_cover_every_target_a_multi_target_mode_shows() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, DUMMY);
        spawn(&s, 810, DUMMY);
        let mut calc = meter_with_npcs(&s);
        hits(&s, 2259, 800, 1_000, 4_000);
        hits(&s, 2259, 810, 2_000, 6_000);
        crate::clock::set_override(Some(7_000));
        calc.set_target_selection_mode("trainTargets");
        let shown = calc.get_dps();
        assert_eq!(shown.target_id, 0, "two targets, no single one");
        let details = calc.get_displayed_details(Some(&[2259]));
        let hits: i32 = details.skills.iter().map(|s| s.time).sum();
        let dmg: i64 = details.skills.iter().map(|s| s.dmg as i64).sum();
        assert_eq!((hits, dmg), (9, 9 * 500), "4 hits on one dummy and 5 on the other");
        assert_eq!(details.battle_time, shown.battle_time);

        // One target: the same as that target's own details.
        calc.set_target_selection_mode("lastHitByMe");
        calc.get_dps();
        let one = calc.get_displayed_details(Some(&[2259]));
        assert_eq!(one.target_id, 810);
        assert_eq!(one.skills.iter().map(|s| s.time).sum::<i32>(), 5);

        // A reset leaves nothing to explain.
        calc.restart_target_selection(true);
        calc.get_dps();
        assert!(calc.get_displayed_details(Some(&[2259])).skills.is_empty());
        crate::clock::set_override(None);
    }

    #[test]
    fn a_fight_cleared_by_a_zone_load_keeps_who_was_who() {
        let s = Arc::new(DataStorage::new());
        s.set_local_identity_from_game(3197, Some("Seralth".into()));
        s.append_nickname_authoritative(3197, "Seralth");
        s.append_summon(3197, 21821);
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        hits(&s, 3197, 800, 1_000, 8_000);
        hits(&s, 21821, 800, 1_000, 8_000);
        crate::clock::set_override(Some(12_000));
        assert!(s.note_zone_change());
        // The load gives you a new id, and your spirit's id to a new spirit.
        s.set_local_identity_from_game(6207, Some("Seralth".into()));
        s.append_nickname_authoritative(6207, "Seralth");
        s.append_summon(6207, 21821);
        let saved = snapshot_at(&mut calc, 12_000);
        assert_eq!(ids(&saved), vec!["auto_800_1000"]);
        let actors: Vec<_> = saved[0].actors.iter().map(|a| (a.actor_id, a.nickname.as_str())).collect();
        assert_eq!(actors, vec![(3197, "Seralth")]);
        assert_eq!(saved[0].details.skills.iter().map(|s| s.dmg as i64).sum::<i64>(), 16 * 500);
        crate::clock::set_override(None);
    }

    #[test]
    fn only_fights_you_or_your_party_hit_are_saved() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        spawn(&s, 810, BOSS);
        spawn(&s, 820, DUMMY);
        let mut calc = meter_with_npcs(&s);
        hits(&s, 2259, 800, 1_000, 8_000);
        hits(&s, 5555, 810, 1_000, 8_000);
        hits(&s, 5555, 820, 1_000, 8_000);
        // A zone load gives you a new id before the save runs.
        crate::clock::set_override(Some(12_000));
        assert!(s.note_zone_change());
        s.set_local_player_id(Some(3197));
        assert_eq!(ids(&snapshot_at(&mut calc, 12_000)), vec!["auto_800_1000"]);
        crate::clock::set_override(None);
    }

    #[test]
    fn fights_survive_a_zone_change_a_reset_and_the_party_ending() {
        use crate::combat::data_storage::PartyMember;
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        hits(&s, 2259, 800, 1_000, 8_000);
        crate::clock::set_override(Some(12_000));
        assert!(s.note_zone_change());
        let saved = snapshot_at(&mut calc, 12_000);
        assert_eq!(ids(&saved), vec!["auto_800_1000"]);
        assert!(calc.fight_finished(&saved[0]), "it cannot go on");

        hits(&s, 2259, 800, 20_000, 30_000);
        crate::clock::set_override(Some(31_000));
        calc.restart_target_selection(true);
        assert_eq!(ids(&snapshot_at(&mut calc, 31_000)), vec!["auto_800_20000"]);

        let member = |slot| PartyMember { slot, ..Default::default() };
        s.set_party_roster(vec![("A".into(), member(1)), ("B".into(), member(2))], true);
        hits(&s, 2259, 800, 40_000, 50_000);
        s.set_party_roster(vec![("A".into(), member(1))], true);
        assert_eq!(ids(&snapshot_at(&mut calc, 51_000)), vec!["auto_800_40000"]);

        // Too short to keep, and not a boss: neither is saved.
        spawn(&s, 900, 1);
        hits(&s, 2259, 900, 60_000, 70_000);
        hits(&s, 2259, 800, 60_000, 62_000);
        crate::clock::set_override(Some(80_000));
        assert!(s.note_zone_change());
        assert!(snapshot_at(&mut calc, 80_000).is_empty());
        crate::clock::set_override(None);
    }

    #[test]
    fn all_targets_time_is_the_time_anything_was_fought() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        hits(&s, 2259, 50_001, 1_000, 11_000);
        hits(&s, 2259, 50_002, 5_000, 15_000);
        hits(&s, 9000, 50_003, 31_000, 41_000);
        let mut calc = meter(&s);
        calc.set_target_selection_mode("allTargets");
        crate::clock::set_override(Some(45_000));
        assert_eq!(calc.get_dps().battle_time, 24_000, "the gap between pulls is left out");

        // A 10 s window at 45 s sees the last pull from 35 s.
        calc.set_all_targets_window_ms(10_000);
        assert_eq!(calc.get_dps().battle_time, 6_000);
        crate::clock::set_override(Some(60_000));
        let shown = calc.get_dps();
        assert_eq!(shown.battle_time, 0);
        assert!(shown.map.is_empty(), "all of it is outside the window");
        crate::clock::set_override(None);
    }

    #[test]
    fn train_is_your_dummies_and_your_time() {
        let s = Arc::new(DataStorage::new());
        spawn(&s, 36734, DUMMY);
        spawn(&s, 36735, DUMMY);
        spawn(&s, 36736, DUMMY);
        hits(&s, 9000, 36736, 1_000, 100_000);
        hits(&s, 9000, 36734, 80_000, 80_000);
        hits(&s, 2259, 36734, 95_000, 100_000);
        hits(&s, 2259, 36735, 90_000, 100_000);
        let mut calc = meter_with_npcs(&s);
        calc.set_target_selection_mode("trainTargets");
        assert!(calc.get_dps().map.is_empty(), "nothing until the meter knows you");

        s.set_local_player_id(Some(2259));
        let shown = calc.get_dps();
        assert_eq!(shown.battle_time, 10_000);
        assert!(shown.map.contains_key(&2259));
        assert_eq!(shown.map[&2259].amount, 17.0 * 500.0);
        assert_eq!(shown.map[&9000].amount, 500.0, "a stranger's dummy is not yours");
        crate::clock::set_override(None);
    }

    #[test]
    fn train_follows_the_dummy_you_hit_not_your_spirits_spill_over() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(100));
        s.append_nickname_authoritative(100, "Owner");
        for id in [36734, 36735, 36736] {
            spawn(&s, id, DUMMY);
        }
        // Your spirit, linked to you.
        s.note_summon_spawn(500);
        s.append_damage(skill_hit(500, 100, 500, 16_990_002, 20));
        for at in (1_000..=20_000).step_by(1_000) {
            crate::clock::set_override(Some(at));
            s.append_damage(skill_hit(100, 36734, at, 16_010_000, 1_000));
            s.append_damage(skill_hit(500, 36734, at, 16_110_004, 200));
            // Its area hits reach the next dummy now and then.
            if at % 5_000 == 0 {
                s.append_damage(skill_hit(500, 36735, at, 16_110_004, 50));
            }
            s.append_damage(skill_hit(9000, 36736, at, 11_010_000, 700));
        }
        let mut calc = meter_with_npcs(&s);
        calc.set_target_selection_mode("trainTargets");
        let shown = calc.get_dps();
        assert_eq!(calc.displayed_targets, vec![36734], "your dummy, not the spill-over or a stranger's");
        assert_eq!(shown.map[&100].amount, 20.0 * 1_200.0, "your spirit's hits on it count");
        let listed: Vec<i32> = calc.details_source().reader().get_details_context().targets.iter().map(|t| t.target_id).collect();
        assert_eq!(listed, vec![36734], "Details lists the meter's dummies only");

        // Summons alone at work: their dummy.
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(100));
        spawn(&s, 36735, DUMMY);
        s.note_summon_spawn(500);
        s.append_damage(skill_hit(500, 100, 500, 16_990_002, 20));
        hits(&s, 500, 36735, 1_000, 5_000);
        let mut calc = meter_with_npcs(&s);
        calc.set_target_selection_mode("trainTargets");
        calc.get_dps();
        assert_eq!(calc.displayed_targets, vec![36735]);
        crate::clock::set_override(None);
    }

    #[test]
    fn boss_mode_follows_your_boss_and_lets_a_dead_one_go() {
        let s = Arc::new(DataStorage::new());
        for id in [801, 802] {
            s.append_mob(id, BOSS);
        }
        s.append_mob(900, 1);
        hits(&s, 2259, 801, 1_000, 5_000);
        hits(&s, 9000, 802, 10_000, 20_000);
        let mut calc = meter_with_npcs(&s);
        assert_eq!(calc.get_dps().target_id, 802, "not identified: any boss");
        s.set_local_player_id(Some(2259));
        assert_eq!(calc.get_dps().target_id, 801, "a stranger's boss is not yours");

        s.mark_entity_dead(801);
        assert_eq!(calc.get_dps().target_id, 801, "dead, and still the last thing you hit");
        hits(&s, 2259, 900, 30_000, 31_000);
        assert_eq!(calc.get_dps().target_id, 900, "dead, and you moved on");
        crate::clock::set_override(None);
    }

    #[test]
    fn idle_targets_the_meter_does_not_show_are_retired() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        hits(&s, 2259, 800, 1_000, 10_000);
        s.mark_entity_dead(800);
        // Each pull a new encounter: the encounter keeps its own.
        hits(&s, 2259, 900, 30_000, 32_000);
        hits(&s, 2259, 901, 60_000, 62_000);
        let mut calc = meter_with_npcs(&s);
        calc.set_target_selection_mode("allTargets");
        crate::clock::set_override(Some(100_000));
        calc.get_dps();
        assert_eq!(s.get_combat_snapshot_light().len(), 3, "ALL without a window shows them all");

        calc.set_target_selection_mode("lastHitByMe");
        crate::clock::set_override(Some(110_000));
        assert_eq!(calc.get_dps().target_id, 901);
        let mut left: Vec<i32> = s.get_combat_snapshot_light().into_keys().collect();
        left.sort();
        assert_eq!(left, vec![901], "the target on screen stays");
        // The boss fight went to the auto-save, as an idle restart sends it.
        assert_eq!(ids(&snapshot_at(&mut calc, 110_000)), vec!["auto_800_1000"]);
        crate::clock::set_override(None);
    }

    #[test]
    fn a_mode_switch_does_not_show_the_last_modes_rows() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        hits(&s, 2259, 900, 1_000, 2_000);
        let mut calc = meter(&s);
        calc.set_target_selection_mode("lastHitByMe");
        assert!(calc.get_dps().map.contains_key(&2259));
        calc.set_target_selection_mode("trainTargets");
        let shown = calc.get_dps();
        assert!(shown.map.is_empty());
        assert_eq!(shown.target_mode, "trainTargets");
        crate::clock::set_override(None);
    }

    #[test]
    fn spans_are_merged_and_clipped() {
        assert_eq!(active_time([(0, 10), (5, 15), (30, 40)].into_iter(), i64::MIN), 25);
        assert_eq!(active_time([(0, 10), (5, 15), (30, 40)].into_iter(), 12), 13);
        assert_eq!(active_time([(0, 10)].into_iter(), 20), 0);
        assert_eq!(active_time(std::iter::empty(), 0), 0);
    }

    #[test]
    fn a_saved_fight_keeps_only_the_healing_done_during_it() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        let healed = |r: &FightRecord| r.details.heal_skills.iter().map(|h| h.dmg as i64).sum::<i64>();
        s.append_heal(2259, 17_800_000, 111, false, 500);
        hits(&s, 2259, 800, 1_000, 8_000);
        s.append_heal(2259, 17_800_000, 222, false, 4_000);
        s.append_heal(2259, 17_800_000, 333, false, 9_500);
        let live = |calc: &DpsCalculator| calc.get_target_details(800, None).heal_skills.iter().map(|h| h.dmg as i64).sum::<i64>();
        assert_eq!(live(&calc), 222, "live Details: the same window");
        calc.set_target_selection_mode("allTargets");
        calc.get_dps();
        let shown = calc.get_displayed_details(None).heal_skills.iter().map(|h| h.dmg as i64).sum::<i64>();
        assert_eq!(shown, 222, "in every mode");
        calc.set_target_selection_mode("bossTargets");
        let saved = snapshot_at(&mut calc, 30_000);
        assert_eq!(ids(&saved), vec!["auto_800_1000"]);
        assert_eq!(healed(&saved[0]), 222, "saved while live");

        // The next pull ends the first; a zone load clears the second.
        hits(&s, 2259, 800, 40_000, 48_000);
        s.append_heal(2259, 17_800_000, 444, false, 45_000);
        crate::clock::set_override(Some(50_000));
        assert!(s.note_zone_change());
        let saved = snapshot_at(&mut calc, 50_000);
        let by_id = |id: &str| saved.iter().find(|r| r.id == id).map(healed);
        assert_eq!(by_id("auto_800_40000"), Some(444), "saved after it was cleared");
        assert!(by_id("auto_800_1000").is_none_or(|h| h == 222));
        crate::clock::set_override(None);
    }

    #[test]
    fn damage_totals_past_two_billion_do_not_wrap() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        s.append_nickname_authoritative(2259, "Seralth");
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        for (i, skill) in [16_010_000, 16_020_000, 16_030_000].into_iter().enumerate() {
            let at = 1_000 + i as i64 * 3_000;
            crate::clock::set_override(Some(at));
            s.append_damage(skill_hit(2259, 800, at, skill, 1_000_000_000));
        }
        let target = calc.get_details_context().targets.into_iter().find(|t| t.target_id == 800).unwrap();
        assert_eq!(target.total_damage as i64, 3_000_000_000);
        assert_eq!(target.actor_damage[&2259] as i64, 3_000_000_000);
        assert_eq!(calc.get_target_details(800, None).total_target_damage as i64, 3_000_000_000);
        let saved = snapshot_at(&mut calc, 30_000);
        assert_eq!(saved[0].total_damage as i64, 3_000_000_000);
        crate::clock::set_override(None);
    }

    #[test]
    fn one_skill_and_one_heal_past_two_billion_do_not_stop_there() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        for i in 0..3 {
            let at = 1_000 + i * 3_000;
            crate::clock::set_override(Some(at));
            s.append_damage(skill_hit(2259, 800, at, 16_010_000, 1_000_000_000));
            s.append_heal(2259, 17_800_000, 1_000_000_000, false, at);
        }
        let details = calc.get_target_details(800, None);
        assert_eq!(details.skills.iter().map(|r| r.dmg).sum::<i64>(), 3_000_000_000);
        assert_eq!(details.heal_skills.iter().map(|r| r.dmg).sum::<i64>(), 3_000_000_000);
        let saved = snapshot_at(&mut calc, 30_000);
        let json = serde_json::to_string(&saved[0]).unwrap();
        let back: FightRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.details.skills.iter().map(|r| r.dmg).sum::<i64>(), 3_000_000_000);
        crate::clock::set_override(None);
    }

    fn skill_hit(actor: i32, target: i32, at: i64, skill: i32, damage: i32) -> ParsedDamagePacket {
        let mut p = hit(actor, target, at);
        p.set_skill_code(skill);
        p.set_damage(damage);
        p
    }

    /// Per-row totals the saved fight would hold for `target`.
    fn saved_totals(calc: &DpsCalculator, target: i32) -> HashMap<i32, i64> {
        let mut totals = HashMap::new();
        for skill in calc.get_target_details(target, None).skills {
            *totals.entry(skill.actor_id).or_insert(0) += skill.dmg as i64;
        }
        totals
    }

    fn live_totals(calc: &mut DpsCalculator) -> HashMap<i32, i64> {
        calc.get_dps().map.iter().filter(|(_, r)| r.amount > 0.0).map(|(&id, r)| (id, r.amount as i64)).collect()
    }

    #[test]
    fn nothing_joins_an_owner_without_a_link() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(100));
        s.append_nickname_authoritative(100, "Owner");
        s.append_damage(skill_hit(100, 900, 1_000, 16_010_000, 1_000));
        // Same class, never linked: a party member's spirit, or a party member.
        s.note_summon_spawn(500);
        s.append_damage(skill_hit(500, 900, 1_100, 16_010_000, 200));
        s.append_damage(skill_hit(501, 900, 1_200, 16_010_000, 300));
        // A mob the player hits, using a class-band skill of the same class.
        s.append_damage(skill_hit(100, 700, 1_300, 16_010_000, 50));
        s.append_damage(skill_hit(700, 100, 1_400, 16_020_000, 5));
        let mut calc = meter(&s);
        let live = live_totals(&mut calc);
        assert_eq!(live.get(&100), Some(&1_000));
        assert_eq!(live.get(&500), Some(&200));
        assert_eq!(live.get(&501), Some(&300));
        let saved = saved_totals(&calc, 900);
        assert_eq!(saved.get(&100), Some(&1_000));
        assert_eq!(saved.get(&500), Some(&200));
    }

    #[test]
    fn a_link_brings_earlier_damage_and_live_and_saved_agree() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(100));
        s.append_nickname_authoritative(100, "Owner");
        s.append_damage(skill_hit(100, 900, 1_000, 16_010_000, 1_000));
        s.note_summon_spawn(500);
        s.append_damage(skill_hit(500, 900, 1_100, 16_110_004, 200));
        s.append_damage(skill_hit(500, 900, 1_150, 100_018, 30));
        s.append_damage(skill_hit(500, 100, 1_200, 16_990_002, 20));
        s.append_damage(skill_hit(500, 900, 1_300, 16_010_000, 70));
        // Someone else's spirit, linked to them.
        s.append_damage(skill_hit(222, 900, 1_000, 16_010_000, 400));
        s.append_damage(skill_hit(222, 600, 1_000, 16_770_000, 197));
        s.append_damage(skill_hit(600, 900, 1_000, 16_010_000, 100));
        let mut calc = meter(&s);
        let live = live_totals(&mut calc);
        assert_eq!(live.get(&100), Some(&1_300), "the spirit's damage before the link too");
        assert_eq!(live.get(&222), Some(&500));
        assert_eq!(live.len(), 2);
        assert_eq!(saved_totals(&calc, 900), live);
    }

    #[test]
    fn boss_mode_shows_your_trash_mob_in_the_open_world_only() {
        let open_world = Arc::new(DataStorage::new());
        open_world.set_local_player_id(Some(2259));
        open_world.append_damage(hit(2259, 50_000, 1_000));
        // A stranger alone on a bigger fight of their own stays off the meter.
        for t in 0..5 {
            open_world.append_damage(hit(11_345, 60_000, 1_000 + t));
        }
        let shown = meter(&open_world).get_dps();
        assert_eq!(shown.target_id, 50_000);
        assert_eq!(shown.map.keys().copied().collect::<Vec<_>>(), vec![2259]);

        // Before you are identified (a meter just opened, a new zone), no
        // mob is anyone's: a stranger's fight is not put up in your place.
        let unknown = Arc::new(DataStorage::new());
        unknown.append_damage(hit(2259, 50_000, 1_000));
        for t in 0..5 {
            unknown.append_damage(hit(11_345, 60_000, 1_000 + t));
        }
        let shown = meter(&unknown).get_dps();
        assert_eq!(shown.target_id, 0);
        assert!(shown.map.is_empty());

        // In a dungeon, a mob that is not a boss is not shown at all.
        let dungeon = Arc::new(DataStorage::new());
        dungeon.set_local_player_id(Some(2259));
        dungeon.set_current_dungeon(600_011);
        dungeon.append_damage(hit(2259, 50_000, 1_000));
        let shown = meter(&dungeon).get_dps();
        assert_eq!(shown.target_id, 0);
        assert!(shown.map.is_empty());
    }

    /// Replay a capture file, calling `before` with each line's time of day
    /// (ms) before that line is parsed.
    fn replay_capture(path: &str, mut before: impl FnMut(i64, &Arc<DataStorage>)) -> Arc<DataStorage> {
        use crate::capture::stream_processor::StreamProcessor;
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data");
        let skills = Arc::new(SkillLookup::new());
        let npcs = Arc::new(NpcLookup::new());
        crate::i18n::lookup::load_language(&skills, &npcs, &data, "en");
        let s = Arc::new(DataStorage::new());
        let mut p = StreamProcessor::new(s.clone(), skills, npcs);
        let dots: Vec<i32> = serde_json::from_str(&std::fs::read_to_string(data.join("dot_skill_ids.json")).unwrap()).unwrap();
        p.set_dot_skill_ids(dots.into_iter().collect());
        for line in std::fs::read_to_string(path).unwrap().lines() {
            let parts: Vec<&str> = line.splitn(3, '|').collect();
            if line.starts_with('#') || parts.len() != 3 { continue; }
            // `2026-10-04T04:21:16.849094363-07:00`
            let t = parts[0];
            let n = |a: usize, b: usize| t[a..b].parse::<i64>().unwrap();
            let ts = ((n(11, 13) * 60 + n(14, 16)) * 60 + n(17, 19)) * 1000 + n(20, 23);
            before(ts, &s);
            p.set_override_timestamp(Some(ts));
            let hex = parts[2];
            let bytes: Vec<u8> = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()).collect();
            p.consume_stream(&bytes);
        }
        p.set_override_timestamp(None);
        s
    }

    /// Spirit damage by owner, and unlinked summon-spawned actors with their damage.
    fn spirit_damage(s: &DataStorage) -> (HashMap<i32, i64>, Vec<(i32, i64)>) {
        let summons = s.get_summon_data();
        let spawned = s.get_summon_spawn_ids();
        let mut per_actor: HashMap<i32, i64> = HashMap::new();
        for t in s.get_combat_snapshot_light().values() {
            for (&a, d) in &t.actors {
                *per_actor.entry(a).or_default() += d.total_damage;
            }
        }
        let (mut by_owner, mut unlinked) = (HashMap::new(), Vec::new());
        for (&a, &dmg) in &per_actor {
            if !spawned.contains(&a) && !summons.contains_key(&a) {
                continue;
            }
            let owner = summon_resolver::resolve(a, &summons);
            if owner == a { unlinked.push((a, dmg)); } else { *by_owner.entry(owner).or_default() += dmg; }
        }
        unlinked.sort_by_key(|&(_, d)| -d);
        (by_owner, unlinked)
    }

    /// The user's capture of 2026-10-04 04:21 (not in the repo): spirits of
    /// three Spiritmasters, then the user's run at scarecrow 36734.
    #[test]
    #[ignore]
    fn spirit_owners_in_a_capture() {
        let path = std::env::var("A2_CAPTURE").unwrap_or("/caps/packets_20261004_042115.txt".into());
        let hms = |h: i64, m: i64, sec: i64| ((h * 60 + m) * 60 + sec) * 1000;
        let mut checked = [false; 2];
        replay_capture(&path, |ts, s| {
            if !checked[0] && ts >= hms(4, 44, 0) {
                checked[0] = true;
                let (by_owner, unlinked) = spirit_damage(s);
                eprintln!("04:44:00 spirit damage by owner {by_owner:?}, unlinked {unlinked:?}");
                let owners: HashSet<i32> = by_owner.keys().copied().collect();
                assert_eq!(owners, HashSet::from([13600, 11147, 14570]));
                assert!(unlinked.is_empty());
                let summons = s.get_summon_data();
                for player in [13600, 11147, 14570] {
                    assert_eq!(summon_resolver::resolve(player, &summons), player);
                }
            }
            if !checked[1] && ts >= hms(4, 46, 45) {
                checked[1] = true;
                let summons = s.get_summon_data();
                let target = &s.get_combat_snapshot()[&36734];
                let mut rows: HashMap<i32, i64> = HashMap::new();
                for (&a, d) in &target.actors {
                    *rows.entry(summon_resolver::resolve(a, &summons)).or_default() += d.total_damage;
                }
                let calc = meter(s);
                eprintln!("scarecrow 36734 rows {rows:?}, saved {:?}", saved_totals(&calc, 36734));
                // 138,109 dealt; the training-dummy rule holds back the 429 of
                // DoT ticks after the last direct hit.
                assert_eq!(rows, HashMap::from([(13600, 137_680)]));
                assert_eq!(saved_totals(&calc, 36734), rows);
            }
        });
        assert_eq!(checked, [true, true]);
    }

    /// Per-skill rows of one target at a moment of a capture, to set beside the
    /// game's Damage Analyzer record of the same fight. A2_CAPTURE, A2_AT
    /// (hh:mm:ss), A2_TARGET (without it: every target's total).
    #[test]
    #[ignore]
    fn skill_rows_at() {
        let path = std::env::var("A2_CAPTURE").unwrap();
        let at: Vec<i64> = std::env::var("A2_AT").unwrap().split(':').map(|x| x.parse().unwrap()).collect();
        let at = ((at[0] * 60 + at[1]) * 60 + at[2]) * 1000;
        let target: Option<i32> = std::env::var("A2_TARGET").ok().map(|t| t.parse().unwrap());
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data");
        let skills = SkillLookup::new();
        crate::i18n::lookup::load_language(&skills, &NpcLookup::new(), &data, "en");
        let mut done = false;
        replay_capture(&path, |ts, s| {
            if done || ts < at { return; }
            done = true;
            let summons = s.get_summon_data();
            for (&t, td) in &s.get_combat_snapshot() {
                if target.is_some_and(|x| x != t) { continue; }
                let mut rows: HashMap<(i32, i32), (i64, i32)> = HashMap::new();
                for (&a, ad) in &td.actors {
                    let owner = summon_resolver::resolve(a, &summons);
                    for sd in ad.skills.values() {
                        let row = rows.entry((owner, crate::entity::skill_group::row_skill(sd.skill_code, &skills))).or_default();
                        row.0 += sd.total_damage as i64;
                        row.1 += sd.hit_count;
                    }
                }
                let total: i64 = rows.values().map(|r| r.0).sum();
                eprintln!("target {t}: {total}");
                if target.is_some() {
                    let mut rows: Vec<_> = rows.into_iter().collect();
                    rows.sort_by_key(|(k, v)| (k.0, -v.0));
                    for ((owner, skill), (dmg, hits)) in rows {
                        eprintln!("  actor {owner} skill {skill} damage {dmg} hits {hits} {}", skills.get_skill_name(skill));
                    }
                }
            }
        });
        assert!(done);
    }

    /// Every capture in /caps: when the dungeon id changes, and the saved
    /// boss fights with their dungeon ids.
    #[test]
    #[ignore]
    fn dungeon_ids_in_all_captures() {
        let mut caps: Vec<_> = std::fs::read_dir("/caps").unwrap().filter_map(|e| e.ok()).map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "txt")).collect();
        caps.sort();
        let clock = |ts: i64| format!("{:02}:{:02}:{:02}", ts / 3_600_000 % 24, ts / 60_000 % 60, ts / 1000 % 60);
        for cap in caps {
            eprintln!("{}", cap.display());
            let mut last = 0;
            let mut last_save = 0;
            let mut calc: Option<DpsCalculator> = None;
            let mut saved: HashMap<String, (i64, i32, i32)> = HashMap::new();
            replay_capture(cap.to_str().unwrap(), |ts, s| {
                let id = s.current_dungeon_id();
                if id != last {
                    eprintln!("  {} dungeon {last} -> {id}", clock(ts));
                    last = id;
                }
                if ts - last_save < 5_000 { return; }
                last_save = ts;
                crate::clock::set_override(Some(ts));
                let calc = calc.get_or_insert_with(|| {
                    let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data");
                    let skills = Arc::new(SkillLookup::new());
                    let npcs = Arc::new(NpcLookup::new());
                    crate::i18n::lookup::load_language(&skills, &npcs, &data, "en");
                    DpsCalculator::new(s.clone(), skills, npcs, Arc::new(PingTracker::new()))
                });
                for r in calc.snapshot_boss_fights() {
                    saved.insert(r.id.clone(), (r.start_time_ms, r.mob_code, r.dungeon_id));
                }
                crate::clock::set_override(None);
            });
            let mut fights: Vec<_> = saved.into_values().collect();
            fights.sort();
            for (start, mob, dungeon) in fights {
                eprintln!("  saved {} mob {mob} dungeon {dungeon}", clock(start));
            }
        }
    }

    /// Every capture in /caps: no player or hit target is anyone's summon, and
    /// how much summon damage is left unlinked.
    #[test]
    #[ignore]
    fn summon_links_in_all_captures() {
        let mut caps: Vec<_> = std::fs::read_dir("/caps").unwrap().filter_map(|e| e.ok()).map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "txt")).collect();
        caps.sort();
        for cap in caps {
            let mut bad_players = HashSet::new();
            let mut bad_targets = HashSet::new();
            let mut last_minute = 0;
            let s = replay_capture(cap.to_str().unwrap(), |ts, s| {
                if ts / 60_000 == last_minute { return; }
                last_minute = ts / 60_000;
                let summons = s.get_summon_data();
                bad_players.extend(s.get_known_player_ids().into_iter().chain(s.get_nicknames().into_keys())
                    .filter(|id| summons.contains_key(id)));
                bad_targets.extend(s.get_combat_snapshot_light().keys().copied().filter(|id| summons.contains_key(id)));
            });
            let (by_owner, unlinked) = spirit_damage(&s);
            eprintln!("{}: players linked as summons {:?}, hit targets linked as summons {:?}, linked owners {}, unlinked {} with {} damage {:?}",
                cap.display(), bad_players, bad_targets, by_owner.len(), unlinked.len(), unlinked.iter().map(|&(_, d)| d).sum::<i64>(),
                &unlinked[..unlinked.len().min(8)]);
        }
    }

    /// Every boss pull in the captures of 2026-10-04 (not in the repo): the ENC
    /// rows a second before and a second after, and the encounter's damage on
    /// other targets just before. A2_CAPS: the capture folder.
    #[test]
    #[ignore]
    fn enc_rows_around_boss_pulls() {
        let dir = std::env::var("A2_CAPS").unwrap_or("/caps".into());
        let mut caps: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path())
            .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("packets_20261004_")).collect();
        caps.sort();
        let clock = |ts: i64| format!("{:02}:{:02}:{:02}", ts / 3_600_000 % 24, ts / 60_000 % 60, ts / 1000 % 60);
        struct Sample {
            ts: i64,
            enc: Option<crate::combat::data_storage::Encounter>,
            keys: HashSet<i32>,
            /// Damage since the encounter's start on its targets that are not bosses.
            trash: i64,
            shown: DpsData,
        }
        let print = |label: &str, x: &Sample, me: Option<i64>| {
            let start = x.enc.as_ref().map_or("-".into(), |e| clock(e.start));
            let total: f64 = x.shown.map.values().map(|r| r.amount).sum();
            eprintln!("    {label} {}: enc start {start}, targets {}, trash dmg in enc {}, ENC total {total:.0}, time {} ms",
                clock(x.ts), x.enc.as_ref().map_or(0, |e| e.targets.len()), x.trash, x.shown.battle_time);
            let mut rows: Vec<_> = x.shown.map.iter().filter(|(_, r)| r.amount > 0.0).collect();
            rows.sort_by(|a, b| b.1.amount.total_cmp(&a.1.amount));
            for (uid, r) in rows.iter().take(4) {
                let you = if me == Some(**uid as i64) { " (you)" } else { "" };
                eprintln!("      {uid}{you}: dmg {:.0} encdps {:.0} own {:.0} last10/30/60 {:.0}/{:.0}/{:.0}",
                    r.amount, r.dps, r.active_dps, r.last10_dps, r.last30_dps, r.last60_dps);
            }
        };
        let mut pulls = 0;
        for cap in caps {
            eprintln!("{}", cap.file_name().unwrap().to_string_lossy());
            let mut calc: Option<DpsCalculator> = None;
            let mut prev: Option<Sample> = None;
            let mut last_sec = 0;
            let mut later: Option<i64> = None;
            replay_capture(cap.to_str().unwrap(), |ts, s| {
                if ts / 1000 == last_sec { return; }
                last_sec = ts / 1000;
                crate::clock::set_override(Some(ts));
                let calc = calc.get_or_insert_with(|| {
                    let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data");
                    let skills = Arc::new(SkillLookup::new());
                    let npcs = Arc::new(NpcLookup::new());
                    crate::i18n::lookup::load_language(&skills, &npcs, &data, "en");
                    let mut c = DpsCalculator::new(s.clone(), skills, npcs, Arc::new(PingTracker::new()));
                    c.set_target_selection_mode("encounter");
                    c
                });
                let snap = s.get_combat_snapshot_light();
                let shown = s.get_encounter_snapshot_light();
                let enc = s.current_encounter();
                let trash = enc.as_ref().map_or(0, |e| e.targets.iter()
                    .filter(|t| !s.is_boss(**t))
                    .filter_map(|t| shown.get(t))
                    .flat_map(|td| td.actors.values().map(|a| a.damage_since(e.start)))
                    .sum());
                let x = Sample { ts, enc, keys: snap.keys().copied().collect(), trash, shown: calc.get_dps() };
                let me = s.local_player_id();
                if let Some(p) = &prev {
                    let new_bosses: Vec<i32> = x.keys.difference(&p.keys).copied().filter(|t| s.is_boss(*t)).collect();
                    if !new_bosses.is_empty() {
                        pulls += 1;
                        let dropped = p.keys.difference(&x.keys).count();
                        let same = matches!((&p.enc, &x.enc), (Some(a), Some(b)) if a.start == b.start);
                        eprintln!("  first hit on boss {new_bosses:?} at {}, dungeon {}, {dropped} targets cleared, same encounter {same}",
                            clock(x.ts), s.current_dungeon_id());
                        print("before  ", p, me);
                        print("after   ", &x, me);
                        later = Some(x.ts + 10_000);
                    }
                }
                if later.is_some_and(|t| x.ts >= t) {
                    later = None;
                    print("after+10", &x, me);
                }
                prev = Some(x);
                crate::clock::set_override(None);
            });
        }
        assert!(pulls > 0);
    }

    /// The hover summary is the full details without hit timelines, healing
    /// and ping.
    fn assert_hover_matches_full(full: TargetDetailsResponse, summary: TargetDetailsResponse) {
        let (mut full, mut summary) = (full, summary);
        assert!(summary.skills.iter().all(|s| s.hit_timestamps.is_empty()));
        assert!(summary.heal_skills.is_empty());
        assert!(summary.ping_history.is_empty());
        full.heal_skills.clear();
        full.ping_history.clear();
        for skill in &mut full.skills { skill.hit_timestamps.clear(); }
        full.skills.sort_by_key(|s| (s.actor_id, s.code, s.is_dot));
        summary.skills.sort_by_key(|s| (s.actor_id, s.code, s.is_dot));
        assert_eq!(serde_json::to_value(full).unwrap(), serde_json::to_value(summary).unwrap());
    }

    #[test]
    fn hover_omits_timelines_without_changing_damage_or_actor_filtering() {
        let storage = Arc::new(DataStorage::new());
        storage.set_local_player_id(Some(2259));
        storage.append_damage(hit(2259, 50_000, 1_000));
        storage.append_damage(hit(2259, 50_000, 1_100));
        storage.append_damage(hit(2260, 50_000, 1_200));
        storage.append_damage(hit(2259, 50_001, 1_300));
        let mut dot = hit(2259, 50_000, 1_400);
        dot.set_dot(true);
        storage.append_damage(dot);
        let calc = meter(&storage);
        assert!(calc.get_hover_details(50_000, Some(&[2259])).skills.iter().any(|s| s.is_dot));
        assert!(!calc.get_target_details(50_000, Some(&[2259])).skills[0].hit_timestamps.is_empty());
        for actors in [None, Some(&[2259][..]), Some(&[2260][..]), Some(&[999][..])] {
            assert_hover_matches_full(calc.get_target_details(50_000, actors), calc.get_hover_details(50_000, actors));
        }
        assert_hover_matches_full(calc.get_target_details(99999, None), calc.get_hover_details(99999, None));
        // Reading the summary must not strip the stored timeline.
        assert_eq!(calc.get_target_details(50_000, Some(&[2259])).skills.iter()
            .find(|s| !s.is_dot).unwrap().hit_timestamps.len(), 2);
    }

    #[test]
    fn independent_details_reader_matches_live_calculator_and_ignores_unrelated_hits() {
        let storage = Arc::new(DataStorage::new());
        storage.set_local_player_id(Some(2259));
        storage.append_damage(hit(2259, 50_000, 1_000));
        let mut live = meter(&storage);
        live.get_dps();
        let reader = live.details_source().reader();
        assert_eq!(serde_json::to_value(reader.get_details_context()).unwrap(),
            serde_json::to_value(live.get_details_context()).unwrap());
        let before = serde_json::to_value(reader.get_target_details(50_000, Some(&[2259]))).unwrap();
        for target in 50_001..50_065 {
            for i in 0..100 { storage.append_damage(hit(2259, target, 2_000 + i)); }
        }
        assert_eq!(serde_json::to_value(reader.get_target_details(50_000, Some(&[2259]))).unwrap(), before);
        let snapshot = storage.get_target_snapshots(&[50_000], false, false);
        assert_eq!(snapshot[&50_000].actors.values().flat_map(|a| a.skills.values())
            .map(|s| s.hit_timestamps.len()).sum::<usize>(), 1);
        assert!(storage.get_target_snapshots(&[99999], false, false).is_empty());
        live.restart_target_selection(true);
        assert_eq!(reader.get_details_context().current_target_id, live.get_details_context().current_target_id);
    }

    #[test]
    fn a_details_reader_follows_the_meters_mode_and_rows() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, 1);
        spawn(&s, 900, BOSS);
        hits(&s, 2259, 800, 1_000, 10_000);
        hits(&s, 2259, 900, 20_000, 29_000);
        hits(&s, 2260, 801, 21_000, 25_000);
        hits(&s, 2259, 800, 30_000, 31_000);
        crate::clock::set_override(Some(31_000));
        let mut live = meter_with_npcs(&s);
        let source = live.details_source();
        // Equal, but for the order of skills (map order).
        let same = |mut a: TargetDetailsResponse, mut b: TargetDetailsResponse| {
            for d in [&mut a, &mut b] {
                d.skills.sort_by_key(|s| (s.actor_id, s.code, s.is_dot));
                d.heal_skills.sort_by_key(|s| (s.actor_id, s.code, s.is_dot));
            }
            assert_eq!(serde_json::to_value(a).unwrap(), serde_json::to_value(b).unwrap());
        };
        for mode in ["encounter", "allTargets", "bossTargets", "lastHitByMe"] {
            live.set_target_selection_mode(mode);
            live.get_dps();
            let reader = source.reader();
            if mode == "allTargets" {
                assert_eq!(reader.get_displayed_details(None).target_id, 0, "several targets, merged");
            }
            for actors in [None, Some(&[2259][..])] {
                same(reader.get_displayed_details(actors), live.get_displayed_details(actors));
                assert_hover_matches_full(live.get_displayed_details(actors), reader.get_displayed_hover_details(actors));
                for target in [800, 801, 900] {
                    same(reader.get_target_details(target, actors), live.get_target_details(target, actors));
                    assert_hover_matches_full(live.get_target_details(target, actors), reader.get_hover_details(target, actors));
                }
            }
        }
        live.set_target_selection_mode("encounter");
        live.get_dps();
        assert_eq!(source.reader().get_target_details(800, None).total_target_damage, 12 * 500,
            "the encounter's carry: the trash before the pull still counts");
        crate::clock::set_override(None);
    }

    /// The meter over a capture, as the live app drives it (stream assembler
    /// per connection, as `replay_report`): BOSS first, then A2_MODE from
    /// A2_SWITCH (hh:mm:ss). Every A2_EVERY seconds (default 5): the targets
    /// on screen, your row, and per target the damage you dealt yourself and
    /// through summons. A2_ME: your id, when the capture starts after the
    /// meter learned it.
    #[test]
    #[ignore]
    fn meter_targets_over_time() {
        use crate::capture::stream_assembler::StreamAssembler;
        use crate::capture::stream_processor::StreamProcessor;
        let path = std::env::var("A2_CAPTURE").unwrap();
        let mode = std::env::var("A2_MODE").unwrap_or("trainTargets".into());
        let switch = std::env::var("A2_SWITCH").unwrap();
        let every: i64 = std::env::var("A2_EVERY").ok().map_or(5, |v| v.parse().unwrap());
        let known_me: Option<i64> = std::env::var("A2_ME").ok().map(|v| v.parse().unwrap());
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data");
        let skills = Arc::new(SkillLookup::new());
        let npcs = Arc::new(NpcLookup::new());
        crate::i18n::lookup::load_language(&skills, &npcs, &data, "en");
        let dots: HashSet<i32> = serde_json::from_str::<Vec<i32>>(&std::fs::read_to_string(data.join("dot_skill_ids.json")).unwrap())
            .unwrap().into_iter().collect();
        let s = Arc::new(DataStorage::new());
        let mut calc = DpsCalculator::new(s.clone(), skills.clone(), npcs.clone(), Arc::new(PingTracker::new()));
        let mut streams: HashMap<String, (StreamAssembler, StreamProcessor)> = HashMap::new();
        let (mut last_sec, mut switched) = (0, false);
        for line in std::fs::read_to_string(&path).unwrap().lines() {
            let parts: Vec<&str> = line.splitn(3, '|').collect();
            if line.starts_with('#') || parts.len() != 3 { continue; }
            let Ok(when) = chrono::DateTime::parse_from_rfc3339(parts[0]) else { continue };
            let (ts, tod) = (when.timestamp_millis(), &parts[0][11..19]);
            crate::clock::set_override(Some(ts));
            if s.local_player_id().is_none() && known_me.is_some() { s.set_local_player_id(known_me); }
            if ts / 1000 != last_sec {
                last_sec = ts / 1000;
                if !switched && tod >= switch.as_str() {
                    switched = true;
                    calc.set_target_selection_mode(&mode);
                }
                let shown = calc.get_dps();
                if last_sec % every == 0 {
                    let me = s.local_player_id().map(|v| v as i32);
                    let summons = s.get_summon_data();
                    let mob = s.get_mob_data();
                    let mine = |a: i32| me.is_some_and(|m| summon_resolver::resolve(a, &summons) == m);
                    let mut per_target: Vec<String> = s.get_combat_snapshot_light().iter()
                        .filter(|(_, td)| td.actors.keys().any(|&a| mine(a)))
                        .map(|(t, td)| {
                            let own: i64 = td.actors.iter().filter(|(a, _)| Some(**a) == me).map(|(_, d)| d.total_damage).sum();
                            let pet: i64 = td.actors.iter().filter(|(a, _)| Some(**a) != me && mine(**a)).map(|(_, d)| d.total_damage).sum();
                            format!("{t}({}) own {own} summons {pet}", mob.get(t).copied().unwrap_or(0))
                        }).collect();
                    per_target.sort();
                    let (row, dps) = me.and_then(|m| shown.map.get(&m)).map_or((0.0, 0.0), |r| (r.amount, r.dps));
                    eprintln!("{tod} {} shown {:?} target {} '{}' rows {} you {me:?} row {row:.0} dps {dps:.0} time {} | {}", shown.target_mode,
                        calc.displayed_targets, shown.target_id, shown.target_name, shown.map.len(), shown.battle_time, per_target.join("; "));
                }
            }
            let bytes: Vec<u8> = (0..parts[2].len() / 2).filter_map(|i| u8::from_str_radix(&parts[2][2 * i..2 * i + 2], 16).ok()).collect();
            let (assembler, processor) = streams.entry(parts[1].to_string()).or_insert_with(|| {
                let mut p = StreamProcessor::new(s.clone(), skills.clone(), npcs.clone());
                p.set_dot_skill_ids(dots.clone());
                (StreamAssembler::new(), p)
            });
            processor.set_override_timestamp(Some(ts));
            assembler.process_chunk(&bytes, processor);
        }
        crate::clock::set_override(None);
        assert!(switched);
    }
}
