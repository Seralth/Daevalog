//! Skill details: the Details panel data for a target, the rows on screen, or a saved fight.

use std::collections::{HashMap, HashSet};

use crate::combat::data_storage::{DeathsBy, HealSkillData, SegmentIdentity, TakenBy, TargetCombatData, UNATTRIBUTED_ID};
use crate::entity::deaths::DeathEntry;
use crate::entity::details_context::*;
use crate::entity::job_class::JobClass;
use crate::entity::taken::{TakenSkillEntry, TakenStats};
use crate::entity::summon_resolver;

use super::meter_rows::{active_time, own_span};
use super::rows::{build_nickname_canonical_map_from_aggregates, resolve_nickname};
use super::{DpsCalculator, TargetSelectionMode};

impl DpsCalculator {
    pub fn get_details_context(&self) -> DetailsContext {
        // Light snapshot: this builds per-target/per-actor summaries only — the
        // per-hit timeline is fetched separately via get_target_details.
        let combat_data = self.data_storage.get_combat_snapshot_light();
        let nickname_data = self.data_storage.get_nicknames();
        let summon_data = self.data_storage.get_summon_data();
        let mob_hp_data = self.data_storage.get_mob_hp_data();
        let mob_data = self.data_storage.get_mob_data();
        let local_id = self.data_storage.local_player_id().map(|v| v as i32);

        // TRAIN and BOSS: the targets on the meter. Details' "All" merged
        // every dummy in the area in TRAIN, other players' too; in BOSS it
        // took every target not yet dropped, the boss with whatever adds were
        // hit in the last 30 s (Kasia 2026-10-06: healing 19.74k on every
        // target, 16.40k for the boss the meter showed).
        let train = self.target_selection_mode == TargetSelectionMode::TrainTargets;
        let on_meter = train || self.target_selection_mode == TargetSelectionMode::BossTargets;
        // Most damage first, then by id. A hash map's order changed with
        // every read, and with it the first listed target.
        let mut listed: Vec<(i32, &TargetCombatData)> = combat_data.iter()
            .filter(|(id, _)| !on_meter || self.displayed_targets.contains(id))
            .map(|(&id, td)| (id, td))
            .collect();
        listed.sort_by_key(|&(id, td)| (std::cmp::Reverse(td.total_damage), id));
        let listed_ids: Vec<i32> = listed.iter().map(|&(id, _)| id).collect();

        // Each target's actors on their canonical ids, and each player's
        // class from all their skills on the listed targets (`JobClass::by_hits`).
        let canonicals: Vec<HashMap<String, i32>> = listed.iter()
            .map(|(_, td)| build_nickname_canonical_map_from_aggregates(
                &td.actors.iter().map(|(&id, ad)| (id, ad.total_damage)).collect(),
                &summon_data,
                &nickname_data,
                local_id,
            ))
            .collect();
        let uid_of = |actor_id: i32, canonical: &HashMap<String, i32>| -> Option<i32> {
            let raw_uid = summon_resolver::resolve(actor_id, &summon_data);
            if raw_uid <= 0 { return None; }
            let nickname = resolve_nickname(raw_uid, &nickname_data, &summon_data);
            Some(*canonical.get(&nickname).unwrap_or(&raw_uid))
        };
        let mut class_hits: HashMap<i32, Vec<(i32, i32)>> = HashMap::new();
        for ((_, td), canonical) in listed.iter().zip(&canonicals) {
            for (&actor_id, actor_data) in &td.actors {
                let Some(uid) = uid_of(actor_id, canonical) else { continue };
                class_hits.entry(uid).or_default()
                    .extend(actor_data.skills.values().map(|s| (s.skill_code, s.hit_count)));
            }
        }
        let class_of = |uid: i32| if uid == UNATTRIBUTED_ID { None } else { class_hits.get(&uid).and_then(|h| JobClass::by_hits(h.iter().copied())) };

        let mut actor_meta: HashMap<i32, (String, String)> = HashMap::new();
        let mut targets = Vec::new();
        for (&(target_id, target_data), canonical) in listed.iter().zip(&canonicals) {
            let mut actor_damage: HashMap<i32, i64> = HashMap::new();
            for (&actor_id, actor_data) in &target_data.actors {
                let Some(uid) = uid_of(actor_id, canonical) else { continue };
                *actor_damage.entry(uid).or_insert(0) += actor_data.total_damage;
                actor_meta.entry(uid).or_insert_with(|| {
                    let job = class_of(uid).map(|j| j.class_name().to_string()).unwrap_or_default();
                    (resolve_nickname(uid, &nickname_data, &summon_data), job)
                });
            }

            // Remove actors with no job and no nickname
            let remove_ids: Vec<i32> = actor_damage.keys()
                .filter(|id| {
                    **id != UNATTRIBUTED_ID
                        && actor_meta.get(id).is_some_and(|(_, job)| job.is_empty() && !nickname_data.contains_key(id))
                })
                .copied().collect();
            for id in remove_ids {
                actor_damage.remove(&id);
                actor_meta.remove(&id);
            }

            let mob_code = mob_data.get(&target_id).copied().unwrap_or(0);
            let target_name = if mob_code != 0 { self.npc_lookup.get_npc_name(mob_code) } else { String::new() };

            targets.push(DetailsTargetSummary {
                target_id,
                target_name,
                mob_code,
                max_hp: mob_hp_data.get(&target_id).copied().unwrap_or(0),
                battle_time: (target_data.last_damage_time - target_data.first_damage_time).max(0),
                last_damage_time: target_data.last_damage_time,
                total_damage: target_data.total_damage,
                actor_damage,
            });
        }

        // Damage taken over the listed fights, on the actor of the same name.
        let by_name: HashMap<&str, i32> = actor_meta.iter().map(|(&id, (nick, _))| (nick.as_str(), id)).collect();
        let mut taken: HashMap<i32, TakenStats> = HashMap::new();
        let mut taken_skills: HashMap<(i32, i32), TakenSkillEntry> = HashMap::new();
        for (player, skills) in self.data_storage.taken_on(&listed_ids, false, None) {
            let nick = resolve_nickname(player, &nickname_data, &summon_data);
            let uid = by_name.get(nick.as_str()).copied().unwrap_or(player);
            let e = taken.entry(uid).or_default();
            for (&code, d) in &skills {
                e.absorb(&d.stats);
                let entry = taken_skills.entry((uid, code)).or_insert_with(|| TakenSkillEntry {
                    actor_id: uid,
                    code,
                    name: self.skill_lookup.lookup_skill_name(code),
                    source_code: d.source_code,
                    stats: TakenStats::default(),
                });
                entry.stats.absorb(&d.stats);
                entry.source_code = entry.source_code.max(d.source_code);
            }
        }
        let mut taken_skills: Vec<TakenSkillEntry> = taken_skills.into_values().collect();
        taken_skills.sort_by_key(|e| (e.actor_id, e.code));
        let deaths = self.data_storage.deaths_on(&listed_ids, false, None);
        let deaths = death_entries(&deaths, |player| {
            let nick = resolve_nickname(player, &nickname_data, &summon_data);
            by_name.get(nick.as_str()).copied().unwrap_or(player)
        });

        // Healing over the listed fights, on the actor of the same name: each
        // target's own list holds the ticks of its span, so targets fought at
        // once would count a tick twice.
        let fights: Vec<&TargetCombatData> = listed.iter().map(|&(_, td)| td).collect();
        let mut heal_skills: HashMap<(i32, i32), DetailSkillEntry> = HashMap::new();
        for (actor_id, skills) in self.data_storage.fights_heals(&fights) {
            let owner = summon_resolver::resolve(actor_id, &summon_data);
            if owner <= 0 { continue; }
            let nick = resolve_nickname(owner, &nickname_data, &summon_data);
            let uid = by_name.get(nick.as_str()).copied().unwrap_or(owner);
            for ((code, is_hot), hd) in skills {
                self.add_heal(&mut heal_skills, uid, code, is_hot, &hd);
            }
        }
        let mut heal_skills: Vec<DetailSkillEntry> = heal_skills.into_values().collect();
        heal_skills.sort_by_key(|e| (e.actor_id, e.code, e.is_dot));

        // The fight's time as the meter counts it: in TRAIN your own time on
        // the dummies, as the meter does (Krao 2026-10-05 17:39:20: 01:22 on
        // the meter, 01:27 in Details, from a stranger's hits on one dummy).
        let battle_time = if train {
            let mine = self.resolve_local_ids(&summon_data).unwrap_or_default();
            active_time(fights.iter().filter_map(|td| own_span(td, &mine, &summon_data)), i64::MIN)
        } else {
            active_time(fights.iter().map(|td| (td.first_damage_time, td.last_damage_time)), i64::MIN)
        };

        let mut actors: Vec<DetailsActorSummary> = actor_meta.iter()
            .map(|(&id, (nick, job))| {
                let job_id = class_of(id).map_or(0, |j| j.class_prefix());
                // Aggregate per-actor stats
                let (mut party_heal, mut regen) = (0i64, 0i64);
                for td in &fights {
                    if let Some(ad) = td.actors.get(&id) {
                        party_heal += ad.party_heal;
                        regen += ad.regen;
                    }
                }
                let received = taken.get(&id).copied().unwrap_or_default();
                DetailsActorSummary {
                    actor_id: id,
                    nickname: nick.clone(),
                    job: job.clone(),
                    job_id,
                    party_heal,
                    regen,
                    damage_received: received.damage,
                    hits_received: received.total(),
                    // The live view is never uploaded: nothing here reads the
                    // identity a saved record keeps.
                    dbid: 0,
                    server_id: 0,
                    // Same reason again: the live rows already show combat
                    // power from the roster; the saved record is where it
                    // has to persist.
                    level: 0,
                    gear_score: 0,
                    combat_power: 0,
                }
            })
            .collect();
        actors.sort_by_key(|a| a.actor_id);

        let numbers = self.player_numbers(actors.iter().map(|a| (a.actor_id, a.nickname.as_str())));
        DetailsContext {
            // From storage, which the meter keeps in step: a details reader
            // has no target of its own.
            current_target_id: self.data_storage.current_target(),
            targets,
            actors,
            numbers,
            taken_skills,
            heal_skills,
            deaths: Some(deaths),
            battle_time,
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
            return self.target_details(*one, actor_ids, summary_only);
        }
        let combat_data = self.target_snapshots(&self.displayed_targets, summary_only);
        let merged = TargetCombatData::merged(self.displayed_targets.iter().filter_map(|t| combat_data.get(t)));
        let Some(merged) = merged else { return self.target_details(0, actor_ids, summary_only) };
        let heals = if summary_only { HashMap::new() } else { self.fight_heals(&merged) };
        let encounter = self.target_selection_mode == TargetSelectionMode::Encounter;
        let taken = if summary_only { TakenBy::new() } else { self.data_storage.taken_on(&self.displayed_targets, encounter, None) };
        let deaths = (!summary_only).then(|| self.data_storage.deaths_on(&self.displayed_targets, encounter, None));
        let mut details = self.details_for(&merged, 0, &heals, &taken, deaths.as_ref(), actor_ids, None);
        if summary_only {
            details.ping_history.clear();
        }
        details.battle_time = self.displayed_battle_time;
        details
    }

    pub fn get_target_details(&self, target_id: i32, actor_ids: Option<&[i32]>) -> TargetDetailsResponse {
        self.target_details(target_id, actor_ids, false)
    }

    /// `get_target_details` for the hover tooltip: the skills only, without
    /// hit timelines, healing or ping.
    pub fn get_hover_details(&self, target_id: i32, actor_ids: Option<&[i32]>) -> TargetDetailsResponse {
        self.target_details(target_id, actor_ids, true)
    }

    /// `get_target_details` for a saved fight.
    pub(super) fn fight_details(&self, target_id: i32) -> TargetDetailsResponse {
        self.target_details(target_id, None, false)
    }

    /// The healing done during a fight, live or saved: from its first hit to
    /// its last, by the people in it, so a fight shows the same healing before
    /// and after saving.
    fn fight_heals(&self, fight: &TargetCombatData) -> HashMap<i32, HashMap<(i32, bool), HealSkillData>> {
        self.data_storage.fight_heals(fight)
    }

    fn target_details(&self, target_id: i32, actor_ids: Option<&[i32]>, summary_only: bool) -> TargetDetailsResponse {
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
                taken_skills: Vec::new(),
                deaths: None,
            },
        };
        let max_hp = self.data_storage.get_mob_hp(target_id).unwrap_or(0);
        if summary_only {
            let mut details = self.details_for(target_data, max_hp, &HashMap::new(), &TakenBy::new(), None, actor_ids, None);
            details.ping_history.clear();
            return details;
        }
        let heals = self.fight_heals(target_data);
        let taken = self.data_storage.fight_taken(target_data);
        let deaths = self.data_storage.fight_deaths(target_data);
        self.details_for(target_data, max_hp, &heals, &taken, Some(&deaths), actor_ids, None)
    }

    /// Adds one healer's ticks of one skill to a heal list, as Details lists
    /// healing: `dmg` = heal amount, `time` = tick count, `is_dot` = HoT.
    fn add_heal(&self, heals: &mut HashMap<(i32, i32), DetailSkillEntry>, uid: i32, skill_code: i32, is_hot: bool, hd: &HealSkillData) {
        let entry = heals.entry((uid, skill_code + if is_hot { 1_000_000_000 } else { 0 })).or_insert_with(|| {
            let mut name = self.skill_lookup.lookup_skill_name(skill_code);
            if is_hot && !name.is_empty() {
                name = format!("{} - HoT", name);
            }
            let job = JobClass::convert_from_skill(skill_code)
                .filter(|_| uid != UNATTRIBUTED_ID)
                .map(|j| j.class_name().to_string())
                .unwrap_or_default();
            DetailSkillEntry {
                actor_id: uid,
                code: skill_code,
                name,
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
            }
        });
        entry.dmg = entry.dmg.saturating_add(hd.total_heal);
        entry.time = entry.time.saturating_add(hd.tick_count);
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
        taken: &TakenBy,
        deaths: Option<&DeathsBy>,
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
                    .filter(|_| uid != UNATTRIBUTED_ID)
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
                    min_dmg: i64::MAX,
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

                entry.time = entry.time.saturating_add(skill_data.hit_count);
                entry.dmg = entry.dmg.saturating_add(skill_data.total_damage);
                entry.multi_hit_count = entry.multi_hit_count.saturating_add(skill_data.multi_hit_count);
                entry.multi_hit_damage = entry.multi_hit_damage.saturating_add(skill_data.multi_hit_damage);
                entry.multi_hit_hits = entry.multi_hit_hits.saturating_add(skill_data.multi_hit_hits);
                if skill_data.min_damage < entry.min_dmg { entry.min_dmg = skill_data.min_damage; }
                if skill_data.max_damage > entry.max_dmg { entry.max_dmg = skill_data.max_damage; }
                entry.crit = entry.crit.saturating_add(skill_data.crit_count);
                entry.back = entry.back.saturating_add(skill_data.back_count);
                entry.frontal = entry.frontal.saturating_add(skill_data.frontal_count);
                entry.shield_block = entry.shield_block.saturating_add(skill_data.shield_block_count);
                entry.parry = entry.parry.saturating_add(skill_data.parry_count);
                entry.perfect = entry.perfect.saturating_add(skill_data.perfect_count);
                entry.double = entry.double.saturating_add(skill_data.double_count);
                entry.iron_wall = entry.iron_wall.saturating_add(skill_data.iron_wall_count);
                entry.regeneration = entry.regeneration.saturating_add(skill_data.regeneration_count);
                entry.perfect_block = entry.perfect_block.saturating_add(skill_data.perfect_block_count);
                entry.miss = entry.miss.saturating_add(skill_data.miss_count);
                entry.resist = entry.resist.saturating_add(skill_data.resist_count);
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
            if entry.min_dmg == i64::MAX { entry.min_dmg = 0; }
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
                self.add_heal(&mut heal_map, uid, skill_code, is_hot, hd);
            }
        }

        // Damage taken this segment, per player and skill, keyed by the
        // canonical actor as the rows are.
        let mut taken_map: HashMap<(i32, i32), TakenSkillEntry> = HashMap::new();
        for (&player, skills) in taken {
            let nickname = resolve_nickname(player, &nickname_data, &summon_data);
            let uid = *canonical.get(&nickname).unwrap_or(&player);
            if let Some(ref filter) = filter_uids {
                if !filter.contains(&uid) { continue; }
            }
            for (&code, d) in skills {
                let entry = taken_map.entry((uid, code)).or_insert_with(|| TakenSkillEntry {
                    actor_id: uid,
                    code,
                    name: self.skill_lookup.lookup_skill_name(code),
                    source_code: d.source_code,
                    stats: TakenStats::default(),
                });
                entry.stats.absorb(&d.stats);
                // Two ids of one player: the same NPC either way, in any order.
                entry.source_code = entry.source_code.max(d.source_code);
            }
        }
        let mut taken_skills: Vec<TakenSkillEntry> = taken_map.into_values().collect();
        taken_skills.sort_by_key(|e| (e.actor_id, e.code));
        let deaths = deaths.map(|d| {
            death_entries(d, |player| {
                let nickname = resolve_nickname(player, &nickname_data, &summon_data);
                *canonical.get(&nickname).unwrap_or(&player)
            })
            .into_iter()
            .filter(|e| filter_uids.as_ref().is_none_or(|f| f.contains(&e.actor_id)))
            .collect()
        });

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
            taken_skills,
            deaths,
        }
    }
}

/// Deaths per player on the id `uid_of` gives each, the deaths of two ids of
/// one player added up, by id.
fn death_entries(deaths: &DeathsBy, uid_of: impl Fn(i32) -> i32) -> Vec<DeathEntry> {
    let mut by_uid: HashMap<i32, u32> = HashMap::new();
    for (&player, &n) in deaths {
        *by_uid.entry(uid_of(player)).or_default() += n;
    }
    let mut out: Vec<DeathEntry> = by_uid.into_iter().map(|(actor_id, deaths)| DeathEntry { actor_id, deaths }).collect();
    out.sort_by_key(|e| e.actor_id);
    out
}
