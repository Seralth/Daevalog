//! What every row set shares: names, canonical ids, cached classes, and party placeholders.

use std::cmp::Reverse;
use std::collections::HashMap;

use crate::combat::data_storage::UNATTRIBUTED_ID;
use crate::entity::dps_data::DpsData;
use crate::entity::personal_data::PersonalData;
use crate::entity::owner_chain;

use super::DpsCalculator;

/// Row ids from here up stand for party members whose entity id the meter
/// has not seen. Entity ids stay under 10,000,000, so no player is ever
/// mistaken for one.
pub const GROUP_ROW_FIRST_ID: i32 = 90_000_000;

/// A party member's row before their entity id is known. Never an entity.
pub fn is_party_row_id(actor_id: i64) -> bool {
    actor_id >= GROUP_ROW_FIRST_ID as i64
}

/// Party slots past this one share its placeholder id.
const TOP_PLACEHOLDER_SLOT: i32 = 64;

/// Where a name's class sits in the cache: trimmed and lowercased. `None`
/// for a blank name or a bare id standing in for one.
fn class_cache_slot(name: &str) -> Option<String> {
    let slot = name.trim().to_lowercase();
    let bare_id = slot.bytes().all(|b| b.is_ascii_digit());
    (!slot.is_empty() && !bare_id).then_some(slot)
}

/// The class a row shows when none is known.
pub(super) const UNKNOWN_CLASS: &str = "Unknown";

/// `UNKNOWN_CLASS` and the empty class tell nothing, so they are never cached.
fn tells_class(class: &str) -> bool {
    !class.is_empty() && class != UNKNOWN_CLASS
}

impl DpsCalculator {
    /// The class first seen under a name this session. Lets a player keep
    /// a class on rows where they have not hit with a class skill.
    pub(super) fn known_class(&self, name: &str) -> Option<String> {
        let slot = class_cache_slot(name)?;
        self.nickname_job_cache.get(&slot).filter(|c| tells_class(c)).cloned()
    }

    /// Notes a name's class; a class already noted for it stays.
    pub(super) fn remember_class(&mut self, name: &str, class: &str) {
        if !tells_class(class) {
            return;
        }
        if let Some(slot) = class_cache_slot(name) {
            self.nickname_job_cache.entry(slot).or_insert_with(|| class.to_string());
        }
    }

    /// Each other player's number, from storage, which hands new players the
    /// next free ones in id order. You and the effects tied to no one get none.
    pub(super) fn player_numbers<'a>(&self, players: impl Iterator<Item = (i32, &'a str)>) -> HashMap<i32, u32> {
        let me = self.data_storage.local_player_id();
        let my_name = self.data_storage.local_character_name();
        let my_name = my_name.as_deref().map(str::trim).filter(|n| !n.is_empty());
        let mut others: Vec<(i32, &str)> = players
            .filter(|&(id, name)| id != UNATTRIBUTED_ID && Some(id as i64) != me && Some(name.trim()) != my_name)
            .collect();
        others.sort_by_key(|&(id, _)| id);
        self.data_storage.player_numbers(others)
    }

    /// The last touch on every reading the meter shows: the party members
    /// with no row yet, then every row's number.
    pub(super) fn complete_row_set(&self, reading: &mut DpsData) {
        self.add_idle_party(&mut reading.map);
        let numbers = self.player_numbers(reading.map.iter().map(|(&id, row)| (id, row.nickname.as_str())));
        for (id, row) in reading.map.iter_mut() {
            row.number = numbers.get(id).copied().unwrap_or(0);
        }
    }

    /// The party members with no row yet, numbered as `complete_row_set`
    /// numbers them; the rows already there keep their numbers.
    pub(super) fn add_idle_party_rows(&self, reading: &mut DpsData) {
        let added = self.add_idle_party(&mut reading.map);
        if added.is_empty() {
            return;
        }
        let numbers = self.player_numbers(reading.map.iter().map(|(&id, row)| (id, row.nickname.as_str())));
        for id in added {
            if let Some(row) = reading.map.get_mut(&id) {
                row.number = numbers.get(&id).copied().unwrap_or(0);
            }
        }
    }

    /// Party members who have dealt nothing, as rows of 0, while storage
    /// still wants them shown. Members go in name order, so two who would
    /// land on one id always resolve the same way: the first keeps it.
    /// Gives the ids it added.
    fn add_idle_party(&self, rows: &mut HashMap<i32, PersonalData>) -> Vec<i32> {
        let mut added = Vec::new();
        if !self.data_storage.ghost_rows_shown() {
            return added;
        }
        let mut roster: Vec<_> = self.data_storage.roster_entries().into_iter().collect();
        roster.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, member) in roster {
            let trimmed = name.trim();
            if rows.values().any(|r| r.nickname.trim() == trimmed) {
                continue;
            }
            let id = match self.data_storage.id_for_name(&name) {
                Some(seen) if !rows.contains_key(&seen) => seen,
                _ => GROUP_ROW_FIRST_ID + i32::from(member.slot).min(TOP_PLACEHOLDER_SLOT),
            };
            if rows.contains_key(&id) {
                continue;
            }
            let class = match member.job {
                Some(job) => job.class_name().to_string(),
                None => self.known_class(&name).unwrap_or_else(|| UNKNOWN_CLASS.to_string()),
            };
            let mut row = PersonalData::with_job(name, class);
            row.combat_power = member.combat_power;
            rows.insert(id, row);
            added.push(id);
        }
        added
    }
}

/// The name a row shows for `uid`: its own, else its owner's, else the id.
pub(super) fn display_name_for(uid: i32, nicknames: &HashMap<i32, String>, summon_data: &HashMap<i32, i32>) -> String {
    let named = nicknames.get(&uid).or_else(|| nicknames.get(&owner_chain::resolve(uid, summon_data)));
    named.map_or_else(|| format!("{uid}"), String::clone)
}

/// The owner with the most damage; on a tie the lowest id.
fn heaviest_owner(owners: impl Iterator<Item = (i32, i64)>) -> Option<i32> {
    owners.max_by_key(|&(id, damage)| (damage, Reverse(id))).map(|(id, _)| id)
}

/// One row id per name, since a player gets a new entity id on every zone
/// load and the old one may still hold damage. `actor_damage` is per raw
/// actor; summons count for their owners. Per name: you if you are among
/// its owners, else the owner the name belongs to directly with the most
/// damage, else the owner with the most damage; ties go to the lowest id.
pub(super) fn merge_name_aliases(
    actor_damage: &HashMap<i32, i64>,
    summon_data: &HashMap<i32, i32>,
    nickname_data: &HashMap<i32, String>,
    local_player_id: Option<i32>,
) -> HashMap<String, i32> {
    let mut per_owner: HashMap<i32, i64> = HashMap::new();
    for (&actor, &damage) in actor_damage {
        let owner = owner_chain::resolve(actor, summon_data);
        if owner > 0 {
            *per_owner.entry(owner).or_default() += damage;
        }
    }
    let mut per_name: HashMap<String, Vec<(i32, i64)>> = HashMap::new();
    for (owner, damage) in per_owner {
        per_name.entry(display_name_for(owner, nickname_data, summon_data)).or_default().push((owner, damage));
    }
    per_name
        .into_iter()
        .map(|(name, owners)| {
            let chosen = match local_player_id.filter(|me| owners.iter().any(|&(id, _)| id == *me)) {
                Some(me) => me,
                None => heaviest_owner(owners.iter().copied().filter(|(id, _)| nickname_data.get(id) == Some(&name)))
                    .or_else(|| heaviest_owner(owners.iter().copied()))
                    .unwrap_or(0),
            };
            (name, chosen)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::test_kit::{meter, storage_for};
    use super::*;
    use crate::clock::set_override;
    use crate::combat::data_storage::{DataStorage, RosterEntry};
    use crate::entity::job_class::JobClass;

    /// Every row the finish adds or keeps, as (id, name, class), by id.
    fn finished(calc: &DpsCalculator, start: Vec<(i32, &str, &str)>) -> (DpsData, Vec<(i32, String, String)>) {
        let mut reading = DpsData::new();
        for (id, name, class) in start {
            reading.map.insert(id, PersonalData::with_job(name.into(), class.into()));
        }
        calc.complete_row_set(&mut reading);
        let mut rows = Vec::new();
        for (id, row) in &reading.map {
            rows.push((*id, row.nickname.clone(), row.job.clone()));
        }
        rows.sort();
        (reading, rows)
    }

    fn named(rows: &[(i32, &str, &str)]) -> Vec<(i32, String, String)> {
        rows.iter().map(|&(id, n, c)| (id, n.to_string(), c.to_string())).collect()
    }

    fn table(pairs: &[(i32, &str)]) -> HashMap<i32, String> {
        pairs.iter().map(|&(id, name)| (id, name.to_string())).collect()
    }

    fn links(pairs: &[(i32, i32)]) -> HashMap<i32, i32> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn the_class_cache_ignores_case_blanks_ids_and_unknown() {
        let mut calc = meter(&storage_for(None));
        calc.remember_class(" Abc ", "X");
        assert_eq!(calc.known_class("abc").as_deref(), Some("X"));
        assert_eq!(calc.known_class("  ABC").as_deref(), Some("X"));
        calc.remember_class("12345", "X");
        assert_eq!(calc.known_class("12345"), None);
        assert!(!calc.nickname_job_cache.contains_key("12345"));
        calc.remember_class("A", "Unknown");
        calc.remember_class("B", "");
        assert_eq!((calc.known_class("a"), calc.known_class("b")), (None, None));
        calc.remember_class("abc", "Y");
        assert_eq!(calc.known_class("Abc").as_deref(), Some("X"), "the first class stays");
        calc.nickname_job_cache.insert("c".into(), "Unknown".into());
        assert_eq!(calc.known_class("c"), None);
        assert_eq!(calc.known_class("  "), None);
    }

    #[test]
    fn a_name_is_its_own_then_its_owners_then_the_id() {
        let names = table(&[(100, "A")]);
        let summons = links(&[(500, 100), (501, 500)]);
        assert_eq!(display_name_for(100, &names, &summons), "A");
        assert_eq!(display_name_for(500, &names, &summons), "A", "an unnamed summon takes its owner's");
        assert_eq!(display_name_for(501, &names, &summons), "A");
        assert_eq!(display_name_for(700, &names, &summons), "700");
        let named_spirit = table(&[(100, "A"), (500, "Spirit")]);
        assert_eq!(display_name_for(500, &named_spirit, &summons), "Spirit");
    }

    #[test]
    fn one_name_on_several_ids_gets_you_then_the_heaviest_then_the_lowest() {
        let names = table(&[(2259, "A"), (1000, "A"), (3000, "B"), (3100, "B")]);
        let summons = links(&[(5000, 2259)]);
        let damage: HashMap<i32, i64> = [(2259, 10), (1000, 900), (3000, 500), (3100, 900), (5000, 5), (4000, 7)].into_iter().collect();
        let map = merge_name_aliases(&damage, &summons, &names, Some(2259));
        assert_eq!(map["A"], 2259, "you, whatever the old id dealt");
        assert_eq!(map["B"], 3100);
        assert_eq!(map["4000"], 4000);
        assert_eq!(map.len(), 3, "the spirit is in its owner's group");
        // Equal damage: the lowest id, on every build.
        let even: HashMap<i32, i64> = [(3000, 500), (3100, 500)].into_iter().collect();
        for _ in 0..12 {
            assert_eq!(merge_name_aliases(&even, &summons, &names, None)["B"], 3000);
        }
        // Without you, the summed group decides: 1000 plus nothing beats 2259 plus its spirit.
        let map = merge_name_aliases(&damage, &summons, &names, None);
        assert_eq!(map["A"], 1000);
    }

    #[test]
    fn an_id_that_only_looks_like_a_name_loses_to_the_owner_of_the_name() {
        // 7000 has no name, so it shows as "7000"; 8000 is called "7000".
        let names = table(&[(8000, "7000")]);
        let damage: HashMap<i32, i64> = [(7000, 900), (8000, 100), (0, 50), (-4, 50)].into_iter().collect();
        let map = merge_name_aliases(&damage, &HashMap::new(), &names, None);
        assert_eq!(map.len(), 1, "ids of 0 or less are no one");
        assert_eq!(map["7000"], 8000);
    }

    #[test]
    fn numbers_skip_you_your_name_and_unattributed_and_go_by_id() {
        let s = storage_for(Some(100));
        s.set_local_character_name(Some("Me".into()));
        let players = [(300, "X"), (200, "Y"), (100, "Me"), (UNATTRIBUTED_ID, "80000000"), (400, " Me ")];
        let mut got: Vec<(i32, u32)> = meter(&s).player_numbers(players.into_iter()).into_iter().collect();
        got.sort();
        assert_eq!(got, [(200, 1), (300, 2)]);
    }

    /// A roster of (name, slot) members, sent at 1 s.
    fn party_of(members: &[(&str, u8)]) -> std::sync::Arc<DataStorage> {
        set_override(Some(1_000));
        let s = storage_for(None);
        let roster = members.iter().map(|&(name, slot)| (name.to_string(), RosterEntry { slot, ..Default::default() }));
        s.replace_roster(roster.collect(), true);
        s
    }

    #[test]
    fn party_members_with_no_row_wait_on_their_slot_or_known_id() {
        let s = party_of(&[("Second", 2), ("Far", 70), ("Known", 4), ("Fighting", 5)]);
        s.name_entity(3000, "Known");
        let healer = RosterEntry { slot: 3, job: Some(JobClass::Cleric), combat_power: 12_345, ..Default::default() };
        s.replace_roster(vec![("Healer".into(), healer)], false);
        let mut calc = meter(&s);
        calc.remember_class("Second", "Z");
        let (reading, rows) = finished(&calc, vec![(77, " Fighting ", "Q")]);
        let cleric = JobClass::Cleric.class_name();
        assert_eq!(
            rows,
            named(&[
                (77, " Fighting ", "Q"),
                (3000, "Known", "Unknown"),
                (90_000_002, "Second", "Z"),
                (90_000_003, "Healer", cleric),
                (90_000_064, "Far", "Unknown"),
            ])
        );
        assert_eq!((reading.map[&90_000_003].combat_power, reading.map[&3000].amount), (12_345, 0.0));
        set_override(None);
    }

    #[test]
    fn two_members_on_one_placeholder_id_go_to_the_first_by_name() {
        for _ in 0..8 {
            let s = party_of(&[("Beta", 64), ("Alpha", 70)]);
            let (_, rows) = finished(&meter(&s), Vec::new());
            assert_eq!(rows, named(&[(90_000_064, "Alpha", "Unknown")]));
        }
        set_override(None);
    }

    #[test]
    fn a_known_id_already_on_a_row_sends_the_member_to_a_placeholder() {
        let s = party_of(&[("Known", 2), ("Other", 3)]);
        s.name_entity(3000, "Known");
        let (_, rows) = finished(&meter(&s), vec![(3000, "Someone", "Q")]);
        assert_eq!(rows, named(&[(3000, "Someone", "Q"), (90_000_002, "Known", "Unknown"), (90_000_003, "Other", "Unknown")]));
        set_override(None);
    }
}
