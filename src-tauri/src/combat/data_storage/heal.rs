//! Healing done, per healer and skill.

use std::collections::HashMap;

use super::{DataStorage, HealSkillData, HealTick, Inner};

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

    /// Healing done from `from_ms` to `to_ms`: one fight's.
    pub fn heals_between(&self, from_ms: i64, to_ms: i64) -> HashMap<i32, HashMap<(i32, bool), HealSkillData>> {
        heals_between(&self.inner.read(), from_ms, to_ms)
    }
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
    let mut out: HashMap<i32, HashMap<(i32, bool), HealSkillData>> = HashMap::new();
    for t in inner.heal_ticks.iter().filter(|t| (from_ms..=to_ms).contains(&t.at)) {
        let e = out.entry(t.actor).or_default().entry((t.skill, t.is_hot)).or_default();
        e.total_heal += t.amount;
        e.tick_count = e.tick_count.saturating_add(1);
    }
    out
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
}
