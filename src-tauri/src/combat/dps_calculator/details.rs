//! Skill details: the Details panel data for a target, the rows on screen, or a saved fight.

use std::collections::{HashMap, HashSet};

use crate::combat::data_storage::{HealSkillData, SegmentIdentity, TargetCombatData};
use crate::entity::details_context::*;
use crate::entity::job_class::JobClass;
use crate::entity::summon_resolver;

use super::rows::{build_nickname_canonical_map_from_aggregates, resolve_nickname};
use super::{DpsCalculator, TargetSelectionMode};

impl DpsCalculator {
    pub fn get_details_context(&self) -> DetailsContext {
        // Light snapshot: this builds per-target/per-actor summaries only — the
        // per-hit timeline is fetched separately via get_target_details.
        let combat_data = self.data_storage.get_combat_snapshot_light();
        let nickname_data = self.data_storage.get_nicknames();
        let summon_data = self.data_storage.get_summon_data();
        let supporters = self.data_storage.supporters();
        let party_members = self.data_storage.get_party_members();
        let mob_hp_data = self.data_storage.get_mob_hp_data();
        let mob_data = self.data_storage.get_mob_data();

        let mut actor_meta: HashMap<i32, (String, String)> = HashMap::new();
        let mut targets = Vec::new();

        for (&target_id, target_data) in &combat_data {
            let mut actor_damage: HashMap<i32, i64> = HashMap::new();
            let canonical = build_nickname_canonical_map_from_aggregates(
                &target_data.actors.iter().map(|(&id, ad)| (id, ad.total_damage)).collect(),
                &summon_data,
                &nickname_data,
                self.data_storage.local_player_id().map(|v| v as i32),
            );

            for (&actor_id, actor_data) in &target_data.actors {
                let raw_uid = summon_resolver::resolve(actor_id, &summon_data);
                if raw_uid <= 0 { continue; }
                let nickname = resolve_nickname(raw_uid, &nickname_data, &summon_data);
                let uid = *canonical.get(&nickname).unwrap_or(&raw_uid);
                *actor_damage.entry(uid).or_insert(0) += actor_data.total_damage;

                actor_meta.entry(uid).or_insert_with(|| {
                    (resolve_nickname(uid, &nickname_data, &summon_data), String::new())
                });

                if actor_meta.get(&uid).unwrap().1.is_empty() {
                    if let Some(job) = actor_data.job {
                        actor_meta.get_mut(&uid).unwrap().1 = job.class_name().to_string();
                    }
                }
            }

            // Remove actors with no job and no nickname
            let remove_ids: Vec<i32> = actor_damage.keys()
                .filter(|id| {
                    actor_meta.get(id).is_some_and(|(_, job)| job.is_empty() && !nickname_data.contains_key(id))
                })
                .copied().collect();
            for id in remove_ids {
                actor_damage.remove(&id);
                actor_meta.remove(&id);
            }

            let target_name = if let Some(&code) = mob_data.get(&target_id) {
                self.npc_lookup.get_npc_name(code)
            } else {
                String::new()
            };

            targets.push(DetailsTargetSummary {
                target_id,
                target_name,
                max_hp: mob_hp_data.get(&target_id).copied().unwrap_or(0),
                battle_time: (target_data.last_damage_time - target_data.first_damage_time).max(0),
                last_damage_time: target_data.last_damage_time,
                total_damage: target_data.total_damage,
                actor_damage,
            });
        }

        let actors: Vec<DetailsActorSummary> = actor_meta.iter()
            .map(|(&id, (nick, job))| {
                let job_id = if let Some(jc) = JobClass::convert_from_skill(
                    // Find a skill code from this actor's aggregate data
                    combat_data.values()
                        .flat_map(|td| td.actors.get(&id))
                        .flat_map(|ad| ad.skills.keys())
                        .find(|&&(sc, _)| JobClass::convert_from_skill(sc).is_some())
                        .map(|&(sc, _)| sc)
                        .unwrap_or(0)
                ) { jc.class_prefix() } else { 0 };
                // Aggregate per-actor stats
                let (mut party_heal, mut regen, mut dmg_recv, mut hits_recv) = (0i64, 0i64, 0i64, 0i32);
                for td in combat_data.values() {
                    if let Some(ad) = td.actors.get(&id) {
                        party_heal += ad.party_heal;
                        regen += ad.regen;
                        dmg_recv += ad.damage_received;
                        hits_recv += ad.hits_received;
                    }
                }
                DetailsActorSummary {
                    actor_id: id,
                    nickname: nick.clone(),
                    job: job.clone(),
                    job_id,
                    party_heal,
                    regen,
                    damage_received: dmg_recv,
                    hits_received: hits_recv,
                    // The live view is never uploaded: nothing here reads the
                    // identity a saved record keeps.
                    dbid: 0,
                    server_id: 0,
                    // Joined to the party roster as the meter rows are: the
                    // published roster is keyed by dbid, and by name alone no
                    // supporter ever turned gold here (2026-10-05).
                    is_supporter: supporters.contains(
                        nick,
                        party_members.get(nick.as_str()).map(|m| m.dbid).unwrap_or(0),
                    ),
                    // Same reason again: the live rows already show combat
                    // power from the roster; the saved record is where it
                    // has to persist.
                    level: 0,
                    gear_score: 0,
                    combat_power: 0,
                }
            })
            .collect();

        DetailsContext {
            // From storage, which the meter keeps in step: a details reader
            // has no target of its own.
            current_target_id: self.data_storage.current_target(),
            targets,
            actors,
        }
    }

    /// Skill details behind a row on screen, in any mode: one target as
    /// `get_target_details`, several (ALL, TRAIN) merged into one before the
    /// rows are resolved, so a row's details match the row.
    pub fn get_displayed_details(&self, actor_ids: Option<&[i32]>) -> TargetDetailsResponse {
        self.displayed_details(actor_ids, false)
    }

    /// `get_displayed_details` for the hover tooltip: the skills only, without
    /// hit timelines, healing or ping.
    pub fn get_displayed_hover_details(&self, actor_ids: Option<&[i32]>) -> TargetDetailsResponse {
        self.displayed_details(actor_ids, true)
    }

    fn displayed_details(&self, actor_ids: Option<&[i32]>, summary_only: bool) -> TargetDetailsResponse {
        if let [one] = self.displayed_targets.as_slice() {
            return self.target_details(*one, actor_ids, false, summary_only);
        }
        let combat_data = self.target_snapshots(&self.displayed_targets, summary_only);
        let merged = TargetCombatData::merged(self.displayed_targets.iter().filter_map(|t| combat_data.get(t)));
        let Some(merged) = merged else { return self.target_details(0, actor_ids, false, summary_only) };
        let heals = if summary_only { HashMap::new() } else { self.data_storage.get_heal_snapshot() };
        let mut details = self.details_for(&merged, 0, &heals, actor_ids, None);
        if summary_only {
            details.ping_history.clear();
        }
        details.battle_time = self.displayed_battle_time;
        details
    }

    pub fn get_target_details(&self, target_id: i32, actor_ids: Option<&[i32]>) -> TargetDetailsResponse {
        self.target_details(target_id, actor_ids, false, false)
    }

    /// `get_target_details` for the hover tooltip: the skills only, without
    /// hit timelines, healing or ping.
    pub fn get_hover_details(&self, target_id: i32, actor_ids: Option<&[i32]>) -> TargetDetailsResponse {
        self.target_details(target_id, actor_ids, false, true)
    }

    /// `get_target_details` for a saved fight: only the healing done during it.
    pub(super) fn fight_details(&self, target_id: i32) -> TargetDetailsResponse {
        self.target_details(target_id, None, true, false)
    }

    fn target_details(&self, target_id: i32, actor_ids: Option<&[i32]>, own_heals: bool, summary_only: bool) -> TargetDetailsResponse {
        let combat_data = self.target_snapshots(&[target_id], summary_only);
        let target_data = match combat_data.get(&target_id) {
            Some(td) => td,
            None => return TargetDetailsResponse {
                target_id,
                max_hp: 0,
                total_target_damage: 0,
                battle_time: 0,
                start_time: 0,
                skills: Vec::new(),
                ping_history: Vec::new(),
                heal_skills: Vec::new(),
            },
        };
        let max_hp = self.data_storage.get_mob_hp(target_id).unwrap_or(0);
        if summary_only {
            let mut details = self.details_for(target_data, max_hp, &HashMap::new(), actor_ids, None);
            details.ping_history.clear();
            return details;
        }
        let heals = if own_heals {
            self.data_storage.heals_between(target_data.first_damage_time, target_data.last_damage_time)
        } else {
            self.data_storage.get_heal_snapshot()
        };
        self.details_for(target_data, max_hp, &heals, actor_ids, None)
    }

    /// The live data of these targets only; in ENC with what a boss pull
    /// cleared of the encounter. `light` leaves out the hit timelines.
    fn target_snapshots(&self, targets: &[i32], light: bool) -> HashMap<i32, TargetCombatData> {
        let encounter = self.target_selection_mode == TargetSelectionMode::Encounter;
        self.data_storage.get_target_snapshots(targets, encounter, light)
    }

    /// Details of one fight segment, live or already cleared out.
    pub(super) fn details_for(
        &self,
        target_data: &TargetCombatData,
        max_hp: i32,
        heals: &HashMap<i32, HashMap<(i32, bool), HealSkillData>>,
        actor_ids: Option<&[i32]>,
        identity: Option<&SegmentIdentity>,
    ) -> TargetDetailsResponse {
        let target_id = target_data.target_id;
        let (summon_data, nickname_data, local_id) = match identity {
            Some(i) => (i.summons.clone(), i.nicknames.clone(), i.local_player_id),
            None => (
                self.data_storage.get_summon_data(),
                self.data_storage.get_nicknames(),
                self.data_storage.local_player_id(),
            ),
        };

        let actor_damage_map: HashMap<i32, i64> = target_data.actors.iter()
            .map(|(&id, ad)| (id, ad.total_damage))
            .collect();
        let canonical = build_nickname_canonical_map_from_aggregates(&actor_damage_map, &summon_data, &nickname_data, local_id.map(|v| v as i32));

        // Build expanded actor ID set for filtering
        let filter_uids: Option<HashSet<i32>> = actor_ids.map(|ids| {
            let canonical_ids: HashSet<i32> = ids.iter()
                .map(|&id| {
                    let nick = resolve_nickname(id, &nickname_data, &summon_data);
                    *canonical.get(&nick).unwrap_or(&id)
                })
                .collect();
            let mut expanded = HashSet::from_iter(ids.iter().copied());
            for &actor_id in target_data.actors.keys() {
                let raw_uid = summon_resolver::resolve(actor_id, &summon_data);
                if raw_uid <= 0 { continue; }
                let nick = resolve_nickname(raw_uid, &nickname_data, &summon_data);
                let uid = *canonical.get(&nick).unwrap_or(&raw_uid);
                if canonical_ids.contains(&uid) {
                    expanded.insert(raw_uid);
                }
            }
            expanded
        });

        // Build skill entries from aggregates (no packet iteration!)
        let mut skill_map: HashMap<(i32, i32), DetailSkillEntry> = HashMap::new();
        let fight_start = target_data.first_damage_time;

        for (&actor_id, actor_data) in &target_data.actors {
            let raw_uid = summon_resolver::resolve(actor_id, &summon_data);
            if raw_uid <= 0 { continue; }

            if let Some(ref filter) = filter_uids {
                if !filter.contains(&raw_uid) { continue; }
            }

            let nickname = resolve_nickname(raw_uid, &nickname_data, &summon_data);
            let uid = *canonical.get(&nickname).unwrap_or(&raw_uid);

            for (&(raw_skill, is_dot), skill_data) in &actor_data.skills {
                let skill_code = crate::entity::skill_group::row_skill(raw_skill, &self.skill_lookup);

                let dot_offset = if is_dot { 1_000_000_000 } else { 0 };
                let key = (uid, skill_code + dot_offset);
                let mut skill_name = self.skill_lookup.lookup_skill_name(skill_code);
                if is_dot && !skill_name.is_empty() {
                    skill_name = format!("{} - DOT", skill_name);
                }
                let job = JobClass::convert_from_skill(skill_code)
                    .map(|j| j.class_name().to_string())
                    .unwrap_or_default();

                let entry = skill_map.entry(key).or_insert_with(|| DetailSkillEntry {
                    actor_id: uid,
                    code: skill_code,
                    name: skill_name,
                    time: 0,
                    dmg: 0,
                    multi_hit_count: 0,
                    multi_hit_damage: 0,
                    multi_hit_hits: 0,
                    min_dmg: i32::MAX,
                    max_dmg: 0,
                    crit: 0,
                    shield_block: 0,
                    parry: 0,
                    back: 0,
                    frontal: 0,
                    perfect: 0,
                    double: 0,
                    iron_wall: 0,
                    regeneration: 0,
                    perfect_block: 0,
                    miss: 0,
                    resist: 0,
                    regen: 0,
                    job,
                    is_dot,
                    hit_timestamps: Vec::new(),
                    specs: skill_data.spec_flags.to_vec(),
                });

                entry.time += skill_data.hit_count;
                // saturating: damage sums are i32 and can exceed i32::MAX across
                // a long fight / many actors — avoid overflow panic (debug) and
                // wrap-to-negative (release).
                entry.dmg = entry.dmg.saturating_add(skill_data.total_damage);
                entry.multi_hit_count += skill_data.multi_hit_count;
                entry.multi_hit_damage = entry.multi_hit_damage.saturating_add(skill_data.multi_hit_damage);
                entry.multi_hit_hits += skill_data.multi_hit_hits;
                if skill_data.min_damage < entry.min_dmg { entry.min_dmg = skill_data.min_damage; }
                if skill_data.max_damage > entry.max_dmg { entry.max_dmg = skill_data.max_damage; }
                entry.crit += skill_data.crit_count;
                entry.back += skill_data.back_count;
                entry.frontal += skill_data.frontal_count;
                entry.shield_block += skill_data.shield_block_count;
                entry.parry += skill_data.parry_count;
                entry.perfect += skill_data.perfect_count;
                entry.double += skill_data.double_count;
                entry.iron_wall += skill_data.iron_wall_count;
                entry.regeneration += skill_data.regeneration_count;
                entry.perfect_block += skill_data.perfect_block_count;
                entry.miss += skill_data.miss_count;
                entry.resist += skill_data.resist_count;
                entry.regen = entry.regen.saturating_add(skill_data.heal_amount);
                // Add timestamps relative to fight start
                for &ts in &skill_data.hit_timestamps {
                    entry.hit_timestamps.push(ts - fight_start);
                }
                // Merge spec flags
                for (i, &flag) in skill_data.spec_flags.iter().enumerate() {
                    if flag && i < entry.specs.len() { entry.specs[i] = true; }
                }
            }
        }

        // Fix min_dmg sentinel
        for entry in skill_map.values_mut() {
            if entry.min_dmg == i32::MAX { entry.min_dmg = 0; }
        }

        // Healing done this segment, per healer/skill. Keyed by the canonical actor
        // (same nickname/summon resolution as damage). Reuses DetailSkillEntry:
        // dmg = heal amount, time = tick count, is_dot = HoT.
        let mut heal_map: HashMap<(i32, i32), DetailSkillEntry> = HashMap::new();
        for (&actor_id, skills) in heals {
            let raw_uid = summon_resolver::resolve(actor_id, &summon_data);
            if raw_uid <= 0 { continue; }
            let nickname = resolve_nickname(raw_uid, &nickname_data, &summon_data);
            let uid = *canonical.get(&nickname).unwrap_or(&raw_uid);
            if let Some(ref filter) = filter_uids {
                if !filter.contains(&uid) { continue; }
            }
            for (&(skill_code, is_hot), hd) in skills {
                let mut skill_name = self.skill_lookup.lookup_skill_name(skill_code);
                if is_hot && !skill_name.is_empty() {
                    skill_name = format!("{} - HoT", skill_name);
                }
                let job = JobClass::convert_from_skill(skill_code)
                    .map(|j| j.class_name().to_string())
                    .unwrap_or_default();
                let key = (uid, skill_code + if is_hot { 1_000_000_000 } else { 0 });
                let entry = heal_map.entry(key).or_insert_with(|| DetailSkillEntry {
                    actor_id: uid,
                    code: skill_code,
                    name: skill_name,
                    time: 0,
                    dmg: 0,
                    multi_hit_count: 0,
                    multi_hit_damage: 0,
                    multi_hit_hits: 0,
                    min_dmg: 0,
                    max_dmg: 0,
                    crit: 0,
                    shield_block: 0,
                    parry: 0,
                    back: 0,
                    frontal: 0,
                    perfect: 0,
                    double: 0,
                    iron_wall: 0,
                    regeneration: 0,
                    perfect_block: 0,
                    miss: 0,
                    resist: 0,
                    regen: 0,
                    job,
                    is_dot: is_hot,
                    hit_timestamps: Vec::new(),
                    specs: Vec::new(),
                });
                entry.dmg = entry.dmg.saturating_add(hd.total_heal.min(i32::MAX as i64) as i32);
                entry.time += hd.tick_count;
            }
        }

        let battle_time = (target_data.last_damage_time - target_data.first_damage_time).max(0);

        let ping_history = self.ping_tracker.get_ping_history(
            target_data.first_damage_time, target_data.last_damage_time
        ).into_iter()
            .map(|(ts, ping)| PingPoint { ts_ms: ts - fight_start, ping_ms: ping })
            .collect();

        TargetDetailsResponse {
            target_id,
            max_hp,
            total_target_damage: target_data.total_damage,
            battle_time,
            start_time: target_data.first_damage_time,
            skills: skill_map.into_values().collect(),
            ping_history,
            heal_skills: heal_map.into_values().collect(),
        }
    }
}
