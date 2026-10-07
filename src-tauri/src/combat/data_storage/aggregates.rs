//! Combat aggregates: per target, actor and skill, and the segments and encounters made of them.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::entity::job_class::JobClass;

use super::DAMAGE_HISTORY_MS;

/// Healing done, aggregated per (healer actor, skill, is_hot). Healing is keyed by
/// the HEALER (not the boss target), since the meter shows "healing done" per player.
#[derive(Debug, Clone, Default)]
pub struct HealSkillData {
    pub total_heal: i64,
    pub tick_count: i32,
}

/// One heal tick, when it landed.
#[derive(Debug, Clone, Copy)]
pub struct HealTick {
    pub at: i64,
    pub actor: i32,
    pub skill: i32,
    pub is_hot: bool,
    pub amount: i64,
}

/// A hit the game reports with no damage, by its hit type (`EHitType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoDamageHit {
    /// Hit type 1.
    Miss,
    /// Hit type 6. In the 2026-10-04 captures nearly every one comes with a
    /// damage record of the same skill on the same target: what was resisted
    /// is the skill's effect, not its damage.
    Resist,
}

impl NoDamageHit {
    pub fn from_hit_type(hit_type: i32) -> Option<Self> {
        match hit_type {
            1 => Some(Self::Miss),
            6 => Some(Self::Resist),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SkillCombatData {
    pub skill_code: i32,
    pub is_dot: bool,
    pub hit_count: i32,
    pub total_damage: i64,
    pub min_damage: i64,
    pub max_damage: i64,
    pub crit_count: i32,
    pub back_count: i32,
    pub frontal_count: i32,
    pub shield_block_count: i32,
    pub parry_count: i32,
    pub perfect_count: i32,
    pub double_count: i32,
    pub iron_wall_count: i32,
    pub regeneration_count: i32,
    pub perfect_block_count: i32,
    /// Hits with no damage: hit type 1 (Miss) and 6 (Resist). Not in
    /// `hit_count`.
    pub miss_count: i32,
    pub resist_count: i32,
    pub multi_hit_count: i32,
    pub multi_hit_damage: i64,
    pub multi_hit_hits: i32,
    pub heal_amount: i64,
    pub hit_timestamps: Vec<i64>,
    pub spec_flags: [bool; 5],
}

impl SkillCombatData {
    /// Clone every aggregate field but leave `hit_timestamps` empty.
    /// The timestamp Vec grows by one entry per hit (unbounded over a long
    /// fight) and is only ever consumed by `get_target_details` (the details
    /// panel chart). Every other consumer clones it for nothing, so the hot
    /// 500ms paths use this to keep per-tick clone cost flat over fight time.
    /// Spelled out manually rather than `Vec::new(), ..self.clone()` because
    /// the latter would copy `hit_timestamps` only to throw it away.
    pub(super) fn clone_light(&self) -> Self {
        Self {
            skill_code: self.skill_code,
            is_dot: self.is_dot,
            hit_count: self.hit_count,
            total_damage: self.total_damage,
            min_damage: self.min_damage,
            max_damage: self.max_damage,
            crit_count: self.crit_count,
            back_count: self.back_count,
            frontal_count: self.frontal_count,
            shield_block_count: self.shield_block_count,
            parry_count: self.parry_count,
            perfect_count: self.perfect_count,
            double_count: self.double_count,
            iron_wall_count: self.iron_wall_count,
            regeneration_count: self.regeneration_count,
            perfect_block_count: self.perfect_block_count,
            miss_count: self.miss_count,
            resist_count: self.resist_count,
            multi_hit_count: self.multi_hit_count,
            multi_hit_damage: self.multi_hit_damage,
            multi_hit_hits: self.multi_hit_hits,
            heal_amount: self.heal_amount,
            hit_timestamps: Vec::new(),
            spec_flags: self.spec_flags,
        }
    }

    /// Add `other`'s hits to these: the same skill, recorded under two ids.
    pub(super) fn absorb(&mut self, other: SkillCombatData) {
        self.hit_count = self.hit_count.saturating_add(other.hit_count);
        self.total_damage = self.total_damage.saturating_add(other.total_damage);
        self.min_damage = self.min_damage.min(other.min_damage);
        self.max_damage = self.max_damage.max(other.max_damage);
        self.crit_count = self.crit_count.saturating_add(other.crit_count);
        self.back_count = self.back_count.saturating_add(other.back_count);
        self.frontal_count = self.frontal_count.saturating_add(other.frontal_count);
        self.shield_block_count = self.shield_block_count.saturating_add(other.shield_block_count);
        self.parry_count = self.parry_count.saturating_add(other.parry_count);
        self.perfect_count = self.perfect_count.saturating_add(other.perfect_count);
        self.double_count = self.double_count.saturating_add(other.double_count);
        self.iron_wall_count = self.iron_wall_count.saturating_add(other.iron_wall_count);
        self.regeneration_count = self.regeneration_count.saturating_add(other.regeneration_count);
        self.perfect_block_count = self.perfect_block_count.saturating_add(other.perfect_block_count);
        self.miss_count = self.miss_count.saturating_add(other.miss_count);
        self.resist_count = self.resist_count.saturating_add(other.resist_count);
        self.multi_hit_count = self.multi_hit_count.saturating_add(other.multi_hit_count);
        self.multi_hit_damage = self.multi_hit_damage.saturating_add(other.multi_hit_damage);
        self.multi_hit_hits = self.multi_hit_hits.saturating_add(other.multi_hit_hits);
        self.heal_amount = self.heal_amount.saturating_add(other.heal_amount);
        self.hit_timestamps.extend(other.hit_timestamps);
        self.hit_timestamps.sort_unstable();
        for (mine, theirs) in self.spec_flags.iter_mut().zip(other.spec_flags) {
            *mine |= theirs;
        }
    }

    pub(super) fn new(skill_code: i32, is_dot: bool) -> Self {
        Self {
            skill_code,
            is_dot,
            hit_count: 0,
            total_damage: 0,
            min_damage: i64::MAX,
            max_damage: 0,
            crit_count: 0,
            back_count: 0,
            frontal_count: 0,
            shield_block_count: 0,
            parry_count: 0,
            perfect_count: 0,
            double_count: 0,
            iron_wall_count: 0,
            regeneration_count: 0,
            perfect_block_count: 0,
            miss_count: 0,
            resist_count: 0,
            multi_hit_count: 0,
            multi_hit_damage: 0,
            multi_hit_hits: 0,
            heal_amount: 0,
            hit_timestamps: Vec::new(),
            spec_flags: [false; 5],
        }
    }
}

/// Who the local player is playing. See `DataStorage::local_profile`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LocalProfile {
    pub name: Option<String>,
    /// 0 when unknown.
    pub server_id: u16,
    pub class: Option<JobClass>,
    pub level: Option<u32>,
}

/// One entry of the party roster packet (`0x9702`). Keyed by character name,
/// because the roster carries the account-level `dbid` rather than the
/// session-scoped entity id — the name is the only field that joins it to the
/// in-world entities the meter tracks.
#[derive(Debug, Clone, Default)]
pub struct PartyMember {
    /// 1-based party slot.
    pub slot: u8,
    pub level: i32,
    /// Equipment item level ("gear score").
    pub gear_score: i32,
    /// Combat power — the number the game shows on the character sheet.
    pub combat_power: i64,
    /// World/server id (the bracket tag next to a cross-server player's name).
    /// This is the top `u16` of `dbid`, kept separately because the roster parse
    /// anchors on it.
    pub server_id: u16,
    /// The roster's own id for this member, server-assigned and stable across
    /// renames — the whole 64 bits, of which `server_id` is the top sixteen.
    ///
    /// Kept because it is the only identifier here that is *not* a name. Log
    /// sharing needs to say "this row is the same person as that row" without
    /// putting a character name on the wire, and a name cannot do that job: it
    /// changes on rename, and it is re-usable by a stranger once freed, which
    /// would silently hand them the previous owner's consent.
    pub dbid: u64,
    /// The member's class, as the roster states it. Lets a member be named
    /// before their entity id is known: see `bind_roster_names_by_class`.
    pub job: Option<JobClass>,
}

#[derive(Debug, Clone)]
pub struct ActorCombatData {
    pub total_damage: i64,
    pub party_heal: i64,
    pub regen: i64,
    /// i64::MAX until the first hit.
    pub first_damage_time: i64,
    pub last_damage_time: i64,
    pub job: Option<JobClass>,
    /// Skills keyed by (raw_skill_code, is_dot)
    pub skills: HashMap<(i32, bool), SkillCombatData>,
    /// Damage and hits per unix second, oldest first, for the meter's
    /// windows (last N minutes, an encounter, last 10/30/60 s). Kept for
    /// DAMAGE_HISTORY_MS.
    pub by_second: VecDeque<(i64, SecondStats)>,
}

/// One second of an actor's damage on a target.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SecondStats {
    pub damage: i64,
    /// Direct hits; ticks over time are left out, as in the details panel.
    pub hits: i64,
    pub crits: i64,
    /// The biggest direct hit, as the details panel's per-skill max.
    pub max_hit: i64,
}

impl SecondStats {
    pub fn add(&mut self, other: &SecondStats) {
        self.damage += other.damage;
        self.hits += other.hits;
        self.crits += other.crits;
        self.max_hit = self.max_hit.max(other.max_hit);
    }
}

impl ActorCombatData {
    /// Add everything `other` recorded: one character, under an old entity id.
    pub(super) fn absorb(&mut self, other: ActorCombatData) {
        self.total_damage = self.total_damage.saturating_add(other.total_damage);
        self.party_heal = self.party_heal.saturating_add(other.party_heal);
        self.regen = self.regen.saturating_add(other.regen);
        self.first_damage_time = self.first_damage_time.min(other.first_damage_time);
        self.last_damage_time = self.last_damage_time.max(other.last_damage_time);
        self.job = self.job.or(other.job);
        for (sec, stats) in other.by_second {
            self.add_at(sec, &stats);
        }
        for (key, skill) in other.skills {
            match self.skills.get_mut(&key) {
                Some(mine) => mine.absorb(skill),
                None => {
                    self.skills.insert(key, skill);
                }
            }
        }
    }

    pub(super) fn new() -> Self {
        Self {
            total_damage: 0,
            party_heal: 0,
            regen: 0,
            first_damage_time: i64::MAX,
            last_damage_time: 0,
            job: None,
            skills: HashMap::new(),
            by_second: VecDeque::new(),
        }
    }

    /// Damage dealt in the window starting at `since_ms` (unix ms).
    pub fn damage_since(&self, since_ms: i64) -> i64 {
        self.stats_since(since_ms).damage
    }

    /// Damage and hits in the window starting at `since_ms` (unix ms).
    pub fn stats_since(&self, since_ms: i64) -> SecondStats {
        let since = since_ms.div_euclid(1000);
        let mut out = SecondStats::default();
        for (_, stats) in self.by_second.iter().rev().take_while(|&&(sec, _)| sec >= since) {
            out.add(stats);
        }
        out
    }

    /// Hits over all the actor's skills, with no window, so not limited to
    /// DAMAGE_HISTORY_MS.
    pub fn stats_total(&self) -> SecondStats {
        let mut out = SecondStats { damage: self.total_damage, ..SecondStats::default() };
        for skill in self.skills.values().filter(|s| !s.is_dot) {
            out.hits += skill.hit_count as i64;
            out.crits += skill.crit_count as i64;
            out.max_hit = out.max_hit.max(skill.max_damage);
        }
        out
    }

    pub(super) fn add_at(&mut self, sec: i64, stats: &SecondStats) {
        match self.by_second.iter().rposition(|&(s, _)| s <= sec) {
            Some(i) if self.by_second[i].0 == sec => self.by_second[i].1.add(stats),
            Some(i) => self.by_second.insert(i + 1, (sec, *stats)),
            None => self.by_second.push_front((sec, *stats)),
        }
        let oldest = sec - DAMAGE_HISTORY_MS / 1000;
        while self.by_second.front().is_some_and(|&(s, _)| s < oldest) {
            self.by_second.pop_front();
        }
    }
}

#[derive(Debug, Clone)]
pub struct TargetCombatData {
    pub target_id: i32,
    pub total_damage: i64,
    pub first_damage_time: i64,
    pub last_damage_time: i64,
    pub last_packet_id: i64,
    /// Per raw-actor aggregated combat data
    pub actors: HashMap<i32, ActorCombatData>,
    /// You or your party hit it, while the meter knew who you are. Decided at
    /// the hit: a zone load gives you a new id before the fight is saved.
    pub ours: bool,
    /// The instance the segment was fought in, as the party roster last named
    /// it at one of its hits; 0 in the open world.
    pub dungeon_id: i32,
    /// Hit while the last map load was into the open world.
    pub open_world: bool,
}

impl TargetCombatData {
    /// Several targets as one, for details over everything a multi-target
    /// mode shows. Target id 0; each actor keeps one entry across the targets.
    pub fn merged<'a>(targets: impl IntoIterator<Item = &'a TargetCombatData>) -> Option<Self> {
        let mut out: Option<Self> = None;
        for td in targets {
            let m = out.get_or_insert_with(|| Self::new(0, td.first_damage_time));
            m.total_damage += td.total_damage;
            m.first_damage_time = m.first_damage_time.min(td.first_damage_time);
            m.last_damage_time = m.last_damage_time.max(td.last_damage_time);
            m.ours |= td.ours;
            m.open_world |= td.open_world;
            if td.dungeon_id != 0 {
                m.dungeon_id = td.dungeon_id;
            }
            for (&actor, data) in &td.actors {
                m.actors.entry(actor).or_insert_with(ActorCombatData::new).absorb(data.clone());
            }
        }
        out
    }

    pub(super) fn new(target_id: i32, timestamp: i64) -> Self {
        Self {
            target_id,
            total_damage: 0,
            first_damage_time: timestamp,
            last_damage_time: timestamp,
            last_packet_id: -1,
            actors: HashMap::new(),
            ours: false,
            dungeon_id: 0,
            open_world: false,
        }
    }
}

/// A fight segment taken out of the live data (an idle restart, a boss
/// pull, a reset or a zone change) before the auto-save wrote it.
#[derive(Debug, Clone)]
pub struct EndedSegment {
    pub data: TargetCombatData,
    pub max_hp: i32,
    pub heals: HashMap<i32, HashMap<(i32, bool), HealSkillData>>,
    /// The damage taken during it, per player and skill.
    pub taken: super::TakenBy,
    pub identity: SegmentIdentity,
}

/// Who was who when a segment ended. A zone load hands every entity id out
/// again, so a record built later from the live tables gave your spirits'
/// damage to whoever holds your name now.
#[derive(Debug, Clone, Default)]
pub struct SegmentIdentity {
    pub summons: HashMap<i32, i32>,
    pub nicknames: HashMap<i32, String>,
    pub local_player_id: Option<i64>,
    pub dungeon_id: i32,
}

/// A stretch of combat by you or your party: from the first hit dealt or
/// taken until the timeout passes without one. A boss you hit that is still
/// alive holds it open, up to `BOSS_HOLD_MAX_MS`.
#[derive(Debug, Clone, Default)]
pub struct Encounter {
    pub start: i64,
    /// Last hit dealt or taken by you or your party.
    pub last_ours: i64,
    /// Last hit by anyone on one of its targets.
    pub last_any: i64,
    /// The enemies fought: hit by you or your party, or hitting you.
    pub targets: HashSet<i32>,
    /// Opened before the meter knew you, when anyone's hit counted as yours.
    pub blind: bool,
}

impl Encounter {
    pub fn duration(&self) -> i64 {
        (self.last_any.max(self.last_ours) - self.start).max(0)
    }
}
