//! Combat aggregates: per target, actor and skill, and the segments and encounters made of them.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::entity::detail_bundle::HealAt;
use crate::entity::job_class::JobClass;

use super::DAMAGE_HISTORY_MS;

/// The healing one healer did with one skill (and HoT or not).
#[derive(Debug, Clone, Default)]
pub struct HealTotals {
    pub total_heal: i64,
    pub tick_count: i32,
    /// Every tick, oldest first.
    pub ticks: Vec<HealAt>,
}

/// One heal tick, when it landed.
#[derive(Debug, Clone, Copy)]
pub struct HealTick {
    pub at: i64,
    pub actor: i32,
    /// Whom it healed.
    pub target: i32,
    pub skill: i32,
    pub is_hot: bool,
    pub amount: i64,
}

/// A hit the game reports with no damage, by its hit type (`EHitType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoDamageHit {
    /// Hit type 1.
    Miss,
    /// Hit type 6. Nearly every one comes with a
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
    pub fn add(&mut self, more: &SecondStats) {
        let SecondStats { damage, hits, crits, max_hit } = *more;
        *self = SecondStats {
            damage: self.damage + damage,
            hits: self.hits + hits,
            crits: self.crits + crits,
            max_hit: max_hit.max(self.max_hit),
        };
    }
}

/// One actor's use of one skill on one target, ticks over time apart from
/// direct hits: the damage, and how the hits landed.
#[derive(Debug, Clone)]
pub struct SkillTotals {
    pub skill_code: i32,
    pub is_dot: bool,
    /// Records counted, ticks included; no-damage hits are not.
    pub hit_count: i32,
    pub total_damage: i64,
    /// Smallest and biggest single hit, multi-hit damage left out.
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
    pub miss_count: i32,
    pub resist_count: i32,
    /// Records with extra hits, their damage, and how many extra hits.
    pub multi_hit_count: i32,
    pub multi_hit_damage: i64,
    pub multi_hit_hits: i32,
    pub heal_amount: i64,
    /// When each record landed, oldest first.
    pub hit_timestamps: Vec<i64>,
    /// Set when any record set it.
    pub spec_flags: [bool; 5],
}

impl SkillTotals {
    pub(in crate::combat) fn new(skill_code: i32, is_dot: bool) -> Self {
        SkillTotals {
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

    /// A copy without the hit timeline, which is the only costly part.
    fn shallow_copy(&self) -> Self {
        Self { hit_timestamps: Vec::new(), ..*self }
    }

    /// Fold `other`, a row of the same skill, into this one.
    pub(in crate::combat) fn absorb(&mut self, other: SkillTotals) {
        let counts = [
            (&mut self.hit_count, other.hit_count),
            (&mut self.crit_count, other.crit_count),
            (&mut self.back_count, other.back_count),
            (&mut self.frontal_count, other.frontal_count),
            (&mut self.shield_block_count, other.shield_block_count),
            (&mut self.parry_count, other.parry_count),
            (&mut self.perfect_count, other.perfect_count),
            (&mut self.double_count, other.double_count),
            (&mut self.iron_wall_count, other.iron_wall_count),
            (&mut self.regeneration_count, other.regeneration_count),
            (&mut self.perfect_block_count, other.perfect_block_count),
            (&mut self.miss_count, other.miss_count),
            (&mut self.resist_count, other.resist_count),
            (&mut self.multi_hit_count, other.multi_hit_count),
            (&mut self.multi_hit_hits, other.multi_hit_hits),
        ];
        for (mine, theirs) in counts {
            *mine = mine.saturating_add(theirs);
        }
        for (mine, theirs) in [
            (&mut self.total_damage, other.total_damage),
            (&mut self.multi_hit_damage, other.multi_hit_damage),
            (&mut self.heal_amount, other.heal_amount),
        ] {
            *mine = mine.saturating_add(theirs);
        }
        self.min_damage = self.min_damage.min(other.min_damage);
        self.max_damage = self.max_damage.max(other.max_damage);
        let mut times = other.hit_timestamps;
        times.append(&mut self.hit_timestamps);
        times.sort_unstable();
        self.hit_timestamps = times;
        self.spec_flags = std::array::from_fn(|i| self.spec_flags[i] || other.spec_flags[i]);
    }
}

/// Who you are, as far as the meter knows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SelfCard {
    pub name: Option<String>,
    /// 0 when unknown.
    pub server_id: u16,
    pub class: Option<JobClass>,
    pub level: Option<u32>,
}

/// One party member, from the party roster.
#[derive(Debug, Clone, Default)]
pub struct RosterEntry {
    /// From 1.
    pub slot: u8,
    pub level: i32,
    pub gear_score: i32,
    pub combat_power: i64,
    pub server_id: u16,
    pub dbid: u64,
    pub job: Option<JobClass>,
}

/// One actor's damage on one target.
#[derive(Debug, Clone)]
pub struct ActorTotals {
    pub total_damage: i64,
    /// Healing done on party members while fighting here.
    pub party_heal: i64,
    /// Healing that came with the actor's own damage records.
    pub regen: i64,
    /// `i64::MAX` and 0 until the first hit.
    pub first_damage_time: i64,
    pub last_damage_time: i64,
    pub job: Option<JobClass>,
    /// By skill code and whether the rows are ticks over time.
    pub skills: HashMap<(i32, bool), SkillTotals>,
    /// Per unix second, oldest first, `DAMAGE_HISTORY_MS` back at most.
    pub by_second: VecDeque<(i64, SecondStats)>,
    /// A number the row takes whenever it changes (`Inner::next_seq`): a
    /// copy of it is up to date while the two numbers match.
    pub seq: u64,
}

impl ActorTotals {
    pub(super) fn new() -> Self {
        ActorTotals {
            first_damage_time: i64::MAX,
            last_damage_time: 0,
            total_damage: 0,
            party_heal: 0,
            regen: 0,
            job: None,
            skills: HashMap::new(),
            by_second: VecDeque::new(),
            seq: 0,
        }
    }

    /// Fold `other`, the same actor's row kept apart until now, into this one.
    pub(super) fn absorb(&mut self, other: ActorTotals) {
        for (sum, more) in [
            (&mut self.total_damage, other.total_damage),
            (&mut self.party_heal, other.party_heal),
            (&mut self.regen, other.regen),
        ] {
            *sum = sum.saturating_add(more);
        }
        self.first_damage_time = self.first_damage_time.min(other.first_damage_time);
        self.last_damage_time = self.last_damage_time.max(other.last_damage_time);
        if self.job.is_none() {
            self.job = other.job;
        }
        for (sec, stats) in &other.by_second {
            self.add_at(*sec, stats);
        }
        for (key, row) in other.skills {
            if let Some(mine) = self.skills.get_mut(&key) {
                mine.absorb(row);
            } else {
                self.skills.insert(key, row);
            }
        }
    }

    /// A copy with every skill's hit timeline left out. Built field by
    /// field, so a new field breaks the build here.
    fn shallow_copy(&self) -> Self {
        let ActorTotals {
            total_damage, party_heal, regen, first_damage_time, last_damage_time, job, ref skills, ref by_second, seq,
        } = *self;
        ActorTotals {
            skills: skills.iter().map(|(key, sk)| (*key, sk.shallow_copy())).collect(),
            by_second: by_second.clone(),
            total_damage, party_heal, regen, first_damage_time, last_damage_time, job, seq,
        }
    }

    /// Damage from the second holding `since_ms` on.
    pub fn damage_since(&self, since_ms: i64) -> i64 {
        self.stats_since(since_ms).damage
    }

    /// Everything from the second holding `since_ms` on.
    pub fn stats_since(&self, since_ms: i64) -> SecondStats {
        let from = since_ms.div_euclid(1000);
        let start = self.by_second.partition_point(|&(sec, _)| sec < from);
        let mut sum = SecondStats::default();
        for (_, stats) in self.by_second.range(start..) {
            sum.add(stats);
        }
        sum
    }

    /// The whole fight from the rows, which keep more than the seconds do.
    pub fn stats_total(&self) -> SecondStats {
        let mut sum = SecondStats { damage: self.total_damage, ..SecondStats::default() };
        for row in self.skills.values().filter(|row| !row.is_dot) {
            sum.hits += i64::from(row.hit_count);
            sum.crits += i64::from(row.crit_count);
            sum.max_hit = sum.max_hit.max(row.max_damage);
        }
        sum
    }

    /// Add `stats` at unix second `sec`, then drop the seconds older than
    /// the history keeps, counted back from `sec`.
    pub(super) fn add_at(&mut self, sec: i64, stats: &SecondStats) {
        let at = self.by_second.partition_point(|&(s, _)| s < sec);
        match self.by_second.get_mut(at) {
            Some((s, there)) if *s == sec => there.add(stats),
            _ => self.by_second.insert(at, (sec, *stats)),
        }
        let oldest = sec - DAMAGE_HISTORY_MS / 1000;
        while self.by_second.front().is_some_and(|&(s, _)| s < oldest) {
            self.by_second.pop_front();
        }
    }
}

/// One target's fight: everyone's damage on it since its first hit.
#[derive(Debug, Clone)]
pub struct TargetTotals {
    pub target_id: i32,
    pub total_damage: i64,
    pub first_damage_time: i64,
    pub last_damage_time: i64,
    /// The newest record's packet id, -1 before one.
    pub last_packet_id: i64,
    /// By actor id as sent; a summon has its own row.
    pub actors: HashMap<i32, ActorTotals>,
    /// Hit by you or your party once the meter knew you.
    pub ours: bool,
    pub dungeon_id: i32,
    pub open_world: bool,
    /// The boss's pull this fight is, as the server's combat records count
    /// them (`BossCombat`); 0 for anything else.
    pub pull: u32,
    /// The enrage deadline the server gave this pull, unix ms.
    pub enrage_at: Option<i64>,
}

impl TargetTotals {
    /// Each row's (hits, damage) change from `before`, the same target read
    /// earlier, by (actor as sent, skill, is a tick), in that order. A row
    /// gone counts as dropped to zero (one actor's rows merged into
    /// another's). A new first-damage time is a new segment: every row
    /// counts from nothing.
    pub fn row_changes(&self, before: Option<&TargetTotals>) -> std::collections::BTreeMap<(i32, i32, bool), (i64, i64)> {
        let rows = |t: &TargetTotals| -> HashMap<(i32, i32, bool), (i64, i64)> {
            let skills = t.actors.iter().flat_map(|(&actor, a)| a.skills.iter().map(move |(&(skill, dot), s)| ((actor, skill, dot), s)));
            skills.map(|(key, s)| (key, (i64::from(s.hit_count), s.total_damage))).collect()
        };
        let base = match before {
            Some(b) if b.first_damage_time == self.first_damage_time => rows(b),
            _ => HashMap::new(),
        };
        let now = rows(self);
        now.keys()
            .chain(base.keys())
            .filter_map(|k| {
                let (h, d) = now.get(k).copied().unwrap_or((0, 0));
                let (bh, bd) = base.get(k).copied().unwrap_or((0, 0));
                (h != bh || d != bd).then_some((*k, (h - bh, d - bd)))
            })
            .collect()
    }

    pub(super) fn new(target_id: i32, timestamp: i64) -> Self {
        TargetTotals {
            target_id,
            first_damage_time: timestamp,
            last_damage_time: timestamp,
            last_packet_id: -1,
            total_damage: 0,
            actors: HashMap::new(),
            dungeon_id: 0,
            ours: false,
            open_world: false,
            pull: 0,
            enrage_at: None,
        }
    }

    /// The numbers with every skill's hit timeline left out, which the meter
    /// does not need on each tick. Built field by field: cloning the actors
    /// first would copy the timelines anyway.
    pub(super) fn shallow_copy(&self) -> Self {
        let TargetTotals {
            target_id, total_damage, first_damage_time, last_damage_time, last_packet_id, ref actors, ours,
            dungeon_id, open_world, pull, enrage_at,
        } = *self;
        let actors = actors.iter().map(|(&who, row)| (who, row.shallow_copy())).collect();
        TargetTotals {
            target_id, total_damage, first_damage_time, last_damage_time, last_packet_id, actors, ours,
            dungeon_id, open_world, pull, enrage_at,
        }
    }

    /// Bring this light copy of `live` up to date, as a new `shallow_copy`
    /// would be, copying only the actor rows whose `seq` moved.
    pub(super) fn catch_up(&mut self, live: &TargetTotals) {
        let rows = std::mem::take(&mut self.actors);
        *self = TargetTotals { actors: rows, ..*live };
        for (&who, row) in &live.actors {
            if self.actors.get(&who).is_none_or(|mine| mine.seq != row.seq) {
                self.actors.insert(who, row.shallow_copy());
            }
        }
        if self.actors.len() != live.actors.len() {
            self.actors.retain(|who, _| live.actors.contains_key(who));
        }
    }

    /// Take out the rows of the actors `gone` picks, and their damage off the
    /// total. The times stay. True when any row went.
    pub(super) fn remove_actors(&mut self, gone: impl Fn(i32) -> bool) -> bool {
        let rows = self.actors.len();
        let total = &mut self.total_damage;
        self.actors.retain(|&who, row| {
            let goes = gone(who);
            if goes {
                *total -= row.total_damage;
            }
            !goes
        });
        self.actors.len() != rows
    }

    /// Several targets as one, each actor's rows added up. Target id 0.
    pub fn merged<'a>(targets: impl IntoIterator<Item = &'a TargetTotals>) -> Option<Self> {
        let mut targets = targets.into_iter().peekable();
        let start = targets.peek()?.first_damage_time;
        let mut all = TargetTotals::new(0, start);
        for td in targets {
            all.total_damage += td.total_damage;
            all.first_damage_time = all.first_damage_time.min(td.first_damage_time);
            all.last_damage_time = all.last_damage_time.max(td.last_damage_time);
            all.ours |= td.ours;
            all.open_world |= td.open_world;
            if td.dungeon_id != 0 {
                all.dungeon_id = td.dungeon_id;
            }
            for (&who, row) in &td.actors {
                all.actors.entry(who).or_insert_with(ActorTotals::new).absorb(row.clone());
            }
        }
        Some(all)
    }
}

/// A fight segment taken out of the live data (an idle restart, a boss
/// pull, a reset or a zone change) before the auto-save wrote it.
#[derive(Debug, Clone)]
pub struct EndedSegment {
    pub data: TargetTotals,
    pub max_hp: i32,
    pub heals: HashMap<i32, HashMap<(i32, bool), HealTotals>>,
    /// The damage taken during it, per player and skill.
    pub taken: super::TakenBy,
    /// The deaths of you and your party during it.
    pub deaths: super::DeathsBy,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn dmg(damage: i64) -> SecondStats {
        SecondStats { damage, ..SecondStats::default() }
    }

    /// The row T3 starts from, and the one it takes in.
    fn rows() -> (SkillTotals, SkillTotals) {
        let mut a = SkillTotals::new(11_010_000, false);
        (a.hit_count, a.total_damage, a.min_damage, a.max_damage, a.crit_count) = (2, 500, 200, 300, 1);
        a.hit_timestamps = vec![1000, 2000];
        a.spec_flags[0] = true;
        let mut b = SkillTotals::new(11_010_000, false);
        (b.hit_count, b.total_damage, b.min_damage, b.max_damage, b.parry_count) = (1, 300, 300, 300, 1);
        b.hit_timestamps = vec![1500];
        b.spec_flags[1] = true;
        (a, b)
    }

    #[test]
    fn a_new_skill_row_is_empty_with_no_smallest_hit() {
        let row = SkillTotals::new(12_020_000, true);
        assert_eq!((row.skill_code, row.is_dot), (12_020_000, true));
        assert_eq!((row.hit_count, row.total_damage, row.max_damage, row.heal_amount), (0, 0, 0, 0));
        assert_eq!((row.multi_hit_damage, row.multi_hit_count, row.miss_count, row.resist_count), (0, 0, 0, 0));
        assert_eq!(row.min_damage, i64::MAX);
        assert!(row.hit_timestamps.is_empty());
        assert_eq!(row.spec_flags, [false; 5]);
    }

    #[test]
    fn a_light_copy_leaves_out_only_the_timeline() {
        let (mut row, _) = rows();
        row.hit_timestamps = vec![1, 2];
        let light = row.shallow_copy();
        assert_eq!(light.hit_count, 2);
        assert!(light.hit_timestamps.is_empty());
        assert_eq!(format!("{:?}", SkillTotals { hit_timestamps: Vec::new(), ..row.clone() }), format!("{light:?}"));
    }

    #[test]
    fn skill_rows_add_up_keep_extremes_and_merge_timelines() {
        let (mut a, b) = rows();
        a.absorb(b);
        assert_eq!((a.hit_count, a.total_damage, a.min_damage, a.max_damage), (3, 800, 200, 300));
        assert_eq!((a.crit_count, a.parry_count), (1, 1));
        assert_eq!(a.hit_timestamps, vec![1000, 1500, 2000]);
        assert_eq!(a.spec_flags, [true, true, false, false, false]);
        assert_eq!((a.skill_code, a.is_dot), (11_010_000, false));

        let mut full = SkillTotals::new(1, false);
        full.hit_count = i32::MAX;
        let mut one = SkillTotals::new(1, false);
        one.hit_count = 1;
        one.total_damage = 5;
        full.total_damage = i64::MAX;
        full.absorb(one);
        assert_eq!((full.hit_count, full.total_damage), (i32::MAX, i64::MAX), "counts stop at the limit");
    }

    #[test]
    fn a_new_actor_row_has_no_hit_yet() {
        let row = ActorTotals::new();
        assert_eq!((row.total_damage, row.party_heal, row.regen), (0, 0, 0));
        assert_eq!((row.first_damage_time, row.last_damage_time), (i64::MAX, 0));
        assert!(row.job.is_none() && row.skills.is_empty() && row.by_second.is_empty());
    }

    #[test]
    fn seconds_stay_in_order_and_age_out_from_the_newest_added() {
        let mut row = ActorTotals::new();
        for (sec, damage) in [(100, 10), (100, 5), (103, 20), (101, 7)] {
            row.add_at(sec, &dmg(damage));
        }
        let damage: Vec<(i64, i64)> = row.by_second.iter().map(|(s, st)| (*s, st.damage)).collect();
        assert_eq!(damage, vec![(100, 15), (101, 7), (103, 20)]);
        row.add_at(1002, &dmg(1));
        assert_eq!(row.by_second.front().map(|(s, st)| (*s, st.damage)), Some((103, 20)));
        assert_eq!(row.by_second.len(), 2);

        let mut late = ActorTotals::new();
        late.add_at(1200, &dmg(1));
        late.add_at(2000, &dmg(1));
        late.add_at(50, &dmg(1));
        let secs: Vec<i64> = late.by_second.iter().map(|(s, _)| *s).collect();
        assert_eq!(secs, vec![50, 1200, 2000], "the cut counts from 50");
    }

    #[test]
    fn a_window_starts_at_the_second_holding_its_start() {
        let mut row = ActorTotals::new();
        row.add_at(100, &SecondStats { damage: 900, hits: 1, crits: 1, max_hit: 900 });
        row.add_at(101, &SecondStats { damage: 300, hits: 1, crits: 0, max_hit: 300 });
        row.add_at(101, &dmg(50));
        let both = SecondStats { damage: 1250, hits: 2, crits: 1, max_hit: 900 };
        assert_eq!(row.stats_since(100_000), both);
        assert_eq!(row.stats_since(100_999), both);
        assert_eq!(row.stats_since(101_000), SecondStats { damage: 350, hits: 1, crits: 0, max_hit: 300 });
        assert_eq!(row.damage_since(104_000), 0);
        assert_eq!(row.stats_since(-1), both, "second -1 on");
        row.add_at(-1, &dmg(4));
        assert_eq!(row.damage_since(-1), 1254);
        assert_eq!(row.damage_since(0), 1250);
    }

    #[test]
    fn the_whole_fight_counts_direct_hits_only_but_all_damage() {
        let mut row = ActorTotals::new();
        row.total_damage = 5000;
        let mut a = SkillTotals::new(1, false);
        (a.hit_count, a.crit_count, a.max_damage) = (3, 1, 500);
        let mut a_ticks = SkillTotals::new(1, true);
        (a_ticks.hit_count, a_ticks.crit_count, a_ticks.max_damage) = (10, 2, 900);
        let mut b = SkillTotals::new(2, false);
        (b.hit_count, b.max_damage) = (2, 700);
        row.skills.extend([((1, false), a), ((1, true), a_ticks), ((2, false), b)]);
        assert_eq!(row.stats_total(), SecondStats { damage: 5000, hits: 5, crits: 1, max_hit: 700 });
        assert_eq!(ActorTotals::new().stats_total(), SecondStats::default());
    }

    #[test]
    fn actor_rows_add_up_and_keep_the_first_class() {
        let (row, other_row) = rows();
        let mut a = ActorTotals::new();
        (a.total_damage, a.first_damage_time, a.last_damage_time) = (500, 1000, 2000);
        a.skills.insert((11_010_000, false), row);
        a.add_at(1, &dmg(500));
        let mut b = ActorTotals::new();
        (b.total_damage, b.party_heal, b.first_damage_time, b.last_damage_time) = (300, 40, 500, 1500);
        b.job = Some(JobClass::Gladiator);
        b.skills.insert((11_010_000, false), other_row);
        let mut ticks = SkillTotals::new(11_020_000, true);
        (ticks.hit_count, ticks.total_damage) = (1, 50);
        b.skills.insert((11_020_000, true), ticks);
        b.add_at(1, &dmg(300));

        a.absorb(b);
        assert_eq!((a.total_damage, a.party_heal, a.first_damage_time, a.last_damage_time), (800, 40, 500, 2000));
        assert_eq!(a.job, Some(JobClass::Gladiator));
        let merged = &a.skills[&(11_010_000, false)];
        assert_eq!((merged.hit_count, merged.total_damage, merged.min_damage), (3, 800, 200));
        assert_eq!(merged.hit_timestamps, vec![1000, 1500, 2000]);
        let moved = &a.skills[&(11_020_000, true)];
        assert_eq!((moved.hit_count, moved.total_damage), (1, 50));
        assert_eq!(a.by_second.iter().map(|(s, st)| (*s, st.damage)).collect::<Vec<_>>(), vec![(1, 800)]);

        let mut c = ActorTotals::new();
        c.job = Some(JobClass::Sorcerer);
        c.absorb(a);
        assert_eq!(c.job, Some(JobClass::Sorcerer), "a class already there stays");
    }

    #[test]
    fn second_stats_add_and_keep_the_biggest_hit() {
        let mut a = SecondStats { damage: 10, hits: 1, crits: 0, max_hit: 10 };
        a.add(&SecondStats { damage: 5, hits: 2, crits: 1, max_hit: 4 });
        assert_eq!(a, SecondStats { damage: 15, hits: 3, crits: 1, max_hit: 10 });
    }

    #[test]
    fn a_new_target_starts_and_ends_at_its_first_hit() {
        let td = TargetTotals::new(900, 1234);
        assert_eq!((td.target_id, td.total_damage, td.first_damage_time, td.last_damage_time), (900, 0, 1234, 1234));
        assert_eq!((td.last_packet_id, td.dungeon_id, td.ours, td.open_world), (-1, 0, false, false));
        assert!(td.actors.is_empty());
    }

    #[test]
    fn targets_merge_into_one_with_every_actor_once() {
        assert!(TargetTotals::merged([]).is_none());
        let with = |id: i32, total: i64, first: i64, last: i64, actors: &[(i32, i64)]| {
            let mut td = TargetTotals::new(id, first);
            (td.total_damage, td.last_damage_time) = (total, last);
            for &(who, damage) in actors {
                let mut row = ActorTotals::new();
                row.total_damage = damage;
                td.actors.insert(who, row);
            }
            td
        };
        let mut a = with(900, 1000, 1000, 5000, &[(1000, 600), (2000, 400)]);
        a.ours = true;
        let mut b = with(901, 300, 2000, 9000, &[(1000, 300)]);
        (b.open_world, b.dungeon_id) = (true, 600_011);
        let m = TargetTotals::merged([&a, &b]).expect("two targets");
        assert_eq!((m.target_id, m.total_damage, m.first_damage_time, m.last_damage_time), (0, 1300, 1000, 9000));
        assert_eq!((m.ours, m.open_world, m.dungeon_id, m.last_packet_id), (true, true, 600_011, -1));
        assert_eq!((m.actors[&1000].total_damage, m.actors[&2000].total_damage), (900, 400));

        // The last non-zero dungeon wins; a first hit later than every
        // last one still bounds the end.
        let mut c = with(902, 0, 7000, 0, &[]);
        c.dungeon_id = 0;
        let m = TargetTotals::merged([&c, &b, &a]).expect("three targets");
        assert_eq!((m.dungeon_id, m.first_damage_time, m.last_damage_time), (600_011, 1000, 9000));
        let lone = with(903, 0, 7000, 0, &[]);
        assert_eq!(TargetTotals::merged([&lone]).map(|m| m.last_damage_time), Some(7000));
    }
}
