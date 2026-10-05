use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use parking_lot::RwLock;

use crate::entity::damage_packet::ParsedDamagePacket;
use crate::entity::job_class::JobClass;

/// Maximum idle gap before a fight is considered ended and a new one begins.
pub const IDLE_RESET_MS: i64 = 30_000;
/// The shortest fight the history keeps.
pub const MIN_SAVED_FIGHT_MS: i64 = 5_000;
/// Ended segments waiting for the auto-save, at most. See `take_ended_segments`.
const MAX_ENDED_SEGMENTS: usize = 32;
/// How far back each actor's per-second damage is kept: the longest
/// "last N minutes" window the meter offers.
pub const DAMAGE_HISTORY_MS: i64 = 900_000;

/// A zone-change auto-reset is ignored if any damage was recorded within this
/// window, so an in-combat self-teleport (boss knockback/pull) can't wipe an
/// active fight. Real zone transitions always follow a travel/load lull.
const ZONE_RESET_LULL_MS: i64 = 1_500;
/// Minimum spacing between two zone-change resets (debounce).
const ZONE_RESET_DEBOUNCE_MS: i64 = 4_000;

/// Capture time while replaying, wall clock while capturing. See `crate::clock`
/// — the idle-reset and zone-reset decisions below are timing decisions, so
/// reading the replaying machine's clock made replays non-deterministic.
/// How long party members who have not fought stay on the meter after the
/// last roster. See `DataStorage::party_placeholders_wanted`.
const PARTY_PLACEHOLDER_MS: i64 = 10 * 60 * 1000;
/// "Has not happened" for the times below. Not 0: a replayed slice's clock
/// starts before 0 (the lead-in runs at negative offsets from the pull), so
/// a 0 there read as "a moment ago" and suppressed every zone reset in the
/// lead-in, which ran a wiped pull's damage into the next pull (Gargaum,
/// 2026-07 capture: 114M derived for a 69M pull).
const NEVER_MS: i64 = i64::MIN;
/// How often, in damage records, unnamed party members are matched to the
/// roster by class. A fight brings a few hundred records a second, so this
/// names them within the first moments of combat.
const ROSTER_BIND_EVERY: u32 = 64;

fn now_ms() -> i64 {
    crate::clock::now_ms()
}

mod aggregates;
mod damage;
mod encounter;
mod entities;
mod heal;
mod identity;
mod names;
mod roster;

pub use aggregates::{
    ActorCombatData, Encounter, EndedSegment, HealSkillData, LocalProfile, NoDamageHit, PartyMember, SecondStats,
    SegmentIdentity, SkillCombatData, TargetCombatData,
};
pub use damage::is_player_skill;
pub use roster::is_open_world_map;

use encounter::{retire_all, with_carry};
use identity::LootIdentity;

// ───── Main storage ─────

pub const DEFAULT_ENCOUNTER_TIMEOUT_MS: i64 = 15_000;
/// A live boss holds an encounter open at most this long after your last hit,
/// so a boss that leaves without dying cannot hold it for good.
const BOSS_HOLD_MAX_MS: i64 = 300_000;

pub struct DataStorage {
    inner: RwLock<Inner>,
    encounter_timeout_ms: AtomicI64,
    damage_generation: AtomicI64,
    /// Wall-clock ms of the last damage record — gates the zone-change lull check.
    last_damage_ms: AtomicI64,
    /// Wall-clock ms of the last honored zone-change reset — debounce.
    last_zone_reset_ms: AtomicI64,
    /// Set when a zone change clears combat; the dps calculator consumes it to
    /// drop its cached snapshot / saved-target state on the next cycle.
    combat_reset_requested: AtomicBool,
}

struct Inner {
    /// Aggregated combat data per target (replaces raw packet storage)
    target_combat: HashMap<i32, TargetCombatData>,
    /// Boss and dummy fights cleared out of `target_combat` before they were
    /// saved. See `take_ended_segments`.
    ended_segments: Vec<EndedSegment>,
    /// Job class detected per actor (across all targets, for summon matching)
    actor_jobs: HashMap<i32, JobClass>,

    nickname_storage: HashMap<i32, String>,
    pending_nicknames: HashMap<i32, String>,
    permanent_nicknames: HashMap<i32, String>,
    summon_storage: HashMap<i32, i32>,
    mob_storage: HashMap<i32, i32>,
    /// Healing done per (healer actor) -> (skill_code, is_hot) -> aggregate.
    heal_storage: HashMap<i32, HashMap<(i32, bool), HealSkillData>>,
    /// Spawn-time / observed-peak MAX HP per entity (denominator for the HP bar).
    mob_hp_data: HashMap<i32, i32>,
    /// Live CURRENT HP per entity, from the in-place `8D <id> 02 01 00 <u32>` feed.
    mob_current_hp: HashMap<i32, i32>,
    known_player_ids: HashSet<i32>,
    /// Ids whose nickname came from an authoritative source (a 45/44 36 player
    /// spawn or the account char-list). Lower-confidence parsers may not steal
    /// such a name onto a different id. Lives parallel to `nickname_storage`:
    /// survives a combat flush, cleared by `reset_nicknames`, evicted alongside.
    authoritative_name_ids: HashSet<i32>,
    confirmed_summon_ids: HashSet<i32>,
    /// Ids that spawned via a `40/41 36` mob/summon spawn (as opposed to a
    /// `44/45 36` player spawn). A real player never spawns this way, so an
    /// entity here that deals class-band damage is a summon / spell-effect
    /// entity — it must not be flagged as a known player.
    summon_spawn_ids: HashSet<i32>,
    /// Entity ids below the usual `>= 100` sanity floor that a spawn or identity
    /// record has proven real. The damage parser uses `>= 100` as a resync gate
    /// while walking varints, which silently discarded every hit from players
    /// whose session entity id happened to be tiny (observed live: an
    /// Elementalist at id 48 lost 939 hits plus all 114 of their pets). Ids
    /// confirmed here are allowed through that gate; unconfirmed low values are
    /// still rejected, so the gate keeps its resync value.
    low_id_entities: HashSet<i32>,
    /// Party roster from the `0x9702` packet, keyed by character name.
    party_members: HashMap<String, PartyMember>,
    /// When the last roster arrived, and whether its members who have not
    /// fought should still get rows. See `party_placeholders_wanted`.
    party_roster_at_ms: i64,
    party_placeholders_hidden: bool,
    /// Damage records since `bind_roster_names_by_class` last ran. Counted in
    /// records rather than time so a replay names players where a live meter
    /// did.
    damage_since_roster_bind: u32,
    /// Instance id the party is in, from the same packet. Encodes the dungeon and
    /// its difficulty tier; resolved to a name by the frontend's dungeon table.
    current_dungeon_id: i32,
    hostile_target_ids: HashSet<i32>,
    dead_entity_ids: HashSet<i32>,
    /// Boss entity IDs identified from NPC DB boss flags
    boss_entity_ids: HashSet<i32>,
    /// Training dummies (scarecrows, punching bags) among the entities spawned,
    /// from the NPC table. Damage on them follows `held_dot_ticks`.
    training_dummy_ids: HashSet<i32>,
    /// On a training dummy, DoT ticks that landed after their actor's latest
    /// direct hit, keyed (target, actor). They are counted when that actor
    /// hits directly again; if the player has stopped attacking, they never
    /// are, and the fight's time ends at the last direct hit. A player asked
    /// for this (issue #6): DoTs ticking on after you stop dragged a training
    /// fight's DPS down. Bosses keep every tick, since there players stop
    /// attacking to dodge.
    held_dot_ticks: HashMap<(i32, i32), Vec<ParsedDamagePacket>>,
    /// Whether the current combat segment has any boss damage
    has_boss_in_segment: bool,
    current_target: i32,

    // Local player
    local_player_id: Option<i64>,
    /// Behind an Arc because `get_dps` reads it every 500ms and the set can hold
    /// thousands of entries; cloning it on each tick would be pure waste.
    supporters: std::sync::Arc<crate::supporters::Roster>,
    local_character_name: Option<String>,
    /// Set once the game itself has said who the local player is (the `33 36`
    /// self record). That outranks the window title and any name the UI has
    /// remembered, which can be a different character entirely. While set,
    /// `local_character_name` is the game's; `None` there means a tutorial
    /// character, which the game names with a `$`-prefixed placeholder until
    /// the player picks a name.
    local_identity_from_game: bool,
    /// Who the loot records (`04 8d` after a kill) say owns the drops, and
    /// what that has been used for. See `note_loot_owner`.
    loot_identity: LootIdentity,
    encounter: Option<Encounter>,
    /// What a boss pull cleared of the open encounter's enemies: the
    /// encounter still counts it. Dropped when the encounter ends.
    encounter_carry: HashMap<i32, TargetCombatData>,
    /// Each character's home server, by name, from the records that state it:
    /// the self record and loot records. See `fight_server_id`.
    player_servers: HashMap<String, u16>,
    /// The class and level your own self record last stated, with the name it
    /// was for, so a character switch does not carry the last one's over.
    self_profile: Option<(String, Option<JobClass>, Option<u32>)>,
}

impl DataStorage {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner {
                target_combat: HashMap::new(),
                ended_segments: Vec::new(),
                actor_jobs: HashMap::new(),
                nickname_storage: HashMap::new(),
                pending_nicknames: HashMap::new(),
                permanent_nicknames: HashMap::new(),
                summon_storage: HashMap::new(),
                mob_storage: HashMap::new(),
                heal_storage: HashMap::new(),
                mob_hp_data: HashMap::new(),
                mob_current_hp: HashMap::new(),
                known_player_ids: HashSet::new(),
                authoritative_name_ids: HashSet::new(),
                confirmed_summon_ids: HashSet::new(),
                summon_spawn_ids: HashSet::new(),
                low_id_entities: HashSet::new(),
                party_members: HashMap::new(),
                party_roster_at_ms: 0,
                damage_since_roster_bind: 0,
                party_placeholders_hidden: false,
                current_dungeon_id: 0,
                hostile_target_ids: HashSet::new(),
                dead_entity_ids: HashSet::new(),
                boss_entity_ids: HashSet::new(),
                training_dummy_ids: HashSet::new(),
                held_dot_ticks: HashMap::new(),
                has_boss_in_segment: false,
                current_target: 0,
                local_player_id: None,
                supporters: std::sync::Arc::new(crate::supporters::Roster::default()),
                local_character_name: None,
                local_identity_from_game: false,
                loot_identity: LootIdentity::default(),
                encounter: None,
                encounter_carry: HashMap::new(),
                player_servers: HashMap::new(),
                self_profile: None,
            }),
            damage_generation: AtomicI64::new(0),
            last_damage_ms: AtomicI64::new(NEVER_MS),
            last_zone_reset_ms: AtomicI64::new(NEVER_MS),
            combat_reset_requested: AtomicBool::new(false),
            encounter_timeout_ms: AtomicI64::new(DEFAULT_ENCOUNTER_TIMEOUT_MS),
        }
    }

    /// Called when a self/world teleport (zone-change opcode) is seen. Resets
    /// combat data only if not in active combat (lull) and not recently reset
    /// (debounce), so the meter starts clean on entering a dungeon/instance
    /// without ever wiping an in-progress fight. Returns true if it reset.
    pub fn note_zone_change(&self) -> bool {
        // The dungeon id is not cleared here: a teleport inside an instance is
        // a load too, and the roster can take minutes to name the instance
        // again. `note_map_load` clears it when the load is into the open world.
        let now = now_ms();
        if now.saturating_sub(self.last_damage_ms.load(Ordering::Relaxed)) < ZONE_RESET_LULL_MS {
            return false; // mid-combat teleport — ignore
        }
        if now.saturating_sub(self.last_zone_reset_ms.load(Ordering::Relaxed)) < ZONE_RESET_DEBOUNCE_MS {
            return false; // already reset moments ago
        }
        {
            let inner = self.inner.read();
            if inner.target_combat.is_empty() {
                return false; // nothing to clear
            }
        }
        self.last_zone_reset_ms.store(now, Ordering::Relaxed);
        // Preserve identity across the reset: a teleport within the same instance
        // keeps everyone's entity ids, so wiping nicknames/known-players/summons
        // would drop your party (and you) to raw ids until they happen to be
        // re-broadcast. Clear only the per-segment damage aggregates.
        self.flush_combat_only();
        self.combat_reset_requested.store(true, Ordering::Relaxed);
        tracing::info!("Zone change detected — combat data reset (identity preserved)");
        true
    }

    /// When the last zone-change combat reset happened (clock ms), `NEVER_MS`
    /// if it has not.
    pub fn last_zone_reset_ms(&self) -> i64 {
        self.last_zone_reset_ms.load(Ordering::Relaxed)
    }

    /// Consumed by the dps calculator to drop its cached snapshot/saved-target
    /// state after a zone-change combat reset.
    pub fn take_combat_reset_requested(&self) -> bool {
        self.combat_reset_requested.swap(false, Ordering::Relaxed)
    }

    pub fn damage_generation(&self) -> i64 {
        self.damage_generation.load(Ordering::Relaxed)
    }

    /// Something the meter shows changed without new damage: recompute.
    fn touch(&self) {
        self.damage_generation.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_current_target(&self, target: i32) {
        self.inner.write().current_target = target;
    }

    pub fn current_target(&self) -> i32 {
        self.inner.read().current_target
    }

    /// Get a snapshot of all target combat aggregates.
    /// This is cheap: clones a small map of aggregates, not raw packets.
    pub fn get_combat_snapshot(&self) -> HashMap<i32, TargetCombatData> {
        self.inner.read().target_combat.clone()
    }

    /// Like `get_combat_snapshot` but without per-skill `hit_timestamps`.
    /// `hit_timestamps` grows unbounded over a fight and is only needed by
    /// `get_target_details`. The 500ms hot paths (`get_dps`,
    /// `get_details_context`, boss auto-save) never read it, so this keeps
    /// their per-tick clone cost flat over fight duration instead of growing
    /// linearly — the root cause of the long-fight FPS drops.
    pub fn get_combat_snapshot_light(&self) -> HashMap<i32, TargetCombatData> {
        let inner = self.inner.read();
        inner.target_combat.iter().map(|(&tid, td)| (tid, light_clone(td))).collect()
    }

    /// `get_combat_snapshot`, with what a boss pull cleared of the open
    /// encounter put back: what ENC reads.
    pub fn get_encounter_snapshot(&self) -> HashMap<i32, TargetCombatData> {
        let inner = self.inner.read();
        with_carry(&inner, inner.target_combat.clone(), TargetCombatData::clone)
    }

    /// `get_combat_snapshot_light` with the encounter's carry, as above.
    pub fn get_encounter_snapshot_light(&self) -> HashMap<i32, TargetCombatData> {
        let inner = self.inner.read();
        let live = inner.target_combat.iter().map(|(&tid, td)| (tid, light_clone(td))).collect();
        with_carry(&inner, live, light_clone)
    }

    /// Clear combat. Who owns which summon is kept: a summon is linked when it
    /// spawns or sends its owner a link record, and one that did that before the
    /// reset would otherwise stay unlinked for good. Ids that are reused get
    /// their links dropped by the new spawn (see `note_summon_spawn`).
    pub fn flush(&self) {
        let mut inner = self.inner.write();
        retire_all(&mut inner);
        inner.encounter = None;
        inner.encounter_carry.clear();
        inner.held_dot_ticks.clear();
        inner.actor_jobs.clear();
        inner.known_player_ids.clear();
        inner.hostile_target_ids.clear();
        inner.dead_entity_ids.clear();
        inner.has_boss_in_segment = false;
        inner.mob_hp_data.clear();
        inner.mob_current_hp.clear();
        inner.heal_storage.clear();
        inner.current_target = 0;
    }

    /// Clear only the per-segment combat/damage aggregates, preserving player
    /// identity: nicknames, known-player ids, summon ownership, and job classes.
    /// Used on a zone-change / in-instance-teleport reset so the meter starts on
    /// clean numbers without dropping who your party and you are.
    pub fn flush_combat_only(&self) {
        let mut inner = self.inner.write();
        retire_all(&mut inner);
        inner.encounter = None;
        inner.encounter_carry.clear();
        inner.held_dot_ticks.clear();
        inner.hostile_target_ids.clear();
        inner.dead_entity_ids.clear();
        inner.has_boss_in_segment = false;
        inner.mob_hp_data.clear();
        inner.mob_current_hp.clear();
        inner.heal_storage.clear();
        inner.current_target = 0;
    }
}

fn light_clone(td: &TargetCombatData) -> TargetCombatData {
    let actors = td
        .actors
        .iter()
        .map(|(&aid, ad)| {
            let skills = ad
                .skills
                .iter()
                .map(|(&k, sd)| (k, sd.clone_light()))
                .collect();
            (
                aid,
                ActorCombatData {
                    total_damage: ad.total_damage,
                    party_heal: ad.party_heal,
                    regen: ad.regen,
                    damage_received: ad.damage_received,
                    hits_received: ad.hits_received,
                    first_damage_time: ad.first_damage_time,
                    last_damage_time: ad.last_damage_time,
                    job: ad.job,
                    skills,
                    by_second: ad.by_second.clone(),
                },
            )
        })
        .collect();
    TargetCombatData {
        target_id: td.target_id,
        total_damage: td.total_damage,
        first_damage_time: td.first_damage_time,
        last_damage_time: td.last_damage_time,
        last_packet_id: td.last_packet_id,
        actors,
        ours: td.ours,
        dungeon_id: td.dungeon_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_window_sums_recent_seconds_only() {
        let dmg = |damage| SecondStats { damage, ..SecondStats::default() };
        let mut a = ActorCombatData::new();
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
        let mut a = ActorCombatData::new();
        a.add_at(100, &SecondStats { damage: 900, hits: 1, crits: 1, max_hit: 900 });
        a.add_at(101, &SecondStats { damage: 300, hits: 1, crits: 0, max_hit: 300 });
        a.add_at(101, &SecondStats { damage: 50, hits: 0, crits: 0, max_hit: 0 });
        assert_eq!(a.stats_since(100_000), SecondStats { damage: 1250, hits: 2, crits: 1, max_hit: 900 });
        assert_eq!(a.stats_since(101_000), SecondStats { damage: 350, hits: 1, crits: 0, max_hit: 300 });
    }

    #[test]
    fn naming_one_of_two_same_class_members_names_the_other_at_once() {
        let s = DataStorage::new();
        let sm = |slot| PartyMember { slot, job: Some(JobClass::Elementalist), ..Default::default() };
        s.set_party_roster(vec![("Thermi".into(), sm(1)), ("Nyxie".into(), sm(2))], true);
        // Two unnamed Spiritmasters, each running a rotation.
        for (actor, base) in [(1792, 16_010_000), (13520, 16_010_000)] {
            for i in 0..4 {
                let mut p = ParsedDamagePacket::new();
                p.set_actor_id(actor);
                p.set_target_id(900);
                p.set_skill_code(base + i * 10_000);
                p.set_damage(100);
                s.append_damage(p);
            }
        }
        assert!(s.get_nickname(13520).is_none(), "two of a class: the roster cannot tell");
        s.append_nickname_authoritative(1792, "Thermi");
        assert_eq!(s.get_nickname(13520).as_deref(), Some("Nyxie"), "the other one, by elimination");
    }

    fn who(s: &DataStorage) -> (Option<i64>, Option<String>, bool) {
        (s.local_player_id(), s.local_character_name(), s.local_identity_from_game())
    }

    #[test]
    fn loot_owner_is_you_until_the_self_record_says_otherwise() {
        let s = DataStorage::new();
        s.set_local_character_name(Some("Aveline".into()));
        s.note_party_scope(1454);
        assert!(s.note_loot_owner(900, 1454, "ApexZ"));
        assert_eq!(who(&s), (Some(1454), Some("ApexZ".into()), true));
        assert!(!s.note_loot_owner(900, 1454, "ApexZ"), "same owner again changes nothing");

        // A zone load brings the self record, which wins.
        assert!(s.set_local_identity_from_game(2001, Some("ApexZ".into())));
        assert!(!s.note_loot_owner(900, 1454, "ApexZ"));
        assert_eq!(who(&s), (Some(2001), Some("ApexZ".into()), true));
    }

    #[test]
    fn loot_naming_two_of_yours_equally_withdraws_the_guess_until_one_leads() {
        let s = DataStorage::new();
        s.note_party_scope(1454);
        s.note_party_scope(3583);
        s.note_loot_owner(900, 1454, "ApexZ");
        assert!(s.note_loot_owner(901, 3583, "Galaaadriel"), "the guess is withdrawn");
        assert_eq!(who(&s), (None, Some("ApexZ".into()), false));
        assert!(!s.note_loot_owner(900, 1454, "ApexZ"), "the same kill again is no new vote");
        assert_eq!(s.local_player_id(), None);
        assert!(s.note_loot_owner(902, 1454, "ApexZ"), "a second kill leads");
        assert_eq!(who(&s), (Some(1454), Some("ApexZ".into()), true));
    }

    /// Issue #12's two orders: you then a bystander, and a bystander then you.
    #[test]
    fn a_bystanders_loot_never_takes_over_whichever_comes_first() {
        let s = DataStorage::new();
        s.set_local_character_name(Some("PlayerA".into()));
        s.note_party_scope(13978);
        assert!(s.note_loot_owner(1, 13978, "PlayerA"));
        assert!(!s.note_loot_owner(2, 15855, "PlayerB"));
        assert_eq!(s.local_player_id(), Some(13978));

        let s = DataStorage::new();
        s.set_local_character_name(Some("PlayerA".into()));
        s.note_party_scope(13978);
        assert!(!s.note_loot_owner(1, 15855, "PlayerB"));
        assert!(s.note_loot_owner(2, 13978, "PlayerA"));
        assert_eq!(s.local_player_id(), Some(13978));
        assert!(!s.local_identity_from_self_record(), "a loot guess is not the game's own word");
    }

    #[test]
    fn a_strangers_kill_says_nothing_about_who_you_are() {
        // Loot from kills by players nearby reaches you too; the server's
        // `06 38` records, which name only you and your party, tell them apart.
        let s = DataStorage::new();
        s.note_party_scope(14957);
        assert!(!s.note_loot_owner(22965, 11937, "Deityclaire"));
        assert!(s.note_loot_owner(46643, 14957, "Naicha"));
        for mob in [40758, 63645, 74428] {
            assert!(!s.note_loot_owner(mob, 892, "Dandelion"));
        }
        assert_eq!(who(&s), (Some(14957), Some("Naicha".into()), true));
    }

    #[test]
    fn your_name_set_by_hand_leaves_old_ids_when_the_game_names_you() {
        let s = DataStorage::new();
        s.set_permanent_nickname(6925, "Seralth");
        s.set_permanent_nickname(6926, "Seralth");
        s.reset_nicknames();
        assert_eq!(s.get_nickname(6925), None, "one id per name");
        // The self record names you on a new id; nothing else touches the old one.
        s.set_local_identity_from_game(7577, Some("Seralth".into()));
        s.reset_nicknames();
        assert_eq!(s.get_nickname(6926), None);
        assert_eq!(s.find_id_by_nickname("Seralth"), Some(7577));
    }

    #[test]
    fn a_name_moving_to_a_new_id_takes_the_kept_name_with_it() {
        let s = DataStorage::new();
        s.set_permanent_nickname(6925, "Seralth");
        s.append_nickname_authoritative(7577, "Seralth");
        s.reset_nicknames();
        assert_eq!(s.get_nickname(6925), None);
    }

    #[test]
    fn you_keep_your_name_through_a_reset() {
        let s = DataStorage::new();
        s.append_nickname_authoritative(13600, "Seralth");
        s.set_local_identity_from_game(13600, Some("Seralth".into()));
        s.append_nickname_authoritative(2001, "Other");
        s.reset_nicknames();
        assert_eq!(s.local_player_id(), Some(13600));
        assert_eq!(s.get_nickname(13600).as_deref(), Some("Seralth"));
        assert_eq!(s.get_nickname(2001), None);
        s.flush();
        assert_eq!(s.get_nickname(13600).as_deref(), Some("Seralth"));
    }

    fn member(slot: u8) -> PartyMember {
        PartyMember { slot, level: 45, gear_score: 3000, combat_power: 39_000, ..Default::default() }
    }

    #[test]
    fn party_members_who_have_not_fought_are_shown_for_a_while() {
        let s = DataStorage::new();
        assert!(!s.party_placeholders_wanted(), "no roster yet");
        s.set_party_roster(vec![("Prenses".into(), member(2)), ("adam".into(), member(3))], true);
        assert!(s.party_placeholders_wanted());

        // A reset clears them until the next roster.
        s.hide_party_placeholders();
        assert!(!s.party_placeholders_wanted());
        s.set_party_roster(vec![("Prenses".into(), member(2)), ("adam".into(), member(3))], true);
        assert!(s.party_placeholders_wanted());

        // And they expire: a dungeon party 15 minutes on is not "your party".
        s.inner.write().party_roster_at_ms -= 15 * 60 * 1000;
        assert!(!s.party_placeholders_wanted());
        assert_eq!(s.get_party_members().len(), 2, "the roster itself is kept, for combat power");
    }

    fn hit(actor: i32, target: i32, at: i64, damage: i32, dot: bool) -> ParsedDamagePacket {
        let mut p = ParsedDamagePacket::new();
        p.set_actor_id(actor);
        p.set_target_id(target);
        p.set_skill_code(11010000);
        p.set_damage(damage);
        p.set_dot(dot);
        p.set_timestamp(at);
        p
    }

    fn of_class(slot: u8, job: JobClass) -> PartyMember {
        PartyMember { job: Some(job), ..member(slot) }
    }

    /// 64 hits from each of `actors`, taking turns, each using `skills`
    /// distinct skills of the class with skill prefix `prefix`.
    fn fight_together(s: &DataStorage, actors: &[i32], prefix: i32, skills: i32) {
        for i in 0..64 {
            for &actor in actors {
                let mut p = hit(actor, 900, i, 100, false);
                p.set_skill_code(prefix * 1_000_000 + 10_000 + (i as i32 % skills) * 10);
                s.append_damage(p);
            }
        }
    }

    fn fight(s: &DataStorage, actor: i32, prefix: i32, skills: i32) {
        fight_together(s, &[actor], prefix, skills);
    }

    #[test]
    fn a_record_without_a_readable_level_keeps_the_known_one() {
        let s = DataStorage::new();
        s.set_local_identity_from_game(14957, Some("Naicha".into()));
        s.note_self_profile("Naicha", Some(JobClass::Cleric), Some(29));
        s.note_self_profile("Naicha", Some(JobClass::Cleric), None);
        assert_eq!(s.local_profile().level, Some(29));
        s.note_self_profile("Naicha", Some(JobClass::Cleric), Some(30));
        assert_eq!(s.local_profile().level, Some(30), "a level-up replaces it");
        s.note_self_profile("Other", None, None);
        s.set_local_identity_from_game(1, Some("Other".into()));
        assert_eq!(s.local_profile().level, None, "another character starts unknown");
    }

    #[test]
    fn a_character_back_as_a_new_entity_keeps_their_damage() {
        let s = DataStorage::new();
        s.append_nickname_authoritative(101, "Cleric");
        s.append_damage(hit(101, 900, 1_000, 500, false));
        s.append_nickname_authoritative(202, "Cleric");
        s.append_damage(hit(202, 900, 2_000, 300, false));
        let snap = s.get_combat_snapshot();
        let boss = &snap[&900];
        assert!(!boss.actors.contains_key(&101));
        assert_eq!(boss.actors[&202].total_damage, 800, "the earlier hits moved with the name");
        assert_eq!(boss.total_damage, 800);

        // A name a fuzzy rule had put on some id says nothing about whose
        // damage that id dealt, so it is not moved.
        let s = DataStorage::new();
        s.append_damage(hit(303, 900, 1_000, 500, false));
        s.append_nickname(303, "Cleric");
        s.append_nickname_authoritative(404, "Cleric");
        assert!(!s.get_combat_snapshot()[&900].actors.contains_key(&404));
    }

    #[test]
    fn a_zone_change_resets_on_a_replays_clock_before_zero() {
        // A slice replays at offsets from the pull, so its lead-in runs at
        // negative times; "never" must still read as long ago there.
        let s = DataStorage::new();
        crate::clock::set_override(Some(-40_000));
        s.append_damage(hit(5, 900, -40_000, 100, false));
        crate::clock::set_override(Some(-30_000));
        assert!(s.note_zone_change(), "a wipe's teleport in the lead-in clears the pull before");
        crate::clock::set_override(None);
    }

    #[test]
    fn fights_are_on_your_server_else_your_partys() {
        let s = DataStorage::new();
        assert_eq!(s.fight_server_id(), 0, "nothing has said");
        let on = |server: u16| PartyMember { server_id: server, ..member(1) };
        s.set_party_roster(vec![("A".into(), on(2304)), ("B".into(), on(2304)), ("C".into(), on(1307))], true);
        assert_eq!(s.fight_server_id(), 2304, "the party's, by majority");
        s.set_local_identity_from_game(7, Some("C".into()));
        assert_eq!(s.fight_server_id(), 1307, "your own place in the roster");
        s.note_player_server("C", 1304);
        assert_eq!(s.fight_server_id(), 1304, "what your own record says");
        s.note_player_server("C", 99);
        assert_eq!(s.fight_server_id(), 1304, "a value no server has is not taken");
    }

    #[test]
    fn a_party_member_is_named_by_class_when_only_one_fits() {
        let s = DataStorage::new();
        s.set_party_roster(
            vec![("Glad".into(), of_class(1, JobClass::Gladiator)), ("Temp".into(), of_class(2, JobClass::Templar))],
            true,
        );
        fight(&s, 101, 11, 6);
        fight(&s, 102, 12, 6);
        assert_eq!(s.get_nickname(101).as_deref(), Some("Glad"));
        assert_eq!(s.get_nickname(102).as_deref(), Some("Temp"));
    }

    #[test]
    fn two_of_a_class_on_either_side_stay_unnamed() {
        // Two Gladiators in the roster, one fighting: either could be them.
        let s = DataStorage::new();
        s.set_party_roster(
            vec![("GladA".into(), of_class(1, JobClass::Gladiator)), ("GladB".into(), of_class(2, JobClass::Gladiator))],
            true,
        );
        fight(&s, 101, 11, 6);
        assert_eq!(s.get_nickname(101), None);

        // One Gladiator in the roster, two with equal rotations fighting.
        let s = DataStorage::new();
        s.set_party_roster(
            vec![("Glad".into(), of_class(1, JobClass::Gladiator)), ("Temp".into(), of_class(2, JobClass::Templar))],
            true,
        );
        fight_together(&s, &[101, 103], 11, 6);
        assert_eq!(s.get_nickname(101), None);
        assert_eq!(s.get_nickname(103), None);

        // Once the other is named by their own spawn, the one left is the match.
        s.append_nickname_authoritative(103, "Stranger");
        fight(&s, 101, 11, 6);
        assert_eq!(s.get_nickname(101).as_deref(), Some("Glad"));
    }

    #[test]
    fn strangers_fighting_alongside_stop_the_match() {
        // Open world: the party's Gladiator plus two players from outside it.
        // Only one is a Gladiator, but with more unnamed players than open
        // roster names the meter cannot know a stranger is not the one.
        let s = DataStorage::new();
        s.set_party_roster(
            vec![("Glad".into(), of_class(1, JobClass::Gladiator)), ("Me".into(), of_class(2, JobClass::Templar))],
            true,
        );
        s.append_nickname_authoritative(100, "Me");
        fight(&s, 100, 12, 6);
        for (actor, prefix) in [(102, 14), (104, 17), (101, 11)] {
            fight(&s, actor, prefix, 6);
        }
        assert_eq!(s.get_nickname(101), None);
    }

    #[test]
    fn a_member_whose_name_is_on_an_entity_is_not_bound_again() {
        let s = DataStorage::new();
        s.append_nickname_authoritative(101, "Glad");
        s.set_party_roster(
            vec![("Glad".into(), of_class(1, JobClass::Gladiator)), ("Temp".into(), of_class(2, JobClass::Templar))],
            true,
        );
        fight(&s, 101, 11, 6);
        fight(&s, 105, 11, 6);
        assert_eq!(s.get_nickname(101).as_deref(), Some("Glad"));
        assert_eq!(s.get_nickname(105), None);
    }

    fn totals(s: &DataStorage, target: i32) -> (i64, i64) {
        let snap = s.get_combat_snapshot();
        let t = &snap[&target];
        (t.total_damage, t.last_damage_time - t.first_damage_time)
    }

    #[test]
    fn a_reset_does_not_forget_the_dummy() {
        // The dummy is known from its spawn, which came before the reset.
        let s = DataStorage::new();
        s.register_training_dummy(500);
        s.flush();
        s.append_damage(hit(1454, 500, 1_000, 100, false));
        s.append_damage(hit(1454, 500, 2_000, 50, true));
        assert_eq!(totals(&s, 500), (100, 0), "ticks after the last hit still wait");
    }

    #[test]
    fn on_a_training_dummy_dot_after_the_last_direct_hit_does_not_count() {
        let s = DataStorage::new();
        s.register_training_dummy(500);
        s.append_damage(hit(1454, 500, 1_000, 100, false));
        s.append_damage(hit(1454, 500, 2_000, 50, true));
        assert_eq!(totals(&s, 500), (100, 0), "the tick waits for the next direct hit");

        s.append_damage(hit(1454, 500, 3_000, 100, false));
        assert_eq!(totals(&s, 500), (250, 2_000), "a direct hit brings the tick in");

        // The player stops; their DoT ticks on.
        s.append_damage(hit(1454, 500, 4_000, 50, true));
        s.append_damage(hit(1454, 500, 5_000, 50, true));
        assert_eq!(totals(&s, 500), (250, 2_000), "time ends at the last direct hit");
    }

    #[test]
    fn on_anything_else_every_dot_tick_counts() {
        let s = DataStorage::new();
        s.append_damage(hit(1454, 600, 1_000, 100, false));
        s.append_damage(hit(1454, 600, 2_000, 50, true));
        assert_eq!(totals(&s, 600), (150, 1_000));
    }

    fn with_skill(mut p: ParsedDamagePacket, skill: i32) -> ParsedDamagePacket {
        p.set_skill_code(skill);
        p
    }

    fn dealt(s: &DataStorage, target: i32, actor: i32) -> i64 {
        s.get_combat_snapshot().get(&target).and_then(|t| t.actors.get(&actor)).map_or(0, |a| a.total_damage)
    }

    #[test]
    fn link_records_link_a_spirit_and_are_not_damage() {
        let s = DataStorage::new();
        s.append_nickname_authoritative(100, "Owner");
        // A spirit with no spawn link hits first; its damage waits under its id.
        s.append_damage(with_skill(hit(500, 900, 1_000, 300, false), 16_010_000));
        assert!(!s.is_summon(500));
        // Spirit to owner.
        s.append_damage(with_skill(hit(500, 100, 1_500, 20, false), 16_990_002));
        assert_eq!(s.get_summon_data().get(&500), Some(&100));
        assert!(s.is_confirmed_summon(500));
        // Owner to spirit.
        s.append_damage(with_skill(hit(100, 501, 1_600, 197, false), 16_770_000));
        assert_eq!(s.get_summon_data().get(&501), Some(&100));

        let snap = s.get_combat_snapshot();
        assert!(!snap.contains_key(&100), "the owner is no target");
        assert!(!snap.contains_key(&501), "nor is the spirit");
        assert_eq!(snap[&900].total_damage, 300);
        assert!(s.get_heal_snapshot().is_empty(), "nor is it healing");
        assert!(!s.is_known_player(500));
    }

    #[test]
    fn a_new_spawn_under_an_id_starts_without_the_old_owner() {
        let s = DataStorage::new();
        s.append_damage(with_skill(hit(500, 100, 1_000, 20, false), 16_990_002));
        s.append_damage(with_skill(hit(500, 900, 1_100, 300, false), 16_010_000));
        // The id comes back as someone else's spirit.
        s.note_summon_spawn(500);
        assert!(!s.is_summon(500));
        assert!(!s.is_confirmed_summon(500));
        assert_eq!(dealt(&s, 900, 100), 300, "the old spirit's damage stays its owner's");
        assert_eq!(dealt(&s, 900, 500), 0);
        s.append_damage(with_skill(hit(500, 200, 2_000, 20, false), 16_990_002));
        s.append_damage(with_skill(hit(500, 900, 2_100, 50, false), 16_010_000));
        assert_eq!(s.get_summon_data().get(&500), Some(&200));
        assert_eq!(dealt(&s, 900, 500), 50);
        assert_eq!(dealt(&s, 900, 100), 300);

        // A player spawn under a summon's id ends the summon too.
        s.note_player_spawn(500);
        assert!(!s.is_summon(500));
        assert_eq!(dealt(&s, 900, 200), 50);
    }

    #[test]
    fn a_link_to_another_owner_keeps_what_the_old_entity_did() {
        // The spawn between the two went unseen.
        let s = DataStorage::new();
        s.append_damage(with_skill(hit(500, 100, 1_000, 20, false), 16_990_002));
        s.append_damage(with_skill(hit(500, 900, 1_100, 300, false), 16_010_000));
        s.append_damage(with_skill(hit(200, 500, 5_000, 197, false), 16_770_000));
        assert_eq!(s.get_summon_data().get(&500), Some(&200));
        assert_eq!(dealt(&s, 900, 100), 300);
        assert_eq!(dealt(&s, 900, 500), 0);
    }

    #[test]
    fn a_summon_using_a_class_skill_is_no_player() {
        let s = DataStorage::new();
        s.register_confirmed_summon_by_id(500, 100);
        s.append_damage(with_skill(hit(500, 900, 1_000, 300, false), 16_010_000));
        assert!(!s.is_known_player(500));
        assert_eq!(s.get_summon_data().get(&500), Some(&100));

        // Spawned as a summon, owner not known yet.
        s.note_summon_spawn(501);
        s.append_damage(with_skill(hit(501, 900, 1_000, 300, false), 16_010_000));
        assert!(!s.is_known_player(501));

        // A link through the summon back to itself is refused.
        s.append_damage(with_skill(hit(100, 500, 1_000, 20, false), 16_990_002));
        assert_eq!(s.get_summon_data().get(&100), None);
    }

    #[test]
    fn summon_links_survive_a_reset() {
        let s = DataStorage::new();
        s.append_damage(with_skill(hit(500, 100, 1_000, 20, false), 16_990_002));
        s.register_confirmed_summon_by_id(501, 100);
        s.note_summon_spawn(502);
        s.flush();
        assert_eq!(s.get_summon_data().get(&500), Some(&100));
        assert_eq!(s.get_summon_data().get(&501), Some(&100));
        assert!(s.get_summon_spawn_ids().contains(&502));
        s.append_damage(with_skill(hit(502, 900, 2_000, 300, false), 16_010_000));
        assert!(!s.is_known_player(502), "still a summon after the reset");

        s.forget_summon_links();
        assert!(s.get_summon_data().is_empty());
    }

    #[test]
    fn a_strangers_boss_hit_does_not_clear_your_fight() {
        let s = DataStorage::new();
        s.set_local_player_id(Some(2259));
        s.register_boss(800);
        s.register_training_dummy(500);
        for t in 0..10 {
            s.append_damage(hit(2259, 500, 1_000 + t * 1_000, 100, false));
        }
        s.append_damage(hit(9000, 800, 12_000, 100, false));
        assert!(s.is_damage_target(500), "a stranger pulling a boss nearby");
        assert!(s.take_ended_segments().is_empty());

        s.append_damage(hit(2259, 800, 13_000, 100, false));
        assert!(!s.is_damage_target(500), "your own pull starts clean");
        let ended = s.take_ended_segments();
        assert_eq!(ended.len(), 1, "the dummy fight before it is kept for saving");
        assert_eq!(ended[0].data.target_id, 500);
        assert_eq!(ended[0].data.total_damage, 1_000);
    }

    #[test]
    fn a_kill_or_a_roster_change_redraws_the_meter() {
        let s = DataStorage::new();
        let before = s.damage_generation();
        s.mark_entity_dead(800);
        assert!(s.damage_generation() > before);
        let before = s.damage_generation();
        s.set_party_roster(vec![("A".into(), member(1))], true);
        assert!(s.damage_generation() > before);
    }

    #[test]
    fn a_configured_name_found_in_the_world_is_kept() {
        let s = DataStorage::new();
        s.set_local_character_name(Some("Misti".into()));
        s.append_nickname_authoritative(4099, "Misti");
        assert_eq!(s.local_player_id(), Some(4099));
        assert!(!s.note_loot_owner(900, 1454, "ApexZ"));
        assert_eq!(who(&s), (Some(4099), Some("Misti".into()), false));
    }
}
