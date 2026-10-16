use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::RwLock;

use crate::capture::ping_tracker::PingTracker;
use crate::combat::data_storage::{DataStorage, TakenSums, TargetTotals};
use crate::entity::dps_data::DpsData;
use crate::i18n::lookup::{NpcLookup, SkillLookup};
use crate::entity::taken::TakenRow;
mod details;
mod fights;
mod meter_rows;
mod rows;
mod target;
#[cfg(test)]
mod test_kit;

pub use rows::{is_party_row_id, GROUP_ROW_FIRST_ID};
pub use target::TargetScope;

/// What the damage taken rows depend on: both generations, the mode, its
/// targets and its window start.
type TakenKey = (i64, i64, TargetScope, Vec<i32>, Option<i64>);

/// The meter: which targets are on screen and their rows (`target.rs`,
/// `meter_rows.rs`, `rows.rs`), Details answers (`details.rs`) and the
/// saved fights (`fights.rs`), all read from the shared combat data.
pub struct DpsCalculator {
    /// What the screen reads: the live storage, or while boss mode holds a
    /// fight a zone change cleared, the storage as it stood before.
    data_storage: Arc<DataStorage>,
    /// The live storage, which saving and idle retirement always read.
    live: Arc<DataStorage>,
    /// The screen boss mode holds (`follow_hold`).
    held: Option<DpsData>,
    /// The last screen built, for a hold to keep.
    last_shown: Option<DpsData>,
    skill_lookup: Arc<SkillLookup>,
    npc_lookup: Arc<NpcLookup>,
    pings: Arc<PingTracker>,
    current_target: i32,
    last_dps_snapshot: Option<DpsData>,
    /// What the Last N columns of that reading add up.
    rolling: Option<meter_rows::Rolling>,
    /// The live targets as of the last reading, without hit timelines
    /// (`DataStorage::refresh_brief`).
    live_copy: HashMap<i32, TargetTotals>,
    /// Each actor's part of the rows, as of the last reading.
    tallied: meter_rows::Tallied,
    last_damage_gen: i64,
    target_selection_mode: TargetScope,
    last_known_local_id: Option<i64>,
    all_targets_window_ms: i64,
    nickname_job_cache: HashMap<String, String>,
    displayed_targets: Vec<i32>,
    displayed_battle_time: i64,
    last_retire_ms: i64,
    taken_cache: Option<(TakenKey, Vec<TakenRow>)>,
    /// What those rows add up, kept between reads.
    taken_sums: TakenSums,
    /// Fights saved for good: (target, first hit) to their last hit then.
    for_good: HashMap<(i32, i64), i64>,
    /// What the meter last put on screen, for readers on other threads.
    screen: Arc<RwLock<OnScreen>>,
}

/// The part of the meter a Details reader starts from.
#[derive(Clone)]
struct OnScreen {
    storage: Arc<DataStorage>,
    mode: TargetScope,
    targets: Vec<i32>,
    battle_time: i64,
}

/// Hands out Details readers that never wait on the meter's lock: each one
/// shares the combat data and tables and copies what is on screen.
#[derive(Clone)]
pub struct DetailsSource {
    skills: Arc<SkillLookup>,
    npcs: Arc<NpcLookup>,
    pings: Arc<PingTracker>,
    screen: Arc<RwLock<OnScreen>>,
}

impl DetailsSource {
    /// A calculator in the meter's mode, on its targets and battle time
    /// as of now.
    pub fn reader(&self) -> DpsCalculator {
        let OnScreen { storage, mode, targets, battle_time } = self.screen.read().clone();
        let mut calc = DpsCalculator::new(storage, self.skills.clone(), self.npcs.clone(), self.pings.clone());
        calc.target_selection_mode = mode;
        calc.displayed_targets = targets;
        calc.displayed_battle_time = battle_time;
        calc.publish_view();
        calc
    }

    /// The storage the meter's screen reads now.
    pub fn storage(&self) -> Arc<DataStorage> {
        self.screen.read().storage.clone()
    }
}

impl DpsCalculator {
    pub fn details_source(&self) -> DetailsSource {
        DetailsSource {
            skills: self.skill_lookup.clone(),
            npcs: self.npc_lookup.clone(),
            pings: self.pings.clone(),
            screen: self.screen.clone(),
        }
    }

    pub fn new(
        data_storage: Arc<DataStorage>,
        skill_lookup: Arc<SkillLookup>,
        npc_lookup: Arc<NpcLookup>,
        ping_tracker: Arc<PingTracker>,
    ) -> Self {
        let screen = OnScreen { storage: data_storage.clone(), mode: TargetScope::BossTargets, targets: Vec::new(), battle_time: 0 };
        DpsCalculator {
            live: data_storage.clone(),
            held: None,
            last_shown: None,
            data_storage,
            skill_lookup,
            npc_lookup,
            pings: ping_tracker,
            current_target: 0,
            last_dps_snapshot: None,
            rolling: None,
            live_copy: HashMap::new(),
            tallied: meter_rows::Tallied::default(),
            last_damage_gen: -1,
            target_selection_mode: screen.mode,
            last_known_local_id: None,
            all_targets_window_ms: 0,
            nickname_job_cache: HashMap::new(),
            displayed_targets: Vec::new(),
            displayed_battle_time: 0,
            last_retire_ms: i64::MIN,
            taken_cache: None,
            taken_sums: TakenSums::default(),
            for_good: HashMap::new(),
            screen: Arc::new(RwLock::new(screen)),
        }
    }

    /// The meter's rows with their damage taken and deaths. Idle targets go
    /// after the rows, so a mode just switched to has chosen its own first.
    pub fn get_dps(&mut self) -> DpsData {
        self.follow_hold();
        let shown = match &self.held {
            Some(held) => held.clone(),
            None => {
                let mut shown = self.compute_dps();
                shown.taken = self.taken_rows(&shown);
                shown.deaths = self.death_rows(&shown);
                self.last_shown = Some(shown.clone());
                shown
            }
        };
        self.retire_idle_targets();
        self.publish_view();
        shown
    }

    /// Boss mode keeps a boss fight a zone change cleared (a revive after a
    /// wipe) on screen as it was until you or your party hit a boss again;
    /// Details reads the storage as it stood before the change. Saving and
    /// idle retirement stay on the live storage. It stays until the next pull
    /// so the party can go over the wipe; boss mode only, as TARGET mode
    /// shows the trash.
    fn follow_hold(&mut self) {
        // The reset first: storage keeps its copy before it asks the reset.
        let reset = self.live.reset_was_asked();
        if let Some(before) = self.live.take_before_zone_change() {
            if self.held.is_none() && self.target_selection_mode == TargetScope::BossTargets {
                self.hold(before);
            }
        }
        if self.held.is_some() {
            if self.live.boss_pulled() {
                self.forget_reading(true);
            }
        } else if reset {
            self.forget_reading(true);
        }
    }

    /// Holds the last screen, read from `before`, when it shows damage. The
    /// screen itself, not a reading from `before`: it can keep the rows of a
    /// fight already cleared, and a fresh choice of boss long after the last
    /// hit can fall on another fight.
    fn hold(&mut self, before: Arc<DataStorage>) {
        let Some(screen) = self.last_shown.clone().filter(|s| s.map.values().any(|row| row.amount > 0.0)) else {
            return;
        };
        self.data_storage = before;
        self.held = Some(screen);
    }

    /// Tell Details readers what the meter shows now.
    fn publish_view(&self) {
        *self.screen.write() = OnScreen {
            storage: self.data_storage.clone(),
            mode: self.target_selection_mode,
            targets: self.displayed_targets.clone(),
            battle_time: self.displayed_battle_time,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    use crate::entity::fight_record::FightRecord;
    use crate::entity::mark_kinds::HitMark;
    use crate::entity::owner_chain;

    use super::meter_rows::active_time;
    use super::target::TARGET_HOLD_MS;
    use super::test_kit::{hit, land, meter, meter_on, read_checked, skill_hit, GLADIATOR_SKILL, SORCERER_SKILL};
    use crate::entity::detail_bundle::{SkillLine, TargetBreakdownReply};
    use crate::combat::data_storage::UNATTRIBUTED_ID;

    const BOSS: i32 = 700;
    const DUMMY: i32 = 701;

    /// A meter whose NPC table knows one boss and one training dummy.
    fn meter_with_npcs(storage: &Arc<DataStorage>) -> DpsCalculator {
        meter_on(storage, r#"{"700":{"name":"Boss","isBoss":true},"701":{"name":"Training Scarecrow"}}"#)
    }

    fn spawn(storage: &DataStorage, id: i32, code: i32) {
        storage.append_mob(id, code);
        if code == BOSS {
            storage.register_boss(id);
        } else if code == DUMMY {
            storage.flag_dummy(id);
        }
    }

    /// One hit a second from `actor` on `target`, `from` to `to` inclusive.
    fn hits(storage: &DataStorage, actor: i32, target: i32, from: i64, to: i64) {
        let mut at = from;
        while at <= to {
            land(storage, actor, target, at, GLADIATOR_SKILL, 500);
            at += 1_000;
        }
    }

    fn snapshot_at(calc: &mut DpsCalculator, now: i64) -> Vec<FightRecord> {
        crate::clock::set_override(Some(now));
        calc.close_boss_fights()
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
    fn a_wipe_and_the_next_pull_of_one_boss_are_two_fights() {
        // The server's combat records for a boss pulled again after a
        // wipe: into combat with an enrage deadline, out at the wipe, back
        // in with a new deadline 10 s later.
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 801, BOSS);
        let mut calc = meter_with_npcs(&s);
        s.note_combat(801, true, Some(300_500), 500);
        hits(&s, 2259, 801, 1_000, 60_000);
        s.note_combat(801, false, None, 60_100);
        let wiped = calc.get_dps();
        assert!(wiped.target_out_of_combat, "the wipe ends the fight");
        assert_eq!((wiped.battle_time, wiped.target_enrage_ms), (59_000, 299_500));
        s.note_combat(801, true, Some(370_000), 70_000);
        assert!(calc.get_dps().target_out_of_combat, "still the wipe's fight on screen");
        hits(&s, 2259, 801, 72_000, 80_000);
        let pull = calc.get_dps();
        assert!(!pull.target_out_of_combat);
        assert_eq!((pull.battle_time, pull.target_enrage_ms), (8_000, 298_000));
        let saved: Vec<(i64, i64)> = snapshot_at(&mut calc, 200_000).iter().map(|r| (r.start_time_ms, r.duration_ms)).collect();
        assert_eq!(saved, [(1_000, 59_000), (72_000, 8_000)]);
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
        s.enter_dungeon(600002);
        hits(&s, 2259, 800, 1_000, 8_000);
        crate::clock::set_override(Some(12_000));
        assert!(s.on_zone_switch(), "the load out of the instance");
        assert_eq!(s.current_dungeon_id(), 600002, "a load alone does not say where to");
        s.note_map_load(1010);
        assert_eq!(s.current_dungeon_id(), 0, "World_L_A is open world");
        let mut saved = snapshot_at(&mut calc, 12_000);
        hits(&s, 2259, 900, 20_000, 30_000);
        crate::clock::set_override(Some(31_000));
        saved.extend(calc.close_boss_fights_now());
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
        s.enter_dungeon(600002);
        hits(&s, 2259, 800, 4_000, 8_000);
        crate::clock::set_override(Some(9_000));
        assert_eq!(dungeon_of(&calc.close_boss_fights_now(), "auto_800_1000"), 600002);
        crate::clock::set_override(None);
    }

    #[test]
    fn a_teleport_during_a_fight_in_an_instance_keeps_the_dungeon() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        s.enter_dungeon(600002);
        hits(&s, 2259, 800, 1_000, 5_000);
        crate::clock::set_override(Some(5_500));
        assert!(!s.on_zone_switch(), "mid-fight: no combat reset");
        s.note_map_load(600002);
        assert_eq!(s.current_dungeon_id(), 600002, "the load names the instance again");
        hits(&s, 2259, 800, 6_000, 9_000);
        crate::clock::set_override(Some(10_000));
        assert_eq!(dungeon_of(&calc.close_boss_fights_now(), "auto_800_1000"), 600002);
        crate::clock::set_override(None);
    }

    #[test]
    fn a_boss_after_a_teleport_inside_an_instance_keeps_the_dungeon() {
        // Dungeon 600011: a teleport to the last boss room, the boss
        // killed before the roster names the instance again.
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        s.enter_dungeon(600011);
        crate::clock::set_override(Some(1_000));
        s.on_zone_switch();
        s.note_map_load(600011);
        hits(&s, 2259, 800, 2_000, 30_000);
        crate::clock::set_override(Some(31_000));
        assert_eq!(dungeon_of(&calc.close_boss_fights_now(), "auto_800_2000"), 600011);
        crate::clock::set_override(None);
    }

    #[test]
    fn world_layers_are_open_world_and_seals_and_the_abyss_are_not() {
        assert!(crate::combat::data_storage::is_open_world_map(1010), "World_L_A");
        assert!(crate::combat::data_storage::is_open_world_map(101021), "a layer of World_L_A");
        assert!(!crate::combat::data_storage::is_open_world_map(20), "Chaotic Lower Reshanta");
        assert!(!crate::combat::data_storage::is_open_world_map(310051), "Seal_Verteron_051");
        assert!(!crate::combat::data_storage::is_open_world_map(600021), "Fire_Temple_Easy");
        assert!(!crate::combat::data_storage::is_open_world_map(999_999_999), "unknown map");

        let s = DataStorage::new();
        s.enter_dungeon(600021);
        s.note_map_load(310051);
        assert_eq!(s.current_dungeon_id(), 310051, "a seal is its own dungeon");
        s.note_map_load(101021);
        assert_eq!(s.current_dungeon_id(), 0, "a world layer ends it");
        // The Abyss's three zones are dungeons 21, 23 and 24 on maps 20, 22
        // and 23, entered from the open world or a party dungeon.
        s.note_map_load(600021);
        s.enter_dungeon(600021);
        for (map, dungeon) in [(20, 21), (22, 23), (23, 24)] {
            s.note_map_load(map);
            assert_eq!(s.current_dungeon_id(), dungeon, "Abyss map {map}");
            s.note_map_load(1010);
            assert_eq!(s.current_dungeon_id(), 0);
        }
    }

    #[test]
    fn a_fight_in_a_sealed_dungeon_is_filed_under_it() {
        // Boss 2701096, fought in Seal_Verteron_051 with no party roster,
        // was saved as open world.
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        s.note_map_load(1010);
        s.note_map_load(310051);
        assert_eq!(s.current_dungeon_id(), 310051);
        s.enter_dungeon(600021);
        assert_eq!(s.current_dungeon_id(), 310051, "the party's queue is not the seal");
        hits(&s, 2259, 800, 1_000, 8_000);
        crate::clock::set_override(Some(12_000));
        s.note_map_load(1010);
        assert_eq!(s.current_dungeon_id(), 0);
        assert!(s.on_zone_switch());
        assert_eq!(dungeon_of(&snapshot_at(&mut calc, 12_000), "auto_800_1000"), 310051);
        s.note_map_load(600021);
        assert_eq!(s.current_dungeon_id(), 600021, "the queue, at the load into it");
        crate::clock::set_override(None);
    }

    #[test]
    fn a_queued_dungeon_waits_through_an_arena() {
        let s = DataStorage::new();
        s.note_map_load(1010);
        s.enter_dungeon(600002);
        s.note_map_load(61);
        assert_eq!(s.current_dungeon_id(), 0, "Impetusium Arena is not Krao Cave");
        s.enter_dungeon(600002);
        assert_eq!(s.current_dungeon_id(), 0, "nor when the roster comes again there");
        s.note_map_load(1010);
        s.note_map_load(600002);
        assert_eq!(s.current_dungeon_id(), 600002);
        // A dungeon on a map of another id.
        s.note_map_load(1010);
        s.enter_dungeon(600151);
        s.note_map_load(600144);
        assert_eq!(s.current_dungeon_id(), 600151, "Citadel of the Fallen Daeva");
        s.note_map_load(600144);
        s.enter_dungeon(600151);
        assert_eq!(s.current_dungeon_id(), 600151, "a teleport inside it");
    }

    #[test]
    fn every_instance_entered_without_a_roster_is_filed_under_its_map() {
        let s = DataStorage::new();
        // Daily, Ascension, Ascension Trial (Awaken) and Nightmare (BossChallenge).
        for map in [900001, 200002, 910011, 200003] {
            s.note_map_load(1010);
            s.note_map_load(map);
            assert_eq!(s.current_dungeon_id(), map);
        }
        // A party's Subjugation map is named by its roster, not its map.
        s.note_map_load(1010);
        s.note_map_load(156000);
        assert_eq!(s.current_dungeon_id(), 0);
    }

    #[test]
    fn a_queued_dungeon_waits_for_the_load_into_it() {
        // A queue pops: the roster names the dungeon (600002) a while
        // before the load into it, while the party is still in the open
        // world.
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        s.note_map_load(1010);
        s.enter_dungeon(600002);
        assert_eq!(s.current_dungeon_id(), 0, "queued, still in the open world");
        hits(&s, 2259, 800, 1_000, 8_000);
        s.note_map_load(1010);
        assert_eq!(s.current_dungeon_id(), 0, "a load inside the open world");
        s.note_map_load(600002);
        assert_eq!(s.current_dungeon_id(), 600002, "the load into it");
        crate::clock::set_override(Some(9_000));
        assert_eq!(dungeon_of(&calc.close_boss_fights_now(), "auto_800_1000"), 0);
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
        s.enter_dungeon(600002);
        crate::clock::set_override(Some(20_000));
        s.note_map_load(600002);
        assert!(s.on_zone_switch(), "the load ends the fight");
        assert_eq!(dungeon_of(&snapshot_at(&mut calc, 20_000), "auto_800_1000"), 0);
        crate::clock::set_override(None);
    }

    #[test]
    fn enc_tank_tab_counts_damage_taken_from_every_target_of_the_encounter() {
        use crate::entity::taken::{TakenHit, TakenKind};
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, 1);
        spawn(&s, 810, 1);
        hits(&s, 2259, 800, 1_000, 3_000);
        hits(&s, 2259, 810, 2_000, 4_000);
        // Each mob hits you once; Details would open on one of them.
        s.append_taken(TakenHit::plain(2_500, 2259, 800, 1_800_510, 300, TakenKind::Hit));
        s.append_taken(TakenHit::plain(3_500, 2259, 810, 1_800_510, 200, TakenKind::Hit));
        let mut calc = meter_with_npcs(&s);
        calc.set_target_selection_mode("encounter");
        let _ = calc.get_dps();
        let context = calc.get_details_context();
        let target = calc.details_opening_target(&context, 0, true, false);
        assert!(target.is_some(), "Details itself opens on one target in ENC");
        let party = calc.party_view_from(&context, target);
        let you = party.tanks.iter().find(|t| t.player.id == 2259).expect("you on the Tank tab");
        assert_eq!(you.player.amount, 500, "the Tank tab takes both mobs of the encounter");
        assert_eq!(you.attacks, 2);
        crate::clock::set_override(None);
    }

    #[test]
    fn enc_names_a_player_seen_only_in_an_earlier_pull() {
        // A Sorcerer fought a boss's earlier pulls but not the kill pull.
        // With each pull its own fight, ENC's Heal and Tank tabs showed that
        // player as an id, with no class.
        use crate::entity::job_class::JobClass;
        use crate::entity::taken::{TakenHit, TakenKind};
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        s.name_entity(2259, "Me_a");
        s.name_entity(3000, "Sorc_a");
        spawn(&s, 801, BOSS);
        s.note_combat(801, true, None, 500);
        hits(&s, 2259, 801, 1_000, 5_000);
        land(&s, 3000, 801, 2_000, SORCERER_SKILL, 500);
        s.add_heal_event(3000, 3000, 15_010_000, 100, false, 3_000);
        s.append_taken(TakenHit::plain(3_500, 3000, 801, 1_800_510, 200, TakenKind::Hit));
        s.note_combat(801, false, None, 6_000);
        s.note_combat(801, true, None, 10_000);
        hits(&s, 2259, 801, 11_000, 15_000);
        let mut calc = meter_with_npcs(&s);
        calc.set_target_selection_mode("encounter");
        let _ = calc.get_dps();
        for context in [calc.party_context(), calc.get_details_context()] {
            let party = calc.party_view_from(&context, None);
            let heal = &party.healers.iter().find(|p| p.player.id == 3000).expect("the earlier pull's healing").player;
            let tank = &party.tanks.iter().find(|t| t.player.id == 3000).expect("the earlier pull's damage taken").player;
            for p in [heal, tank] {
                assert_eq!((p.name.as_str(), p.job.as_str()), ("Sorc_a", JobClass::Sorcerer.class_name()));
            }
        }
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
        assert!(s.on_zone_switch());
        assert!(s.current_encounter().is_none());

        hits(&s, 2259, 800, 20_000, 21_000);
        let mut calc = meter_with_npcs(&s);
        calc.reset_focus(true);
        assert!(s.current_encounter().is_none());
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
        crit.set_mark_list(vec![HitMark::Critical]);
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

    const MODES: [&str; 7] = ["bossTargets", "mostDamage", "mostRecent", "lastHitByMe", "allTargets", "trainTargets", "encounter"];

    /// The rows with damage in `mode`, by id.
    fn rows_in(calc: &mut DpsCalculator, mode: &str) -> Vec<(i32, i64)> {
        calc.set_target_selection_mode(mode);
        rows_with_damage(&calc.get_dps())
    }

    /// The (row id, amount) of every row with damage, by id.
    fn rows_with_damage(shown: &DpsData) -> Vec<(i32, i64)> {
        let mut rows: Vec<(i32, i64)> = shown.map.iter().filter(|(_, r)| r.amount > 0.0).map(|(&id, r)| (id, r.amount as i64)).collect();
        rows.sort();
        rows
    }

    /// Reads the meter at `at`, checked against a reading built afresh. 1
    /// when it had rows.
    fn checked_at(calc: &mut DpsCalculator, at: i64) -> usize {
        crate::clock::set_override(Some(at));
        usize::from(!read_checked(calc).map.is_empty())
    }

    /// Hits on `target` every 700 ms from `from` to `to`: you, your spirit
    /// 500 and players 200 and 300, 100 ms apart, then a hit you take after
    /// the last of them. The meter reads at the end of each round.
    fn rounds(s: &DataStorage, calc: &mut DpsCalculator, target: i32, from: i64, to: i64) -> usize {
        use crate::entity::taken::{TakenHit, TakenKind};
        let hitters = [(100, GLADIATOR_SKILL), (500, 16_010_000), (200, 15_010_000), (300, 15_010_000)];
        let mut with_rows = 0;
        for at in (from..to).step_by(700) {
            for (n, (actor, skill)) in hitters.into_iter().enumerate() {
                land(s, actor, target, at + 100 * n as i64, skill, 100 + at / 1_000 + i64::from(actor));
            }
            s.append_taken(TakenHit::plain(at + 400, 100, 800, 1_800_510, 50, TakenKind::Hit));
            with_rows += checked_at(calc, at + 450);
        }
        with_rows
    }

    /// A fight read in `mode` (ALL with a window of `window_ms`), every
    /// reading checked against one built afresh. How many had rows.
    fn read_through_a_fight(mode: &str, window_ms: i64) -> usize {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(100));
        for (id, name) in [(100, "Me"), (200, "Ally"), (300, "Other")] {
            s.name_entity(id, name);
        }
        spawn(&s, 800, BOSS);
        spawn(&s, 900, DUMMY);
        land(&s, 500, 100, 500, 16_990_002, 20);
        let mut calc = meter_with_npcs(&s);
        calc.set_all_targets_window_ms(window_ms);
        calc.set_target_selection_mode(mode);
        let mut with_rows = rounds(&s, &mut calc, 800, 1_000, 20_000);
        // Player 300 alone hits, then nobody: the rows leave the Last N windows.
        for at in (20_000..33_000).step_by(1_300) {
            land(&s, 300, 800, at, 15_010_000, 300);
            with_rows += checked_at(&mut calc, at + 50);
        }
        with_rows += [36_000, 41_500, 55_000, 89_000, 91_000].map(|at| checked_at(&mut calc, at)).iter().sum::<usize>();
        // You move on to the dummy, then the spirit moves to player 200.
        with_rows += rounds(&s, &mut calc, 900, 92_000, 96_000);
        land(&s, 200, 500, 96_100, 16_770_000, 197);
        with_rows += rounds(&s, &mut calc, 900, 96_200, 99_000);
        // Player 300 comes back under another id, which takes its rows.
        land(&s, 301, 900, 99_100, 15_010_000, 400);
        with_rows += checked_at(&mut calc, 99_200);
        s.name_entity(301, "Other");
        with_rows += checked_at(&mut calc, 99_300);
        // Idle targets retired between two reads, a zone change, then the
        // dummy, a boss pull that keeps it for the encounter, and the dummy
        // again. Last, a reset and a new fight.
        crate::clock::set_override(Some(105_000));
        calc.get_dps();
        checked_at(&mut calc, 105_000);
        assert!(s.on_zone_switch());
        checked_at(&mut calc, 105_100);
        for (target, from) in [(900, 106_000), (800, 107_500), (900, 109_000)] {
            with_rows += rounds(&s, &mut calc, target, from, from + 1_500);
        }
        calc.reset_combat();
        checked_at(&mut calc, 111_000);
        with_rows + rounds(&s, &mut calc, 800, 112_000, 115_000)
    }

    #[test]
    fn a_reading_built_on_the_last_is_a_reading_built_afresh() {
        for mode in MODES {
            assert!(read_through_a_fight(mode, 0) > 5, "{mode}");
        }
        assert!(read_through_a_fight("allTargets", 10_000) > 5);
        crate::clock::set_override(None);
    }

    /// An enemy Assassin (14587) hit
    /// you five times, and the meter counted the hits as the enemy's
    /// healing, and your attack on them as yours.
    #[test]
    fn an_attack_on_a_player_is_never_healing() {
        use crate::combat::data_storage::RosterEntry;
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(14256));
        s.name_entity(14256, "Me");
        s.name_entity(101, "Cleric");
        s.replace_roster(vec![("Me".into(), RosterEntry::default()), ("Cleric".into(), RosterEntry::default())], true);
        spawn(&s, 900, DUMMY);
        s.append_damage(skill_hit(14256, 900, 1_000, 16_010_000, 1_000));
        s.append_damage(skill_hit(101, 900, 1_000, 17_010_000, 1_000));
        for (t, skill) in [(2_000, 13_070_000), (2_300, 13_130_000), (2_400, 13_770_000), (2_500, 13_350_000), (3_500, 13_220_000)] {
            s.append_damage(skill_hit(14587, 14256, t, skill, 2_000));
        }
        s.append_damage(skill_hit(14256, 14587, 4_000, 16_010_000, 1_500));
        // The party's Cleric heals you: still healing.
        s.append_damage(skill_hit(101, 14256, 4_500, 18_120_000, 700));
        s.append_damage(skill_hit(14256, 900, 5_000, 16_010_000, 1_000));

        let heals = s.heals_between(0, 10_000);
        let healers: Vec<i32> = heals.keys().copied().collect();
        assert_eq!(healers, vec![101]);
        assert_eq!(heals[&101][&(18_120_000, false)].total_heal, 700);
        let snap = s.copy_targets();
        assert_eq!(snap[&900].actors[&14256].party_heal, 0, "your attack on the enemy");
        assert_eq!(snap[&900].actors[&101].party_heal, 700);
        assert!(s.is_enemy(14587));
    }

    #[test]
    fn a_player_who_becomes_an_enemy_leaves_the_rows_they_were_on() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        s.note_player_spawn(3000);
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        // Only a stranger on the meter. ALL keeps the last rows on screen
        // when nothing is left to show, but not theirs.
        s.note_player_spawn(3002);
        hits(&s, 3002, 801, 1_000, 3_000);
        crate::clock::set_override(Some(3_500));
        assert_eq!(rows_in(&mut calc, "allTargets"), vec![(3002, 1_500)]);
        s.append_damage(skill_hit(3002, 2259, 3_600, 11_010_000, 100));
        assert_eq!(rows_in(&mut calc, "allTargets"), vec![]);
        // You and the stranger on your boss, then a teleport: the fight
        // waits for the auto-save.
        hits(&s, 2259, 800, 4_000, 12_000);
        hits(&s, 3000, 800, 2_000, 13_000);
        s.add_heal_event(3000, 3000, 17_120_000, 300, false, 6_000);
        crate::clock::set_override(Some(20_000));
        assert!(s.on_zone_switch());
        hits(&s, 2259, 800, 30_000, 40_000);
        hits(&s, 3000, 800, 31_000, 41_000);
        crate::clock::set_override(Some(41_500));
        assert_eq!(rows_in(&mut calc, "bossTargets"), vec![(2259, 5_500), (3000, 5_500)]);

        crate::clock::set_override(Some(42_000));
        s.append_damage(skill_hit(3000, 2259, 42_000, 11_010_000, 100));
        for mode in MODES {
            assert!(!rows_in(&mut calc, mode).iter().any(|&(id, _)| id == 3000), "{mode}");
        }
        assert_eq!(rows_in(&mut calc, "bossTargets"), vec![(2259, 5_500)]);
        let fight = calc.target_breakdown(800, None);
        assert!(fight.skills.iter().all(|k| k.actor_id == 2259));
        assert_eq!(fight.battle_time, 10_000, "your time, not the enemy's");
        let saved = calc.close_boss_fights_now();
        assert_eq!(ids(&saved), vec!["auto_800_30000", "auto_800_4000"]);
        for r in &saved {
            assert!(r.actors.iter().all(|a| a.actor_id == 2259), "{}", r.id);
            assert!(r.details.heal_skills.is_empty(), "{}: no enemy healing", r.id);
        }
        let totals: Vec<(i64, i64)> = saved.iter().map(|r| (r.start_time_ms, r.total_damage)).collect();
        assert_eq!(totals, vec![(4_000, 4_500), (30_000, 5_500)], "the ended fight first");
        crate::clock::set_override(None);
    }

    #[test]
    fn whoever_attacks_your_party_is_an_enemy() {
        use crate::combat::data_storage::RosterEntry;
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        s.name_entity(2259, "Me");
        s.name_entity(101, "Mate");
        s.replace_roster(vec![("Me".into(), RosterEntry::default()), ("Mate".into(), RosterEntry::default())], true);
        s.note_player_spawn(3000);
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        hits(&s, 101, 800, 1_000, 6_000);
        hits(&s, 3000, 800, 1_000, 6_000);
        taken(&s, 800, 101, 2_000, 40);
        taken(&s, 800, 3000, 3_000, 50);
        crate::clock::set_override(Some(6_500));
        assert_eq!(rows_in(&mut calc, "bossTargets"), vec![(101, 3_000), (3000, 3_000)]);
        let taken_ids = |calc: &mut DpsCalculator| calc.get_dps().taken.iter().map(|r| r.actor_id).collect::<Vec<i32>>();
        assert_eq!(taken_ids(&mut calc), vec![3000, 101]);

        // The stranger attacks your party member; your party member hits back.
        s.append_damage(skill_hit(3000, 101, 7_000, 11_020_000, 800));
        crate::clock::set_override(Some(7_500));
        assert_eq!(rows_in(&mut calc, "bossTargets"), vec![(101, 3_000)]);
        assert_eq!(taken_ids(&mut calc), vec![101], "the enemy's damage taken goes too");
        assert!(s.is_enemy(3000));

        // A stranger the party member hits is one too.
        s.note_player_spawn(3001);
        hits(&s, 3001, 800, 8_000, 9_000);
        s.append_damage(skill_hit(101, 3001, 9_500, 11_020_000, 800));
        crate::clock::set_override(Some(10_000));
        assert_eq!(rows_in(&mut calc, "bossTargets"), vec![(101, 3_000)]);
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
        calc.reset_focus(true);
        calc.get_dps();
        assert!(calc.get_displayed_details(Some(&[2259])).skills.is_empty());
        crate::clock::set_override(None);
    }

    #[test]
    fn a_fight_cleared_by_a_zone_load_keeps_who_was_who() {
        let s = Arc::new(DataStorage::new());
        s.set_local_identity_from_game(3197, Some("Corvane".into()));
        s.name_entity(3197, "Corvane");
        s.link_pet_owner(3197, 21821);
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        hits(&s, 3197, 800, 1_000, 8_000);
        hits(&s, 21821, 800, 1_000, 8_000);
        crate::clock::set_override(Some(12_000));
        assert!(s.on_zone_switch());
        // The load gives you a new id, and your spirit's id to a new spirit.
        s.set_local_identity_from_game(6207, Some("Corvane".into()));
        s.name_entity(6207, "Corvane");
        s.link_pet_owner(6207, 21821);
        let saved = snapshot_at(&mut calc, 12_000);
        assert_eq!(ids(&saved), vec!["auto_800_1000"]);
        let actors: Vec<_> = saved[0].actors.iter().map(|a| (a.actor_id, a.nickname.as_str())).collect();
        assert_eq!(actors, vec![(3197, "Corvane")]);
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
        assert!(s.on_zone_switch());
        s.set_local_player_id(Some(3197));
        assert_eq!(ids(&snapshot_at(&mut calc, 12_000)), vec!["auto_800_1000"]);
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
        s.note_player_record(9000);
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
        s.name_entity(100, "Owner");
        for id in [36734, 36735, 36736] {
            spawn(&s, id, DUMMY);
        }
        // Your spirit, linked to you.
        s.remember_pet_spawn(500);
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
        s.remember_pet_spawn(500);
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

        s.note_death(801);
        assert_eq!(calc.get_dps().target_id, 801, "dead, and still the last thing you hit");
        hits(&s, 2259, 900, 30_000, 31_000);
        assert_eq!(calc.get_dps().target_id, 900, "dead, and you moved on");
        crate::clock::set_override(None);
    }

    #[test]
    fn boss_mode_holds_a_boss_fight_a_zone_load_cleared_until_the_next_boss_pull() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        s.name_entity(2259, "Tank_a");
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        hits(&s, 2259, 800, 1_000, 8_000);
        let before = calc.get_dps();
        assert_eq!((before.target_id, rows_with_damage(&before)), (800, vec![(2259, 4_000)]));

        // The wipe, then the revive's zone load: a new id for you.
        crate::clock::set_override(Some(30_000));
        assert!(s.on_zone_switch());
        s.set_local_player_id(Some(6207));
        s.name_entity(6207, "Tank_a");
        let held = calc.get_dps();
        assert_eq!((held.target_id, rows_with_damage(&held)), (800, vec![(2259, 4_000)]), "the fight stays");
        let details = calc.details_source().reader().get_displayed_details(None);
        assert_eq!(details.total_target_damage, 4_000, "Details reads the fight from before the load");

        // Trash on the way back, and another load: still held.
        spawn(&s, 900, 1);
        hits(&s, 6207, 900, 40_000, 42_000);
        assert_eq!(rows_with_damage(&calc.get_dps()), vec![(2259, 4_000)], "trash is target mode's");
        crate::clock::set_override(Some(60_000));
        assert!(s.on_zone_switch());
        assert_eq!(rows_with_damage(&calc.get_dps()), vec![(2259, 4_000)]);

        // The next boss pull ends it: the new fight alone.
        spawn(&s, 801, BOSS);
        hits(&s, 6207, 801, 70_000, 72_000);
        let after = calc.get_dps();
        assert_eq!((after.target_id, rows_with_damage(&after)), (801, vec![(6207, 1_500)]));
        assert_eq!(calc.details_source().reader().get_displayed_details(None).total_target_damage, 1_500);
        crate::clock::set_override(None);
    }

    #[test]
    fn the_hold_keeps_the_boss_on_screen_not_a_fresh_choice() {
        // A fresh choice long after the
        // last hit took another boss, hit later but far less.
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        spawn(&s, 801, BOSS);
        let mut calc = meter_with_npcs(&s);
        hits(&s, 2259, 800, 1_000, 20_000);
        hits(&s, 2259, 801, 21_000, 21_000);
        let before = calc.get_dps();
        assert_eq!((before.target_id, rows_with_damage(&before)), (800, vec![(2259, 10_000)]));
        crate::clock::set_override(Some(60_000));
        assert!(s.on_zone_switch());
        let held = calc.get_dps();
        assert_eq!((held.target_id, rows_with_damage(&held)), (800, vec![(2259, 10_000)]));
        crate::clock::set_override(None);
    }

    #[test]
    fn a_held_boss_fight_is_saved_once_from_the_live_data() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        hits(&s, 2259, 800, 1_000, 8_000);
        calc.get_dps();
        crate::clock::set_override(Some(30_000));
        assert!(s.on_zone_switch());
        assert_eq!(rows_with_damage(&calc.get_dps()), vec![(2259, 4_000)], "held");
        assert_eq!(ids(&snapshot_at(&mut calc, 31_000)), vec!["auto_800_1000"]);
        assert!(snapshot_at(&mut calc, 62_000).is_empty(), "not again from the copy");
        assert_eq!(rows_with_damage(&calc.get_dps()), vec![(2259, 4_000)], "still held after saving");
        crate::clock::set_override(None);
    }

    #[test]
    fn only_boss_mode_holds_and_a_mode_pick_or_a_reset_ends_the_hold() {
        let fought = || {
            let s = Arc::new(DataStorage::new());
            s.set_local_player_id(Some(2259));
            spawn(&s, 800, BOSS);
            spawn(&s, 900, 1);
            hits(&s, 2259, 800, 1_000, 8_000);
            s
        };
        let load = |s: &DataStorage| {
            crate::clock::set_override(Some(30_000));
            assert!(s.on_zone_switch());
        };
        for mode in ["mostDamage", "encounter", "allTargets"] {
            let s = fought();
            let mut calc = meter_with_npcs(&s);
            calc.set_target_selection_mode(mode);
            calc.get_dps();
            load(&s);
            assert!(rows_with_damage(&calc.get_dps()).is_empty(), "{mode} clears as before");
        }

        // Boss mode on trash alone: nothing to hold.
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 900, 1);
        let mut calc = meter_with_npcs(&s);
        hits(&s, 2259, 900, 1_000, 8_000);
        assert_eq!(rows_with_damage(&calc.get_dps()), vec![(2259, 4_000)]);
        load(&s);
        assert!(rows_with_damage(&calc.get_dps()).is_empty(), "no boss was pulled");

        let s = fought();
        let mut calc = meter_with_npcs(&s);
        calc.get_dps();
        load(&s);
        assert!(!rows_with_damage(&calc.get_dps()).is_empty());
        calc.set_target_selection_mode("mostDamage");
        assert!(rows_with_damage(&calc.get_dps()).is_empty(), "another mode reads the live data");
        calc.set_target_selection_mode("bossTargets");
        assert!(rows_with_damage(&calc.get_dps()).is_empty(), "and the hold is gone");

        let s = fought();
        let mut calc = meter_with_npcs(&s);
        calc.get_dps();
        load(&s);
        assert!(!rows_with_damage(&calc.get_dps()).is_empty());
        calc.reset_combat();
        assert!(rows_with_damage(&calc.get_dps()).is_empty(), "Save & Clear ends it");
        crate::clock::set_override(None);
    }

    #[test]
    fn boss_mode_keeps_your_boss_through_spill_over_and_a_strangers_hits() {
        // Two boss-flagged scarecrows
        // side by side, flipping several times a second.
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(100));
        s.name_entity(100, "Owner");
        for id in [801, 802] {
            spawn(&s, id, BOSS);
        }
        s.remember_pet_spawn(500);
        s.append_damage(skill_hit(500, 100, 500, 16_990_002, 20));
        let mut calc = meter_with_npcs(&s);
        let mut shown = Vec::new();
        let mut tick = |at: i64, calc: &mut DpsCalculator| {
            crate::clock::set_override(Some(at));
            shown.push(calc.get_dps().target_id);
        };

        // You on 801; your spirit's area hits reach 802, where a stranger
        // fights and so hits last.
        for at in (1_000..=30_000).step_by(250) {
            crate::clock::set_override(Some(at));
            s.append_damage(skill_hit(100, 801, at, 16_010_000, 1_000));
            if at % 2_000 == 0 {
                s.append_damage(skill_hit(500, 801, at, 16_110_004, 200));
                s.append_damage(skill_hit(500, 802, at, 16_110_004, 200));
            }
            land(&s, 9000, 802, at + 50, 11_010_000, 3_000);
            tick(at + 50, &mut calc);
        }
        // You move on to 802.
        for at in (30_250..=45_000).step_by(250) {
            land(&s, 100, 802, at, 16_010_000, 1_000);
            tick(at, &mut calc);
        }
        // One area skill hits both in the same ms for the same damage.
        for at in (45_250..=60_000).step_by(250) {
            crate::clock::set_override(Some(at));
            s.append_damage(skill_hit(100, 801, at, 16_020_000, 800));
            s.append_damage(skill_hit(100, 802, at, 16_020_000, 800));
            tick(at, &mut calc);
        }
        let switch = shown.iter().position(|&t| t == 802).expect("you moved on");
        assert!(shown[..switch].iter().all(|&t| t == 801), "your scarecrow, not the stranger's: {shown:?}");
        assert!(shown[switch..].iter().all(|&t| t == 802), "no flip back: {shown:?}");
        assert!(switch > 117 && switch <= 117 + 40, "switched within 10 s of moving on, at tick {switch}");
        crate::clock::set_override(None);
    }

    #[test]
    fn idle_targets_the_meter_does_not_show_are_retired() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        hits(&s, 2259, 800, 1_000, 10_000);
        s.note_death(800);
        // Each pull a new encounter: the encounter keeps its own.
        hits(&s, 2259, 900, 30_000, 32_000);
        hits(&s, 2259, 901, 60_000, 62_000);
        let mut calc = meter_with_npcs(&s);
        calc.set_target_selection_mode("allTargets");
        crate::clock::set_override(Some(100_000));
        calc.get_dps();
        assert_eq!(s.copy_targets_brief().len(), 3, "ALL without a window shows them all");

        calc.set_target_selection_mode("lastHitByMe");
        crate::clock::set_override(Some(110_000));
        assert_eq!(calc.get_dps().target_id, 901);
        let mut left: Vec<i32> = s.copy_targets_brief().into_keys().collect();
        left.sort();
        assert_eq!(left, vec![901], "the target on screen stays");
        // The boss fight went to the auto-save, as an idle restart sends it.
        assert_eq!(ids(&snapshot_at(&mut calc, 110_000)), vec!["auto_800_1000"]);
        crate::clock::set_override(None);
    }

    #[test]
    fn target_mode_keeps_its_target_while_you_still_hit_it() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        let mut calc = meter(&s);
        calc.set_target_selection_mode("lastHitByMe");
        hits(&s, 2259, 900, 1_000, 5_000);
        assert_eq!(calc.get_dps().target_id, 900);
        // An area skill hits a second mob too, and last: the meter stays.
        hits(&s, 2259, 901, 6_000, 6_000);
        hits(&s, 2259, 900, 7_000, 7_000);
        hits(&s, 2259, 901, 8_000, 8_000);
        assert_eq!(calc.get_dps().target_id, 900, "still hit within the hold");
        // You stopped hitting 900 more than the hold ago: the newest.
        hits(&s, 2259, 901, 9_000, 7_000 + TARGET_HOLD_MS + 1_000);
        assert_eq!(calc.get_dps().target_id, 901);
        // Hits in one batch share their time: the same target wins each read.
        let t = s.copy_targets_brief().values().map(|d| d.last_damage_time).max().unwrap() + 1_000;
        let fresh = Arc::new(DataStorage::new());
        fresh.set_local_player_id(Some(2259));
        for target in [905, 903, 904] {
            crate::clock::set_override(Some(t));
            fresh.append_damage(hit(2259, target, t));
        }
        let picks: Vec<i32> = (0..5).map(|_| {
            let mut calc = meter(&fresh);
            calc.set_target_selection_mode("lastHitByMe");
            calc.get_dps().target_id
        }).collect();
        assert!(picks.iter().all(|&p| p == picks[0]), "a tie picks the same target every time: {picks:?}");
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
        s.add_heal_event(2259, 2259, 17_800_000, 111, false, 500);
        hits(&s, 2259, 800, 1_000, 8_000);
        s.add_heal_event(2259, 2259, 17_800_000, 222, false, 4_000);
        s.add_heal_event(2259, 2259, 17_800_000, 333, false, 9_500);
        let live = |calc: &DpsCalculator| calc.target_breakdown(800, None).heal_skills.iter().map(|h| h.dmg as i64).sum::<i64>();
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
        s.add_heal_event(2259, 2259, 17_800_000, 444, false, 45_000);
        crate::clock::set_override(Some(50_000));
        assert!(s.on_zone_switch());
        let saved = snapshot_at(&mut calc, 50_000);
        let by_id = |id: &str| saved.iter().find(|r| r.id == id).map(healed);
        assert_eq!(by_id("auto_800_40000"), Some(444), "saved after it was cleared");
        assert!(by_id("auto_800_1000").is_none_or(|h| h == 222));
        crate::clock::set_override(None);
    }

    /// A boss hit from `actor` on `player` at `at`, Saraswati's Mutation
    /// Breath parried from the front.
    fn taken(s: &DataStorage, actor: i32, player: i32, at: i64, damage: i64) {
        use crate::entity::taken::{TakenHit, TakenKind};
        let mut hit = TakenHit::plain(at, player, actor, 1_218_730, damage, TakenKind::Hit);
        hit.flags = 0x02;
        hit.angle = 0x02;
        s.append_taken(hit);
    }

    #[test]
    fn a_saved_fight_keeps_the_damage_taken_during_it() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        let took = |details: &TargetBreakdownReply| {
            details.taken_skills.iter().filter(|e| e.actor_id == 2259).map(|e| e.stats.damage).sum::<i64>()
        };
        taken(&s, 800, 2259, 500, 1);
        hits(&s, 2259, 800, 1_000, 8_000);
        taken(&s, 800, 2259, 4_000, 300);
        taken(&s, 800, 2259, 9_500, 1);
        assert_eq!(took(&calc.target_breakdown(800, None)), 300, "live Details: the fight's window");
        let row = calc.get_dps().taken;
        assert_eq!((row[0].actor_id, row[0].stats.damage, row[0].stats.parry, row[0].stats.front), (2259, 300, 1, 1));
        let saved = snapshot_at(&mut calc, 30_000);
        assert_eq!(took(&saved[0].details), 300, "saved while live");
        let you = saved[0].actors.iter().find(|a| a.actor_id == 2259).unwrap();
        assert_eq!((you.damage_received, you.hits_received), (300, 1));

        // Rule 7: the next pull, cleared by a zone load, is saved with its hits.
        hits(&s, 2259, 800, 40_000, 48_000);
        taken(&s, 800, 2259, 45_000, 700);
        crate::clock::set_override(Some(50_000));
        assert!(s.on_zone_switch());
        let saved = snapshot_at(&mut calc, 50_000);
        let second = saved.iter().find(|r| r.id == "auto_800_40000").unwrap();
        assert_eq!(took(&second.details), 700, "saved after it was cleared");
        let entry = &second.details.taken_skills[0];
        assert_eq!((entry.code, entry.source_code, entry.stats.hits), (1_218_730, BOSS, 1));
        crate::clock::set_override(None);
    }

    #[test]
    fn a_saved_fight_keeps_its_deaths_and_the_meter_shows_them() {
        use crate::combat::data_storage::RosterEntry;
        use crate::entity::deaths::{DeathEntry, DeathRow};
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        s.name_entity(3000, "Healer");
        s.name_entity(4000, "Away");
        s.replace_roster(vec![("Healer".into(), RosterEntry::default()), ("Away".into(), RosterEntry::default())], true);
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        let hp = |t: i64, id: i32, value: i64| {
            crate::clock::set_override(Some(t));
            s.note_hp(id, value);
        };
        hits(&s, 2259, 800, 1_000, 8_000);
        hp(2_000, 2259, 600);
        hp(2_000, 3000, 900);
        hp(3_000, 2259, 0);
        hp(4_000, 2259, 6000);
        let rows = calc.get_dps().deaths;
        let row = |id: i32| rows.iter().find(|r: &&DeathRow| r.actor_id == id).map(|r| r.deaths);
        assert_eq!((row(2259), row(3000), row(4000)), (Some(Some(1)), Some(Some(0)), Some(None)), "Away sent no HP");
        let saved = snapshot_at(&mut calc, 30_000);
        assert_eq!(
            saved[0].details.deaths,
            Some(vec![DeathEntry { actor_id: 2259, deaths: 1 }, DeathEntry { actor_id: 3000, deaths: 0 }])
        );
        let json = serde_json::to_string(&saved[0]).unwrap();
        let back: FightRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.details.deaths, saved[0].details.deaths);
        // A fight saved before deaths were counted: not known, not 0.
        let old = json.replace(r#","deaths":[{"actorId":2259,"deaths":1},{"actorId":3000,"deaths":0}]"#, "");
        assert_ne!(old, json);
        assert_eq!(serde_json::from_str::<FightRecord>(&old).unwrap().details.deaths, None);
        crate::clock::set_override(None);
    }

    #[test]
    fn the_meter_lists_damage_taken_by_players_who_dealt_none() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        s.name_entity(3000, "Healer");
        let mut calc = meter_with_npcs(&s);
        hits(&s, 2259, 800, 1_000, 8_000);
        taken(&s, 800, 3000, 2_000, 50);
        taken(&s, 800, 2259, 3_000, 20);
        taken(&s, 800, 4000, 3_000, 10); // nobody the game named
        let rows = calc.get_dps().taken;
        let shown: Vec<(i32, &str, i64)> = rows.iter().map(|r| (r.actor_id, r.nickname.as_str(), r.stats.damage)).collect();
        assert_eq!(shown, vec![(3000, "Healer", 50), (2259, "2259", 20)]);
        taken(&s, 800, 2259, 4_000, 5);
        assert_eq!(calc.get_dps().taken[1].stats.damage, 25, "a new hit with no new damage dealt");
        crate::clock::set_override(None);
    }

    #[test]
    fn details_lists_a_hit_taken_during_two_fights_once() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        s.name_entity(2259, "Me");
        spawn(&s, 800, BOSS);
        spawn(&s, 801, BOSS);
        let mut calc = meter_with_npcs(&s);
        hits(&s, 2259, 800, 1_000, 8_000);
        hits(&s, 2259, 801, 2_000, 9_000);
        taken(&s, 800, 2259, 4_000, 300);
        calc.set_target_selection_mode("allTargets");
        calc.get_dps();
        let each: i64 = [800, 801]
            .iter()
            .flat_map(|&t| calc.target_breakdown(t, None).taken_skills)
            .map(|e| e.stats.damage)
            .sum();
        assert_eq!(each, 600, "both fights took the hit");
        let context = calc.get_details_context();
        let listed: Vec<(i32, i32, i64, i32)> =
            context.taken_skills.iter().map(|e| (e.actor_id, e.code, e.stats.damage, e.source_code)).collect();
        assert_eq!(listed, vec![(2259, 1_218_730, 300, BOSS)]);
        let you = context.actors.iter().find(|a| a.actor_id == 2259).unwrap();
        assert_eq!((you.damage_received, you.hits_received), (300, 1));
        crate::clock::set_override(None);
    }

    #[test]
    fn details_lists_the_healing_of_fights_at_once_once_and_the_same_every_read() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        s.name_entity(2259, "Me");
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        hits(&s, 2259, 800, 1_000, 20_000); // the boss
        hits(&s, 2259, 801, 5_000, 8_000); // an add
        hits(&s, 2259, 802, 12_000, 15_000); // another add
        for at in (1_000..=20_000).step_by(1_000) {
            s.add_heal_event(2259, 2259, 17_010_000, 100, false, at);
        }
        let each: Vec<i64> = [800, 801, 802]
            .iter()
            .map(|&t| calc.target_breakdown(t, None).heal_skills.iter().map(|e| e.dmg).sum())
            .collect();
        assert_eq!(each, vec![2_000, 400, 400], "each target holds the ticks of its own span");
        calc.set_target_selection_mode("allTargets");
        calc.get_dps();
        let context = calc.get_details_context();
        let listed: Vec<(i32, i32, i64, i32, bool)> =
            context.heal_skills.iter().map(|e| (e.actor_id, e.code, e.dmg, e.time, e.is_dot)).collect();
        assert_eq!(listed, vec![(2259, 17_010_000, 2_000, 20, false)], "the fight's healing, each tick once");
        for _ in 0..8 {
            let again = calc.get_details_context();
            assert_eq!(serde_json::to_value(&again.heal_skills).unwrap(), serde_json::to_value(&context.heal_skills).unwrap());
        }
        crate::clock::set_override(None);
    }

    #[test]
    fn details_lists_its_targets_most_damage_first_on_every_read() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        for (target, last) in [(801, 3_000), (802, 9_000), (803, 6_000), (804, 5_000)] {
            hits(&s, 2259, target, 1_000, last);
        }
        let mut calc = meter(&s);
        calc.set_target_selection_mode("allTargets");
        calc.get_dps();
        for _ in 0..8 {
            let listed: Vec<i32> = calc.get_details_context().targets.iter().map(|t| t.target_id).collect();
            assert_eq!(listed, vec![802, 803, 804, 801], "most damage first, the same every read");
        }
        crate::clock::set_override(None);
    }

    #[test]
    fn boss_details_on_every_target_is_the_boss_on_the_meter() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        s.name_entity(2259, "Me");
        s.name_entity(2260, "Other");
        spawn(&s, 800, BOSS);
        hits(&s, 2259, 800, 1_000, 20_000);
        hits(&s, 2260, 801, 5_000, 8_000); // an add only another player hit
        s.add_heal_event(2260, 2260, 17_010_000, 1_000, false, 6_000);
        s.add_heal_event(2259, 2259, 17_010_000, 100, false, 10_000);
        let mut calc = meter_with_npcs(&s);
        let healed = |e: &[SkillLine]| e.iter().map(|h| h.dmg).sum::<i64>();

        calc.set_target_selection_mode("bossTargets");
        let shown = calc.get_dps();
        assert_eq!(shown.target_id, 800);
        let context = calc.details_source().reader().get_details_context();
        assert_eq!(context.targets.iter().map(|t| t.target_id).collect::<Vec<_>>(), vec![800], "the boss the meter shows");
        assert_eq!(context.battle_time, shown.battle_time);
        assert_eq!(healed(&context.heal_skills), healed(&calc.get_displayed_details(None).heal_skills), "the meter's healing");
        assert_eq!(healed(&context.heal_skills), 100);

        // ALL shows both, and so does Details.
        calc.set_target_selection_mode("allTargets");
        calc.get_dps();
        let context = calc.details_source().reader().get_details_context();
        assert_eq!(context.targets.iter().map(|t| t.target_id).collect::<Vec<_>>(), vec![800, 801]);
        assert_eq!(healed(&context.heal_skills), 1_100);
        crate::clock::set_override(None);
    }

    #[test]
    fn train_details_count_your_time_on_the_dummies_as_the_meter_does() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 36734, DUMMY);
        hits(&s, 9000, 36734, 1_000, 20_000); // a stranger on the same dummy
        hits(&s, 2259, 36734, 10_000, 20_000);
        let mut calc = meter_with_npcs(&s);
        calc.set_target_selection_mode("trainTargets");
        let shown = calc.get_dps();
        assert_eq!(shown.battle_time, 10_000, "your time");
        let context = calc.details_source().reader().get_details_context();
        assert_eq!(context.battle_time, 10_000, "Details counts it too");
        assert_eq!(context.targets[0].battle_time, 19_000, "the dummy's own span stays");
        crate::clock::set_override(None);
    }

    #[test]
    fn a_players_class_in_details_and_saved_fights_is_the_class_of_most_hits() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        // A Templar whose first hit on each target is a Gladiator-coded skill
        // (11340000) that other classes use too.
        for target in [800, 801, 802] {
            for at in (1_000..=10_000).step_by(1_000) {
                let skill = if at == 1_000 { 11_340_000 } else { 12_010_000 };
                land(&s, 2259, target, at, skill, 100);
            }
        }
        let mut calc = meter_with_npcs(&s);
        calc.set_target_selection_mode("allTargets");
        calc.get_dps();
        for _ in 0..8 {
            let me = calc.get_details_context().actors.into_iter().find(|a| a.actor_id == 2259).unwrap();
            assert_eq!((me.job.as_str(), me.job_id), ("수호성", 12));
        }
        calc.set_target_selection_mode("bossTargets");
        let saved = snapshot_at(&mut calc, 30_000);
        let me = saved[0].actors.iter().find(|a| a.actor_id == 2259).unwrap();
        assert_eq!((me.job.as_str(), me.job_id), ("수호성", 12));
        crate::clock::set_override(None);
    }

    #[test]
    fn a_saved_fights_classes_and_players_are_in_one_order_on_every_save() {
        // Through a hash set, each save wrote the classes in another order.
        for _ in 0..6 {
            let s = Arc::new(DataStorage::new());
            s.set_local_player_id(Some(2259));
            spawn(&s, 800, BOSS);
            // A Sorcerer, a Templar, a Ranger and a Gladiator.
            for (actor, skill) in [(2259, 15_010_000), (3000, 12_010_000), (3001, 14_010_000), (3002, 11_010_000)] {
                s.note_player_spawn(actor);
                for at in (1_000..=10_000).step_by(1_000) {
                    land(&s, actor, 800, at, skill, 100);
                }
            }
            let mut calc = meter_with_npcs(&s);
            let saved = snapshot_at(&mut calc, 30_000);
            assert_eq!(saved[0].job_ids, [11, 12, 14, 15]);
            assert_eq!(saved[0].jobs, ["검성", "궁성", "마도성", "수호성"]);
            assert_eq!(saved[0].actors.iter().map(|a| a.actor_id).collect::<Vec<_>>(), [2259, 3000, 3001, 3002]);
        }
        crate::clock::set_override(None);
    }

    #[test]
    fn damage_totals_past_two_billion_do_not_wrap() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        s.name_entity(2259, "Corvane");
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        for (i, skill) in [16_010_000, 16_020_000, 16_030_000].into_iter().enumerate() {
            let at = 1_000 + i as i64 * 3_000;
            land(&s, 2259, 800, at, skill, 1_000_000_000);
        }
        calc.get_dps();
        let target = calc.get_details_context().targets.into_iter().find(|t| t.target_id == 800).unwrap();
        assert_eq!(target.total_damage as i64, 3_000_000_000);
        assert_eq!(target.actor_damage[&2259] as i64, 3_000_000_000);
        assert_eq!(calc.target_breakdown(800, None).total_target_damage as i64, 3_000_000_000);
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
            s.add_heal_event(2259, 2259, 17_800_000, 1_000_000_000, false, at);
        }
        let details = calc.target_breakdown(800, None);
        assert_eq!(details.skills.iter().map(|r| r.dmg).sum::<i64>(), 3_000_000_000);
        assert_eq!(details.heal_skills.iter().map(|r| r.dmg).sum::<i64>(), 3_000_000_000);
        let saved = snapshot_at(&mut calc, 30_000);
        let json = serde_json::to_string(&saved[0]).unwrap();
        let back: FightRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.details.skills.iter().map(|r| r.dmg).sum::<i64>(), 3_000_000_000);
        crate::clock::set_override(None);
    }

    /// Per-row totals the saved fight would hold for `target`.
    fn saved_totals(calc: &DpsCalculator, target: i32) -> HashMap<i32, i64> {
        let mut totals = HashMap::new();
        for skill in calc.target_breakdown(target, None).skills {
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
        s.name_entity(100, "Owner");
        s.append_damage(skill_hit(100, 900, 1_000, 16_010_000, 1_000));
        // Same class, never linked: a party member's spirit, or a party member.
        s.remember_pet_spawn(500);
        s.append_damage(skill_hit(500, 900, 1_100, 16_010_000, 200));
        s.append_damage(skill_hit(501, 900, 1_200, 16_010_000, 300));
        // A player whose record carried no name.
        s.note_player_record(502);
        s.append_damage(skill_hit(502, 900, 1_250, 16_010_000, 400));
        // A mob the player hits, using a class-band skill of the same class.
        s.append_damage(skill_hit(100, 700, 1_300, 16_010_000, 50));
        s.append_damage(skill_hit(700, 100, 1_400, 16_020_000, 5));
        let mut calc = meter(&s);
        let live = live_totals(&mut calc);
        assert_eq!(live.get(&100), Some(&1_000));
        assert_eq!(live.get(&UNATTRIBUTED_ID), Some(&500), "the summon and the effect share one row");
        assert_eq!(live.get(&502), Some(&400));
        assert!(!live.contains_key(&500) && !live.contains_key(&501));
        let saved = saved_totals(&calc, 900);
        assert_eq!(saved.get(&100), Some(&1_000));
        assert_eq!(saved.get(&UNATTRIBUTED_ID), Some(&500));
    }

    #[test]
    fn effects_tied_to_no_one_are_one_row_and_no_player_of_a_saved_fight() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(100));
        s.name_entity(100, "Me");
        s.name_entity(200, "Other");
        spawn(&s, 900, BOSS);
        let mut calc = meter_with_npcs(&s);
        for i in 0..10 {
            let at = 1_000 + i * 1_000;
            crate::clock::set_override(Some(at));
            s.append_damage(skill_hit(100, 900, at, 15_010_000, 1_000));
            s.append_damage(skill_hit(200, 900, at, 17_010_000, 500));
            // Effects of strangers in the crowd, never spawned: a Sorcerer's
            // and a Cleric's.
            s.append_damage(skill_hit(7001 + i as i32, 900, at, 15_020_000, 100));
            s.append_damage(skill_hit(8001, 900, at, 17_020_000, 50));
        }
        let shown = calc.get_dps();
        let rows: Vec<i32> = { let mut r: Vec<i32> = shown.map.keys().copied().collect(); r.sort(); r };
        assert_eq!(rows, vec![100, 200, UNATTRIBUTED_ID]);
        assert_eq!(shown.map[&UNATTRIBUTED_ID].amount, 1_500.0);
        assert_eq!(shown.map[&UNATTRIBUTED_ID].job, "Unknown", "no class icon");

        let context = calc.get_details_context();
        let pseudo = context.actors.iter().find(|a| a.actor_id == UNATTRIBUTED_ID).expect("in Details");
        assert_eq!((pseudo.job.as_str(), pseudo.job_id), ("", 0));

        let saved = snapshot_at(&mut calc, 60_000);
        assert_eq!(saved.len(), 1);
        let record = &saved[0];
        let mut actors: Vec<i32> = record.actors.iter().map(|a| a.actor_id).collect();
        actors.sort();
        assert_eq!(actors, vec![100, 200], "an upload counts two players");
        assert_eq!(record.party_jobs().len(), 2);
        assert_eq!(record.jobs.len(), 2);
        let effects: Vec<_> = record.details.skills.iter().filter(|k| k.actor_id == UNATTRIBUTED_ID).collect();
        assert_eq!(effects.iter().map(|k| k.dmg).sum::<i64>(), 1_500, "their damage stays in the fight");
        assert!(effects.iter().all(|k| k.job.is_empty()));
        assert_eq!(record.details.skills.iter().map(|k| k.dmg).sum::<i64>(), record.total_damage);
        crate::clock::set_override(None);
    }

    #[test]
    fn other_players_have_one_number_on_the_meter_and_in_details() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(100));
        s.name_entity(100, "Me");
        s.name_entity(200, "Second");
        s.name_entity(300, "First");
        s.append_damage(skill_hit(100, 900, 1_000, 15_010_000, 1_000));
        s.append_damage(skill_hit(300, 900, 1_000, 17_010_000, 500));
        let mut calc = meter(&s);
        let shown = calc.get_dps();
        assert_eq!(shown.map[&100].number, 0, "you are shown by name");
        assert_eq!(shown.map[&300].number, 1);
        s.append_damage(skill_hit(200, 900, 1_500, 16_010_000, 700));
        let shown = calc.get_dps();
        assert_eq!((shown.map[&300].number, shown.map[&200].number), (1, 2), "the later player gets the next number");
        let context = calc.get_details_context();
        assert_eq!((context.numbers.get(&300), context.numbers.get(&200)), (Some(&1), Some(&2)));
        assert!(!context.numbers.contains_key(&100));
    }

    #[test]
    fn other_players_keep_their_numbers_all_session() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(100));
        s.name_entity(100, "Me");
        s.name_entity(200, "Second");
        s.name_entity(300, "First");
        let mut calc = meter_with_npcs(&s);
        // Each player hits `target` a few times from `at`, with a skill of
        // their class.
        let fight = |players: &[(i32, i32)], target: i32, at: i64| {
            spawn(&s, target, BOSS);
            for i in 0..3 {
                for &(id, skill) in players {
                    land(&s, id, target, at + i * 1_000, skill, 700);
                }
            }
        };
        let numbers = |calc: &mut DpsCalculator| -> Vec<(i32, u32)> {
            let mut rows: Vec<(i32, u32)> = calc.get_dps().map.iter().map(|(&id, d)| (id, d.number)).collect();
            rows.sort();
            rows
        };
        fight(&[(300, 17_010_000), (200, 16_010_000), (100, 15_010_000)], 800, 1_000);
        let first = numbers(&mut calc);
        assert_eq!(first.len(), 3, "{first:?}");
        let (second, first_n) = (first[1].1, first[2].1);
        assert!(second > 0 && first_n > 0 && second != first_n, "{first:?}");
        let wanted = vec![(100, 0), (200, second), (300, first_n)];
        assert_eq!(first, wanted);

        for mode in ["allTargets", "encounter", "lastHitByMe", "bossTargets"] {
            calc.set_target_selection_mode(mode);
            assert_eq!(numbers(&mut calc), wanted, "{mode}");
        }

        // The next fight, after the reset button; the game names them again,
        // and a newcomer with a lower id joins.
        calc.reset_focus(true);
        s.forget_names();
        for (id, name) in [(150, "Third"), (200, "Second"), (300, "First")] {
            s.name_entity(id, name);
        }
        fight(&[(150, 18_010_000), (200, 16_010_000), (300, 17_010_000), (100, 15_010_000)], 801, 40_000);
        let third = numbers(&mut calc)[1];
        assert_eq!(numbers(&mut calc), vec![(100, 0), third, (200, second), (300, first_n)], "after a reset");
        assert!(third.1 != second && third.1 != first_n);

        // Another zone: new ids, the same names, now in the other id order.
        s.note_map_load(1010);
        calc.reset_focus(true);
        s.set_local_player_id(Some(1000));
        for (id, name) in [(1000, "Me"), (1200, "First"), (1300, "Second")] {
            s.name_entity(id, name);
        }
        fight(&[(1300, 16_010_000), (1200, 17_010_000), (1000, 15_010_000)], 802, 80_000);
        assert_eq!(numbers(&mut calc), vec![(1000, 0), (1200, first_n), (1300, second)], "after a zone load");
        crate::clock::set_override(None);
    }

    #[test]
    fn a_link_brings_earlier_damage_and_live_and_saved_agree() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(100));
        s.name_entity(100, "Owner");
        s.append_damage(skill_hit(100, 900, 1_000, 16_010_000, 1_000));
        s.remember_pet_spawn(500);
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
    fn boss_mode_keeps_one_of_two_equal_trash_mobs() {
        // One area skill on two mobs, same ms, same damage: each update's
        // map order picked one, so the meter flipped between them.
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        let mut calc = meter(&s);
        let mut shown = Vec::new();
        for at in (1_000..=20_000).step_by(500) {
            crate::clock::set_override(Some(at));
            s.append_damage(hit(2259, 60_000, at));
            s.append_damage(hit(2259, 50_000, at));
            shown.push(calc.get_dps().target_id);
        }
        assert!(shown.iter().all(|&t| t == 50_000), "one mob throughout: {shown:?}");
        crate::clock::set_override(None);
    }

    /// Replay a capture file, calling `before` with each line's time of day
    /// (ms) before that line is parsed.
    fn replay_capture(path: &str, mut before: impl FnMut(i64, &Arc<DataStorage>)) -> Arc<DataStorage> {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data");
        let skills = Arc::new(SkillLookup::new());
        let npcs = Arc::new(NpcLookup::new());
        crate::i18n::lookup::load_language(&skills, &npcs, &data, "en");
        let s = Arc::new(DataStorage::new());
        use crate::capture::decoder::PacketDecoder;
        let mut p = PacketDecoder::new(s.clone(), skills, npcs);
        let dots: Vec<i32> = serde_json::from_str(&std::fs::read_to_string(data.join("dot_skill_ids.json")).unwrap()).unwrap();
        p.use_tick_skills(dots.into_iter().collect());
        for line in std::fs::read_to_string(path).unwrap().lines() {
            let parts: Vec<&str> = line.splitn(3, '|').collect();
            if line.starts_with('#') || parts.len() != 3 { continue; }
            // `2026-01-02T12:00:00.000000000+00:00`
            let t = parts[0];
            let n = |a: usize, b: usize| t[a..b].parse::<i64>().unwrap();
            let ts = ((n(11, 13) * 60 + n(14, 16)) * 60 + n(17, 19)) * 1000 + n(20, 23);
            before(ts, &s);
            p.pin_clock(Some(ts));
            let hex = parts[2];
            let bytes: Vec<u8> = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()).collect();
            p.decode_buffer(&bytes);
        }
        p.pin_clock(None);
        s
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
            let summons = s.pet_owners();
            for (&t, td) in &s.copy_targets() {
                if target.is_some_and(|x| x != t) { continue; }
                let mut rows: HashMap<(i32, i32), (i64, i32)> = HashMap::new();
                for (&a, ad) in &td.actors {
                    let owner = owner_chain::resolve(a, &summons);
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
                        eprintln!("  actor {owner} skill {skill} damage {dmg} hits {hits} {}", skills.skill_title(skill));
                    }
                }
            }
        });
        assert!(done);
    }

    /// The hover summary is the full details without hit timelines, healing,
    /// ping and deaths.
    fn assert_hover_matches_full(full: TargetBreakdownReply, summary: TargetBreakdownReply) {
        let (mut full, mut summary) = (full, summary);
        assert!(summary.skills.iter().all(|s| s.hit_timestamps.is_empty()));
        assert!(summary.heal_skills.is_empty());
        assert!(summary.ping_history.is_empty());
        assert!(summary.deaths.is_none());
        full.heal_skills.clear();
        full.ping_history.clear();
        full.deaths = None;
        for skill in &mut full.skills { skill.hit_timestamps.clear(); }
        full.skills.sort_by_key(|s| (s.actor_id, s.code, s.is_dot));
        summary.skills.sort_by_key(|s| (s.actor_id, s.code, s.is_dot));
        assert_eq!(serde_json::to_value(full).unwrap(), serde_json::to_value(summary).unwrap());
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
        let same = |mut a: TargetBreakdownReply, mut b: TargetBreakdownReply| {
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
                    same(reader.target_breakdown(target, actors), live.target_breakdown(target, actors));
                    assert_hover_matches_full(live.target_breakdown(target, actors), reader.get_hover_details(target, actors));
                }
            }
        }
        live.set_target_selection_mode("encounter");
        live.get_dps();
        assert_eq!(source.reader().target_breakdown(800, None).total_target_damage, 12 * 500,
            "the encounter's carry: the trash before the pull still counts");
        crate::clock::set_override(None);
    }

    /// The rows with damage, and the battle time.
    fn shown_rows(calc: &mut DpsCalculator) -> (Vec<(i32, i64)>, i64) {
        let shown = calc.get_dps();
        (rows_with_damage(&shown), shown.battle_time)
    }

    /// A party roster entry in `slot`.
    fn member(name: &str, slot: u8) -> (String, crate::combat::data_storage::RosterEntry) {
        (name.to_string(), crate::combat::data_storage::RosterEntry { slot, ..Default::default() })
    }

    fn listed_targets(calc: &DpsCalculator) -> Vec<i32> {
        calc.get_details_context().targets.iter().map(|t| t.target_id).collect()
    }

    #[test]
    fn dungeon_from_the_npc_table_wins_over_the_stamped_one() {
        let npcs = NpcLookup::new();
        npcs.load_from_json(
            r#"{"2310218":{"name":"First","isBoss":true,"dungeonId":600011},
                "2310206":{"name":"Second","isBoss":true,"dungeonId":600011},
                "2300475":{"name":"Third","isBoss":true},
                "2701090":{"name":"Fourth","isBoss":true},
                "2310219":{"name":"Fifth"}}"#,
        );
        let cases = [(2310218, 0, 600011), (2310206, 600072, 600011), (2300475, 610073, 610073), (2701090, 0, 0), (2310219, 600011, 600011)];
        for (code, stamped, filed) in cases {
            assert_eq!(super::fights::filed_dungeon(&npcs, code, stamped, false, 0), filed, "mob {code}");
        }
        // Neither the table nor the hits name one: the open world has none,
        // a cleared segment keeps its own.
        assert_eq!(super::fights::filed_dungeon(&npcs, 2701090, 0, true, 600002), 0);
        assert_eq!(super::fights::filed_dungeon(&npcs, 2701090, 0, false, 600002), 600002);
    }

    #[test]
    fn a_queued_partys_roster_leaves_a_strangers_boss_off_the_meter() {
        use crate::entity::job_class::JobClass;
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(15740));
        s.name_entity(15740, "Me");
        s.note_map_load(1010);
        s.append_mob(800, 702);
        s.append_mob(65389, BOSS);
        let mut calc = meter_with_npcs(&s);
        for (n, at) in (1_000..=10_000).step_by(500).enumerate() {
            let turn = (n % 6) as i32 * 10_000;
            land(&s, 15740, 800, at, 11_010_000, 500);
            land(&s, 1458, 65389, at, 14_010_000 + turn, 2_000);
            land(&s, 14536, 65389, at, 18_010_000 + turn, 2_000);
        }
        assert_eq!(calc.get_dps().target_id, 800);

        s.enter_dungeon(600031);
        let mut roster = vec![member("Me", 1), member("Rr", 2), member("Cc", 3), member("Ll", 4), member("Tt", 5)];
        let classes = [JobClass::Gladiator, JobClass::Ranger, JobClass::Chanter, JobClass::Cleric, JobClass::Templar];
        for ((_, entry), class) in roster.iter_mut().zip(classes) {
            entry.job = Some(class);
        }
        s.replace_roster(roster, true);
        assert_eq!(s.current_dungeon_id(), 0, "queued, not entered");
        for (n, at) in (10_100..=15_100).step_by(500).enumerate() {
            let turn = (n % 6) as i32 * 10_000;
            land(&s, 1458, 65389, at, 14_010_000 + turn, 2_000);
            land(&s, 14536, 65389, at, 18_010_000 + turn, 2_000);
        }
        let names = s.all_names();
        assert!(!names.contains_key(&1458) && !names.contains_key(&14536), "no class names a stranger");
        assert!(s.copy_targets_brief().contains_key(&800));
        assert_eq!(calc.get_dps().target_id, 800);
        crate::clock::set_override(None);
    }

    #[test]
    fn enc_ends_after_a_quiet_timeout_unless_a_live_boss_holds_it() {
        let start = |s: &DataStorage| s.current_encounter().map(|e| e.start);

        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, 1);
        spawn(&s, 810, 1);
        hits(&s, 2259, 800, 1_000, 5_000);
        hits(&s, 2259, 810, 30_000, 33_000);
        let encounter = s.current_encounter().expect("an encounter");
        assert_eq!((encounter.start, encounter.targets.len()), (30_000, 1));
        let mut calc = meter(&s);
        calc.set_target_selection_mode("encounter");
        let (rows, battle) = shown_rows(&mut calc);
        assert_eq!(battle, 3_000);
        assert_eq!(rows.iter().map(|r| r.1).sum::<i64>(), 2_000);

        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 900, BOSS);
        spawn(&s, 910, 1);
        hits(&s, 2259, 900, 1_000, 3_000);
        hits(&s, 2259, 910, 40_000, 41_000);
        assert_eq!(start(&s), Some(1_000), "the boss lives");
        s.note_death(900);
        hits(&s, 2259, 910, 80_000, 81_000);
        assert_eq!(start(&s), Some(80_000));

        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        s.set_encounter_timeout_ms(30_000);
        spawn(&s, 800, 1);
        hits(&s, 2259, 800, 1_000, 2_000);
        hits(&s, 2259, 800, 25_000, 26_000);
        assert_eq!(start(&s), Some(1_000), "inside the longer timeout");
        crate::clock::set_override(None);
    }

    #[test]
    fn enc_waits_for_you_then_shows_only_you_and_your_party() {
        let s = Arc::new(DataStorage::new());
        s.name_entity(3000, "A");
        s.replace_roster(vec![member("A", 1)], true);
        let mut calc = meter(&s);
        calc.set_target_selection_mode("encounter");
        hits(&s, 9000, 900, 1_000, 10_000);
        hits(&s, 2259, 901, 5_000, 12_000);
        hits(&s, 3000, 901, 6_000, 7_000);
        crate::clock::set_override(Some(12_500));
        assert_eq!(shown_rows(&mut calc), (vec![], 0));
        s.set_local_player_id(Some(2259));
        assert!(shown_rows(&mut calc).0.is_empty(), "no hit since the meter knew you");

        hits(&s, 2259, 901, 14_000, 14_000);
        let (rows, battle) = shown_rows(&mut calc);
        assert_eq!(rows.iter().map(|r| r.0).collect::<Vec<_>>(), vec![2259, 3000]);
        assert_eq!(rows[0].1, 4_500);
        assert_eq!(calc.displayed_targets, vec![901]);
        assert_eq!(battle, 9_000);
        crate::clock::set_override(None);
    }

    #[test]
    fn enc_rows_carry_dps_active_dps_and_recent_dps() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, 1);
        hits(&s, 2259, 800, 1_000, 70_000);
        s.note_player_record(3000);
        hits(&s, 3000, 800, 68_000, 70_000);
        let mut calc = meter(&s);
        calc.set_target_selection_mode("encounter");
        crate::clock::set_override(Some(70_000));
        let shown = calc.get_dps();
        assert_eq!(shown.battle_time, 69_000);
        let near = |got: f64, want: f64| assert!((got - want).abs() < 0.01, "{got} against {want}");
        let (you, other) = (&shown.map[&2259], &shown.map[&3000]);
        near(you.dps, 35_000.0 / 69.0);
        near(other.dps, 1_500.0 / 69.0);
        near(other.active_dps, 750.0);
        near(you.last10_dps, 550.0);
        near(you.last60_dps, 30_500.0 / 60.0);
        crate::clock::set_override(None);
    }

    #[test]
    fn no_mode_shows_a_hit_on_a_player() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, DUMMY);
        hits(&s, 3000, 2259, 1_000, 3_000);
        hits(&s, 2259, 800, 2_000, 6_000);
        s.note_player_record(3001);
        hits(&s, 2259, 3001, 7_000, 14_000);
        s.remember_pet_spawn(7000);
        hits(&s, 7000, 2259, 15_000, 16_000);
        crate::clock::set_override(Some(16_500));
        let mut calc = meter_with_npcs(&s);
        for mode in MODES {
            assert_eq!(rows_in(&mut calc, mode), vec![(2259, 2_500)], "{mode}");
            assert_eq!(calc.displayed_targets, vec![800], "{mode}");
            assert_eq!(listed_targets(&calc), vec![800], "{mode}");
        }
        assert_eq!(s.current_encounter().map(|e| e.targets), Some(HashSet::from([800])));
        crate::clock::set_override(None);
    }

    #[test]
    fn an_enemy_who_hit_you_has_no_row() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(14256));
        s.note_player_spawn(16156);
        spawn(&s, 16726, 1);
        spawn(&s, 19612, 2600012);
        let mut calc = meter_with_npcs(&s);
        land(&s, 16156, 16726, 10_000, 14_380_000, 1_604);
        land(&s, 16156, 16726, 10_500, 14_100_000, 3_482);
        crate::clock::set_override(Some(20_000));
        assert_eq!(rows_in(&mut calc, "allTargets"), vec![(16156, 5_086)]);

        land(&s, 16156, 14256, 32_598, 14_020_000, 379);
        for (at, skill, on_you, on_mob) in [(34_500, 14_140_000, 1_200, 6_035), (34_750, 14_360_000, 1_186, 10_062), (35_098, 14_080_000, 1_794, 11_210)] {
            land(&s, 16156, 14256, at, skill, on_you);
            land(&s, 16156, 19612, at, skill, on_mob);
        }
        land(&s, 14256, 19612, 35_098, 16_010_000, 1_375);
        land(&s, 16156, 19612, 36_000, 14_340_000, 2_002);
        crate::clock::set_override(Some(38_000));
        assert!(s.is_enemy(16156));
        for mode in MODES {
            let rows = rows_in(&mut calc, mode);
            assert!(rows.iter().all(|r| r.0 != 16156), "{mode}: {rows:?}");
            assert!(!listed_targets(&calc).contains(&16726), "{mode}");
            if ["encounter", "allTargets", "mostRecent", "lastHitByMe"].contains(&mode) {
                assert_eq!(rows, vec![(14256, 1_375)], "{mode}");
            }
        }
        assert_eq!(saved_totals(&calc, 19612), HashMap::from([(14256, 1_375)]));
        assert!(s.heals_between(0, 40_000).is_empty(), "an attack is no heal");
        crate::clock::set_override(None);
    }

    #[test]
    fn saved_fights_outlast_a_zone_change_a_reset_and_a_party_change() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        hits(&s, 2259, 800, 1_000, 8_000);
        crate::clock::set_override(Some(12_000));
        s.on_zone_switch();
        let saved = snapshot_at(&mut calc, 12_000);
        assert_eq!(ids(&saved), vec!["auto_800_1000"]);
        assert!(calc.fight_finished(&saved[0]));

        hits(&s, 2259, 800, 20_000, 30_000);
        crate::clock::set_override(Some(31_000));
        calc.reset_focus(true);
        assert_eq!(ids(&snapshot_at(&mut calc, 31_000)), vec!["auto_800_20000"]);

        s.replace_roster(vec![member("A", 1), member("B", 2)], true);
        hits(&s, 2259, 800, 40_000, 50_000);
        s.replace_roster(vec![member("A", 1)], true);
        assert_eq!(ids(&snapshot_at(&mut calc, 51_000)), vec!["auto_800_40000"]);

        spawn(&s, 900, 1);
        hits(&s, 2259, 900, 60_000, 70_000);
        hits(&s, 2259, 800, 60_000, 62_000);
        crate::clock::set_override(Some(80_000));
        s.on_zone_switch();
        assert!(snapshot_at(&mut calc, 80_000).is_empty(), "no boss, and the boss too short");
        crate::clock::set_override(None);
    }

    #[test]
    fn most_recent_and_most_damage_pick_one_target_every_read() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        let targets = [902, 902, 904, 905, 905, 903, 904];
        let times = [1_000, 1_500, 1_000, 1_000, 2_000, 2_000, 2_000];
        for (target, at) in targets.into_iter().zip(times) {
            hits(&s, 2259, target, at, at);
        }
        for mode in ["mostRecent", "mostDamage"] {
            for _ in 0..12 {
                let mut calc = meter(&s);
                calc.set_target_selection_mode(mode);
                assert_eq!(calc.get_dps().target_id, 904, "{mode}");
            }
        }
        crate::clock::set_override(None);
    }

    #[test]
    fn one_targets_answer_lists_entries_by_actor_skill_and_dot() {
        let s = Arc::new(DataStorage::new());
        s.note_player_spawn(3000);
        s.set_local_player_id(Some(2259));
        s.confirm_pet(5000, 2259);
        let order = [
            (3000, 16_010_000, 1_000),
            (2259, 16_030_000, 1_200),
            (2259, 16_010_000, 1_400),
            (5000, 16_010_000, 1_500),
            (2259, 16_020_000, 1_800),
            (2259, 16_010_000, 2_000),
            (5000, 16_010_000, 2_500),
        ];
        for (actor, skill, at) in order {
            land(&s, actor, 900, at, skill, 100);
            s.add_heal_event(actor, actor, 17_010_000 + actor, 50, false, at);
        }
        let calc = meter(&s);
        for _ in 0..8 {
            let answer = calc.target_breakdown(900, None);
            let keys: Vec<(i32, i32)> = answer.skills.iter().map(|e| (e.actor_id, e.code)).collect();
            assert_eq!(keys, vec![(2259, 16_010_000), (2259, 16_020_000), (2259, 16_030_000), (3000, 16_010_000)]);
            assert_eq!(answer.skills[0].hit_timestamps, vec![400, 500, 1_000, 1_500], "you and your spirit");
            assert!(answer.heal_skills.windows(2).all(|w| w[0].actor_id <= w[1].actor_id));
        }
        crate::clock::set_override(None);
    }

    #[test]
    fn boss_mode_shows_your_own_mob_only_in_the_open_world() {
        let fight = |local: Option<i64>, dungeon: Option<i32>, stranger: bool| {
            let s = Arc::new(DataStorage::new());
            s.set_local_player_id(local);
            if let Some(d) = dungeon {
                s.enter_dungeon(d);
            }
            land(&s, 2259, 50_000, 1_000, 11_010_000, 500);
            if stranger {
                for at in 1_000..=1_004 {
                    land(&s, 11_345, 60_000, at, 11_010_000, 500);
                }
            }
            let mut calc = meter(&s);
            let shown = calc.get_dps();
            let mut rows: Vec<i32> = shown.map.keys().copied().collect();
            rows.sort();
            (shown.target_id, rows)
        };
        assert_eq!(fight(Some(2259), None, true), (50_000, vec![2259]));
        assert_eq!(fight(None, None, true), (0, vec![]));
        assert_eq!(fight(Some(2259), Some(600_011), false), (0, vec![]));
        crate::clock::set_override(None);
    }

    #[test]
    fn the_hover_answer_is_the_full_answer_without_timelines() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        for (actor, target, at) in [(2259, 50_000, 1_000), (2259, 50_000, 1_100), (2260, 50_000, 1_200), (2259, 50_001, 1_300)] {
            hits(&s, actor, target, at, at);
        }
        crate::clock::set_override(Some(1_400));
        let mut tick = hit(2259, 50_000, 1_400);
        tick.set_dot(true);
        s.append_damage(tick);
        let calc = meter(&s);
        assert!(calc.get_hover_details(50_000, Some(&[2259])).skills.iter().any(|e| e.is_dot));
        assert!(!calc.target_breakdown(50_000, Some(&[2259])).skills[0].hit_timestamps.is_empty());
        for actors in [None, Some(&[2259][..]), Some(&[2260][..]), Some(&[999][..])] {
            assert_hover_matches_full(calc.target_breakdown(50_000, actors), calc.get_hover_details(50_000, actors));
        }
        assert_hover_matches_full(calc.target_breakdown(99_999, None), calc.get_hover_details(99_999, None));
        let full = calc.target_breakdown(50_000, Some(&[2259]));
        let plain = full.skills.iter().find(|e| !e.is_dot).unwrap();
        assert_eq!(plain.hit_timestamps.len(), 2, "a hover read takes nothing away");
        crate::clock::set_override(None);
    }

    #[test]
    fn a_reader_answers_as_the_meter_and_copies_no_other_target() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        hits(&s, 2259, 50_000, 1_000, 1_000);
        let mut live = meter(&s);
        live.get_dps();
        let reader = live.details_source().reader();
        let json = |c: &crate::entity::detail_bundle::DetailBundle| serde_json::to_value(c).unwrap();
        assert_eq!(json(&reader.get_details_context()), json(&live.get_details_context()));

        let before = serde_json::to_value(reader.target_breakdown(50_000, Some(&[2259]))).unwrap();
        for round in 0..100 {
            for target in 50_001..=50_064 {
                let at = 2_000 + round * 10;
                hits(&s, 2259, target, at, at);
            }
        }
        assert_eq!(serde_json::to_value(reader.target_breakdown(50_000, Some(&[2259]))).unwrap(), before);
        let times = |targets: &[i32]| -> usize {
            s.get_target_snapshots(targets, false, false)
                .values()
                .flat_map(|t| t.actors.values())
                .flat_map(|a| a.skills.values())
                .map(|k| k.hit_timestamps.len())
                .sum()
        };
        assert_eq!(times(&[50_000]), 1);
        assert_eq!(times(&[99_999]), 0);
        live.reset_focus(true);
        assert_eq!(reader.get_details_context().current_target_id, live.get_details_context().current_target_id);
        crate::clock::set_override(None);
    }

    #[test]
    fn heal_ticks_and_hit_times_taken_run_in_time_order() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        s.name_entity(2259, "Me");
        s.confirm_pet(5000, 2259);
        spawn(&s, 800, BOSS);
        hits(&s, 2259, 800, 1_000, 8_000);
        // Ticks on you and on your spirit, which go under you, out of order.
        for (target, at) in [(2259, 4_000), (5000, 3_000), (2259, 2_000), (5000, 5_000)] {
            s.add_heal_event(2259, target, 17_010_000, 100, false, at);
        }
        taken(&s, 800, 2259, 6_000, 30);
        taken(&s, 800, 2259, 2_500, 20);
        taken(&s, 800, 2259, 4_500, 10);
        let calc = meter_with_npcs(&s);
        let answer = calc.target_breakdown(800, None);
        let heal = answer.heal_skills.iter().find(|e| e.actor_id == 2259).expect("your healing");
        let at: Vec<i64> = heal.heal_ticks.iter().map(|t| t.at).collect();
        assert_eq!(at, vec![2_000, 3_000, 4_000, 5_000]);
        assert!(heal.heal_ticks.iter().all(|t| t.target == 2259), "{:?}", heal.heal_ticks);
        let yours: Vec<i64> = answer.taken_skills.iter().filter(|e| e.actor_id == 2259).flat_map(|e| e.hit_times.iter().map(|h| h.0)).collect();
        assert_eq!(yours, vec![2_500, 4_500, 6_000]);
        crate::clock::set_override(None);
    }

    #[test]
    fn details_puts_a_shared_name_on_the_lowest_listed_id() {
        // The storage keeps names unique, so no read has shown two listed
        // ids of one name; the choice is pinned here.
        let names = HashMap::from([(3100, "Twin"), (3000, "Twin"), (2259, "Me")]);
        let ids = std::collections::BTreeSet::from([3100, 2259, 3000, 4000]);
        let by_name = super::details::ids_by_name(&ids, |id| names.get(&id).map_or(id.to_string(), |n| n.to_string()));
        assert_eq!(by_name.get("Twin"), Some(&3000));
        assert_eq!(by_name.get("Me"), Some(&2259));
        assert_eq!(by_name.get("4000"), Some(&4000));
    }

    #[test]
    fn saved_fights_come_in_target_order_cleared_ones_first() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        for id in [930, 920, 910] {
            spawn(&s, id, BOSS);
        }
        let mut calc = meter_with_npcs(&s);
        hits(&s, 2259, 930, 1_000, 8_000);
        hits(&s, 2259, 910, 1_000, 8_000);
        crate::clock::set_override(Some(12_000));
        assert!(s.on_zone_switch());
        hits(&s, 2259, 930, 20_000, 28_000);
        hits(&s, 2259, 920, 20_000, 28_000);
        hits(&s, 2259, 910, 20_000, 28_000);
        crate::clock::set_override(Some(29_000));
        let order: Vec<String> = calc.close_boss_fights_now().into_iter().map(|r| r.id).collect();
        assert_eq!(order, ["auto_910_1000", "auto_930_1000", "auto_910_20000", "auto_920_20000", "auto_930_20000"]);
        crate::clock::set_override(None);
    }

    #[test]
    fn a_fight_is_finished_only_at_the_last_hit_it_was_saved_with() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        spawn(&s, 800, BOSS);
        let mut calc = meter_with_npcs(&s);
        hits(&s, 2259, 800, 1_000, 10_000);
        let saved = snapshot_at(&mut calc, 50_000);
        assert!(calc.fight_finished(&saved[0]), "the last hit it was saved with");
        let mut earlier = saved[0].clone();
        earlier.duration_ms -= 1_000;
        assert!(!calc.fight_finished(&earlier), "the book holds a later last hit");
        let mut other = saved[0].clone();
        other.start_time_ms += 1;
        assert!(!calc.fight_finished(&other), "never saved for good");
        crate::clock::set_override(None);
    }

    #[test]
    fn ticks_are_marked_in_their_names_and_hits_keep_their_extremes() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        let skills = SkillLookup::new();
        skills.load_from_json(r#"{"16010000":"Fire","17010000":"Mend"}"#);
        let calc = DpsCalculator::new(s.clone(), Arc::new(skills), Arc::new(NpcLookup::new()), Arc::new(PingTracker::new()));
        land(&s, 2259, 900, 1_000, 16_010_000, 300);
        land(&s, 2259, 900, 1_500, 16_010_000, 700);
        let mut tick = skill_hit(2259, 900, 2_000, 16_010_000, 50);
        tick.set_dot(true);
        s.append_damage(tick);
        let mut unnamed = skill_hit(2259, 900, 2_500, 16_050_000, 60);
        unnamed.set_dot(true);
        s.append_damage(unnamed);
        s.add_heal_event(2259, 2259, 17_010_000, 40, true, 1_800);
        let answer = calc.target_breakdown(900, None);
        let names: Vec<(&str, bool)> = answer.skills.iter().map(|e| (e.name.as_str(), e.is_dot)).collect();
        assert_eq!(names, vec![("Fire", false), ("Fire - DOT", true), ("", true)]);
        assert_eq!((answer.skills[0].min_dmg, answer.skills[0].max_dmg), (300, 700));
        assert_eq!(answer.heal_skills[0].name, "Mend - HoT");
        crate::clock::set_override(None);
    }

    #[test]
    fn details_lists_no_one_without_a_name_or_a_class() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        s.name_entity(2259, "Me");
        land(&s, 2259, 900, 1_000, 16_010_000, 300);
        land(&s, 4000, 900, 1_200, 1_234_567, 200);
        let mut calc = meter(&s);
        calc.set_target_selection_mode("allTargets");
        calc.get_dps();
        let context = calc.get_details_context();
        assert_eq!(context.actors.iter().map(|a| a.actor_id).collect::<Vec<_>>(), vec![2259]);
        assert!(!context.targets[0].actor_damage.contains_key(&4000));
        assert_eq!(context.deaths, Some(vec![]));
        crate::clock::set_override(None);
    }

    #[test]
    fn an_answer_carries_the_ping_over_its_fight() {
        use crate::built::{reply_frame, segment, YEAR_ONE_MS};
        // The server's reply frame echoing the client's .NET wall time. The
        // tracker files each reading at the wall clock.
        let reply = |at: i64, ping: i64| segment(None, 13_328, 50_000, &reply_frame(at - ping + YEAR_ONE_MS, at), at);
        let pings = Arc::new(PingTracker::new());
        let before = crate::clock::system_ms();
        pings.observe_segment(&reply(before, 80), 13_328);
        let after = crate::clock::system_ms();
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(2259));
        land(&s, 2259, 900, before - 1_000, 11_010_000, 500);
        land(&s, 2259, 900, after + 1_000, 11_010_000, 500);
        let calc = DpsCalculator::new(s.clone(), Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()), pings);
        let points: Vec<(i64, i32)> = calc.target_breakdown(900, None).ping_history.iter().map(|p| (p.ts_ms, p.ping_ms)).collect();
        assert_eq!(points.len(), 1, "{points:?}");
        assert_eq!(points[0].1, 80);
        assert!((1_000..=1_000 + after - before).contains(&points[0].0), "from the first hit: {points:?}");
        assert!(calc.get_hover_details(900, None).ping_history.is_empty());
        crate::clock::set_override(None);
    }

    /// The repo's `en` skill and NPC tables.
    fn english_tables() -> (Arc<SkillLookup>, Arc<NpcLookup>) {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data");
        let (skills, npcs) = (Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
        crate::i18n::lookup::load_language(&skills, &npcs, &data, "en");
        (skills, npcs)
    }

    /// `hh:mm:ss.mmm` of a time of day in ms.
    fn clock_text(ms: i64) -> String {
        format!("{:02}:{:02}:{:02}.{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1_000 % 60, ms % 1_000)
    }

    /// Prints what the meter shows over a capture: BOSS until A2_SWITCH,
    /// then A2_MODE, every A2_EVERY seconds. Each connection has its own
    /// stream assembler.
    #[cfg(feature = "backend")]
    #[test]
    #[ignore]
    fn meter_targets_over_a_capture() {
        use crate::capture::packet_log::{LogLine, PacketLogReplay};
        let path = std::env::var("A2_CAPTURE").expect("A2_CAPTURE");
        let mode = std::env::var("A2_MODE").unwrap_or("trainTargets".into());
        let every: i64 = std::env::var("A2_EVERY").ok().and_then(|v| v.parse().ok()).unwrap_or(5) * 1_000;
        let switch_at: Option<i64> = std::env::var("A2_SWITCH").ok().map(|v| {
            let p: Vec<i64> = v.split(':').map(|x| x.parse().expect("hh:mm:ss")).collect();
            ((p[0] * 60 + p[1]) * 60 + p[2]) * 1_000
        });
        let me: Option<i64> = std::env::var("A2_ME").ok().and_then(|v| v.parse().ok());

        let s = Arc::new(DataStorage::new());
        let (skills, npcs) = english_tables();
        let mut calc = DpsCalculator::new(Arc::clone(&s), Arc::clone(&skills), Arc::clone(&npcs), Arc::new(PingTracker::new()));
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data");
        let mut replay = PacketLogReplay::new(Arc::clone(&s), skills, npcs, crate::live_feed::dot_skill_ids(&data));
        let (mut due, mut switched) = (i64::MIN, false);
        let text = std::fs::read_to_string(&path).unwrap();
        for line in text.lines().filter_map(LogLine::parse) {
            // `hh:mm:ss.fff…` as ms of the day.
            let tod = line.time_of_day();
            let n = |a: usize, b: usize| tod.get(a..b).and_then(|d| d.parse::<i64>().ok()).unwrap_or(0);
            let ts = ((n(0, 2) * 60 + n(3, 5)) * 60 + n(6, 8)) * 1_000 + n(9, 12);
            if let Some(id) = me.filter(|_| s.local_player_id().is_none()) {
                s.set_local_player_id(Some(id));
            }
            replay.feed(&line);
            if !switched && switch_at.is_some_and(|at| ts >= at) {
                switched = true;
                calc.set_target_selection_mode(&mode);
                eprintln!("{} mode {mode}", clock_text(ts));
            }
            if ts >= due {
                due = ts + every;
                let shown = calc.get_dps();
                let mut rows: Vec<(i32, i64)> = shown.map.iter().map(|(&id, r)| (id, r.amount as i64)).collect();
                rows.sort();
                let you = s.local_player_id().map(|id| id as i32);
                let yours = you.and_then(|id| shown.map.get(&id)).map(|r| r.amount as i64);
                eprintln!("{} shown {:?} target {} rows {rows:?} yours {yours:?}", clock_text(ts), calc.displayed_targets, shown.target_id);
                if let Some(you) = you {
                    let links = s.pet_owners();
                    for (target, fight) in s.copy_targets_brief() {
                        let own = fight.actors.get(&you).map_or(0, |a| a.total_damage);
                        let summons: i64 = fight
                            .actors
                            .iter()
                            .filter(|&(&a, _)| a != you && owner_chain::resolve(a, &links) == you)
                            .map(|(_, a)| a.total_damage)
                            .sum();
                        if own + summons > 0 {
                            eprintln!("    on {target}: you {own}, your summons {summons}");
                        }
                    }
                }
            }
        }
        crate::clock::set_override(None);
        assert!(switched, "the capture never reached A2_SWITCH");
    }
}
