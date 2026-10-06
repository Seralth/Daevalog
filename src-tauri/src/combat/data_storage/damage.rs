//! Damage records: counting them into the aggregates, and telling party from foe.

use std::sync::atomic::Ordering;

use crate::entity::damage_packet::ParsedDamagePacket;
use crate::entity::job_class::JobClass;
use crate::entity::special_damage::SpecialDamage;
use crate::entity::summon_resolver;

use super::encounter::{carry_encounter, encounter_ended, note_encounter, retire_all, retire_segment};
use super::entities::{link_summon, owner_link};
use super::heal::record_heal;
use super::names::apply_pending_nickname;
use super::roster::bind_roster_names_by_class;
use super::{
    now_ms, ActorCombatData, DataStorage, HealTick, Inner, NoDamageHit, SecondStats, SkillCombatData, TargetCombatData,
    IDLE_RESET_MS, ROSTER_BIND_EVERY,
};

impl DataStorage {
    pub fn append_damage(&self, pdp: ParsedDamagePacket) {
        let mut inner = self.inner.write();
        let skill_code = pdp.skill_code();
        let actor_id = pdp.actor_id();
        let target_id = pdp.target_id();

        // Not damage: a spirit and its owner naming each other.
        if let Some((summon, owner)) = owner_link(skill_code, actor_id, target_id) {
            if link_summon(&mut inner, summon, owner) {
                tracing::debug!("Summon {} linked to owner {} by skill {}", summon, owner, skill_code);
                self.damage_generation.fetch_add(1, Ordering::Relaxed);
            }
            return;
        }

        // NPC actors using NPC skills: track damage received on the player target, then skip
        let uses_npc_skill = (1_000_000..=9_999_999).contains(&skill_code);
        if inner.mob_storage.contains_key(&actor_id)
            && !inner.summon_storage.contains_key(&actor_id)
            && uses_npc_skill
        {
            // Track damage received on the player target
            let resolved_target = summon_resolver::resolve(target_id, &inner.summon_storage);
            if inner.known_player_ids.contains(&resolved_target) && is_ours(&inner, resolved_target) {
                let timeout = self.encounter_timeout_ms.load(Ordering::Relaxed);
                note_encounter(&mut inner, timeout, pdp.timestamp(), actor_id, true);
            }
            if inner.known_player_ids.contains(&resolved_target) {
                let dmg = pdp.total_damage();
                if let Some(actor_data) = fight_of(&mut inner, resolved_target, Some(actor_id)) {
                    actor_data.damage_received += dmg;
                    actor_data.hits_received = actor_data.hits_received.saturating_add(1);
                }
            }
            return;
        }

        // Track player skill usage. Exclude anything that spawned via a mob/summon
        // spawn (40/41 36) or has an owner: a real player never does, so such an
        // entity dealing class-band damage is a summon / spell-effect, not a player.
        if is_player_skill(skill_code)
            && !inner.confirmed_summon_ids.contains(&actor_id)
            && !inner.summon_spawn_ids.contains(&actor_id)
            && !inner.summon_storage.contains_key(&actor_id)
            && inner.known_player_ids.insert(actor_id)
        {
            purge_friendly_damage(&mut inner, actor_id);
        }

        // Party healing: player-on-player damage is actually healing/buffs
        if is_friendly_action(&inner, actor_id, target_id) {
            let heal_amount = pdp.total_damage();
            if heal_amount > 0 {
                if let Some(actor_data) = fight_of(&mut inner, actor_id, None) {
                    actor_data.party_heal += heal_amount;
                }
                // Also record per-skill so ally heals show in the HEAL view (the
                // self-heal path does this via append_heal; mirror it for ally heals).
                let tick = HealTick {
                    at: pdp.timestamp(),
                    actor: actor_id,
                    skill: pdp.skill_code(),
                    is_hot: false,
                    amount: heal_amount,
                };
                record_heal(&mut inner, tick);
            }
            return;
        }

        // Track hostile targets
        let resolved = summon_resolver::resolve(actor_id, &inner.summon_storage);
        if inner.known_player_ids.contains(&resolved) {
            inner.hostile_target_ids.insert(target_id);
        }

        // Track actor job
        if let Some(job) = JobClass::convert_from_skill(skill_code) {
            inner.actor_jobs.entry(actor_id).or_insert(job);
            inner.actor_skills.entry(actor_id).or_default().add(job, skill_code);
        }

        // Boss encounter auto-reset: if this target is a boss and the current
        // segment has no boss yet, clear the trash segment so boss gets clean data.
        // Once you are identified, only when you or your party pull it: a
        // stranger hitting a field boss nearby wiped everything you were
        // fighting.
        if inner.boss_entity_ids.contains(&target_id) && is_ours(&inner, actor_id) {
            if !inner.has_boss_in_segment && (!inner.target_combat.is_empty() || inner.idle_retired) {
                tracing::info!("Boss encounter auto-reset: boss entity {} hit, clearing trash segment", target_id);
                let timeout = self.encounter_timeout_ms.load(Ordering::Relaxed);
                if !encounter_ended(&inner, timeout, pdp.timestamp()) {
                    carry_encounter(&mut inner);
                }
                retire_all(&mut inner);
                inner.held_dot_ticks.clear();
                inner.dead_entity_ids.clear();
            }
            inner.has_boss_in_segment = true;
        }

        if inner.training_dummy_ids.contains(&target_id) {
            let key = (target_id, actor_id);
            if pdp.is_dot() {
                inner.held_dot_ticks.entry(key).or_default().push(pdp);
                return;
            }
            // A direct hit: the DoT ticks since the last one count after all.
            for tick in inner.held_dot_ticks.remove(&key).unwrap_or_default() {
                apply_damage(&mut inner, &tick);
            }
        }
        apply_damage(&mut inner, &pdp);
        let ours = is_ours(&inner, actor_id);
        let timeout = self.encounter_timeout_ms.load(Ordering::Relaxed);
        note_encounter(&mut inner, timeout, pdp.timestamp(), target_id, ours);

        self.damage_generation.fetch_add(1, Ordering::Relaxed);
        self.last_damage_ms.store(now_ms(), Ordering::Relaxed);

        // Apply pending nickname
        apply_pending_nickname(&mut inner, actor_id);

        inner.damage_since_roster_bind += 1;
        if inner.damage_since_roster_bind >= ROSTER_BIND_EVERY {
            inner.damage_since_roster_bind = 0;
            bind_roster_names_by_class(&mut inner);
        }
    }

    /// A hit that did no damage, on the skill's row: only where the actor
    /// already has damage on this target, so no target or meter row appears
    /// and no total, hit count or fight time moves. False when not counted.
    pub fn append_no_damage_hit(&self, target_id: i32, actor_id: i32, skill_code: i32, kind: NoDamageHit) -> bool {
        let mut inner = self.inner.write();
        let Some(actor) = inner.target_combat.get_mut(&target_id).and_then(|t| t.actors.get_mut(&actor_id)) else {
            return false;
        };
        let skill = actor
            .skills
            .entry((skill_code, false))
            .or_insert_with(|| SkillCombatData::new(skill_code, false));
        match kind {
            NoDamageHit::Miss => skill.miss_count = skill.miss_count.saturating_add(1),
            NoDamageHit::Resist => skill.resist_count = skill.resist_count.saturating_add(1),
        }
        drop(inner);
        self.touch();
        true
    }
}

/// Count one damage record into its target's and actor's aggregates.
fn apply_damage(inner: &mut Inner, pdp: &ParsedDamagePacket) {
    let skill_code = pdp.skill_code();
    let actor_id = pdp.actor_id();
    let target_id = pdp.target_id();
    let timestamp = pdp.timestamp();
    let packet_id = pdp.id();

    // Idle reset check (30s gap): a new fight on the same mob. The one before
    // goes to the auto-save first; it used to be thrown away unsaved.
    if let Some(old) = inner.target_combat.get(&target_id)
        && old.last_damage_time > 0
        && timestamp - old.last_damage_time > IDLE_RESET_MS
    {
        tracing::info!("Idle reset: target {} — gap {}ms", target_id,
            timestamp - old.last_damage_time);
        if let Some(old) = inner.target_combat.remove(&target_id) {
            retire_segment(inner, old);
        }
    }

    // Get or create target combat data
    let ours = is_ours(inner, actor_id);
    let dungeon_id = inner.current_dungeon_id;
    let open_world = inner.map_kind == super::MapKind::OpenWorld;
    let target_data = inner.target_combat.entry(target_id).or_insert_with(|| {
        TargetCombatData::new(target_id, timestamp)
    });
    // Saved fights are yours only. Before the meter knows you, `is_ours` says
    // yes to everyone (for display); a stranger's world boss was saved and
    // offered for upload that way (2026-10-05).
    target_data.ours |= ours && inner.local_player_id.is_some();
    // The roster names the instance only after it arrives, so a hit before it
    // leaves the id for a later hit to fill.
    if dungeon_id != 0 {
        target_data.dungeon_id = dungeon_id;
    }
    target_data.open_world |= open_world;

    // Update target timing
    if timestamp < target_data.first_damage_time {
        target_data.first_damage_time = timestamp;
    }
    if timestamp > target_data.last_damage_time {
        target_data.last_damage_time = timestamp;
    }
    let total_dmg = pdp.total_damage();
    target_data.total_damage += total_dmg;
    target_data.last_packet_id = packet_id;

    // Update actor data within target
    let actor_data = target_data.actors.entry(actor_id).or_insert_with(ActorCombatData::new);
    actor_data.total_damage += total_dmg;
    let direct = !pdp.is_dot();
    actor_data.add_at(timestamp.div_euclid(1000), &SecondStats {
        damage: total_dmg,
        hits: direct as i64,
        crits: (direct && pdp.is_crit()) as i64,
        max_hit: if direct { pdp.damage() } else { 0 },
    });
    if timestamp < actor_data.first_damage_time {
        actor_data.first_damage_time = timestamp;
    }
    if timestamp > actor_data.last_damage_time {
        actor_data.last_damage_time = timestamp;
    }
    if actor_data.job.is_none() {
        actor_data.job = JobClass::convert_from_skill(skill_code);
    }

    // Update skill data
    let skill_key = (skill_code, pdp.is_dot());
    let skill_data = actor_data.skills.entry(skill_key).or_insert_with(|| {
        SkillCombatData::new(skill_code, pdp.is_dot())
    });
    skill_data.hit_count = skill_data.hit_count.saturating_add(1);
    skill_data.total_damage = skill_data.total_damage.saturating_add(total_dmg);
    let hit_dmg = pdp.damage();
    if hit_dmg < skill_data.min_damage { skill_data.min_damage = hit_dmg; }
    if hit_dmg > skill_data.max_damage { skill_data.max_damage = hit_dmg; }
    if pdp.is_crit() { skill_data.crit_count = skill_data.crit_count.saturating_add(1); }
    if pdp.specials().contains(&SpecialDamage::Back) { skill_data.back_count = skill_data.back_count.saturating_add(1); }
    if pdp.specials().contains(&SpecialDamage::Frontal) { skill_data.frontal_count = skill_data.frontal_count.saturating_add(1); }
    for special in pdp.specials() {
        match special {
            SpecialDamage::ShieldBlock => skill_data.shield_block_count = skill_data.shield_block_count.saturating_add(1),
            SpecialDamage::Parry => skill_data.parry_count = skill_data.parry_count.saturating_add(1),
            SpecialDamage::Perfect => skill_data.perfect_count = skill_data.perfect_count.saturating_add(1),
            SpecialDamage::Double => skill_data.double_count = skill_data.double_count.saturating_add(1),
            SpecialDamage::IronWall => skill_data.iron_wall_count = skill_data.iron_wall_count.saturating_add(1),
            SpecialDamage::Regeneration => skill_data.regeneration_count = skill_data.regeneration_count.saturating_add(1),
            SpecialDamage::PerfectBlock => skill_data.perfect_block_count = skill_data.perfect_block_count.saturating_add(1),
            SpecialDamage::Back | SpecialDamage::Frontal | SpecialDamage::Critical => {}
        }
    }
    if pdp.multi_hit_count() > 0 {
        skill_data.multi_hit_count = skill_data.multi_hit_count.saturating_add(1);
        skill_data.multi_hit_damage = skill_data.multi_hit_damage.saturating_add(pdp.multi_hit_damage());
        skill_data.multi_hit_hits = skill_data.multi_hit_hits.saturating_add(pdp.multi_hit_count());
    }
    skill_data.heal_amount = skill_data.heal_amount.saturating_add(pdp.heal_amount());
    // Track regen (life-steal) on the actor aggregate
    if pdp.heal_amount() > 0 {
        actor_data.regen += pdp.heal_amount();
    }
    skill_data.hit_timestamps.push(timestamp);
    for (i, &flag) in pdp.spec_flags().iter().enumerate() {
        if flag { skill_data.spec_flags[i] = true; }
    }
}

/// Whether `actor_id` is you, your party, or a summon of either. Anyone
/// counts until the meter knows who you are.
pub(super) fn is_ours(inner: &Inner, actor_id: i32) -> bool {
    let Some(local) = inner.local_player_id else { return true };
    let owner = summon_resolver::resolve(actor_id, &inner.summon_storage);
    owner == local as i32
        || inner.nickname_storage.get(&owner).is_some_and(|n| inner.party_members.contains_key(n.as_str()))
}

/// Where healing or damage taken by `actor` counts: neither has a target of
/// its own. The fight against `mob` when the actor is in it, else the target
/// the actor hit last (ties to the lowest id), so it is never left to the
/// map's order.
fn fight_of(inner: &mut Inner, actor: i32, mob: Option<i32>) -> Option<&mut ActorCombatData> {
    let fights = |tid: &i32| inner.target_combat.get(tid).is_some_and(|td| td.actors.contains_key(&actor));
    let tid = mob.filter(fights).or_else(|| {
        inner
            .target_combat
            .iter()
            .filter_map(|(&tid, td)| td.actors.get(&actor).map(|a| (a.last_damage_time, std::cmp::Reverse(tid))))
            .max()
            .map(|(_, std::cmp::Reverse(tid))| tid)
    })?;
    inner.target_combat.get_mut(&tid)?.actors.get_mut(&actor)
}

fn is_friendly_action(inner: &Inner, actor_id: i32, target_id: i32) -> bool {
    let resolved_actor = summon_resolver::resolve(actor_id, &inner.summon_storage);
    let resolved_target = summon_resolver::resolve(target_id, &inner.summon_storage);
    inner.known_player_ids.contains(&resolved_actor) && inner.known_player_ids.contains(&resolved_target)
}

/// Remove friendly-fire damage from aggregates when a new player is identified.
pub(super) fn purge_friendly_damage(inner: &mut Inner, _uid: i32) {
    let mut to_remove: Vec<(i32, Vec<i32>)> = Vec::new();

    for (&target_id, target_data) in &inner.target_combat {
        let mut actors_to_remove = Vec::new();
        for &actor_id in target_data.actors.keys() {
            if is_friendly_action(inner, actor_id, target_id) {
                actors_to_remove.push(actor_id);
            }
        }
        if !actors_to_remove.is_empty() {
            to_remove.push((target_id, actors_to_remove));
        }
    }

    for (target_id, actor_ids) in to_remove {
        if let Some(target_data) = inner.target_combat.get_mut(&target_id) {
            for actor_id in actor_ids {
                if let Some(actor_data) = target_data.actors.remove(&actor_id) {
                    target_data.total_damage -= actor_data.total_damage;
                }
            }
            if target_data.actors.is_empty() {
                inner.target_combat.remove(&target_id);
            }
        }
    }
}

pub fn is_player_skill(skill_code: i32) -> bool {
    // Class skills: 11M-19M (post-divide 110K-190K, encodes class in first 2 digits)
    // Alternate band: 3M-3.99M (post-divide 30K-39.9K)
    // Basic/special attacks: 100K-199K (post-divide 1K-1.9K)
    (11_000_000..=19_999_999).contains(&skill_code)
        || (3_000_000..=3_999_999).contains(&skill_code)
        || (100_000..=199_999).contains(&skill_code)
}
