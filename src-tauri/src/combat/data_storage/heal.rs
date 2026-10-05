//! Healing done, per healer and skill.

use std::collections::HashMap;

use super::{DataStorage, HealSkillData, HealTick, Inner};

/// Heal ticks kept for saved fights; the oldest go first.
const MAX_HEAL_TICKS: usize = 100_000;

impl DataStorage {
    /// Record a heal tick done by `actor_id` with `skill_code` (is_hot marks a HoT).
    /// Keyed by the healer so "healing done" can be shown per player. Self-heals count.
    pub fn append_heal(&self, actor_id: i32, skill_code: i32, amount: i64, is_hot: bool, at: i64) {
        if amount <= 0 || !self.is_plausible_entity_id(actor_id) {
            return;
        }
        record_heal(&mut self.inner.write(), HealTick { at, actor: actor_id, skill: skill_code, is_hot, amount });
    }

    pub fn get_heal_snapshot(&self) -> HashMap<i32, HashMap<(i32, bool), HealSkillData>> {
        self.inner.read().heal_storage.clone()
    }

    /// Healing done from `from_ms` to `to_ms`: one fight's.
    pub fn heals_between(&self, from_ms: i64, to_ms: i64) -> HashMap<i32, HashMap<(i32, bool), HealSkillData>> {
        heals_between(&self.inner.read(), from_ms, to_ms)
    }
}

pub(super) fn record_heal(inner: &mut Inner, tick: HealTick) {
    let e = inner.heal_storage.entry(tick.actor).or_default().entry((tick.skill, tick.is_hot)).or_default();
    e.total_heal += tick.amount;
    e.tick_count += 1;
    if inner.heal_ticks.len() >= MAX_HEAL_TICKS {
        inner.heal_ticks.pop_front();
    }
    inner.heal_ticks.push_back(tick);
}

pub(super) fn heals_between(inner: &Inner, from_ms: i64, to_ms: i64) -> HashMap<i32, HashMap<(i32, bool), HealSkillData>> {
    let mut out: HashMap<i32, HashMap<(i32, bool), HealSkillData>> = HashMap::new();
    for t in inner.heal_ticks.iter().filter(|t| (from_ms..=to_ms).contains(&t.at)) {
        let e = out.entry(t.actor).or_default().entry((t.skill, t.is_hot)).or_default();
        e.total_heal += t.amount;
        e.tick_count += 1;
    }
    out
}
