//! The encounter, the fight segments retired from the live data, and where
//! the server's combat records end a boss fight.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;

use super::damage::is_ours;
use super::deaths::deaths_in;
use super::heal::fights_heals;
use super::roster::settle_leave;
use super::taken::taken_in;
use super::{
    ById, DataStorage, Encounter, EndedSegment, Inner, SegmentIdentity, TargetTotals, BOSS_HOLD_MAX_MS,
    QUIET_LIMIT_MS, MAX_ENDED_SEGMENTS, MIN_SAVED_FIGHT_MS,
};

impl DataStorage {
    /// The open encounter, or the last one.
    pub fn current_encounter(&self) -> Option<Encounter> {
        self.inner.read().encounter.clone()
    }

    /// Seconds without combat by you or your party that end an encounter.
    pub fn set_encounter_timeout_ms(&self, ms: i64) {
        self.encounter_timeout_ms.store(ms.clamp(5_000, 300_000), Ordering::Relaxed);
    }

    /// Boss and dummy fights that were cleared out of the live data (an idle
    /// restart on the same mob, a boss pull, a reset, a zone change, the
    /// party ending) before the auto-save wrote them. Each is returned once.
    pub fn take_ended_segments(&self) -> Vec<EndedSegment> {
        std::mem::take(&mut self.inner.write().ended_segments)
    }

    /// Retire the segments last hit over `QUIET_LIMIT_MS` before `now`, as a
    /// new hit on them would, unless `keep` holds them (what the meter
    /// shows) or the encounter has them. Fights worth saving go to the
    /// auto-save. A party left during a fight goes here once that fight is
    /// over. Returns how many went.
    pub fn retire_idle(&self, now: i64, keep: impl Fn(i32, &TargetTotals) -> bool) -> usize {
        let mut inner = self.inner.write();
        let idle: Vec<i32> = inner.target_combat.iter()
            .filter(|&(&tid, td)| {
                now - td.last_damage_time > QUIET_LIMIT_MS
                    && !inner.encounter.as_ref().is_some_and(|e| e.targets.contains(&tid))
                    && !keep(tid, td)
            })
            .map(|(&tid, _)| tid)
            .collect();
        for tid in &idle {
            if let Some(td) = inner.target_combat.remove(tid) {
                retire_segment(&mut inner, td);
            }
        }
        if !idle.is_empty() {
            inner.idle_retired = true;
        }
        // On the meter's turn, which the save shares, not on a packet: the
        // save of an ended fight reads the party before it goes.
        let party_gone = settle_leave(&mut inner, now);
        drop(inner);
        if !idle.is_empty() || party_gone {
            self.touch();
        }
        idle.len()
    }

    /// The server's combat record (`21 8D`) of `entity` at `at`: into combat
    /// or out of it, with the enrage deadline when the boss has a timed one.
    /// Only bosses are followed: their fights end where the server says.
    pub fn note_combat(&self, entity: i32, on: bool, enrage_at: Option<i64>, at: i64) {
        let mut inner = self.inner.write();
        if !inner.boss_entity_ids.contains(&entity) {
            return;
        }
        let state = inner.boss_combat.entry(entity).or_default();
        if !on {
            state.off_at = Some(at);
        } else if state.off_at.take().is_some() {
            state.next_pull();
        }
        state.enrage_at = enrage_at.or(state.enrage_at);
        let state = *state;
        let Some(fight) = inner.target_combat.get_mut(&entity) else { return };
        if fight.pull == state.pull {
            fight.enrage_at = state.enrage_at;
        }
        drop(inner);
        self.touch();
    }

    /// Whether the server ended `target`'s live fight: the boss is out of
    /// combat, or back in for its next pull.
    pub fn fight_ended_by_server(&self, target: i32) -> bool {
        let inner = self.inner.read();
        let Some(state) = inner.boss_combat.get(&target) else { return false };
        state.off_at.is_some() || inner.target_combat.get(&target).is_some_and(|f| f.pull != state.pull)
    }
}

/// A direct hit on a boss this long after the server took it out of
/// combat, with no record of it coming back, is its next pull: an opener's
/// hit comes about 50 ms before the boss's record. The last pull's own hits
/// come up to 1.5 s after it.
const STRAGGLER_MS: i64 = 2_000;

/// A boss's combat as the server's records give it.
#[derive(Default, Clone, Copy)]
pub(super) struct BossCombat {
    /// When it last left combat; `None` while in it.
    off_at: Option<i64>,
    /// Its pulls so far: one more at each return to combat.
    pull: u32,
    /// The enrage deadline the server gave this pull, unix ms.
    enrage_at: Option<i64>,
}

impl BossCombat {
    fn next_pull(&mut self) {
        self.pull += 1;
        self.enrage_at = None;
    }
}

/// Whether a hit at `ts` on `target` starts the boss's next fight: the
/// server ended its live fight and the boss is back in combat, or this
/// direct hit comes `STRAGGLER_MS` after it left. Ticks of damage over time
/// start none, nor does a dead boss.
pub(super) fn server_restart(inner: &mut Inner, target: i32, ts: i64, tick: bool) -> bool {
    if tick || inner.dead_entity_ids.contains(&target) {
        return false;
    }
    let Some(state) = inner.boss_combat.get_mut(&target) else { return false };
    if state.off_at.is_some_and(|off| ts - off > STRAGGLER_MS) {
        state.off_at = None;
        state.next_pull();
    }
    inner.target_combat.get(&target).is_some_and(|f| f.pull != state.pull)
}

/// A new fight on `target` from `ts`: a boss's is of its current pull.
pub(super) fn new_fight(combat: &ById<BossCombat>, target: i32, ts: i64) -> TargetTotals {
    let state = combat.get(&target).copied().unwrap_or_default();
    TargetTotals { pull: state.pull, enrage_at: state.enrage_at, ..TargetTotals::new(target, ts) }
}

/// Keep a boss fight the server ended for the open encounter that has the
/// boss, with what it keeps of the boss's earlier pulls: an encounter goes
/// on through a wipe and the next pull, as before the pulls were told apart.
pub(super) fn carry_pull(inner: &mut Inner, fight: &TargetTotals) {
    let tid = fight.target_id;
    if !inner.encounter.as_ref().is_some_and(|e| e.targets.contains(&tid)) {
        return;
    }
    let kept = match inner.encounter_carry.remove(&tid) {
        Some(earlier) => TargetTotals { target_id: tid, ..TargetTotals::merged([&earlier, fight]).expect("two fights") },
        None => fight.clone(),
    };
    inner.encounter_carry.insert(tid, kept);
}

/// Keep a segment that is leaving the live data when the history would save
/// it (a boss or a training dummy, long enough), for `take_ended_segments`.
pub(super) fn retire_segment(inner: &mut Inner, data: TargetTotals) {
    let tid = data.target_id;
    let fight = inner.boss_entity_ids.contains(&tid) || inner.training_dummy_ids.contains(&tid);
    if !fight || data.total_damage <= 0 || data.last_damage_time - data.first_damage_time < MIN_SAVED_FIGHT_MS {
        return;
    }
    if inner.ended_segments.len() >= MAX_ENDED_SEGMENTS {
        inner.ended_segments.remove(0);
    }
    let max_hp = inner.mob_hp_data.get(&tid).copied().unwrap_or(0);
    let heals = fights_heals(inner, &[&data], true);
    let taken = taken_in(inner, &[&data], i64::MIN);
    let deaths = deaths_in(inner, &[&data], i64::MIN);
    let identity = SegmentIdentity {
        summons: super::entities::owners_in(inner, std::iter::once(&data)),
        nicknames: inner.nickname_storage.clone(),
        local_player_id: inner.local_player_id,
        dungeon_id: inner.current_dungeon_id,
    };
    inner.ended_segments.push(EndedSegment { data, max_hp, heals, taken, deaths, identity });
}

/// Put the open encounter's carry back into `out`, merged with any live data
/// of the same target. Only the targets `keep` takes.
pub(super) fn with_carry(
    inner: &Inner,
    mut out: HashMap<i32, TargetTotals>,
    clone: fn(&TargetTotals) -> TargetTotals,
    keep: impl Fn(i32) -> bool,
) -> HashMap<i32, TargetTotals> {
    for (&tid, carried) in inner.encounter_carry.iter().filter(|(tid, _)| keep(**tid)) {
        let td = match out.remove(&tid) {
            Some(live) => {
                let mut m = TargetTotals::merged([&clone(carried), &live]).expect("two segments");
                m.target_id = tid;
                m.last_packet_id = live.last_packet_id;
                m
            }
            None => clone(carried),
        };
        out.insert(tid, td);
    }
    out
}

/// Keep the open encounter's segments for it before a boss pull clears them.
pub(super) fn carry_encounter(inner: &mut Inner) {
    let Some(e) = &inner.encounter else { return };
    let kept: Vec<(i32, TargetTotals)> = e.targets.iter()
        .filter_map(|t| inner.target_combat.get(t).map(|td| (*t, td.clone())))
        .collect();
    inner.encounter_carry.extend(kept);
}

/// Clear every target's segment, keeping the fights worth saving.
pub(super) fn retire_all(inner: &mut Inner) {
    inner.idle_retired = false;
    let segments: Vec<TargetTotals> = inner.target_combat.drain().map(|(_, td)| td).collect();
    for td in segments {
        retire_segment(inner, td);
    }
}

/// Extend the encounter with a hit at `ts` on (or by) `enemy`. A hit by you or
/// your party starts a new encounter once the old one timed out; anyone
/// else's hit only counts toward an encounter it belongs to.
pub(super) fn note_encounter(inner: &mut Inner, timeout: i64, ts: i64, enemy: i32, ours: bool) {
    if !ours {
        if let Some(e) = inner.encounter.as_mut().filter(|e| e.targets.contains(&enemy)) {
            e.last_any = e.last_any.max(ts);
        }
        return;
    }
    if inner.local_player_id.is_some() && inner.encounter.as_ref().is_some_and(|e| e.blind) {
        refocus(inner);
    }
    if encounter_ended(inner, timeout, ts) || inner.encounter.is_none() {
        let blind = inner.local_player_id.is_none();
        inner.encounter = Some(Encounter { start: ts, last_ours: ts, last_any: ts, targets: HashSet::new(), blind });
        inner.encounter_carry.clear();
    }
    if let Some(e) = inner.encounter.as_mut() {
        e.last_ours = e.last_ours.max(ts);
        e.last_any = e.last_any.max(ts);
        e.targets.insert(enemy);
    }
}

/// Once the meter knows you, an encounter opened before keeps only the
/// enemies you or your party hit, from the first of those hits; with none,
/// it goes. Until then every player's fight nearby was in it.
fn refocus(inner: &mut Inner) {
    let Some(e) = &inner.encounter else { return };
    let (mut first, mut last, mut targets) = (i64::MAX, i64::MIN, HashSet::new());
    for &t in &e.targets {
        let ours = inner.target_combat.get(&t).into_iter().chain(inner.encounter_carry.get(&t))
            .flat_map(|td| td.actors.iter())
            .filter(|(a, _)| is_ours(inner, **a));
        for (_, ad) in ours {
            first = first.min(ad.first_damage_time);
            last = last.max(ad.last_damage_time);
            targets.insert(t);
        }
    }
    if targets.is_empty() {
        inner.encounter = None;
        return;
    }
    inner.encounter_carry.retain(|t, _| targets.contains(t));
    let e = inner.encounter.as_mut().expect("checked above");
    e.start = e.start.max(first);
    e.last_ours = last;
    e.targets = targets;
    e.blind = false;
}

/// Whether a hit of yours at `ts` comes after the encounter ended.
pub(super) fn encounter_ended(inner: &Inner, timeout: i64, ts: i64) -> bool {
    inner.encounter.as_ref().is_some_and(|e| {
        let quiet = ts - e.last_ours;
        let boss_alive = e.targets.iter().any(|t| {
            inner.boss_entity_ids.contains(t) && !inner.dead_entity_ids.contains(t)
        });
        quiet > timeout && !(boss_alive && quiet <= BOSS_HOLD_MAX_MS)
    })
}
