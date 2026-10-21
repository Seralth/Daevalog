//! Everything the meter knows about the fights in this session, behind one lock.

use std::collections::{HashMap, HashSet, VecDeque};
use parking_lot::{Mutex, RwLock};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering::Relaxed};

use crate::capture::decoder::{PetCollectionEvent, ShardEvent};
use crate::entity::job_class::JobClass;

/// A fight segment nobody has hit for longer than this is over.
pub const QUIET_LIMIT_MS: i64 = 30_000;
/// The shortest fight the history keeps.
pub const MIN_SAVED_FIGHT_MS: i64 = 5_000;
/// Ended segments waiting for the auto-save, at most. See `take_ended_segments`.
const MAX_ENDED_SEGMENTS: usize = 32;
/// How far back each actor's per-second damage is kept: the longest
/// "last N minutes" window the meter offers.
pub const DAMAGE_HISTORY_MS: i64 = 900_000;
mod aggregates;
mod buffs;
mod damage;
mod deaths;
mod encounter;
mod enemies;
mod entities;
mod heal;
mod identity;
mod names;
mod numbers;
mod pet_collection;
mod power_shards;
mod roster;
mod taken;
pub use aggregates::{
    ActorTotals, Encounter, EndedSegment, HealTotals, HealTick, SelfCard, NoDamageHit, RosterEntry, SecondStats,
    SegmentIdentity, SkillTotals, TargetTotals,
};
pub use damage::skill_from_player;
#[cfg(test)]
pub(crate) use damage::record;
pub use deaths::{deaths_between, DeathTick, DeathsBy, HpKnown, DEATH_SLACK_MS};
pub use entities::UNATTRIBUTED_ID;
pub use heal::HealsBy;
pub use roster::is_open_world_map;
pub use taken::{add_tick, TakenBy, TakenSkillData, TakenSums, TakenTick};

use encounter::{retire_all, with_carry};
use identity::{LootClaim, OwnRecords};
use roster::MapKind;

pub const DEFAULT_ENCOUNTER_TIMEOUT_MS: i64 = 15_000;
/// A live boss holds an encounter open at most this long after your last hit,
/// so a boss that leaves without dying cannot hold it for good.
const BOSS_HOLD_MAX_MS: i64 = 300_000;

/// A zone change this soon after a damage record is a knockback or a pull
/// inside the fight, not a load.
const ZONE_QUIET_NEEDED_MS: i64 = 1_500;
/// One load can bring several zone changes; only the first one resets.
const ZONE_RESET_GAP_MS: i64 = 4_000;
/// The stamp of something that has not happened yet. A replay's lead-in runs
/// at negative clock times, so 0 would read as "just now" there.
const NOT_YET_MS: i64 = i64::MIN;
/// The party roster leaves placeholder rows on the meter this long.
const GHOST_ROW_MS: i64 = 600_000;
/// Damage records between two tries at naming players from the roster by
/// class. Counted in records, so a replay names them where live did.
const CLASS_MATCH_PERIOD: u32 = 64;

/// Capture time in a replay, the wall clock live.
fn now_ms() -> i64 {
    crate::clock::now_ms()
}

/// The session's combat state. Shared between the capture thread and the
/// meter, so every method takes `&self`.
pub struct DataStorage {
    inner: RwLock<Inner>,
    encounter_timeout_ms: AtomicI64,
    /// Goes up whenever the damage the meter shows may have changed.
    damage_generation: AtomicI64,
    taken_generation: AtomicI64,
    /// Clock time of the newest damage record.
    last_damage_ms: AtomicI64,
    zone_changed_at: AtomicI64,
    /// Set when the meter should start over; read once by the meter.
    combat_reset_requested: AtomicBool,
    player_numbers: Mutex<numbers::PlayerNumbers>,
    buffs: Mutex<buffs::Buffs>,
    /// The storage as it stood before a zone change cleared a boss fight
    /// you or your party pulled, for boss mode to keep on screen (a revive
    /// after a wipe); taken once by the meter.
    before_zone_change: Mutex<Option<Arc<DataStorage>>>,
    /// Pet collection records for the pet soul notices; no fight clears them.
    pet_collection: Mutex<VecDeque<PetCollectionEvent>>,
    /// Your shard hits and counts for the shard notice; no fight clears them.
    shards: Mutex<VecDeque<ShardEvent>>,
}

type Ids = HashSet<i32>;
type ById<T> = HashMap<i32, T>;
type ByName<T> = HashMap<String, T>;

/// Every field starts at its `Default`.
#[derive(Default, Clone)]
struct Inner {
    // Live fights.
    target_combat: ById<TargetTotals>,
    idle_retired: bool,
    ended_segments: Vec<EndedSegment>,
    encounter: Option<Encounter>,
    encounter_carry: ById<TargetTotals>,
    /// Each boss's combat as the server's records give it.
    boss_combat: ById<encounter::BossCombat>,
    current_target: i32,
    has_boss_in_segment: bool,
    /// The last number an actor row took (`next_seq`).
    row_seq: u64,

    // Who did what.
    actor_jobs: ById<JobClass>,
    actor_skills: ById<entities::SkillUse>,
    known_player_ids: Ids,
    authoritative_name_ids: Ids,
    player_spawn_ids: Ids,
    low_id_entities: Ids,

    // Names.
    nickname_storage: ById<String>,
    pending_nicknames: ById<String>,
    permanent_nicknames: ById<String>,

    // Summons and effects: summon id to owner id.
    summon_storage: ById<i32>,
    confirmed_summon_ids: Ids,
    summon_spawn_ids: Ids,
    despawned_summon_ids: Ids,
    effect_parents: ById<i32>,

    // Mobs: entity id to NPC code, max HP, current HP.
    mob_storage: ById<i32>,
    mob_hp_data: ById<i32>,
    mob_current_hp: ById<i32>,
    hostile_target_ids: Ids,
    enemy_ids: Ids,
    dead_entity_ids: Ids,
    boss_entity_ids: Ids,
    training_dummy_ids: Ids,

    // Heals, hits taken, deaths.
    heal_ticks: VecDeque<HealTick>,
    heal_sums: heal::HealSums,
    taken: VecDeque<taken::TakenTick>,
    taken_seq: u64,
    hp_now: ById<i64>,
    deaths: VecDeque<deaths::DeathTick>,
    death_seq: u64,
    hp_known: deaths::HpKnown,
    party_hp_ids: Ids,

    // Party.
    party_members: ByName<RosterEntry>,
    /// The key of the party seated now (`02 97 <key>`).
    party_key: Option<u32>,
    /// You left the party during these fights (target, first hit); it goes
    /// once they are over (`roster::settle_leave`).
    party_left_during: Option<Vec<(i32, i64)>>,
    /// Starts at 0 rather than "never": the replay baselines were made so.
    party_roster_at_ms: i64,
    party_placeholders_hidden: bool,
    damage_since_roster_bind: u32,
    player_servers: ByName<u16>,

    // Where we are.
    current_dungeon_id: i32,
    map_kind: MapKind,
    map_id: i32,
    queued_dungeon_id: i32,

    // Who you are.
    local_player_id: Option<i64>,
    local_character_name: Option<String>,
    self_known_from_game: bool,
    loot_identity: LootClaim,
    own_records: OwnRecords,
    self_profile: Option<(String, Option<JobClass>, Option<u32>)>,
}

impl Inner {
    /// A number no actor row has had yet, for a row that changes.
    fn next_seq(&mut self) -> u64 {
        self.row_seq += 1;
        self.row_seq
    }

    /// End every live fight and drop what only a fight needs.
    fn drop_live_fights(&mut self) {
        // Retiring reads heals, hits taken, deaths and max HP, so it goes first.
        retire_all(self);
        self.current_target = 0;
        self.encounter = None;
        self.encounter_carry.clear();
        self.boss_combat.clear();
        self.has_boss_in_segment = false;
        self.hostile_target_ids.clear();
        self.dead_entity_ids.clear();
        self.mob_current_hp.clear();
        self.mob_hp_data.clear();
        self.heal_ticks.clear();
        self.heal_sums = heal::HealSums::default();
        self.taken.clear();
        self.deaths.clear();
    }

    /// Forget who played which class and skills, and which ids are known
    /// players (`actor_jobs`, `actor_skills`, `known_player_ids`).
    fn forget_who_played(&mut self) {
        self.actor_jobs.clear();
        self.actor_skills.clear();
        self.known_player_ids.clear();
    }

    /// The live fights against `targets`, then with `encounter` what a boss
    /// pull cleared of them for the encounter.
    fn fights_on(&self, targets: &[i32], encounter: bool) -> Vec<&TargetTotals> {
        let carry = targets.iter().filter_map(|t| self.encounter_carry.get(t)).filter(|_| encounter);
        targets.iter().filter_map(|t| self.target_combat.get(t)).chain(carry).collect()
    }

    /// Where the heal, hit-taken and death queues may be cut: the first hit
    /// of the oldest fight still open, or `slack` before `newest` if earlier.
    fn prune_cut(&self, newest: i64, slack: i64) -> i64 {
        let oldest_fight = self
            .target_combat
            .values()
            .chain(self.encounter_carry.values())
            .map(|td| td.first_damage_time)
            .min()
            .unwrap_or(i64::MAX);
        oldest_fight.min(newest.saturating_sub(slack))
    }
}

/// Copies of the live targets `pick` keeps.
fn copy_live(
    inner: &Inner,
    copy: fn(&TargetTotals) -> TargetTotals,
    pick: impl Fn(i32) -> bool,
) -> HashMap<i32, TargetTotals> {
    inner.target_combat.iter().filter(|(id, _)| pick(**id)).map(|(&id, td)| (id, copy(td))).collect()
}

impl DataStorage {
    pub fn new() -> Self {
        let never = || AtomicI64::new(NOT_YET_MS);
        DataStorage {
            inner: RwLock::new(Inner::default()),
            encounter_timeout_ms: AtomicI64::new(DEFAULT_ENCOUNTER_TIMEOUT_MS),
            damage_generation: AtomicI64::default(),
            taken_generation: AtomicI64::default(),
            last_damage_ms: never(),
            zone_changed_at: never(),
            combat_reset_requested: AtomicBool::default(),
            player_numbers: Mutex::default(),
            buffs: Mutex::default(),
            before_zone_change: Mutex::default(),
            pet_collection: Mutex::default(),
            shards: Mutex::default(),
        }
    }

    /// A copy of this storage holding `state`, with no reset asked of it.
    fn copy_with(&self, state: &Inner) -> DataStorage {
        let copy = |n: &AtomicI64| AtomicI64::new(n.load(Relaxed));
        DataStorage {
            inner: RwLock::new(state.clone()),
            encounter_timeout_ms: copy(&self.encounter_timeout_ms),
            damage_generation: copy(&self.damage_generation),
            taken_generation: copy(&self.taken_generation),
            last_damage_ms: copy(&self.last_damage_ms),
            zone_changed_at: copy(&self.zone_changed_at),
            combat_reset_requested: AtomicBool::default(),
            player_numbers: Mutex::new(self.player_numbers.lock().clone()),
            buffs: Mutex::new(self.buffs.lock().clone()),
            before_zone_change: Mutex::default(),
            pet_collection: Mutex::default(),
            shards: Mutex::default(),
        }
    }

    /// Tell the meter the damage it shows may be stale.
    fn touch(&self) {
        self.damage_generation.fetch_add(1, Relaxed);
    }

    pub fn damage_generation(&self) -> i64 {
        self.damage_generation.load(Relaxed)
    }

    /// The local player teleported. Starts the meter over when this is a
    /// zone load: not mid-fight, not twice for one load, and only when there
    /// is a fight to clear. True when it reset.
    pub fn on_zone_switch(&self) -> bool {
        let now = now_ms();
        let elapsed = |stamp: &AtomicI64| now.saturating_sub(stamp.load(Relaxed));
        if elapsed(&self.last_damage_ms) < ZONE_QUIET_NEEDED_MS || elapsed(&self.zone_changed_at) < ZONE_RESET_GAP_MS {
            return false;
        }
        let mut state = self.inner.write();
        let nothing_to_clear = state.target_combat.is_empty() && !state.idle_retired;
        if nothing_to_clear {
            return false;
        }
        // Kept before the fight goes, so the meter never sees the reset
        // without it.
        *self.before_zone_change.lock() = state.has_boss_in_segment.then(|| Arc::new(self.copy_with(&state)));
        // The dungeon id stays: a teleport inside an instance comes here too.
        self.zone_changed_at.store(now, Relaxed);
        state.drop_live_fights();
        drop(state);
        self.combat_reset_requested.store(true, Relaxed);
        tracing::info!("zone change at {now}: combat reset");
        true
    }

    /// Clock time of the last zone reset, `i64::MIN` before the first.
    pub fn zone_changed_at(&self) -> i64 {
        self.zone_changed_at.load(Relaxed)
    }

    /// The storage as it stood before the last zone change cleared a boss
    /// fight, once.
    pub fn take_before_zone_change(&self) -> Option<Arc<DataStorage>> {
        self.before_zone_change.lock().take()
    }

    /// You or your party hit a boss since the last zone change or reset.
    pub fn boss_pulled(&self) -> bool {
        self.inner.read().has_boss_in_segment
    }

    /// Whether a reset was asked for since the last call.
    pub fn reset_was_asked(&self) -> bool {
        self.combat_reset_requested.swap(false, Relaxed)
    }

    pub fn focus_target(&self, target: i32) {
        self.inner.write().current_target = target;
    }

    pub fn focused_target(&self) -> i32 {
        self.inner.read().current_target
    }

    /// Every live target, hit timelines included.
    pub fn copy_targets(&self) -> HashMap<i32, TargetTotals> {
        copy_live(&self.inner.read(), TargetTotals::clone, |_| true)
    }

    /// Every live target without the per-skill hit timelines, which the
    /// meter does not need on each tick.
    pub fn copy_targets_brief(&self) -> HashMap<i32, TargetTotals> {
        copy_live(&self.inner.read(), TargetTotals::shallow_copy, |_| true)
    }

    /// Bring `copy`, made by `copy_targets_brief`, up to date with the live
    /// targets, copying only the actor rows that changed since.
    pub fn refresh_brief(&self, copy: &mut HashMap<i32, TargetTotals>) {
        let state = self.inner.read();
        copy.retain(|id, _| state.target_combat.contains_key(id));
        for (&id, td) in &state.target_combat {
            match copy.get_mut(&id) {
                Some(mine) => mine.catch_up(td),
                None => {
                    copy.insert(id, td.shallow_copy());
                }
            }
        }
    }

    /// The light snapshot with the encounter's earlier segments added back.
    pub fn get_encounter_snapshot_light(&self) -> HashMap<i32, TargetTotals> {
        let state = self.inner.read();
        let light = TargetTotals::shallow_copy;
        with_carry(&state, copy_live(&state, light, |_| true), light, |_| true)
    }

    /// Only `targets`, full or light. With `encounter`, their earlier
    /// segments in the encounter are added back.
    pub fn get_target_snapshots(&self, targets: &[i32], encounter: bool, light: bool) -> HashMap<i32, TargetTotals> {
        let copy: fn(&TargetTotals) -> TargetTotals =
            if light { TargetTotals::shallow_copy } else { TargetTotals::clone };
        let listed = |id: i32| targets.contains(&id);
        let state = self.inner.read();
        let live = copy_live(&state, copy, listed);
        if encounter {
            with_carry(&state, live, copy, listed)
        } else {
            live
        }
    }

    /// Manual reset: end every fight and forget who played what. Names,
    /// summons, marks, the roster and the map stay.
    pub fn flush(&self) {
        let mut state = self.inner.write();
        state.drop_live_fights();
        state.forget_who_played();
        *self.before_zone_change.lock() = None;
    }

    /// As `flush`, but who played which class and skills stays.
    pub fn clear_fight_totals(&self) {
        self.inner.write().drop_live_fights();
        *self.before_zone_change.lock() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Clock;
    use crate::entity::damage_packet::ParsedDamagePacket;

    const N1: &str = "Namone";

    /// A hit with skill 11,010,000, not yet sent.
    fn pkt(from: i32, to: i32, ms: i64, amount: i64, dot: bool) -> ParsedDamagePacket {
        let mut rec = damage::record(from, to, 11_010_000, amount, ms);
        rec.set_dot(dot);
        rec
    }

    fn land(st: &DataStorage, from: i32, to: i32, ms: i64, amount: i64) {
        st.append_damage(pkt(from, to, ms, amount, false));
    }

    pub(super) fn member(slot: u8) -> RosterEntry {
        RosterEntry { slot, level: 45, gear_score: 3000, combat_power: 39_000, ..RosterEntry::default() }
    }

    pub(super) fn member_as(slot: u8, class: JobClass) -> RosterEntry {
        RosterEntry { job: Some(class), ..member(slot) }
    }

    pub(super) fn seat(st: &DataStorage, members: &[(&str, RosterEntry)]) {
        roster::party(st, members, true);
    }

    type Me = (Option<i64>, Option<String>, bool);

    pub(super) fn whoami(st: &DataStorage) -> Me {
        (st.local_player_id(), st.local_character_name(), st.self_known_from_game())
    }

    pub(super) fn me(id: Option<i64>, name: &str, from_game: bool) -> Me {
        (id, Some(name.to_owned()), from_game)
    }

    pub(super) fn call_me(st: &DataStorage, who: &str) {
        st.set_local_character_name(Some(who.to_owned()));
    }

    pub(super) fn game_says(st: &DataStorage, id: i64, who: &str) -> bool {
        st.set_local_identity_from_game(id, Some(who.to_owned()))
    }

    /// The name of `id`, or "" without one.
    fn name(st: &DataStorage, id: i32) -> String {
        st.stored_name(id).unwrap_or_default()
    }

    /// Total and length of a live target's fight.
    fn total_and_span(st: &DataStorage, target: i32) -> (i64, i64) {
        let all = st.copy_targets();
        let one = &all[&target];
        (one.total_damage, one.last_damage_time - one.first_damage_time)
    }

    // ---- storage state ----

    #[test]
    fn fresh_storage_starts_blank() {
        let st = DataStorage::new();
        let blank = (st.damage_generation(), st.focused_target(), st.zone_changed_at());
        assert_eq!(blank, (0, 0, i64::MIN));
        assert!(!st.reset_was_asked());
        assert_eq!(st.copy_targets().len(), 0);
        assert!(!st.on_zone_switch());
        // Nothing was cleared, so no debounce started.
        assert_eq!(st.zone_changed_at(), i64::MIN);
    }

    #[test]
    fn a_second_death_mark_leaves_generation() {
        let st = DataStorage::new();
        st.note_death(800);
        let once = st.damage_generation();
        assert!(once > 0);
        st.note_death(800);
        assert_eq!(once, st.damage_generation());
    }

    #[test]
    fn selected_target_cleared_by_either_reset() {
        let st = DataStorage::new();
        st.focus_target(900);
        assert_eq!(900, st.focused_target());
        st.flush();
        assert_eq!(0, st.focused_target());
        st.focus_target(901);
        st.clear_fight_totals();
        assert_eq!(0, st.focused_target());
    }

    #[test]
    fn light_copy_drops_only_hit_timelines() {
        let st = DataStorage::new();
        land(&st, 1454, 900, 1_000, 100);
        land(&st, 1454, 900, 2_000, 100);
        let full = st.copy_targets();
        let light = st.copy_targets_brief();
        let (f, l) = (&full[&900], &light[&900]);
        assert_eq!((f.total_damage, f.first_damage_time, f.last_damage_time), (l.total_damage, l.first_damage_time, l.last_damage_time));
        let (fa, la) = (&f.actors[&1454], &l.actors[&1454]);
        assert_eq!(fa.total_damage, la.total_damage);
        assert_eq!(fa.by_second, la.by_second);
        let key = (11_010_000, false);
        assert_eq!(fa.skills[&key].hit_timestamps.len(), 2);
        assert_eq!(la.skills[&key].hit_timestamps.len(), 0);
        assert_eq!(fa.skills[&key].hit_count, la.skills[&key].hit_count);
        assert_eq!(fa.skills[&key].total_damage, la.skills[&key].total_damage);
    }

    /// You hit 600, then boss 800: the boss pull moves 600 out of the live data.
    fn boss_pull_after_trash() -> DataStorage {
        let st = DataStorage::new();
        st.set_local_player_id(Some(2259));
        st.register_boss(800);
        land(&st, 2259, 600, 1_000, 100);
        land(&st, 2259, 800, 2_000, 100);
        st
    }

    #[test]
    fn encounter_view_puts_back_carried_targets() {
        let st = boss_pull_after_trash();
        let live: Vec<i32> = st.copy_targets_brief().into_keys().collect();
        assert_eq!(live, [800]);
        let whole = st.get_encounter_snapshot_light();
        assert_eq!(100, whole[&600].total_damage);
        assert!(whole.contains_key(&800));
    }

    #[test]
    fn picked_targets_copy_only_what_was_asked() {
        let st = boss_pull_after_trash();
        let ids = |list: &[i32], enc, light| {
            let mut got: Vec<i32> = st.get_target_snapshots(list, enc, light).into_keys().collect();
            got.sort();
            got
        };
        assert_eq!(ids(&[600], true, true), [600]);
        assert!(ids(&[600], false, true).is_empty());
        assert!(ids(&[], true, true).is_empty());
        let full = st.get_target_snapshots(&[800], false, false);
        let timeline = &full[&800].actors[&2259].skills[&(11_010_000, false)].hit_timestamps;
        assert_eq!((full.len(), timeline.len()), (1, 1));
    }

    #[test]
    fn manual_reset_saves_the_dummy_fight_with_max_hp() {
        let st = DataStorage::new();
        st.flag_dummy(500);
        st.remember_enemy_hp(500, 50_000);
        land(&st, 1454, 500, 1_000, 100);
        land(&st, 1454, 500, 7_000, 100);
        st.flush();
        let saved: Vec<(i32, i64, i32)> =
            st.take_ended_segments().iter().map(|seg| (seg.data.target_id, seg.data.total_damage, seg.max_hp)).collect();
        assert_eq!(saved, [(500, 200, 50_000)]);
    }

    #[test]
    fn combat_only_reset_keeps_known_players() {
        let st = DataStorage::new();
        st.name_entity(300, N1);
        assert!(st.seen_as_player(300));
        st.clear_fight_totals();
        assert!(st.seen_as_player(300) && name(&st, 300) == N1);
        st.flush();
        assert!(!st.seen_as_player(300) && name(&st, 300) == N1);
    }

    #[test]
    fn resets_leave_generation_and_request_alone() {
        let st = DataStorage::new();
        land(&st, 1454, 900, 1_000, 100);
        let before = st.damage_generation();
        st.clear_fight_totals();
        st.flush();
        assert_eq!(before, st.damage_generation());
        assert!(!st.reset_was_asked());
    }

    #[test]
    fn zone_change_waits_for_a_lull_then_debounces() {
        let st = DataStorage::new();
        let clock = Clock::at(100_000);
        land(&st, 1454, 900, 100_000, 100);
        clock.to(101_499);
        assert!(!st.on_zone_switch(), "inside the lull");
        assert_eq!(i64::MIN, st.zone_changed_at());
        assert_eq!(st.copy_targets().len(), 1);
        clock.to(101_500);
        assert!(st.on_zone_switch());

        clock.to(101_600);
        land(&st, 1454, 901, 101_600, 100);
        clock.to(105_499);
        assert!(!st.on_zone_switch(), "inside the debounce");
        assert_eq!(101_500, st.zone_changed_at());
        assert_eq!(st.copy_targets().len(), 1);
        clock.to(105_500);
        assert!(st.on_zone_switch());
        assert_eq!(105_500, st.zone_changed_at());
    }

    #[test]
    fn zone_change_after_idle_retire_still_resets() {
        let st = DataStorage::new();
        let clock = Clock::at(1_000_000);
        st.set_local_player_id(Some(1));
        // A stranger's fight: no encounter opens for it.
        land(&st, 2000, 900, 1_000_000, 100);
        assert_eq!(1, st.retire_idle(1_031_000, |_, _| false));
        assert_eq!(st.copy_targets().len(), 0);
        clock.to(1_031_000);
        assert!(st.on_zone_switch());
    }

    #[test]
    fn zone_reset_clears_fights_but_not_names_or_dungeon() {
        let st = DataStorage::new();
        let clock = Clock::at(50_000);
        st.enter_dungeon(600011);
        st.name_entity(300, N1);
        land(&st, 300, 900, 50_000, 100);
        clock.to(60_000);
        assert!(st.on_zone_switch());
        let requests = [st.reset_was_asked(), st.reset_was_asked()];
        assert_eq!(requests, [true, false]);
        assert_eq!(60_000, st.zone_changed_at());
        assert_eq!(st.copy_targets().len(), 0);
        assert!(st.seen_as_player(300) && name(&st, 300) == N1);
        assert_eq!(600011, st.current_dungeon_id());
    }

    #[test]
    fn zone_reset_works_at_negative_clock() {
        let st = DataStorage::new();
        let clock = Clock::at(-40_000);
        land(&st, 5, 900, -40_000, 100);
        clock.to(-30_000);
        assert!(st.on_zone_switch());
    }

    // ---- damage and fights ----

    #[test]
    fn fight_is_ours_only_after_local_id_known() {
        let st = DataStorage::new();
        let ours = |st: &DataStorage, t: i32| st.copy_targets().get(&t).map(|f| f.ours);
        land(&st, 2000, 900, 0, 100);
        assert_eq!(ours(&st, 900), Some(false));
        st.set_local_player_id(Some(1000));
        land(&st, 2000, 901, 1_000, 100);
        land(&st, 1000, 902, 2_000, 100);
        assert_eq!((ours(&st, 901), ours(&st, 902)), (Some(false), Some(true)));
    }

    #[test]
    fn dummy_mark_survives_flush() {
        let st = DataStorage::new();
        st.flag_dummy(500);
        st.flush();
        for mob in [500, 600] {
            land(&st, 1454, mob, 1_000, 100);
            land(&st, 1454, mob, 7_000, 100);
        }
        st.flush();
        let saved: Vec<i32> = st.take_ended_segments().into_iter().map(|seg| seg.data.target_id).collect();
        assert_eq!(saved, [500]);
    }

    #[test]
    fn dummy_counts_trailing_dot() {
        let st = DataStorage::new();
        st.flag_dummy(500);
        land(&st, 1454, 500, 1_000, 100);
        for ms in [2_000, 3_000] {
            st.append_damage(pkt(1454, 500, ms, 148, true));
        }
        assert_eq!((396, 2_000), total_and_span(&st, 500));
    }

    #[test]
    fn ordinary_target_counts_every_dot() {
        let st = DataStorage::new();
        land(&st, 1454, 600, 1_000, 100);
        st.append_damage(pkt(1454, 600, 2_000, 50, true));
        assert_eq!((150, 1_000), total_and_span(&st, 600));
    }

    #[test]
    fn stranger_boss_pull_keeps_our_fight() {
        let st = DataStorage::new();
        st.set_local_player_id(Some(2259));
        st.register_boss(800);
        st.flag_dummy(500);
        for ms in (1_000..=10_000).step_by(1_000) {
            land(&st, 2259, 500, ms, 100);
        }
        land(&st, 9000, 800, 12_000, 100);
        assert!(st.was_hit(500));
        assert_eq!(st.take_ended_segments().len(), 0);
        land(&st, 2259, 800, 13_000, 100);
        assert!(!st.was_hit(500));
        let saved: Vec<(i32, i64)> =
            st.take_ended_segments().iter().map(|seg| (seg.data.target_id, seg.data.total_damage)).collect();
        assert_eq!(saved, [(500, 1_000)]);
    }

    #[test]
    fn death_and_roster_bump_generation() {
        let st = DataStorage::new();
        let start = st.damage_generation();
        st.note_death(800);
        let after_death = st.damage_generation();
        assert!(after_death > start);
        seat(&st, &[(N1, member(1))]);
        assert!(st.damage_generation() > after_death);
    }

    // ---- kept ----

    #[test]
    fn damage_window_sums_recent_seconds_only() {
        let dmg = |damage| SecondStats { damage, ..SecondStats::default() };
        let mut a = ActorTotals::new();
        a.add_at(100, &dmg(10));
        a.add_at(100, &dmg(5));
        a.add_at(103, &dmg(20));
        a.add_at(101, &dmg(7)); // out of order
        let damages: Vec<(i64, i64)> = a.by_second.iter().map(|&(s, st)| (s, st.damage)).collect();
        assert_eq!(damages, vec![(100, 15), (101, 7), (103, 20)]);
        assert_eq!(a.damage_since(101_000), 27);
        assert_eq!(a.damage_since(100_000), 42);
        assert_eq!(a.damage_since(104_000), 0);
        // Seconds older than the history are dropped.
        a.add_at(100 + DAMAGE_HISTORY_MS / 1000 + 2, &dmg(1));
        assert_eq!(a.by_second.front(), Some(&(103, dmg(20))));
    }

    #[test]
    fn hit_window_counts_direct_hits_crits_and_the_biggest_hit() {
        let mut a = ActorTotals::new();
        a.add_at(100, &SecondStats { damage: 900, hits: 1, crits: 1, max_hit: 900 });
        a.add_at(101, &SecondStats { damage: 300, hits: 1, crits: 0, max_hit: 300 });
        a.add_at(101, &SecondStats { damage: 50, hits: 0, crits: 0, max_hit: 0 });
        assert_eq!(a.stats_since(100_000), SecondStats { damage: 1250, hits: 2, crits: 1, max_hit: 900 });
        assert_eq!(a.stats_since(101_000), SecondStats { damage: 350, hits: 1, crits: 0, max_hit: 300 });
    }

    fn with_skill(mut p: ParsedDamagePacket, skill: i32) -> ParsedDamagePacket {
        p.set_skill_code(skill);
        p
    }

    pub(super) fn dealt(s: &DataStorage, target: i32, actor: i32) -> i64 {
        s.copy_targets().get(&target).and_then(|t| t.actors.get(&actor)).map_or(0, |a| a.total_damage)
    }

    #[test]
    fn party_heal_lands_on_the_fight_the_healer_was_in_last() {
        let s = DataStorage::new();
        // You and a party member, on fifty mobs; mob 830 last.
        for (i, t) in (800..850).filter(|&t| t != 830).chain([830]).enumerate() {
            s.append_mob(t, 1);
            s.append_damage(pkt(100, t, 1_000 + i as i64, 500, false));
            s.append_damage(pkt(200, t, 1_000 + i as i64, 500, false));
        }
        s.append_damage(with_skill(pkt(200, 100, 3_200, 400, false), 18_120_000));
        let snapshot = s.copy_targets_brief();
        assert_eq!(snapshot[&830].actors[&200].party_heal, 400);
        assert_eq!(snapshot.values().map(|t| t.actors[&200].party_heal).sum::<i64>(), 400);
    }
}
