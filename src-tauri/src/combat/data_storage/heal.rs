//! Healing done, per healer and skill.

use std::collections::HashMap;

use super::{DataStorage, HealSkillData};

impl DataStorage {
    /// Record a heal tick done by `actor_id` with `skill_code` (is_hot marks a HoT).
    /// Keyed by the healer so "healing done" can be shown per player. Self-heals count.
    pub fn append_heal(&self, actor_id: i32, skill_code: i32, amount: i64, is_hot: bool) {
        if amount <= 0 || !self.is_plausible_entity_id(actor_id) {
            return;
        }
        let mut inner = self.inner.write();
        let e = inner
            .heal_storage
            .entry(actor_id)
            .or_default()
            .entry((skill_code, is_hot))
            .or_default();
        e.total_heal += amount;
        e.tick_count += 1;
    }

    pub fn get_heal_snapshot(&self) -> HashMap<i32, HashMap<(i32, bool), HealSkillData>> {
        self.inner.read().heal_storage.clone()
    }
}
