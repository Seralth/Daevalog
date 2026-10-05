//! Target selection: the modes, and which targets each one puts on the meter.

use std::collections::{HashMap, HashSet};

use crate::clock::now_ms;
use crate::combat::data_storage::TargetCombatData;
use crate::entity::summon_resolver;

use super::DpsCalculator;

/// How often idle targets are retired. See `retire_idle_targets`.
const RETIRE_EVERY_MS: i64 = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetSelectionMode {
    BossTargets,
    MostDamage,
    MostRecent,
    LastHitByMe,
    AllTargets,
    TrainTargets,
    Encounter,
}

impl TargetSelectionMode {
    pub fn from_id(id: &str) -> Self {
        match id {
            "bossTargets" => Self::BossTargets,
            "mostDamage" => Self::MostDamage,
            "mostRecent" => Self::MostRecent,
            "lastHitByMe" => Self::LastHitByMe,
            "allTargets" => Self::AllTargets,
            "trainTargets" => Self::TrainTargets,
            "encounter" => Self::Encounter,
            _ => Self::LastHitByMe,
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            Self::BossTargets => "bossTargets",
            Self::MostDamage => "mostDamage",
            Self::MostRecent => "mostRecent",
            Self::LastHitByMe => "lastHitByMe",
            Self::AllTargets => "allTargets",
            Self::TrainTargets => "trainTargets",
            Self::Encounter => "encounter",
        }
    }
}

impl DpsCalculator {
    pub fn set_target_selection_mode(&mut self, id: &str) {
        let mode = TargetSelectionMode::from_id(id);
        if mode != self.target_selection_mode {
            // Recompute on the next update even without new damage: the
            // cached result still names the old mode and its target, so the
            // meter went on as if nothing had changed until someone hit
            // something. Its rows are the old mode's too, so they go.
            self.last_damage_gen = -1;
            self.last_dps_snapshot = None;
            self.displayed_targets.clear();
        }
        self.target_selection_mode = mode;
        self.publish_view();
    }

    /// ALL mode's "last N minutes" window; 0 turns it off (everything since
    /// the zone change).
    pub fn set_all_targets_window_ms(&mut self, ms: i64) {
        let ms = if ms <= 0 { 0 } else { ms.clamp(10_000, crate::combat::data_storage::DAMAGE_HISTORY_MS) };
        if ms != self.all_targets_window_ms {
            self.last_damage_gen = -1;
        }
        self.all_targets_window_ms = ms;
    }

    /// Start of ALL mode's window in unix ms, when the window is on.
    pub(super) fn window_since(&self) -> Option<i64> {
        (self.target_selection_mode == TargetSelectionMode::AllTargets && self.all_targets_window_ms > 0)
            .then(|| now_ms() - self.all_targets_window_ms)
    }

    pub fn restart_target_selection(&mut self, clear_damage: bool) {
        self.current_target = 0;
        self.last_dps_snapshot = None;
        self.displayed_targets.clear();
        self.last_damage_gen = -1;
        if clear_damage {
            self.data_storage.flush();
        }
        self.data_storage.set_current_target(0);
        self.publish_view();
    }

    /// Retire the idle targets this mode does not show, every few seconds:
    /// in the open world they piled up for as long as the meter ran. The
    /// rows on screen stay; so does everything ALL shows, which without a
    /// window is everything since the zone change.
    pub(super) fn retire_idle_targets(&mut self) {
        let now = now_ms();
        if now.saturating_sub(self.last_retire_ms) < RETIRE_EVERY_MS {
            return;
        }
        self.last_retire_ms = now;
        let shown: HashSet<i32> = self.displayed_targets.iter().copied().collect();
        let mode = self.target_selection_mode;
        let since = self.window_since();
        let mob_data = if mode == TargetSelectionMode::TrainTargets { self.data_storage.get_mob_data() } else { HashMap::new() };
        let npc_lookup = &self.npc_lookup;
        self.data_storage.retire_idle(now, |tid, td| {
            shown.contains(&tid)
                || match mode {
                    TargetSelectionMode::AllTargets => since.is_none_or(|since| td.last_damage_time >= since),
                    TargetSelectionMode::TrainTargets => mob_data.get(&tid).is_some_and(|&code| npc_lookup.is_training_dummy(code)),
                    _ => false,
                }
        });
    }

    pub(super) fn decide_target(
        &mut self,
        combat_data: &HashMap<i32, TargetCombatData>,
        nickname_data: &HashMap<i32, String>,
        summon_data: &HashMap<i32, i32>,
    ) -> (HashSet<i32>, String, i32) {
        let mob_data = self.data_storage.get_mob_data();

        match self.target_selection_mode {
            TargetSelectionMode::MostDamage => {
                let best = combat_data.iter()
                    .max_by_key(|(_, td)| td.total_damage);
                match best {
                    Some((&id, _)) => {
                        let name = self.resolve_target_name(id);
                        (HashSet::from([id]), name, id)
                    }
                    None => (HashSet::new(), String::new(), 0),
                }
            }
            TargetSelectionMode::MostRecent => {
                let best = combat_data.iter()
                    .max_by_key(|(_, td)| td.last_damage_time);
                match best {
                    Some((&id, _)) => {
                        let name = self.resolve_target_name(id);
                        (HashSet::from([id]), name, id)
                    }
                    None => (HashSet::new(), String::new(), 0),
                }
            }
            TargetSelectionMode::BossTargets => {
                // Once you are identified, only what you or your party hit:
                // a stranger's field boss nearby took the meter over.
                let ours = self.ours_ids(nickname_data, summon_data);
                let is_ours = |td: &TargetCombatData| ours.as_ref().is_none_or(|ids| td.actors.keys()
                    .any(|&a| ids.contains(&summon_resolver::resolve(a, summon_data))));
                let newest_hit = combat_data.values()
                    .filter(|td| is_ours(*td))
                    .map(|td| td.last_damage_time)
                    .max()
                    .unwrap_or(i64::MIN);
                let is_boss = |tid: i32| mob_data.get(&tid).is_some_and(|&code| self.npc_lookup.is_boss(code));
                let boss_targets: Vec<_> = combat_data.iter()
                    .filter(|(tid, td)| {
                        is_boss(**tid)
                            && is_ours(*td)
                            // A dead boss gives way once you fight something newer.
                            && !(td.last_damage_time < newest_hit && self.data_storage.is_entity_dead(**tid))
                    })
                    .map(|(&tid, _)| tid)
                    .collect();

                if let Some(&best) = boss_targets.iter()
                    .max_by_key(|&&tid| combat_data.get(&tid).map(|td| td.last_damage_time).unwrap_or(0))
                {
                    let name = self.resolve_target_name(best);
                    (HashSet::from([best]), name, best)
                } else if self.data_storage.current_dungeon_id() > 0 {
                    // No boss yet in a dungeon: show nothing. Every mob in an
                    // instance is on the way to a boss, so the fallback below
                    // put the first trash pull of each run on the meter.
                    (HashSet::new(), String::new(), 0)
                } else {
                    // No boss: the mob with the most damage that you or your
                    // party hit, and nothing until the meter knows who you
                    // are. Any mob within range used to count then, and in
                    // the open world that put strangers fighting their own
                    // mobs on your meter (2026-10-04: one player, then
                    // another, each alone on a mob you never touched), and
                    // still did for the seconds after opening the meter or
                    // entering a zone, before you were identified
                    // (taengu/A2Tools-DPS-Meter db1079f).
                    // Any boss here is one that gave way above.
                    let best = combat_data.iter()
                        .filter(|(tid, td)| !is_boss(**tid) && ours.is_some() && is_ours(*td))
                        .max_by_key(|(_, td)| td.total_damage);
                    match best {
                        Some((&id, _)) => {
                            let name = self.resolve_target_name(id);
                            (HashSet::from([id]), name, id)
                        }
                        None => (HashSet::new(), String::new(), 0),
                    }
                }
            }
            TargetSelectionMode::Encounter => {
                // The enemies of the current encounter, while their data lasts.
                let targets: HashSet<i32> = self.data_storage.current_encounter()
                    .map(|e| e.targets.into_iter().filter(|t| combat_data.contains_key(t)).collect())
                    .unwrap_or_default();
                (targets, "Encounter".to_string(), 0)
            }
            TargetSelectionMode::AllTargets => {
                let since = self.window_since().unwrap_or(i64::MIN);
                let all: HashSet<i32> = combat_data.iter()
                    .filter(|(_, td)| td.last_damage_time >= since)
                    .map(|(&id, _)| id)
                    .collect();
                (all, "All Targets".to_string(), 0)
            }
            TargetSelectionMode::TrainTargets => {
                // The dummies you hit yourself, your summons' hits on them
                // included; nothing until the meter knows you. A dummy only
                // your summons touched is spill-over (a spirit's area hit on
                // the next dummy), unless summons are all that is hitting.
                let Some(me) = self.data_storage.local_player_id().map(|id| id as i32) else {
                    return (HashSet::new(), "Train".to_string(), 0);
                };
                let mine = self.resolve_local_ids(summon_data).unwrap_or_default();
                let dummies = || combat_data.iter()
                    .filter(|(tid, _)| mob_data.get(*tid).is_some_and(|&code| self.npc_lookup.is_training_dummy(code)));
                let mut trains: HashSet<i32> = dummies()
                    .filter(|(_, td)| td.actors.contains_key(&me))
                    .map(|(&tid, _)| tid)
                    .collect();
                if trains.is_empty() {
                    trains = dummies()
                        .filter(|(_, td)| td.actors.keys().any(|&a| mine.contains(&summon_resolver::resolve(a, summon_data))))
                        .map(|(&tid, _)| tid)
                        .collect();
                }
                (trains, "Train".to_string(), 0)
            }
            TargetSelectionMode::LastHitByMe => {
                let local_ids = self.resolve_local_ids(summon_data);
                if let Some(ref ids) = local_ids {
                    // Find the target most recently damaged by the local player
                    let mut best_target: Option<(i32, i64)> = None;
                    for (&target_id, target_data) in combat_data {
                        for (&actor_id, actor_data) in &target_data.actors {
                            let resolved = summon_resolver::resolve(actor_id, summon_data);
                            if ids.contains(&resolved) {
                                let ts = actor_data.last_damage_time;
                                if best_target.is_none() || ts > best_target.unwrap().1 {
                                    best_target = Some((target_id, ts));
                                }
                            }
                        }
                    }
                    match best_target {
                        Some((id, _)) => {
                            let name = self.resolve_target_name(id);
                            (HashSet::from([id]), name, id)
                        }
                        None => (HashSet::new(), String::new(), 0),
                    }
                } else {
                    // Not identified: nothing. Falling back to the mob anyone
                    // hit last put strangers' fights on the meter while it
                    // waited, and hid the UI's "Identifying you..." label.
                    (HashSet::new(), String::new(), 0)
                }
            }
        }
    }

    pub(super) fn resolve_target_name(&self, target_id: i32) -> String {
        let mob_data = self.data_storage.get_mob_data();
        if let Some(&code) = mob_data.get(&target_id) {
            let name = self.npc_lookup.get_npc_name(code);
            if !name.is_empty() {
                return name;
            }
        }
        String::new()
    }

    pub(super) fn resolve_local_ids(&self, summon_data: &HashMap<i32, i32>) -> Option<HashSet<i32>> {
        let local_id = self.data_storage.local_player_id()? as i32;
        let mut ids = HashSet::new();
        ids.insert(local_id);
        for (&summon, &owner) in summon_data {
            if summon_resolver::resolve(owner, summon_data) == local_id {
                ids.insert(summon);
            }
        }
        Some(ids)
    }

    /// You, your summons and your party, by entity id; `None` until the
    /// meter knows who you are.
    fn ours_ids(&self, nickname_data: &HashMap<i32, String>, summon_data: &HashMap<i32, i32>) -> Option<HashSet<i32>> {
        self.resolve_local_ids(summon_data).map(|mut ids| {
            let party = self.data_storage.get_party_members();
            ids.extend(nickname_data.iter()
                .filter(|(_, name)| party.contains_key(name.as_str()))
                .map(|(&id, _)| id));
            ids
        })
    }
}
