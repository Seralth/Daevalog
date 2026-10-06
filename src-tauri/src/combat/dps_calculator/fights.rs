//! Saved fights: which fights are kept, and the records built from them.

use std::collections::{HashMap, HashSet};

use crate::combat::data_storage::{SegmentIdentity, TargetCombatData, IDLE_RESET_MS, MIN_SAVED_FIGHT_MS};
use crate::entity::details_context::*;
use crate::entity::fight_record::FightRecord;
use crate::entity::job_class::JobClass;
use crate::i18n::lookup::NpcLookup;

use super::rows::resolve_nickname;
use super::DpsCalculator;

impl DpsCalculator {
    pub fn mark_all_targets_saved(&mut self) {
        let combat = self.data_storage.get_combat_snapshot_light();
        for (&tid, td) in &combat {
            self.saved_fights.insert((tid, td.first_damage_time), td.last_damage_time);
        }
    }

    /// Whether `record` is a fight that is over and saved for good: it cannot
    /// go on, so it is safe to upload.
    pub fn fight_finished(&self, record: &FightRecord) -> bool {
        self.saved_fights.get(&(record.target_id, record.start_time_ms))
            == Some(&(record.start_time_ms + record.duration_ms))
    }

    pub fn snapshot_boss_fights(&mut self) -> Vec<FightRecord> {
        self.snapshot_boss_fights_inner(false)
    }

    pub fn snapshot_boss_fights_force(&mut self) -> Vec<FightRecord> {
        self.snapshot_boss_fights_inner(true)
    }

    fn snapshot_boss_fights_inner(&mut self, force: bool) -> Vec<FightRecord> {
        let mob_data = self.data_storage.get_mob_data();
        let now_ms = crate::clock::now_ms();
        let mut records = Vec::new();

        // Fights already cleared out of the live data (an idle restart on the
        // same mob, a boss pull, a reset, a zone change): saved as they ended.
        for seg in self.data_storage.take_ended_segments() {
            let td = &seg.data;
            if !self.is_saved_fight_target(&mob_data, td) || self.saved_fights.get(&(td.target_id, td.first_damage_time)) == Some(&td.last_damage_time) {
                continue;
            }
            let details = self.details_for(td, seg.max_hp, &seg.heals, None, Some(&seg.identity));
            let stats = actor_stats([td].into_iter());
            records.push(self.build_record(td, details, &stats, &mob_data, Some(&seg.identity)));
            self.saved_fights.insert((td.target_id, td.first_damage_time), td.last_damage_time);
        }

        // Light snapshot: only used for target filtering + per-actor aggregate
        // stats here; the saved record's timestamps come from get_target_details.
        let combat_data = self.data_storage.get_combat_snapshot_light();
        let candidates: Vec<i32> = combat_data.iter()
            .filter(|(tid, td)| {
                self.saved_fights.get(&(**tid, td.first_damage_time)) != Some(&td.last_damage_time)
                    && self.is_saved_fight_target(&mob_data, td)
            })
            .map(|(&tid, _)| tid)
            .collect();

        if !candidates.is_empty() {
            tracing::trace!("snapshot_boss_fights: {} candidate targets", candidates.len());
        }
        for target_id in candidates {
            let Some(target_data) = combat_data.get(&target_id) else { continue };
            let battle_time = target_data.last_damage_time - target_data.first_damage_time;
            let idle_time = now_ms - target_data.last_damage_time;
            let is_ended = idle_time >= 10_000;
            let is_periodic = battle_time >= 15_000;
            if !force && !is_ended && !is_periodic {
                continue;
            }

            let details = self.fight_details(target_id);
            let stats = actor_stats([target_data].into_iter());
            records.push(self.build_record(target_data, details, &stats, &mob_data, None));
            // Saved for good only once the next hit would start a new fight:
            // a pause in a boss fight is not its end, and the record is
            // saved again (same id) while the fight goes on.
            if idle_time > IDLE_RESET_MS {
                self.saved_fights.insert((target_id, target_data.first_damage_time), target_data.last_damage_time);
            }
        }

        // Bounded: entries for fights over an hour ago are no use.
        if self.saved_fights.len() > 256 {
            self.saved_fights.retain(|_, &mut last| now_ms - last < 3_600_000);
        }
        records
    }

    /// A boss or a training dummy that you or your party fought, long enough
    /// to keep. A stranger's field boss nearby is not your fight, and was
    /// uploaded as one.
    fn is_saved_fight_target(&self, mob_data: &HashMap<i32, i32>, td: &TargetCombatData) -> bool {
        mob_data.get(&td.target_id).is_some_and(|&code| self.npc_lookup.is_boss(code) || self.npc_lookup.is_training_dummy(code))
            && td.ours
            && td.total_damage > 0
            && td.last_damage_time - td.first_damage_time >= MIN_SAVED_FIGHT_MS
    }

    fn build_record(
        &self,
        target_data: &TargetCombatData,
        details: TargetDetailsResponse,
        stats: &HashMap<i32, (i64, i64, i64, i32)>,
        mob_data: &HashMap<i32, i32>,
        identity: Option<&SegmentIdentity>,
    ) -> FightRecord {
        let target_id = target_data.target_id;
        let party_members = self.data_storage.get_party_members();
        let battle_time = (target_data.last_damage_time - target_data.first_damage_time).max(0);
        let (nickname_data, summon_data_snap, local_id, identity_dungeon) = match identity {
            Some(i) => (i.nicknames.clone(), i.summons.clone(), i.local_player_id, i.dungeon_id),
            None => (
                self.data_storage.get_nicknames(),
                self.data_storage.get_summon_data(),
                self.data_storage.local_player_id(),
                0,
            ),
        };
        // Where the fight happened, not where the player is now. An open-world
        // fight takes nothing from the end of its segment: the load into an
        // instance ends it, after that instance is known.
        let dungeon_id = if target_data.dungeon_id != 0 {
            target_data.dungeon_id
        } else if target_data.open_world {
            0
        } else {
            identity_dungeon
        };

        let mut record_actors: HashMap<i32, (String, String)> = HashMap::new();
        for skill in &details.skills {
            let uid = skill.actor_id;
            record_actors.entry(uid).or_insert_with(|| {
                let nick = resolve_nickname(uid, &nickname_data, &summon_data_snap);
                let job = if !skill.job.is_empty() { skill.job.clone() }
                    else { JobClass::convert_from_skill(skill.code).map(|j| j.class_name().to_string()).unwrap_or_default() };
                (nick, job)
            });
            let entry = record_actors.get_mut(&uid).unwrap();
            if entry.1.is_empty() && !skill.job.is_empty() {
                entry.1 = skill.job.clone();
            }
        }

        let local_id = local_id.unwrap_or(-1) as i32;
        let actors: Vec<DetailsActorSummary> = record_actors.iter()
            .map(|(&id, (nick, job))| {
                let display_nick = if id == local_id {
                    nick.clone()
                } else {
                    crate::entity::fight_record::obscure_nickname(nick)
                };
                let job_class = JobClass::convert_from_skill(
                    details.skills.iter()
                        .find(|s| s.actor_id == id && !s.job.is_empty())
                        .map(|s| s.code)
                        .unwrap_or(0)
                );
                let (party_heal, regen, dmg_recv, hits_recv) = stats.get(&id).copied().unwrap_or_default();
                // Joined on the unobscured nickname: the roster is keyed by
                // name, and `display_nick` above has already been masked for
                // everyone but the local player.
                let roster = party_members.get(nick.as_str());
                DetailsActorSummary {
                    actor_id: id,
                    nickname: display_nick,
                    job: job.clone(),
                    job_id: job_class.map(|j| j.class_prefix()).unwrap_or(0),
                    party_heal,
                    regen,
                    damage_received: dmg_recv,
                    hits_received: hits_recv,
                    dbid: roster.map(|m| m.dbid).unwrap_or(0),
                    server_id: roster.map(|m| m.server_id).unwrap_or(0),
                    level: roster.map(|m| m.level).unwrap_or(0),
                    gear_score: roster.map(|m| m.gear_score).unwrap_or(0),
                    combat_power: roster.map(|m| m.combat_power).unwrap_or(0),
                }
            })
            .collect();

        let mob_code = mob_data.get(&target_id).copied().unwrap_or(0);
        let boss_name = self.resolve_target_name(target_id);

        let job_ids: Vec<i32> = actors.iter()
            .filter(|a| a.job_id > 0)
            .map(|a| a.job_id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let jobs: Vec<String> = actors.iter()
            .filter(|a| !a.job.is_empty() && a.job != "Unknown")
            .map(|a| a.job.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        FightRecord {
            id: format!("auto_{}_{}", target_id, target_data.first_damage_time),
            boss_name,
            target_id,
            start_time_ms: target_data.first_damage_time,
            duration_ms: battle_time,
            total_damage: target_data.total_damage,
            jobs,
            job_ids,
            details,
            actors,
            is_train: self.npc_lookup.is_training_dummy(mob_code),
            app_version: crate::version::UPLOAD_COMPAT_VERSION.to_string(),
            mob_code,
            dungeon_id: fight_dungeon(&self.npc_lookup, mob_code, dungeon_id),
            server_id: self.data_storage.fight_server_id(),
        }
    }
}

/// Healing, regen and damage taken per actor over `targets`.
fn actor_stats<'a>(targets: impl Iterator<Item = &'a TargetCombatData>) -> HashMap<i32, (i64, i64, i64, i32)> {
    let mut stats: HashMap<i32, (i64, i64, i64, i32)> = HashMap::new();
    for td in targets {
        for (&id, ad) in &td.actors {
            let e = stats.entry(id).or_default();
            e.0 += ad.party_heal;
            e.1 += ad.regen;
            e.2 += ad.damage_received;
            e.3 += ad.hits_received;
        }
    }
    stats
}

/// The instance a fight on NPC `mob_code` was in, given the one the fight was
/// stamped with (`stamped`, 0 for none). The NPC table names the instance of
/// many bosses, and that wins: it also places a boss fought after a teleport
/// inside an instance, before the roster names it. Any other target keeps the
/// stamped id (from taengu/A2Tools-DPS-Meter 19b98ba and c6e08a9).
pub(super) fn fight_dungeon(npcs: &NpcLookup, mob_code: i32, stamped: i32) -> i32 {
    npcs.dungeon_of(mob_code).unwrap_or(stamped)
}
