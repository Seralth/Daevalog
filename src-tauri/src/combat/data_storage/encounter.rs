//! The encounter, and the fight segments retired from the live data.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;

use super::damage::is_ours;
use super::heal::fight_heals;
use super::taken::taken_in;
use super::{
    DataStorage, Encounter, EndedSegment, Inner, SegmentIdentity, TargetCombatData, BOSS_HOLD_MAX_MS,
    IDLE_RESET_MS, MAX_ENDED_SEGMENTS, MIN_SAVED_FIGHT_MS,
};

impl DataStorage {
    /// Seconds without combat by you or your party that end an encounter.
    pub fn set_encounter_timeout_ms(&self, ms: i64) {
        self.encounter_timeout_ms.store(ms.clamp(5_000, 300_000), Ordering::Relaxed);
    }

    pub fn current_encounter(&self) -> Option<Encounter> {
        self.inner.read().encounter.clone()
    }

    /// Boss and dummy fights that were cleared out of the live data (an idle
    /// restart on the same mob, a boss pull, a reset, a zone change, the
    /// party ending) before the auto-save wrote them. Each is returned once.
    pub fn take_ended_segments(&self) -> Vec<EndedSegment> {
        std::mem::take(&mut self.inner.write().ended_segments)
    }

    /// Retire the segments last hit over `IDLE_RESET_MS` before `now`, as a
    /// new hit on them would, unless `keep` holds them (what the meter
    /// shows) or the encounter has them. Fights worth saving go to the
    /// auto-save. Returns how many went.
    pub fn retire_idle(&self, now: i64, keep: impl Fn(i32, &TargetCombatData) -> bool) -> usize {
        let mut inner = self.inner.write();
        let idle: Vec<i32> = inner.target_combat.iter()
            .filter(|&(&tid, td)| {
                now - td.last_damage_time > IDLE_RESET_MS
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
            drop(inner);
            self.touch();
        }
        idle.len()
    }
}

/// Keep a segment that is leaving the live data when the history would save
/// it (a boss or a training dummy, long enough), for `take_ended_segments`.
pub(super) fn retire_segment(inner: &mut Inner, data: TargetCombatData) {
    let tid = data.target_id;
    let fight = inner.boss_entity_ids.contains(&tid) || inner.training_dummy_ids.contains(&tid);
    if !fight || data.total_damage <= 0 || data.last_damage_time - data.first_damage_time < MIN_SAVED_FIGHT_MS {
        return;
    }
    if inner.ended_segments.len() >= MAX_ENDED_SEGMENTS {
        inner.ended_segments.remove(0);
    }
    let max_hp = inner.mob_hp_data.get(&tid).copied().unwrap_or(0);
    let heals = fight_heals(inner, &data);
    let taken = taken_in(inner, &[&data], i64::MIN);
    let identity = SegmentIdentity {
        summons: super::entities::owners_in(inner, std::iter::once(&data)),
        nicknames: inner.nickname_storage.clone(),
        local_player_id: inner.local_player_id,
        dungeon_id: inner.current_dungeon_id,
    };
    inner.ended_segments.push(EndedSegment { data, max_hp, heals, taken, identity });
}

/// Put the open encounter's carry back into `out`, merged with any live data
/// of the same target. Only the targets `keep` takes.
pub(super) fn with_carry(
    inner: &Inner,
    mut out: HashMap<i32, TargetCombatData>,
    clone: fn(&TargetCombatData) -> TargetCombatData,
    keep: impl Fn(i32) -> bool,
) -> HashMap<i32, TargetCombatData> {
    for (&tid, carried) in inner.encounter_carry.iter().filter(|(tid, _)| keep(**tid)) {
        let td = match out.remove(&tid) {
            Some(live) => {
                let mut m = TargetCombatData::merged([&clone(carried), &live]).expect("two segments");
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
    let kept: Vec<(i32, TargetCombatData)> = e.targets.iter()
        .filter_map(|t| inner.target_combat.get(t).map(|td| (*t, td.clone())))
        .collect();
    inner.encounter_carry.extend(kept);
}

/// Clear every target's segment, keeping the fights worth saving.
pub(super) fn retire_all(inner: &mut Inner) {
    inner.idle_retired = false;
    let segments: Vec<TargetCombatData> = inner.target_combat.drain().map(|(_, td)| td).collect();
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
