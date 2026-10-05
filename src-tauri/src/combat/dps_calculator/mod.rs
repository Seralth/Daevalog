use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::combat::data_storage::{DataStorage, SegmentIdentity, TargetCombatData, IDLE_RESET_MS, MIN_SAVED_FIGHT_MS};
use crate::combat::ping_tracker::PingTracker;
use crate::entity::details_context::*;
use crate::entity::dps_data::DpsData;
use crate::entity::fight_record::FightRecord;
use crate::entity::job_class::JobClass;
use crate::i18n::lookup::{NpcLookup, SkillLookup};

mod details;
mod meter_rows;
mod rows;
mod target;

pub use rows::PARTY_ROW_ID_BASE;
pub use target::TargetSelectionMode;

use rows::resolve_nickname;

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
        }
    }

    pub fn mark_all_targets_saved(&mut self) {
        let combat = self.data_storage.get_combat_snapshot_light();
        for (&tid, td) in &combat {
            self.saved_fights.insert((tid, td.first_damage_time), td.last_damage_time);
        }
    }

    /// Whether `record` is a fight that is over and saved for good: it cannot
    /// go on, so it is safe to upload.
    pub fn fight_finished(&self, record: &FightRecord) -> bool {
        self.saved_fights.get(&(record.target_id, record.start_time_ms))
            == Some(&(record.start_time_ms + record.duration_ms))
    }

    pub fn snapshot_boss_fights(&mut self) -> Vec<FightRecord> {
        self.snapshot_boss_fights_inner(false)
    }

    pub fn snapshot_boss_fights_force(&mut self) -> Vec<FightRecord> {
        self.snapshot_boss_fights_inner(true)
    }

    fn snapshot_boss_fights_inner(&mut self, force: bool) -> Vec<FightRecord> {
        let mob_data = self.data_storage.get_mob_data();
        let now_ms = crate::clock::now_ms();
        let mut records = Vec::new();

        // Fights already cleared out of the live data (an idle restart on the
        // same mob, a boss pull, a reset, a zone change): saved as they ended.
        for seg in self.data_storage.take_ended_segments() {
            let td = &seg.data;
            if !self.is_saved_fight_target(&mob_data, td) || self.saved_fights.get(&(td.target_id, td.first_damage_time)) == Some(&td.last_damage_time) {
                continue;
            }
            let details = self.details_for(td, seg.max_hp, &seg.heals, None, Some(&seg.identity));
            let stats = actor_stats([td].into_iter());
            records.push(self.build_record(td, details, &stats, &mob_data, Some(&seg.identity)));
            self.saved_fights.insert((td.target_id, td.first_damage_time), td.last_damage_time);
        }

        // Light snapshot: only used for target filtering + per-actor aggregate
        // stats here; the saved record's timestamps come from get_target_details.
        let combat_data = self.data_storage.get_combat_snapshot_light();
        let candidates: Vec<i32> = combat_data.iter()
            .filter(|(tid, td)| {
                self.saved_fights.get(&(**tid, td.first_damage_time)) != Some(&td.last_damage_time)
                    && self.is_saved_fight_target(&mob_data, td)
            })
            .map(|(&tid, _)| tid)
            .collect();

        if !candidates.is_empty() {
            tracing::trace!("snapshot_boss_fights: {} candidate targets", candidates.len());
        }
        let stats = actor_stats(combat_data.values());

        for target_id in candidates {
            let Some(target_data) = combat_data.get(&target_id) else { continue };
            let battle_time = target_data.last_damage_time - target_data.first_damage_time;
            let idle_time = now_ms - target_data.last_damage_time;
            let is_ended = idle_time >= 10_000;
            let is_periodic = battle_time >= 15_000;
            if !force && !is_ended && !is_periodic {
                continue;
            }

            let details = self.get_target_details(target_id, None);
            records.push(self.build_record(target_data, details, &stats, &mob_data, None));
            // Saved for good only once the next hit would start a new fight:
            // a pause in a boss fight is not its end, and the record is
            // saved again (same id) while the fight goes on.
            if idle_time > IDLE_RESET_MS {
                self.saved_fights.insert((target_id, target_data.first_damage_time), target_data.last_damage_time);
            }
        }

        // Bounded: entries for fights over an hour ago are no use.
        if self.saved_fights.len() > 256 {
            self.saved_fights.retain(|_, &mut last| now_ms - last < 3_600_000);
        }
        records
    }

    /// A boss or a training dummy that you or your party fought, long enough
    /// to keep. A stranger's field boss nearby is not your fight, and was
    /// uploaded as one.
    fn is_saved_fight_target(&self, mob_data: &HashMap<i32, i32>, td: &TargetCombatData) -> bool {
        mob_data.get(&td.target_id).is_some_and(|&code| self.npc_lookup.is_boss(code) || self.npc_lookup.is_training_dummy(code))
            && td.ours
            && td.total_damage > 0
            && td.last_damage_time - td.first_damage_time >= MIN_SAVED_FIGHT_MS
    }

    fn build_record(
        &self,
        target_data: &TargetCombatData,
        details: TargetDetailsResponse,
        stats: &HashMap<i32, (i64, i64, i64, i32)>,
        mob_data: &HashMap<i32, i32>,
        identity: Option<&SegmentIdentity>,
    ) -> FightRecord {
        let target_id = target_data.target_id;
        let party_members = self.data_storage.get_party_members();
        let supporters = self.data_storage.supporters();
        let battle_time = (target_data.last_damage_time - target_data.first_damage_time).max(0);
        let (nickname_data, summon_data_snap, local_id, identity_dungeon) = match identity {
            Some(i) => (i.nicknames.clone(), i.summons.clone(), i.local_player_id, i.dungeon_id),
            None => (
                self.data_storage.get_nicknames(),
                self.data_storage.get_summon_data(),
                self.data_storage.local_player_id(),
                0,
            ),
        };
        // Where the fight happened, not where the player is now.
        let dungeon_id = if target_data.dungeon_id != 0 { target_data.dungeon_id } else { identity_dungeon };

        let mut record_actors: HashMap<i32, (String, String)> = HashMap::new();
        for skill in &details.skills {
            let uid = skill.actor_id;
            record_actors.entry(uid).or_insert_with(|| {
                let nick = resolve_nickname(uid, &nickname_data, &summon_data_snap);
                let job = if !skill.job.is_empty() { skill.job.clone() }
                    else { JobClass::convert_from_skill(skill.code).map(|j| j.class_name().to_string()).unwrap_or_default() };
                (nick, job)
            });
            let entry = record_actors.get_mut(&uid).unwrap();
            if entry.1.is_empty() && !skill.job.is_empty() {
                entry.1 = skill.job.clone();
            }
        }

        let local_id = local_id.unwrap_or(-1) as i32;
        let actors: Vec<DetailsActorSummary> = record_actors.iter()
            .map(|(&id, (nick, job))| {
                let display_nick = if id == local_id {
                    nick.clone()
                } else {
                    crate::entity::fight_record::obscure_nickname(nick)
                };
                let job_class = JobClass::convert_from_skill(
                    details.skills.iter()
                        .find(|s| s.actor_id == id && !s.job.is_empty())
                        .map(|s| s.code)
                        .unwrap_or(0)
                );
                let (party_heal, regen, dmg_recv, hits_recv) = stats.get(&id).copied().unwrap_or_default();
                // Joined on the unobscured nickname: the roster is keyed by
                // name, and `display_nick` above has already been masked for
                // everyone but the local player.
                let roster = party_members.get(nick.as_str());
                DetailsActorSummary {
                    actor_id: id,
                    nickname: display_nick,
                    job: job.clone(),
                    job_id: job_class.map(|j| j.class_prefix()).unwrap_or(0),
                    party_heal,
                    regen,
                    damage_received: dmg_recv,
                    hits_received: hits_recv,
                    dbid: roster.map(|m| m.dbid).unwrap_or(0),
                    server_id: roster.map(|m| m.server_id).unwrap_or(0),
                    is_supporter: supporters
                        .contains(nick, roster.map(|m| m.dbid).unwrap_or(0)),
                    level: roster.map(|m| m.level).unwrap_or(0),
                    gear_score: roster.map(|m| m.gear_score).unwrap_or(0),
                    combat_power: roster.map(|m| m.combat_power).unwrap_or(0),
                }
            })
            .collect();

        let mob_code = mob_data.get(&target_id).copied().unwrap_or(0);
        let boss_name = self.resolve_target_name(target_id);

        let job_ids: Vec<i32> = actors.iter()
            .filter(|a| a.job_id > 0)
            .map(|a| a.job_id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let jobs: Vec<String> = actors.iter()
            .filter(|a| !a.job.is_empty() && a.job != "Unknown")
            .map(|a| a.job.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        FightRecord {
            id: format!("auto_{}_{}", target_id, target_data.first_damage_time),
            boss_name,
            target_id,
            start_time_ms: target_data.first_damage_time,
            duration_ms: battle_time,
            total_damage: target_data.total_damage as i32,
            jobs,
            job_ids,
            details,
            actors,
            is_train: self.npc_lookup.is_training_dummy(mob_code),
            app_version: crate::entity::fight_record::APP_VERSION.to_string(),
            mob_code,
            dungeon_id: fight_dungeon(&self.npc_lookup, mob_code, dungeon_id),
            server_id: self.data_storage.fight_server_id(),
        }
    }
}

/// Healing, regen and damage taken per actor over `targets`.
fn actor_stats<'a>(targets: impl Iterator<Item = &'a TargetCombatData>) -> HashMap<i32, (i64, i64, i64, i32)> {
    let mut stats: HashMap<i32, (i64, i64, i64, i32)> = HashMap::new();
    for td in targets {
        for (&id, ad) in &td.actors {
            let e = stats.entry(id).or_default();
            e.0 += ad.party_heal;
            e.1 += ad.regen;
            e.2 += ad.damage_received;
            e.3 += ad.hits_received;
        }
    }
    stats
}

/// The instance a fight on NPC `mob_code` was in, given the one the fight was
/// stamped with (`stamped`, 0 for none). The NPC table names the instance of
/// many bosses, and that wins: it also places a boss fought after a teleport
/// inside an instance, before the roster names it. Any other target keeps the
/// stamped id (from taengu/A2Tools-DPS-Meter 19b98ba and c6e08a9).
fn fight_dungeon(npcs: &NpcLookup, mob_code: i32, stamped: i32) -> i32 {
    npcs.dungeon_of(mob_code).unwrap_or(stamped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::meter_rows::active_time;
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
    fn world_layers_are_open_world_and_seals_are_not() {
        assert!(crate::combat::data_storage::is_open_world_map(1010), "World_L_A");
        assert!(crate::combat::data_storage::is_open_world_map(101021), "a layer of World_L_A");
        assert!(!crate::combat::data_storage::is_open_world_map(310051), "Seal_Verteron_051");
        assert!(!crate::combat::data_storage::is_open_world_map(600021), "Fire_Temple_Easy");
        assert!(!crate::combat::data_storage::is_open_world_map(999_999_999), "unknown map");

        let s = DataStorage::new();
        s.set_current_dungeon(600021);
        s.note_map_load(310051);
        assert_eq!(s.current_dungeon_id(), 600021, "a seal does not end it");
        s.note_map_load(101021);
        assert_eq!(s.current_dungeon_id(), 0, "a world layer does");
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
}
