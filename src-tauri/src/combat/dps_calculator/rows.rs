//! What every row set shares: names, canonical ids, cached classes, and party placeholders.

use std::collections::{HashMap, HashSet};

use crate::combat::data_storage::UNATTRIBUTED_ID;
use crate::entity::dps_data::DpsData;
use crate::entity::personal_data::PersonalData;
use crate::entity::summon_resolver;

use super::DpsCalculator;

/// Synthetic row ids for party members whose entity id we do not know yet. Sits
/// above the entity-id range (real ids top out at 9,999,999) so it can never
/// collide, and stays positive because the frontend discards non-positive ids.
pub const PARTY_ROW_ID_BASE: i32 = 90_000_000;

impl DpsCalculator {
    /// Give every party member a row as soon as they join, damage or not, so the
    /// meter shows the group you are actually in rather than only whoever has
    /// swung. The roster is keyed by character name — its `dbid` is an account
    /// id, unrelated to the session entity ids used everywhere else — so bind to
    /// the entity id when it is known and otherwise use a synthetic key placed
    /// above the entity-id range (ids top out at 9,999,999), so it cannot collide
    /// with a real one. The placeholder disappears on its own once real damage
    /// arrives under the player's true id. Not a negative key: the frontend drops
    /// non-positive ids as junk.
    /// Everything that has to happen to a row set before it goes on screen,
    /// in one place so a new return path cannot quietly skip half of it.
    pub(super) fn finalize_rows(&self, dps_data: &mut DpsData) {
        self.add_party_rows(dps_data);
        let numbers = self.player_numbers(dps_data.map.iter().map(|(&id, d)| (id, d.nickname.as_str())));
        for (id, data) in dps_data.map.iter_mut() {
            data.number = numbers.get(id).copied().unwrap_or(0);
        }
    }

    /// The numbers of the other players among `players`: not you, and not the
    /// unattributed row. New ones are numbered by id, for a fixed order.
    pub(super) fn player_numbers<'a>(&self, players: impl Iterator<Item = (i32, &'a str)>) -> HashMap<i32, u32> {
        let local_id = self.data_storage.local_player_id().map(|v| v as i32);
        let local_name = self.data_storage.local_character_name().filter(|n| !n.trim().is_empty());
        let mut others: Vec<(i32, &str)> = players
            .filter(|&(id, name)| {
                id != UNATTRIBUTED_ID && Some(id) != local_id && local_name.as_deref().map(str::trim) != Some(name.trim())
            })
            .collect();
        others.sort_unstable_by_key(|&(id, _)| id);
        self.data_storage.player_numbers(others)
    }

    fn add_party_rows(&self, dps_data: &mut DpsData) {
        if !self.data_storage.party_placeholders_wanted() {
            return;
        }
        let party_members = self.data_storage.get_party_members();
        if party_members.is_empty() {
            return;
        }
        let present: HashSet<String> = dps_data
            .map
            .values()
            .map(|d| d.nickname.trim().to_string())
            .collect();
        for (name, member) in &party_members {
            if present.contains(name.trim()) {
                continue;
            }
            let uid = self
                .data_storage
                .find_id_by_nickname(name)
                .filter(|id| !dps_data.map.contains_key(id))
                .unwrap_or(PARTY_ROW_ID_BASE + member.slot.min(64) as i32);
            let mut entry = PersonalData::new(name.clone());
            // The row filter drops anything without a job. These have not
            // attacked yet: their class from the roster, else from earlier
            // this session, else "Unknown", which draws no icon (issue #9).
            entry.job = member
                .job
                .map(|j| j.class_name().to_string())
                .or_else(|| self.cached_job(name))
                .unwrap_or_else(|| "Unknown".to_string());
            entry.combat_power = member.combat_power;
            dps_data.map.entry(uid).or_insert(entry);
        }
    }

    pub(super) fn cached_job(&self, nickname: &str) -> Option<String> {
        let key = nickname.trim().to_lowercase();
        if key.is_empty() || key.chars().all(|c| c.is_ascii_digit()) { return None; }
        self.nickname_job_cache.get(&key)
            .filter(|j| !j.is_empty() && *j != "Unknown")
            .cloned()
    }

    pub(super) fn cache_job(&mut self, nickname: &str, job: &str) {
        if job.is_empty() || job == "Unknown" { return; }
        let key = nickname.trim().to_lowercase();
        if key.is_empty() || key.chars().all(|c| c.is_ascii_digit()) { return; }
        self.nickname_job_cache.insert(key, job.to_string());
    }
}

pub(super) fn resolve_nickname(uid: i32, nicknames: &HashMap<i32, String>, summon_data: &HashMap<i32, i32>) -> String {
    if let Some(name) = nicknames.get(&uid) {
        return name.clone();
    }
    let resolved = summon_resolver::resolve(uid, summon_data);
    if let Some(name) = nicknames.get(&resolved) {
        return name.clone();
    }
    uid.to_string()
}

pub(super) fn build_nickname_canonical_map_from_aggregates(
    actor_damage: &HashMap<i32, i64>,
    summon_data: &HashMap<i32, i32>,
    nickname_data: &HashMap<i32, String>,
    local_player_id: Option<i32>,
) -> HashMap<String, i32> {
    let mut nickname_damage: HashMap<String, HashMap<i32, i64>> = HashMap::new();

    for (&actor_id, &damage) in actor_damage {
        let uid = summon_resolver::resolve(actor_id, summon_data);
        if uid <= 0 { continue; }
        let nickname = resolve_nickname(uid, nickname_data, summon_data);
        *nickname_damage.entry(nickname).or_default().entry(uid).or_insert(0) += damage;
    }

    let mut result = HashMap::new();
    for (nickname, id_damage) in &nickname_damage {
        // Pin the local player's row to their bound id so it doesn't oscillate
        // between co-existing self-ids as damage accumulates (which made the
        // frontend re-bind and thrash the meter).
        if let Some(lid) = local_player_id {
            if id_damage.contains_key(&lid) {
                result.insert(nickname.clone(), lid);
                continue;
            }
        }
        let direct_owner = id_damage.keys().find(|&&id| nickname_data.get(&id).is_some_and(|n| n == nickname));
        let canonical = direct_owner.copied()
            .or_else(|| id_damage.iter().max_by_key(|(_, d)| *d).map(|(id, _)| *id));
        if let Some(id) = canonical {
            result.insert(nickname.clone(), id);
        }
    }
    result
}
