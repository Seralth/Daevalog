//! Saved fights: which fights are kept, and the records built from them.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::clock::now_ms;
use crate::combat::data_storage::{TargetTotals, QUIET_LIMIT_MS, MIN_SAVED_FIGHT_MS, UNATTRIBUTED_ID};
use crate::entity::detail_bundle::*;
use crate::entity::fight_record::{mask_name, FightRecord};
use crate::entity::job_class::JobClass;
use crate::i18n::lookup::NpcLookup;
use crate::version::UPLOAD_COMPAT_VERSION;

use super::details::{actor_summary, Extras, FightParts, Names};
use super::DpsCalculator;

/// A live fight quiet this long is saved, though it may go on.
const QUIET_SAVE_MS: i64 = 10_000;
/// A live fight this long is saved while it goes on.
const LONG_FIGHT_MS: i64 = 15_000;
/// Past this many fights saved for good, the old ones are forgotten.
const FOR_GOOD_LIMIT: usize = 256;
/// How old a fight's last hit must be to be forgotten.
const FORGET_AFTER_MS: i64 = 3_600_000;

/// The dungeon a fight is filed under: the instance the NPC table gives
/// the mob, else the one stamped on the fight's hits, else none in the open
/// world, else `fallback` (a cleared segment's own, 0 for a live fight).
pub(super) fn filed_dungeon(npcs: &NpcLookup, mob_code: i32, stamped: i32, open_world: bool, fallback: i32) -> i32 {
    match npcs.dungeon_of(mob_code) {
        Some(dungeon) => dungeon,
        None if stamped != 0 => stamped,
        None if open_world => 0,
        None => fallback,
    }
}

/// Saved fights: boss and training dummy fights, written once they end
/// and while they go on.
impl DpsCalculator {
    /// The fights due a save now: those the storage cleared, then the live
    /// ones quiet for a while or long enough.
    pub fn close_boss_fights(&mut self) -> Vec<FightRecord> {
        self.due_fights(false)
    }

    /// True once `record`'s fight is saved for good with the last hit it has.
    pub fn fight_finished(&self, record: &FightRecord) -> bool {
        let key = (record.target_id, record.start_time_ms);
        self.for_good.get(&key).is_some_and(|&last| last == record.start_time_ms + record.duration_ms)
    }

    /// Every live fight worth a save, at once (on quitting, or the last
    /// save of a replay).
    pub fn close_boss_fights_now(&mut self) -> Vec<FightRecord> {
        self.due_fights(true)
    }

    /// A boss or training dummy you or your party fought for long enough.
    fn worth_saving(&self, fight: &TargetTotals) -> bool {
        let Some(code) = self.data_storage.enemy_code(fight.target_id) else {
            return false;
        };
        let kind = self.npc_lookup.is_boss(code) || self.npc_lookup.npc_is_dummy(code);
        kind && fight.ours && fight.total_damage > 0 && fight.last_damage_time - fight.first_damage_time >= MIN_SAVED_FIGHT_MS
    }

    /// True when `fight` is saved for good as it stands: no hit since.
    fn saved_for_good(&self, fight: &TargetTotals) -> bool {
        self.for_good.get(&(fight.target_id, fight.first_damage_time)) == Some(&fight.last_damage_time)
    }

    /// Book `fight` as saved for good with its last hit.
    fn save_for_good(&mut self, fight: &TargetTotals) {
        self.for_good.insert((fight.target_id, fight.first_damage_time), fight.last_damage_time);
    }

    /// Saving reads the live storage, also while boss mode holds a fight.
    fn due_fights(&mut self, forced: bool) -> Vec<FightRecord> {
        let screen = std::mem::replace(&mut self.data_storage, self.live.clone());
        let due = self.live_due_fights(forced);
        self.data_storage = screen;
        due
    }

    fn live_due_fights(&mut self, forced: bool) -> Vec<FightRecord> {
        let now = now_ms();
        let mut cleared = Vec::new();
        for segment in self.data_storage.take_ended_segments() {
            let fight = &segment.data;
            if !self.worth_saving(fight) || self.saved_for_good(fight) {
                continue;
            }
            let names = Names::of_segment(&segment.identity);
            let extras = Some(Extras { heals: &segment.heals, taken: &segment.taken, deaths: &segment.deaths });
            let details = self.answer_from(&FightParts { fight, max_hp: segment.max_hp, extras }, &names, None);
            cleared.push(self.record_of(fight, details, &names, segment.identity.dungeon_id));
            self.save_for_good(fight);
        }
        cleared.sort_by_key(|r| r.target_id);

        let names = self.live_names();
        let mut live = Vec::new();
        for fight in self.data_storage.copy_targets_brief().into_values() {
            if !self.worth_saving(&fight) || self.saved_for_good(&fight) {
                continue;
            }
            let quiet = now - fight.last_damage_time;
            let length = fight.last_damage_time - fight.first_damage_time;
            if !forced && quiet < QUIET_SAVE_MS && length < LONG_FIGHT_MS {
                continue;
            }
            let details = self.target_breakdown(fight.target_id, None);
            live.push(self.record_of(&fight, details, &names, 0));
            // A pause in a boss fight is not its end: saved again later.
            if quiet > QUIET_LIMIT_MS {
                self.save_for_good(&fight);
            }
        }
        live.sort_by_key(|r| r.target_id);

        if self.for_good.len() > FOR_GOOD_LIMIT {
            self.for_good.retain(|_, &mut last| now - last < FORGET_AFTER_MS);
        }
        cleared.extend(live);
        cleared
    }

    /// The saved fight of `fight` with its answer, players named as in
    /// `names`; `fallback_dungeon` as `filed_dungeon` takes it.
    fn record_of(&self, fight: &TargetTotals, details: TargetBreakdownReply, names: &Names, fallback_dungeon: i32) -> FightRecord {
        let target = fight.target_id;
        let mob_code = self.data_storage.enemy_code(target).unwrap_or(0);
        let you = names.you.unwrap_or(-1);
        let roster = self.data_storage.roster_entries();
        let heal_and_regen = actor_stats(std::iter::once(fight));
        let players: BTreeSet<i32> = details.skills.iter().map(|s| s.actor_id).filter(|&id| id != UNATTRIBUTED_ID).collect();

        let actors: Vec<ActorBreakdown> = players
            .iter()
            .map(|&id| {
                let name = names.name(id);
                let class = JobClass::by_hits(details.skills.iter().filter(|s| s.actor_id == id).map(|s| (s.code, s.time)));
                let stats = heal_and_regen.get(&id).copied().unwrap_or_default();
                let nickname = if id == you { name.clone() } else { mask_name(&name) };
                let summary = actor_summary(id, nickname, class, stats, &details.taken_skills);
                match roster.get(&name) {
                    Some(member) => ActorBreakdown {
                        dbid: member.dbid,
                        server_id: member.server_id,
                        level: member.level,
                        gear_score: member.gear_score,
                        combat_power: member.combat_power,
                        ..summary
                    },
                    None => summary,
                }
            })
            .collect();
        let jobs: BTreeSet<&str> = actors.iter().map(|a| a.job.as_str()).filter(|j| !j.is_empty() && *j != "Unknown").collect();
        let job_ids: BTreeSet<i32> = actors.iter().map(|a| a.job_id).filter(|&j| j > 0).collect();

        let player_ids: HashSet<i32> = players.iter().copied().collect();
        FightRecord {
            id: format!("auto_{}_{}", target, fight.first_damage_time),
            boss_name: self.target_label(target),
            target_id: target,
            start_time_ms: fight.first_damage_time,
            duration_ms: (fight.last_damage_time - fight.first_damage_time).max(0),
            total_damage: fight.total_damage,
            jobs: jobs.into_iter().map(str::to_string).collect(),
            job_ids: job_ids.into_iter().collect(),
            timeline: self.saved_timeline(fight, &player_ids, &names.links, you),
            details,
            actors,
            is_train: self.npc_lookup.npc_is_dummy(mob_code),
            app_version: UPLOAD_COMPAT_VERSION.to_string(),
            mob_code,
            dungeon_id: filed_dungeon(&self.npc_lookup, mob_code, fight.dungeon_id, fight.open_world, fallback_dungeon),
            server_id: self.data_storage.home_server(),
        }
    }
}

/// Healing and regen per actor over `targets`.
pub(super) fn actor_stats<'a>(targets: impl Iterator<Item = &'a TargetTotals>) -> HashMap<i32, (i64, i64)> {
    let mut stats: HashMap<i32, (i64, i64)> = HashMap::new();
    for td in targets {
        for (&id, ad) in &td.actors {
            let e = stats.entry(id).or_default();
            e.0 += ad.party_heal;
            e.1 += ad.regen;
        }
    }
    stats
}
