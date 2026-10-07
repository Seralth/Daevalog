//! Healing done, per healer and skill.

use std::collections::{HashMap, HashSet};

use super::{DataStorage, HealSkillData, HealTick, Inner, TargetCombatData};
use crate::entity::summon_resolver;

/// Heal ticks kept at most, a memory backstop only: ticks go when no open
/// fight needs them (`prune_heal_ticks`).
const MAX_HEAL_TICKS: usize = 1_000_000;
/// How often, in ticks, the ones no fight needs are dropped.
const PRUNE_EVERY: usize = 4096;
/// Ticks this recent are kept even with no fight open: a fight's first hit
/// can arrive after healing that landed during it.
const HEAL_SLACK_MS: i64 = 60_000;

impl DataStorage {
    /// Record a heal tick done by `actor_id` with `skill_code` (is_hot marks a HoT).
    /// Keyed by the healer so "healing done" can be shown per player. Self-heals count.
    pub fn append_heal(&self, actor_id: i32, skill_code: i32, amount: i64, is_hot: bool, at: i64) {
        if amount <= 0 || !self.is_plausible_entity_id(actor_id) {
            return;
        }
        record_heal(&mut self.inner.write(), HealTick { at, actor: actor_id, skill: skill_code, is_hot, amount });
    }

    /// Healing done from `from_ms` to `to_ms`, by anyone.
    pub fn heals_between(&self, from_ms: i64, to_ms: i64) -> HashMap<i32, HashMap<(i32, bool), HealSkillData>> {
        heals_between(&self.inner.read(), from_ms, to_ms)
    }

    /// One fight's healing: see `fights_heals`.
    pub fn fight_heals(&self, fight: &TargetCombatData) -> HashMap<i32, HashMap<(i32, bool), HealSkillData>> {
        fights_heals(&self.inner.read(), &[fight])
    }

    /// The healing of several fights at once, each tick once: see `fights_heals`.
    pub fn fights_heals(&self, fights: &[&TargetCombatData]) -> HashMap<i32, HashMap<(i32, bool), HealSkillData>> {
        fights_heals(&self.inner.read(), fights)
    }
}

/// The healing done during fights by the people in them: you, your party, and
/// whoever hit one of the fights' targets, their summons included. Players
/// nearby who only healed are someone else's fight (strangers at the next
/// training dummy filled a solo dummy fight's HEAL, 2026-10-05). Several
/// fights take the ticks while any of them was being fought, each tick once,
/// the gaps between pulls left out as the fight time leaves them out.
pub(super) fn fights_heals(inner: &Inner, fights: &[&TargetCombatData]) -> HashMap<i32, HashMap<(i32, bool), HealSkillData>> {
    let fighters: HashSet<i32> = fights.iter()
        .flat_map(|f| f.actors.keys())
        .map(|&a| summon_resolver::resolve(a, &inner.summon_storage))
        .collect();
    let spans = merged_spans(fights.iter().map(|f| (f.first_damage_time, f.last_damage_time)));
    let mut out = heals_within(inner, &spans);
    out.retain(|&actor, _| {
        let owner = summon_resolver::resolve(actor, &inner.summon_storage);
        fighters.contains(&owner)
            || inner.local_player_id.is_some_and(|l| l as i32 == owner)
            || inner.nickname_storage.get(&owner).is_some_and(|n| inner.party_members.contains_key(n.as_str()))
    });
    out
}

pub(super) fn record_heal(inner: &mut Inner, tick: HealTick) {
    inner.heal_ticks.push_back(tick);
    if inner.heal_ticks.len() % PRUNE_EVERY == 0 {
        prune_heal_ticks(inner);
    }
    if inner.heal_ticks.len() > MAX_HEAL_TICKS {
        inner.heal_ticks.pop_front();
    }
}

/// Drop the ticks from before every fight still open: a fight takes only the
/// healing done during it (`heals_between`), and ended fights took theirs
/// when they were retired.
fn prune_heal_ticks(inner: &mut Inner) {
    let Some(newest) = inner.heal_ticks.back().map(|t| t.at) else { return };
    let oldest_fight = inner.target_combat.values()
        .chain(inner.encounter_carry.values())
        .map(|td| td.first_damage_time)
        .min()
        .unwrap_or(i64::MAX);
    let keep_from = oldest_fight.min(newest.saturating_sub(HEAL_SLACK_MS));
    while inner.heal_ticks.front().is_some_and(|t| t.at < keep_from) {
        inner.heal_ticks.pop_front();
    }
}

pub(super) fn heals_between(inner: &Inner, from_ms: i64, to_ms: i64) -> HashMap<i32, HashMap<(i32, bool), HealSkillData>> {
    heals_within(inner, &[(from_ms, to_ms)])
}

/// Spans that overlap or touch joined into one, in time order.
fn merged_spans(spans: impl Iterator<Item = (i64, i64)>) -> Vec<(i64, i64)> {
    let mut spans: Vec<(i64, i64)> = spans.collect();
    spans.sort_unstable();
    let mut out: Vec<(i64, i64)> = Vec::new();
    for (first, last) in spans {
        match out.last_mut() {
            Some((_, end)) if first <= *end => *end = (*end).max(last),
            _ => out.push((first, last)),
        }
    }
    out
}

/// Healing done inside any of these spans (each from..=to), by anyone.
fn heals_within(inner: &Inner, spans: &[(i64, i64)]) -> HashMap<i32, HashMap<(i32, bool), HealSkillData>> {
    let mut out: HashMap<i32, HashMap<(i32, bool), HealSkillData>> = HashMap::new();
    let inside = |at: i64| spans.iter().any(|&(from, to)| (from..=to).contains(&at));
    for t in inner.heal_ticks.iter().filter(|t| inside(t.at) && !is_mob(inner, t.actor)) {
        let e = out.entry(t.actor).or_default().entry((t.skill, t.is_hot)).or_default();
        e.total_heal += t.amount;
        e.tick_count = e.tick_count.saturating_add(1);
    }
    out
}

/// A mob is no healer. Protection Circle HoT ticks on a player carry a mob
/// in the healer field (the boss, 2026-10-05), and saved boss fights listed
/// the boss as a healer. A summon spawns like a mob, so a linked one is kept;
/// so is an id the game named as a player since.
fn is_mob(inner: &Inner, id: i32) -> bool {
    inner.mob_storage.contains_key(&id)
        && !inner.summon_storage.contains_key(&id)
        && !inner.known_player_ids.contains(&id)
        && !inner.nickname_storage.contains_key(&id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::damage_packet::ParsedDamagePacket;

    fn hit(s: &DataStorage, at: i64) {
        let mut p = ParsedDamagePacket::new();
        p.set_actor_id(1000);
        p.set_target_id(900);
        p.set_skill_code(11_010_000);
        p.set_damage(100);
        p.set_timestamp(at);
        s.append_damage(p);
    }

    #[test]
    fn a_long_fight_keeps_the_healing_from_its_start() {
        let s = DataStorage::new();
        hit(&s, 0);
        for at in 0..150_000 {
            s.append_heal(2000, 17_010_000, 10, false, at);
            if at % 20_000 == 0 {
                hit(&s, at);
            }
        }
        let heals = s.heals_between(0, 150_000);
        assert_eq!(heals[&2000][&(17_010_000, false)].tick_count, 150_000);
    }

    #[test]
    fn only_players_and_their_summons_heal() {
        let s = DataStorage::new();
        hit(&s, 0);
        s.append_mob(22809, 2310171);
        s.append_mob(500, 1);
        s.append_summon(14409, 500);
        s.append_nickname_authoritative(14274, "Templar");
        for (actor, skill) in [(22809, 18_730_003), (500, 16_770_000), (14274, 18_730_003), (14409, 2_011_101)] {
            s.append_heal(actor, skill, 100, true, 1_000);
        }
        let mut healers: Vec<i32> = s.heals_between(0, 2_000).into_keys().collect();
        healers.sort();
        assert_eq!(healers, vec![500, 14274, 14409], "the boss is not one");
    }

    #[test]
    fn healing_from_before_every_open_fight_is_dropped() {
        let s = DataStorage::new();
        for at in 0..PRUNE_EVERY as i64 {
            s.append_heal(2000, 17_010_000, 10, false, at);
        }
        assert_eq!(s.inner.read().heal_ticks.len(), PRUNE_EVERY, "within the slack");
        hit(&s, 200_000);
        for at in 200_000..200_000 + PRUNE_EVERY as i64 {
            s.append_heal(2000, 17_010_000, 10, false, at);
        }
        let inner = s.inner.read();
        assert_eq!(inner.heal_ticks.len(), PRUNE_EVERY);
        assert_eq!(inner.heal_ticks.front().map(|t| t.at), Some(200_000));
    }

    #[test]
    fn fights_at_once_take_each_tick_once_and_leave_out_the_gaps() {
        let s = DataStorage::new();
        let on = |target: i32, at: i64| {
            let mut p = ParsedDamagePacket::new();
            p.set_actor_id(1000);
            p.set_target_id(target);
            p.set_skill_code(11_010_000);
            p.set_damage(100);
            p.set_timestamp(at);
            s.append_damage(p);
        };
        on(900, 0);
        on(900, 10_000); // a boss, 0 to 10 s
        on(901, 4_000);
        on(901, 6_000); // its add, 4 to 6 s
        on(902, 30_000);
        on(902, 35_000); // the next pull, 30 to 35 s
        for at in [2_000, 5_000, 20_000, 32_000] {
            s.append_heal(1000, 17_010_000, 100, false, at);
        }
        let inner = s.inner.read();
        let fights: Vec<&TargetCombatData> = [900, 901, 902].iter().map(|t| &inner.target_combat[t]).collect();
        let each: i32 = fights.iter().map(|f| fights_heals(&inner, &[f])[&1000][&(17_010_000, false)].tick_count).sum();
        assert_eq!(each, 4, "the tick at 5 s is in the boss's span and the add's");
        let all = &fights_heals(&inner, &fights)[&1000][&(17_010_000, false)];
        assert_eq!((all.tick_count, all.total_heal), (3, 300), "once each; the tick at 20 s fell between pulls");
    }

    #[test]
    fn a_fight_takes_the_healing_of_the_people_in_it_only() {
        let s = DataStorage::new();
        s.set_local_player_id(Some(1000));
        hit(&s, 0); // you, on target 900
        let mut p = ParsedDamagePacket::new();
        p.set_actor_id(5000);
        p.set_target_id(900);
        p.set_skill_code(11_010_000);
        p.set_damage(100);
        p.set_timestamp(500);
        s.append_damage(p); // a stranger who joined the fight
        hit(&s, 2_000);
        s.append_summon(1000, 3000); // your spirit
        s.append_nickname_authoritative(4000, "Cleric");
        s.set_party_roster(vec![("Cleric".into(), super::super::PartyMember::default())], true);
        for actor in [1000, 3000, 4000, 5000, 6000] {
            s.append_heal(actor, 17_010_000, 100, false, 1_000);
        }
        let fight = s.inner.read().target_combat[&900].clone();
        let mut healers: Vec<i32> = s.fight_heals(&fight).into_keys().collect();
        healers.sort();
        assert_eq!(healers, vec![1000, 3000, 4000, 5000], "6000 only healed nearby");
    }
}
