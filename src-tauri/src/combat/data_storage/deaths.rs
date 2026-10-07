//! Deaths: your or a party member's HP falling from above 0 to 0, kept one by
//! one so each fight takes the deaths during it, as damage taken does.
//!
//! HP 0 is death: in the 2026-10-04 to 10-06 captures each of the 34 times
//! a player's HP reached 0 came with the game's death notice (`04 8D
//! <victim> <skill> <killer>`) or, for a party member out of sight, the
//! party record's last byte 0; no player acted later than 0.2 s after it
//! before a revive or a respawn (one skill's 60 s timer ending aside).
//!
//! The game sends your HP and your party's on every change, the party's at
//! any distance, so from the first reading on the meter sees every death of
//! the player: their HP is known until a zone load hands out new ids or
//! they leave the party.

use std::collections::{HashMap, VecDeque};

use super::{now_ms, DataStorage, Inner, TargetCombatData};

/// A death this long after a fight's last hit still counts for it: in a wipe
/// the last players fall after the last hit on the boss.
pub const DEATH_SLACK_MS: i64 = 10_000;
/// Deaths kept at most, a memory backstop only: deaths go when no open fight
/// needs them.
const MAX_DEATHS: usize = 4096;
/// Stretches of known HP kept per player, and players kept, at most.
const MAX_HP_SPANS: usize = 256;
const MAX_HP_PLAYERS: usize = 1024;
/// Entities whose last HP is kept, at most; the map starts over when full.
const MAX_HP_IDS: usize = 16_384;

/// One fall to 0 HP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeathTick {
    /// Counts up from 1, one per death. See `deaths_since`.
    pub seq: u64,
    pub at: i64,
    pub player: i32,
    /// The HP read before it.
    pub before: i64,
}

/// Deaths per player in a fight. A player is here only when the game sent
/// their HP during it; with no reading the count is not known.
pub type DeathsBy = HashMap<i32, u32>;

/// When the HP of each of you and your party was known: stretches from the
/// first reading to a zone load or leaving the party, `i64::MAX` while open.
pub type HpKnown = HashMap<i32, VecDeque<(i64, i64)>>;

impl DataStorage {
    /// A current HP reading from an `00 8D` record.
    pub fn note_hp(&self, id: i32, hp: i64) {
        self.note_hp_reading(id, hp, false);
    }

    /// A current HP reading from the party record (`1B 92`), which the game
    /// sends for party members only.
    pub fn note_party_hp(&self, id: i32, hp: i64) {
        self.note_hp_reading(id, hp, true);
    }

    fn note_hp_reading(&self, id: i32, hp: i64, party_record: bool) {
        let at = now_ms();
        let mut inner = self.inner.write();
        if party_record {
            inner.party_hp_ids.insert(id);
        }
        if inner.hp_now.len() >= MAX_HP_IDS && !inner.hp_now.contains_key(&id) {
            inner.hp_now.clear();
        }
        let before = inner.hp_now.insert(id, hp);
        if !counts_deaths(&inner, id) {
            return;
        }
        if inner.hp_known.len() >= MAX_HP_PLAYERS && !inner.hp_known.contains_key(&id) {
            let oldest = inner.hp_known.iter().min_by_key(|(_, s)| s.back().map_or(i64::MIN, |s| s.0)).map(|(&p, _)| p);
            if let Some(p) = oldest {
                inner.hp_known.remove(&p);
            }
        }
        let spans = inner.hp_known.entry(id).or_default();
        if spans.back().is_none_or(|s| s.1 != i64::MAX) {
            spans.push_back((at, i64::MAX));
            if spans.len() > MAX_HP_SPANS {
                spans.pop_front();
            }
        }
        if let Some(before) = before.filter(|&b| b > 0 && hp == 0) {
            inner.death_seq += 1;
            let seq = inner.death_seq;
            inner.deaths.push_back(DeathTick { seq, at, player: id, before });
            prune_deaths(&mut inner, at);
            drop(inner);
            self.touch();
        }
    }

    /// The deaths after the one numbered `seq`, oldest first.
    pub fn deaths_since(&self, seq: u64) -> Vec<DeathTick> {
        let inner = self.inner.read();
        let start = inner.deaths.partition_point(|t| t.seq <= seq);
        inner.deaths.range(start..).copied().collect()
    }

    /// When the HP of each of you and your party was known. See `HpKnown`.
    pub fn hp_known(&self) -> HpKnown {
        self.inner.read().hp_known.clone()
    }

    /// One fight's deaths, from its first hit to `DEATH_SLACK_MS` after its
    /// last.
    pub fn fight_deaths(&self, fight: &TargetCombatData) -> DeathsBy {
        deaths_in(&self.inner.read(), &[fight], i64::MIN)
    }

    /// The deaths over the live fights against `targets` (with what a boss
    /// pull cleared of the encounter when `encounter`), from `since` on.
    pub fn deaths_on(&self, targets: &[i32], encounter: bool, since: Option<i64>) -> DeathsBy {
        let inner = self.inner.read();
        let carry = targets.iter().filter_map(|t| inner.encounter_carry.get(t)).filter(|_| encounter);
        let fights: Vec<&TargetCombatData> = targets.iter().filter_map(|t| inner.target_combat.get(t)).chain(carry).collect();
        deaths_in(&inner, &fights, since.unwrap_or(i64::MIN))
    }

    /// You and the party members the roster names that the meter has an id
    /// for.
    pub fn party_ids(&self) -> Vec<i32> {
        let inner = self.inner.read();
        let mut ids: Vec<i32> = inner
            .nickname_storage
            .iter()
            .filter(|(_, n)| inner.party_members.contains_key(n.as_str()))
            .map(|(&id, _)| id)
            .chain(inner.local_player_id.map(|v| v as i32))
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }
}

/// The HP of the players `gone` picks is no longer known from `at` on.
pub(super) fn end_hp_known(inner: &mut Inner, at: i64, gone: impl Fn(&Inner, i32) -> bool) {
    let ids: Vec<i32> = inner.hp_known.keys().copied().filter(|&id| gone(inner, id)).collect();
    for id in ids {
        if let Some(last) = inner.hp_known.get_mut(&id).and_then(|s| s.back_mut()).filter(|s| s.1 == i64::MAX) {
            last.1 = at;
        }
    }
}

/// Whether `id`'s deaths count: you, or a member of your party.
fn counts_deaths(inner: &Inner, id: i32) -> bool {
    inner.local_player_id == Some(id as i64)
        || inner.party_hp_ids.contains(&id)
        || inner.nickname_storage.get(&id).is_some_and(|n| inner.party_members.contains_key(n.as_str()))
}

/// The deaths during `fights` (from the first one's first hit, not before
/// `since`, to `DEATH_SLACK_MS` after the last one's last hit).
pub(super) fn deaths_in(inner: &Inner, fights: &[&TargetCombatData], since: i64) -> DeathsBy {
    let Some(from) = fights.iter().map(|f| f.first_damage_time).min() else { return DeathsBy::new() };
    let until = fights.iter().map(|f| f.last_damage_time).max().unwrap_or(from).saturating_add(DEATH_SLACK_MS);
    deaths_between(inner.deaths.iter(), &inner.hp_known, from.max(since), until)
}

/// The deaths from `from` to `until` per player, every player whose HP was
/// known then counted, 0 included.
pub fn deaths_between<'a>(deaths: impl Iterator<Item = &'a DeathTick>, known: &HpKnown, from: i64, until: i64) -> DeathsBy {
    let mut out: DeathsBy = known
        .iter()
        .filter(|(_, spans)| spans.iter().any(|&(a, b)| a <= until && b >= from))
        .map(|(&p, _)| (p, 0))
        .collect();
    for d in deaths.filter(|d| (from..=until).contains(&d.at)) {
        *out.entry(d.player).or_default() += 1;
    }
    out
}

/// Drop the deaths from before every fight still open, as for damage taken.
fn prune_deaths(inner: &mut Inner, now: i64) {
    let oldest_fight = inner
        .target_combat
        .values()
        .chain(inner.encounter_carry.values())
        .map(|td| td.first_damage_time)
        .min()
        .unwrap_or(i64::MAX);
    let keep_from = oldest_fight.min(now.saturating_sub(super::taken::TAKEN_SLACK_MS));
    while inner.deaths.front().is_some_and(|d| d.at < keep_from) || inner.deaths.len() > MAX_DEATHS {
        inner.deaths.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::data_storage::PartyMember;
    use crate::entity::damage_packet::ParsedDamagePacket;

    fn at(t: i64) {
        crate::clock::set_override(Some(t));
    }

    fn hp(s: &DataStorage, t: i64, id: i32, value: i64) {
        at(t);
        s.note_hp(id, value);
    }

    fn hit(s: &DataStorage, actor: i32, target: i32, t: i64) {
        let mut p = ParsedDamagePacket::new();
        p.set_actor_id(actor);
        p.set_target_id(target);
        p.set_skill_code(11_010_000);
        p.set_damage(100);
        p.set_timestamp(t);
        at(t);
        s.append_damage(p);
    }

    fn party(s: &DataStorage, names: &[&str]) {
        let members = names.iter().map(|n| (n.to_string(), PartyMember::default())).collect();
        s.set_party_roster(members, true);
    }

    fn fight(s: &DataStorage, target: i32) -> DeathsBy {
        let td = s.inner.read().target_combat[&target].clone();
        s.fight_deaths(&td)
    }

    #[test]
    fn a_fight_counts_the_deaths_of_you_and_your_party_during_it() {
        let s = DataStorage::new();
        s.set_local_player_id(Some(1000));
        s.append_nickname_authoritative(2000, "Healer");
        s.append_nickname_authoritative(2500, "Quiet");
        s.append_nickname_authoritative(3000, "Stranger");
        party(&s, &["Me", "Healer", "Quiet", "Tank"]);
        s.append_nickname_authoritative(4000, "Tank");
        hp(&s, 500, 4000, 9000); // before the fight, never again
        hit(&s, 1000, 900, 1_000);
        hp(&s, 2_000, 1000, 500);
        hp(&s, 2_000, 2000, 700);
        hp(&s, 2_000, 3000, 300);
        hp(&s, 4_000, 1000, 0);
        hp(&s, 4_100, 1000, 0); // the same death
        hp(&s, 4_200, 3000, 0); // not your party
        hp(&s, 6_000, 1000, 800);
        hit(&s, 1000, 900, 9_000);
        hp(&s, 9_500, 2000, 0); // after the last hit, in the slack
        hp(&s, 30_000, 1000, 0); // long after
        let by = fight(&s, 900);
        let mut got: Vec<(i32, u32)> = by.into_iter().collect();
        got.sort();
        assert_eq!(got, vec![(1000, 1), (2000, 1), (4000, 0)], "Quiet sent no HP: not known");
        crate::clock::set_override(None);
    }

    #[test]
    fn hp_is_known_until_a_zone_load() {
        let s = DataStorage::new();
        s.set_local_player_id(Some(1000));
        hp(&s, 500, 1000, 9000);
        at(600);
        s.note_map_load(1010);
        hit(&s, 1000, 900, 1_000);
        hit(&s, 1000, 900, 8_000);
        assert!(fight(&s, 900).is_empty(), "the load handed out new ids");
        hp(&s, 3_000, 1000, 8000);
        assert_eq!(fight(&s, 900).get(&1000), Some(&0));
        crate::clock::set_override(None);
    }

    #[test]
    fn a_member_counts_only_while_in_the_party() {
        let s = DataStorage::new();
        s.set_local_player_id(Some(1000));
        s.append_nickname_authoritative(2000, "Joiner");
        party(&s, &["Me", "Other"]);
        hit(&s, 1000, 900, 1_000);
        hp(&s, 2_000, 2000, 500);
        hp(&s, 3_000, 2000, 0); // not in the party yet
        hp(&s, 4_000, 2000, 900);
        at(5_000);
        party(&s, &["Me", "Other", "Joiner"]);
        hp(&s, 6_000, 2000, 400);
        hp(&s, 7_000, 2000, 0);
        hp(&s, 8_000, 2000, 900);
        at(9_000);
        party(&s, &["Me", "Other"]);
        hp(&s, 10_000, 2000, 300);
        hp(&s, 11_000, 2000, 0); // left again
        hit(&s, 1000, 900, 12_000);
        assert_eq!(fight(&s, 900).get(&2000), Some(&1));
        let deaths: Vec<i64> = s.deaths_since(0).iter().map(|d| d.at).collect();
        assert_eq!(deaths, vec![7_000]);

        // A fight after they left: not theirs.
        hit(&s, 1000, 901, 20_000);
        hit(&s, 1000, 901, 26_000);
        let td = s.inner.read().target_combat[&901].clone();
        assert!(!s.fight_deaths(&td).contains_key(&2000));
        crate::clock::set_override(None);
    }

    #[test]
    fn a_fight_cleared_by_a_reset_keeps_its_deaths() {
        let s = DataStorage::new();
        s.set_local_player_id(Some(1000));
        s.append_mob(900, 1);
        s.register_boss(900);
        hit(&s, 1000, 900, 1_000);
        hp(&s, 2_000, 1000, 500);
        hp(&s, 3_000, 1000, 0);
        hit(&s, 1000, 900, 7_000);
        s.flush();
        assert!(s.deaths_since(0).is_empty());
        let ended = s.take_ended_segments();
        assert_eq!(ended[0].deaths.get(&1000), Some(&1));
        crate::clock::set_override(None);
    }
}
