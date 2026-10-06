//! The meter rows: damage, DPS and the per-row stats for the targets on screen.

use std::collections::HashMap;

use crate::clock::now_ms;
use crate::combat::data_storage::{SecondStats, UNATTRIBUTED_ID};
use crate::entity::dps_data::DpsData;
use crate::entity::job_class::JobClass;
use crate::entity::personal_data::PersonalData;
use crate::entity::summon_resolver;

use super::rows::{build_nickname_canonical_map_from_aggregates, resolve_nickname};
use super::{DpsCalculator, TargetSelectionMode};

impl DpsCalculator {
    pub(super) fn compute_dps(&mut self) -> DpsData {
        // A zone change flushed combat data; drop our cached snapshot and saved-target
        // state so the meter resets this cycle instead of returning the stale snapshot.
        if self.data_storage.take_combat_reset_requested() {
            self.last_dps_snapshot = None;
            self.displayed_targets.clear();
            self.last_damage_gen = -1;
            self.current_target = 0;
            self.data_storage.set_current_target(0);
        }

        let current_local_id = self.data_storage.local_player_id();
        if current_local_id != self.last_known_local_id {
            // The local player can churn entity ids several times per fight, so
            // this fires repeatedly. Only invalidate the snapshot so the "you"
            // highlight follows the new id — do NOT restart target selection,
            // which would drop the current target/segment on every change.
            self.last_known_local_id = current_local_id;
            self.last_damage_gen = -1;
        }

        // If no new damage since last cycle, return cached result. A rolling
        // window changes with the clock alone, so it is recomputed every time.
        let window_since = self.window_since();
        // An encounter opened before the meter knew you is anyone's fights
        // nearby: shown once your next hit narrows it to yours.
        let encounter = (self.target_selection_mode == TargetSelectionMode::Encounter)
            .then(|| self.data_storage.current_encounter())
            .flatten()
            .filter(|e| !e.blind);
        // Damage counted from here: the ALL window's start, or the encounter's.
        let since = window_since.or(encounter.as_ref().map(|e| e.start));
        let clock_driven = window_since.is_some() || self.target_selection_mode == TargetSelectionMode::Encounter;
        let current_gen = self.data_storage.damage_generation();
        if current_gen == self.last_damage_gen && self.last_dps_snapshot.is_some() && !clock_driven {
            return self.last_dps_snapshot.as_ref().unwrap().clone();
        }
        self.last_damage_gen = current_gen;

        // Get pre-computed aggregates (cheap — small map, not 17K packets).
        // Light snapshot: skips per-hit timestamps (unused here, grows unbounded).
        let combat_data = if self.target_selection_mode == TargetSelectionMode::Encounter {
            self.data_storage.get_encounter_snapshot_light()
        } else {
            self.data_storage.get_combat_snapshot_light()
        };
        let nickname_data = self.data_storage.get_nicknames();
        let summon_data = self.data_storage.get_summon_data();

        let mut dps_data = DpsData::new();
        dps_data.local_player_id = current_local_id;
        dps_data.dungeon_id = self.data_storage.current_dungeon_id();

        // Decide target
        let (target_ids, target_name, tracking_id) = self.decide_target(&combat_data, &nickname_data, &summon_data);
        // No target keeps the rows already on screen (see below), so their
        // targets stay too.
        if !target_ids.is_empty() {
            self.displayed_targets = target_ids.iter().copied().collect();
            self.displayed_targets.sort_unstable();
        }
        dps_data.target_name = target_name;
        dps_data.target_mode = self.target_selection_mode.id().to_string();
        self.current_target = tracking_id;
        dps_data.target_id = self.current_target;
        dps_data.target_mob_code = self.data_storage.mob_code(self.current_target).unwrap_or(0);
        self.data_storage.set_current_target(self.current_target);

        // Boss HP bar source: spawn-time max HP of the single boss target. Only
        // meaningful for a single target (multi-target HP can't be summed sanely),
        // so leave it 0 otherwise and let the frontend hide the bar.
        let target_max_hp = if self.current_target != 0 {
            self.data_storage.get_mob_hp(self.current_target).unwrap_or(0) as i64
        } else {
            0
        };
        dps_data.target_max_hp = target_max_hp;

        // Real current HP from the live feed (-1 if none seen). Preferred over the
        // derived max-minus-damage bar when available.
        let target_current_hp = if self.current_target != 0 {
            self.data_storage
                .get_mob_current_hp(self.current_target)
                .map(|h| h as i64)
                .unwrap_or(-1)
        } else {
            -1
        };
        dps_data.target_current_hp = target_current_hp;

        // Collect actors from selected targets
        let now = now_ms();
        let mut combined_actors: HashMap<i32, i64> = HashMap::new();
        let mut combined_jobs: HashMap<i32, Option<JobClass>> = HashMap::new();
        // Per actor: own first and last hit, and damage in the last 10/30/60 s.
        let mut actor_extra: HashMap<i32, ActorExtra> = HashMap::new();
        for &tid in &target_ids {
            if let Some(target_data) = combat_data.get(&tid) {
                for (&actor_id, actor_data) in &target_data.actors {
                    let damage = match since {
                        Some(since) => actor_data.damage_since(since),
                        None => actor_data.total_damage,
                    };
                    if damage == 0 && since.is_some() {
                        continue;
                    }
                    *combined_actors.entry(actor_id).or_insert(0) += damage;
                    let extra = actor_extra.entry(actor_id).or_default();
                    extra.add_span(
                        actor_data.first_damage_time.max(since.unwrap_or(i64::MIN)),
                        actor_data.last_damage_time,
                    );
                    for (i, secs) in LAST_WINDOWS_S.iter().enumerate() {
                        extra.last[i] += actor_data.damage_since(now - secs * 1000);
                    }
                    extra.hits.add(&match since {
                        Some(since) => actor_data.stats_since(since),
                        None => actor_data.stats_total(),
                    });
                    if actor_data.job.is_some() && combined_jobs.get(&actor_id).and_then(|j| j.as_ref()).is_none() {
                        combined_jobs.insert(actor_id, actor_data.job);
                    }
                }
            }
        }

        // Calculate battle time
        let battle_time = if let Some(e) = &encounter {
            e.duration()
        } else if self.current_target != 0 {
            combat_data.get(&self.current_target)
                .map(|td| (td.last_damage_time - td.first_damage_time).max(0))
                .unwrap_or(0)
        } else if self.target_selection_mode == TargetSelectionMode::TrainTargets {
            // Your time on the dummies, not anyone's who hit them before you.
            let mine = self.resolve_local_ids(&summon_data).unwrap_or_default();
            active_time(target_ids.iter().filter_map(|tid| combat_data.get(tid)).filter_map(|td| {
                let ours = td.actors.iter()
                    .filter(|(a, _)| mine.contains(&summon_resolver::resolve(**a, &summon_data)));
                let (first, last) = ours.fold((i64::MAX, i64::MIN), |(f, l), (_, ad)| {
                    (f.min(ad.first_damage_time), l.max(ad.last_damage_time))
                });
                (first <= last).then_some((first, last))
            }), i64::MIN)
        } else if !target_ids.is_empty() {
            // Multi-target: the time anything selected was being fought, gaps
            // between pulls left out. The longest single target made ten
            // pulls over five minutes read as one pull's time.
            active_time(target_ids.iter()
                .filter_map(|tid| combat_data.get(tid))
                .map(|td| (td.first_damage_time, td.last_damage_time)),
                window_since.unwrap_or(i64::MIN))
        } else {
            0
        };

        if (battle_time == 0 && combined_actors.is_empty()) || combined_actors.is_empty() {
            // Keeping the last fight on screen is right until a window says
            // it is over: then the meter empties.
            if window_since.is_some() {
                self.last_dps_snapshot = None;
            }
            if let Some(ref mut snapshot) = self.last_dps_snapshot {
                snapshot.target_name = dps_data.target_name.clone();
                snapshot.target_mode = dps_data.target_mode.clone();
                snapshot.target_id = dps_data.target_id;
                snapshot.target_max_hp = target_max_hp;
                snapshot.target_total_damage = 0;
                snapshot.target_current_hp = target_current_hp;
                snapshot.dungeon_id = dps_data.dungeon_id;
                let mut snap = snapshot.clone();
                self.finalize_rows(&mut snap);
                return snap;
            }
            self.finalize_rows(&mut dps_data);
            self.last_dps_snapshot = Some(dps_data.clone());
            return dps_data;
        }

        // Build canonical nickname map from aggregates
        let canonical = build_nickname_canonical_map_from_aggregates(&combined_actors, &summon_data, &nickname_data, current_local_id.map(|v| v as i32));

        let mut total_damage: f64 = 0.0;
        let mut row_extra: HashMap<i32, ActorExtra> = HashMap::new();

        // Build PersonalData from aggregates (no packet iteration!)
        for (&actor_id, &damage) in &combined_actors {
            let raw_uid = summon_resolver::resolve(actor_id, &summon_data);
            if raw_uid <= 0 { continue; }
            let nickname = resolve_nickname(raw_uid, &nickname_data, &summon_data);
            let uid = *canonical.get(&nickname).unwrap_or(&raw_uid);

            total_damage += damage as f64;
            if let Some(extra) = actor_extra.get(&actor_id) {
                row_extra.entry(uid).or_default().absorb(extra);
            }

            let entry = dps_data.map.entry(uid).or_insert_with(|| {
                let cached_job = self.cached_job(&nickname);
                if let Some(job) = cached_job {
                    PersonalData::with_job(nickname.clone(), job)
                } else {
                    PersonalData::new(nickname.clone())
                }
            });

            if entry.nickname != nickname {
                entry.nickname = nickname.clone();
            }

            entry.amount += damage as f64;

            if uid == UNATTRIBUTED_ID {
                // Many casters of many classes: no class icon.
                entry.job = "Unknown".to_string();
            } else if entry.job.is_empty() {
                if let Some(job) = combined_jobs.get(&actor_id).and_then(|j| *j) {
                    entry.job = job.class_name().to_string();
                    self.cache_job(&nickname, job.class_name());
                }
            }
        }

        // A summon with no owner link is on the unattributed row until a link
        // arrives; then all it did, before the link too, is its owner's. No
        // guessing by class or power scalar: both are shared between players,
        // and guesses put a mob, a party member and the player into other
        // rows (2026-10-04).

        // Filter and compute DPS
        let local_ids = self.resolve_local_ids(&summon_data);
        let party_members = self.data_storage.get_party_members();
        let bt = battle_time.max(1000);
        let mut to_remove = Vec::new();
        for (&uid, data) in &mut dps_data.map {
            // Combat power joins on the character name: the roster carries an
            // account-level dbid, not the session entity id keyed here.
            data.combat_power = party_members
                .get(&data.nickname)
                .map(|m| m.combat_power)
                .unwrap_or(0);
            if data.job.is_empty() {
                if local_ids.as_ref().is_some_and(|ids| ids.contains(&uid)) {
                    // A class seen earlier this session survives a reset
                    // (issue #9); "Unknown" draws no icon.
                    data.job = self.cached_job(&data.nickname).unwrap_or_else(|| "Unknown".to_string());
                } else {
                    to_remove.push(uid);
                    continue;
                }
            }
            data.dps = data.amount / bt as f64 * 1000.0;
            if let Some(extra) = row_extra.get(&uid) {
                data.active_dps = extra.active_dps(data.amount);
                for (i, secs) in LAST_WINDOWS_S.iter().enumerate() {
                    let value = extra.last[i] as f64 / *secs as f64;
                    match i { 0 => data.last10_dps = value, 1 => data.last30_dps = value, _ => data.last60_dps = value }
                }
                data.hits = extra.hits.hits;
                data.crit_hits = extra.hits.crits;
                data.max_hit = extra.hits.max_hit;
            }
            data.damage_contribution = if total_damage > 0.0 {
                data.amount / total_damage * 100.0
            } else {
                0.0
            };
        }
        for uid in to_remove {
            dps_data.map.remove(&uid);
        }

        self.finalize_rows(&mut dps_data);

        dps_data.battle_time = battle_time;
        self.displayed_battle_time = battle_time;
        // total_damage here is the cumulative damage to the selected target(s).
        // Paired with target_max_hp it yields remaining = max(0, max_hp - dealt).
        dps_data.target_total_damage = total_damage as i64;
        // If the boss is dead, force the bar to empty. The derived remaining can
        // leave a sliver because the meter never observes every last hit (there is
        // no live boss HP packet), so a kill should still read 0%.
        if target_max_hp > 0
            && self.current_target != 0
            && self.data_storage.is_entity_dead(self.current_target)
        {
            dps_data.target_total_damage = target_max_hp;
            dps_data.target_current_hp = 0;
        }
        self.last_dps_snapshot = Some(dps_data.clone());
        dps_data
    }
}

/// Total time covered by `spans` of (first hit, last hit), overlaps counted
/// once, each clipped to start no earlier than `since`.
pub(super) fn active_time(spans: impl Iterator<Item = (i64, i64)>, since: i64) -> i64 {
    let mut spans: Vec<(i64, i64)> = spans
        .filter(|&(_, last)| last >= since)
        .map(|(first, last)| (first.max(since), last))
        .collect();
    spans.sort_unstable();
    let mut total = 0;
    let mut current: Option<(i64, i64)> = None;
    for (first, last) in spans {
        current = match current {
            Some((start, end)) if first <= end => Some((start, end.max(last))),
            Some((start, end)) => {
                total += end - start;
                Some((first, last))
            }
            None => Some((first, last)),
        };
    }
    total + current.map_or(0, |(start, end)| end - start)
}

/// The Last N columns, in seconds.
const LAST_WINDOWS_S: [i64; 3] = [10, 30, 60];

/// A row's own time in combat, its recent damage and its hits, gathered
/// over the actors (player and summons) the row stands for.
#[derive(Default, Clone)]
struct ActorExtra {
    first: Option<i64>,
    last_hit: Option<i64>,
    last: [i64; 3],
    hits: SecondStats,
}

impl ActorExtra {
    fn add_span(&mut self, first: i64, last: i64) {
        if first > last { return; }
        self.first = Some(self.first.map_or(first, |f| f.min(first)));
        self.last_hit = Some(self.last_hit.map_or(last, |l| l.max(last)));
    }

    fn absorb(&mut self, other: &ActorExtra) {
        if let (Some(f), Some(l)) = (other.first, other.last_hit) {
            self.add_span(f, l);
        }
        for i in 0..3 {
            self.last[i] += other.last[i];
        }
        self.hits.add(&other.hits);
    }

    /// Damage over first-to-last hit, at least one second.
    fn active_dps(&self, damage: f64) -> f64 {
        match (self.first, self.last_hit) {
            (Some(f), Some(l)) => damage / ((l - f).max(1000) as f64) * 1000.0,
            _ => 0.0,
        }
    }
}
