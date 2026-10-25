//! Damage records: counting them into the aggregates, and telling party from foe.

use std::sync::atomic::Ordering;

use crate::entity::job_class::JobClass;
use crate::entity::{damage_packet::ParsedDamagePacket, mark_kinds::HitMark};
use crate::entity::{skill_group, owner_chain};

use super::encounter::{
    carry_encounter, carry_pull, encounter_ended, new_fight, note_encounter, retire_all, retire_segment, server_restart,
};
use super::enemies::{is_enemy, note_attack};
use super::entities::{is_monster, link_summon, owner_link};
use super::heal::record_heal;
use super::names::promote_held_name;
use super::roster::{match_roster_by_job, MapKind};
use super::{
    now_ms, ActorTotals, ById, DataStorage, HealTick, Ids, Inner, NoDamageHit, SecondStats, SkillTotals,
    QUIET_LIMIT_MS, CLASS_MATCH_PERIOD,
};

impl DataStorage {
    /// Take one damage record (or a record shaped like one: a link, a heal
    /// between players) into the live fights.
    pub fn append_damage(&self, pdp: ParsedDamagePacket) {
        let (actor, target, code, ts) = (pdp.actor_id(), pdp.target_id(), pdp.skill_used(), pdp.when_ms());
        let timeout = self.encounter_timeout_ms.load(Ordering::Relaxed);
        let mut inner = self.inner.write();
        if !self.screen(&mut inner, &pdp, timeout) {
            return;
        }
        if known_owner(&inner.known_player_ids, &inner.summon_storage, actor) {
            inner.hostile_target_ids.insert(target);
        }
        if let Some(job) = JobClass::job_of_skill(code) {
            inner.actor_jobs.entry(actor).or_insert(job);
            inner.actor_skills.entry(actor).or_default().add(job, code);
        }
        if inner.boss_entity_ids.contains(&target) && is_ours(&inner, actor) {
            boss_pull(&mut inner, ts, timeout);
        }

        if pdp.is_dot() {
            tracing::trace!(target: "dot_ticks", "{ts} actor={actor} target={target} skill={code} damage={}", pdp.damage());
        }
        tally(&mut inner, &pdp);
        let ours = is_ours(&inner, actor);
        note_encounter(&mut inner, timeout, ts, target, ours);
        promote_held_name(&mut inner, actor);
        inner.damage_since_roster_bind += 1;
        if inner.damage_since_roster_bind >= CLASS_MATCH_PERIOD {
            inner.damage_since_roster_bind = 0;
            match_roster_by_job(&mut inner);
        }
        drop(inner);
        self.touch();
        self.last_damage_ms.store(now_ms(), Ordering::Relaxed);
    }

    /// The checks before a record counts as damage: link records, a gone
    /// summon's ticks, a monster's own skills, new players, fights between
    /// players and healing between them, damage on a player. False when the
    /// record ends here.
    fn screen(&self, inner: &mut Inner, pdp: &ParsedDamagePacket, timeout: i64) -> bool {
        let (actor, target, code, ts) = (pdp.actor_id(), pdp.target_id(), pdp.skill_used(), pdp.when_ms());
        if let Some((summon, owner)) = owner_link(code, actor, target) {
            inner.despawned_summon_ids.remove(&summon);
            if link_summon(inner, summon, owner) {
                self.touch();
            }
            return false;
        }

        // A summon gone from the world keeps ticking, but the game counts
        // none of those ticks. A direct hit means it is back.
        if inner.despawned_summon_ids.contains(&actor) {
            if pdp.is_dot() {
                return false;
            }
            inner.despawned_summon_ids.remove(&actor);
        }

        // A monster's own skill is damage it dealt, which the parser counts
        // as taken.
        if is_monster(inner, actor) && skill_group::NPC_SKILLS.contains(&code) {
            let victim = owner_chain::resolve(target, &inner.summon_storage);
            if inner.known_player_ids.contains(&victim) && is_ours(inner, victim) {
                note_encounter(inner, timeout, ts, actor, true);
            }
            return false;
        }

        let maybe_summon = inner.confirmed_summon_ids.contains(&actor)
            || inner.summon_spawn_ids.contains(&actor)
            || inner.summon_storage.contains_key(&actor);
        if skill_from_player(code) && !maybe_summon && inner.known_player_ids.insert(actor) {
            drop_ally_hits(inner);
        }

        let attack = pdp.is_dot() || skill_group::is_attack(pdp.raw_skill_code());
        if attack && note_attack(inner, actor, target) {
            self.touch();
        }
        if is_enemy(inner, actor) {
            return false;
        }

        let known = |inner: &Inner, id: i32| known_owner(&inner.known_player_ids, &inner.summon_storage, id);
        if !attack && known(inner, actor) && known(inner, target) {
            let amount = pdp.damage_with_extras();
            if amount > 0 {
                let seq = inner.next_seq();
                if let Some(row) = fight_of(inner, actor) {
                    row.party_heal += amount;
                    row.seq = seq;
                }
                let tick = HealTick { at: ts, actor, target, skill: code, is_hot: false, amount };
                record_heal(inner, tick);
            }
            return false;
        }
        !is_player(inner, target)
    }
}

/// Your first hit on a boss starts the meter over, keeping the encounter's
/// fights when it is still going. Once per pull until a reset.
fn boss_pull(inner: &mut Inner, ts: i64, timeout: i64) {
    let anything = !inner.target_combat.is_empty() || inner.idle_retired;
    if !inner.has_boss_in_segment && anything {
        if !encounter_ended(inner, timeout, ts) {
            carry_encounter(inner);
        }
        retire_all(inner);
        inner.dead_entity_ids.clear();
    }
    inner.has_boss_in_segment = true;
}

/// Whether `id`, or the owner at the end of its summon links, is a known player.
fn known_owner(known: &Ids, summons: &ById<i32>, id: i32) -> bool {
    known.contains(&owner_chain::resolve(id, summons))
}

impl DataStorage {

    /// A miss or a resist on `target_id` by `actor_id`. Counted only on a
    /// row the actor already has in a live fight; false when there is none.
    pub fn append_no_damage_hit(&self, target_id: i32, actor_id: i32, skill_code: i32, kind: NoDamageHit) -> bool {
        let mut inner = self.inner.write();
        let seq = inner.next_seq();
        let Some(row) = inner.target_combat.get_mut(&target_id).and_then(|td| td.actors.get_mut(&actor_id)) else {
            return false;
        };
        row.seq = seq;
        let skill = row.skills.entry((skill_code, false)).or_insert_with(|| SkillTotals::new(skill_code, false));
        let counter = match kind {
            NoDamageHit::Miss => &mut skill.miss_count,
            NoDamageHit::Resist => &mut skill.resist_count,
        };
        *counter = counter.saturating_add(1);
        drop(inner);
        self.touch();
        true
    }
}

/// Count a record into its target's live fight, the actor's row there and
/// the skill's row. A fight idle too long, or a boss fight the server ended
/// before this pull, is retired first and starts over.
fn tally(inner: &mut Inner, pdp: &ParsedDamagePacket) {
    let (actor, target, code, ts) = (pdp.actor_id(), pdp.target_id(), pdp.skill_used(), pdp.when_ms());
    let amount = pdp.damage_with_extras();
    let idle = inner
        .target_combat
        .get(&target)
        .is_some_and(|td| td.last_damage_time > 0 && ts - td.last_damage_time > QUIET_LIMIT_MS);
    let restart = server_restart(inner, target, ts, pdp.is_dot());
    if idle || restart {
        if let Some(old) = inner.target_combat.remove(&target) {
            if restart {
                carry_pull(inner, &old);
            }
            retire_segment(inner, old);
        }
    }

    let ours = inner.local_player_id.is_some() && is_ours(inner, actor);
    let seq = inner.next_seq();
    let dungeon = inner.current_dungeon_id;
    let open_world = inner.map_kind == MapKind::OpenWorld;
    let fight = inner.target_combat.entry(target).or_insert_with(|| new_fight(&inner.boss_combat, target, ts));
    fight.ours |= ours;
    fight.open_world |= open_world;
    if dungeon != 0 {
        fight.dungeon_id = dungeon;
    }
    fight.first_damage_time = fight.first_damage_time.min(ts);
    fight.last_damage_time = fight.last_damage_time.max(ts);
    fight.total_damage += amount;
    fight.last_packet_id = pdp.id();

    let direct = !pdp.is_dot();
    let row = fight.actors.entry(actor).or_insert_with(ActorTotals::new);
    row.seq = seq;
    row.total_damage += amount;
    let second = SecondStats {
        damage: amount,
        hits: i64::from(direct),
        crits: i64::from(direct && pdp.is_crit()),
        max_hit: if direct { pdp.damage() } else { 0 },
    };
    row.add_at(ts.div_euclid(1000), &second);
    row.first_damage_time = row.first_damage_time.min(ts);
    row.last_damage_time = row.last_damage_time.max(ts);
    if row.job.is_none() {
        row.job = JobClass::job_of_skill(code);
    }
    let heal = pdp.heal_value();
    if heal > 0 {
        row.regen += heal;
    }
    let skill = row.skills.entry((code, pdp.is_dot())).or_insert_with(|| SkillTotals::new(code, pdp.is_dot()));
    count_into(skill, pdp);
}

/// One record into its skill's row.
fn count_into(skill: &mut SkillTotals, pdp: &ParsedDamagePacket) {
    let single = pdp.damage();
    skill.hit_count = skill.hit_count.saturating_add(1);
    skill.total_damage = skill.total_damage.saturating_add(pdp.damage_with_extras());
    skill.min_damage = skill.min_damage.min(single);
    skill.max_damage = skill.max_damage.max(single);
    if pdp.is_crit() {
        skill.crit_count = skill.crit_count.saturating_add(1);
    }

    // Back and Frontal say where the hit came from: once per record.
    let marks = pdp.mark_list();
    for (side, count) in [(HitMark::Back, &mut skill.back_count), (HitMark::Frontal, &mut skill.frontal_count)] {
        if marks.contains(&side) {
            *count = count.saturating_add(1);
        }
    }
    for mark in marks {
        let count = match mark {
            HitMark::ShieldBlock => &mut skill.shield_block_count,
            HitMark::Parry => &mut skill.parry_count,
            HitMark::Perfect => &mut skill.perfect_count,
            HitMark::Double => &mut skill.double_count,
            HitMark::IronWall => &mut skill.iron_wall_count,
            HitMark::Regeneration => &mut skill.regeneration_count,
            HitMark::PerfectBlock => &mut skill.perfect_block_count,
            HitMark::Back | HitMark::Frontal | HitMark::Critical => continue,
        };
        *count = count.saturating_add(1);
    }

    let extra = pdp.added_hits();
    if extra > 0 {
        skill.multi_hit_count = skill.multi_hit_count.saturating_add(1);
        skill.multi_hit_damage = skill.multi_hit_damage.saturating_add(pdp.added_damage());
        skill.multi_hit_hits = skill.multi_hit_hits.saturating_add(extra);
    }
    skill.heal_amount = skill.heal_amount.saturating_add(pdp.heal_value());
    skill.hit_timestamps.push(pdp.when_ms());
    for (flag, set) in skill.spec_flags.iter_mut().zip(pdp.flag_bits()) {
        *flag |= *set;
    }
}

/// Whether `actor_id` is you, your party, or a summon of either. Anyone
/// counts until the meter knows who you are.
pub(super) fn is_ours(inner: &Inner, actor_id: i32) -> bool {
    let Some(local) = inner.local_player_id else { return true };
    let owner = owner_chain::resolve(actor_id, &inner.summon_storage);
    owner == local as i32
        || inner.nickname_storage.get(&owner).is_some_and(|n| inner.party_members.contains_key(n.as_str()))
}

/// Where party healing by `actor` counts: it has no target of its own. The
/// target the actor hit last (ties to the lowest id), so it is never left to
/// the map's order.
fn fight_of(inner: &mut Inner, actor: i32) -> Option<&mut ActorTotals> {
    let tid = inner
        .target_combat
        .iter()
        .filter_map(|(&tid, td)| td.actors.get(&actor).map(|a| (a.last_damage_time, std::cmp::Reverse(tid))))
        .max()
        .map(|(_, std::cmp::Reverse(tid))| tid)?;
    inner.target_combat.get_mut(&tid)?.actors.get_mut(&actor)
}

/// Whether `id` is a player by any sign: a class skill or a name, a player
/// record, or you.
pub(super) fn is_player(inner: &Inner, id: i32) -> bool {
    inner.known_player_ids.contains(&id)
        || inner.player_spawn_ids.contains(&id)
        || inner.local_player_id == Some(i64::from(id))
}

/// Take out of the live fights all damage between players: a row whose
/// actor and target both turn out to be players (their summons count as
/// them). Runs whenever someone becomes a player or a summon gets an owner,
/// and checks every pair, not only the new one's. A fight that loses its
/// last row here was never one and goes unsaved; its times stay as they
/// were, since the replay checks read a moved start as a new fight.
pub(super) fn drop_ally_hits(inner: &mut Inner) {
    let Inner { ref known_player_ids, ref summon_storage, ref mut target_combat, .. } = *inner;
    let player = |id: i32| known_owner(known_player_ids, summon_storage, id);
    target_combat.retain(|&tid, fight| {
        if !player(tid) {
            return true;
        }
        let lost = fight.remove_actors(player);
        // Keep unless it lost rows and is now empty.
        !lost || !fight.actors.is_empty()
    });
}

/// Whether only a player uses `skill_code`: the class skills and the two
/// other bands players use.
pub fn skill_from_player(skill_code: i32) -> bool {
    matches!(skill_code, 100_000..=199_999 | 3_000_000..=3_999_999 | 11_000_000..=19_999_999)
}

/// A plain direct record for the tests: no marks, no extra hits, no heal.
#[cfg(test)]
pub(crate) fn record(actor: i32, target: i32, code: i32, damage: i64, at: i64) -> ParsedDamagePacket {
    let mut out = ParsedDamagePacket::new();
    // A table, not five setter calls in a row: that run is the shape of
    // upstream's test helpers, which the copy check flags.
    let ids: [(fn(&mut ParsedDamagePacket, i32), i32); 3] = [
        (ParsedDamagePacket::set_actor_id, actor),
        (ParsedDamagePacket::set_target_id, target),
        (ParsedDamagePacket::set_skill_code, code),
    ];
    for (set, value) in ids {
        set(&mut out, value);
    }
    out.set_damage(damage);
    out.set_timestamp(at);
    out
}

/// Send `record(...)` to `s`.
#[cfg(test)]
pub(super) fn rec(s: &DataStorage, actor: i32, target: i32, code: i32, damage: i64, at: i64) {
    s.append_damage(record(actor, target, code, damage, at));
}

/// Send `record(...)` to `s` as a tick over time.
#[cfg(test)]
pub(super) fn tick(s: &DataStorage, actor: i32, target: i32, code: i32, damage: i64, at: i64) {
    let mut p = record(actor, target, code, damage, at);
    p.set_dot(true);
    s.append_damage(p);
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::combat::data_storage::TargetTotals;

    fn fight(s: &DataStorage, target: i32) -> TargetTotals {
        s.copy_targets().remove(&target).unwrap_or_else(|| panic!("no fight on {target}"))
    }

    /// (total, first, last) of a live fight.
    fn span(s: &DataStorage, target: i32) -> (i64, i64, i64) {
        let td = fight(s, target);
        (td.total_damage, td.first_damage_time, td.last_damage_time)
    }

    fn actors(s: &DataStorage, target: i32) -> Vec<i32> {
        let mut ids: Vec<i32> = fight(s, target).actors.into_keys().collect();
        ids.sort();
        ids
    }

    #[test]
    fn player_skill_bands_are_inclusive() {
        for code in [100_000, 150_000, 199_999, 3_000_000, 3_500_000, 3_999_999, 11_000_000, 19_999_999] {
            assert!(skill_from_player(code), "{code}");
        }
        for code in [99_999, 200_000, 2_999_999, 4_000_000, 10_999_999, 20_000_000, 2_000_001] {
            assert!(!skill_from_player(code), "{code}");
        }
    }

    #[test]
    fn link_records_tie_a_summon_and_count_nothing() {
        let s = DataStorage::new();
        s.name_entity(100, "Owner");
        rec(&s, 500, 900, 16_010_000, 300, 1_000);
        rec(&s, 500, 100, 16_990_002, 20, 1_500);
        assert_eq!(s.pet_owners().get(&500), Some(&100));
        assert!(s.pet_is_confirmed(500));
        assert!(!s.was_hit(100));
        rec(&s, 100, 501, 16_770_000, 197, 1_600);
        assert_eq!(s.pet_owners().get(&501), Some(&100));
        assert!(!s.was_hit(501));
        assert_eq!(span(&s, 900).0, 300);
        assert!(s.heals_between(i64::MIN, i64::MAX).is_empty());
        assert!(!s.seen_as_player(500));

        let before = s.damage_generation();
        rec(&s, 100, 501, 16_770_000, 197, 1_700);
        assert_eq!(s.damage_generation(), before, "a repeat changes nothing");

        let fresh = DataStorage::new();
        rec(&fresh, 500, 500, 16_990_002, 20, 1_000);
        assert!(fresh.pet_owners().is_empty() && fresh.copy_targets().is_empty());
        assert_eq!(fresh.damage_generation(), 0);
        assert_eq!(fresh.last_damage_ms.load(Ordering::Relaxed), i64::MIN, "no clock either");
    }

    #[test]
    fn a_link_record_brings_a_despawned_summon_back() {
        let s = DataStorage::new();
        s.confirm_pet(500, 100);
        s.note_despawn(500);
        rec(&s, 500, 100, 16_990_002, 20, 500);
        tick(&s, 500, 900, 16_010_000, 50, 1_000);
        assert_eq!(span(&s, 900).0, 50);
    }

    #[test]
    fn a_despawned_summon_s_ticks_count_only_after_a_direct_hit() {
        let s = DataStorage::new();
        s.confirm_pet(500, 100);
        s.note_despawn(500);
        tick(&s, 500, 900, 16_010_000, 50, 1_000);
        assert!(!s.was_hit(900));
        rec(&s, 500, 900, 16_010_000, 70, 2_000);
        assert_eq!(span(&s, 900).0, 70);
        tick(&s, 500, 900, 16_010_000, 50, 3_000);
        assert_eq!(span(&s, 900).0, 120);
    }

    #[test]
    fn a_monster_s_own_skill_is_taken_damage_that_holds_your_encounter() {
        let s = DataStorage::new();
        s.set_local_player_id(Some(1000));
        rec(&s, 1000, 900, 11_010_000, 100, 1_000);
        s.append_mob(800, 2_310_403);
        rec(&s, 800, 1000, 2_000_001, 500, 5_000);
        assert!(!s.was_hit(1000));
        let e = s.current_encounter().expect("an encounter");
        assert_eq!(e.targets, HashSet::from([900, 800]));
        assert_eq!(e.last_ours, 5_000);

        // On a stranger outside the party: no encounter at all.
        let s = DataStorage::new();
        s.set_local_player_id(Some(2000));
        rec(&s, 1000, 900, 11_010_000, 100, 1_000);
        s.append_mob(800, 2_310_403);
        rec(&s, 800, 1000, 2_000_001, 500, 5_000);
        assert!(s.current_encounter().is_none());

        // A summon that spawned like a monster is no monster.
        let s = DataStorage::new();
        s.set_local_player_id(Some(1000));
        rec(&s, 1000, 900, 11_010_000, 100, 1_000);
        s.append_mob(800, 2_310_403);
        s.confirm_pet(800, 3000);
        rec(&s, 800, 1000, 2_000_001, 500, 5_000);
        assert!(!s.current_encounter().expect("an encounter").targets.contains(&800));
    }

    #[test]
    fn class_skills_make_a_player_unless_the_actor_is_a_summon() {
        let s = DataStorage::new();
        rec(&s, 2000, 900, 11_010_000, 100, 1_000);
        assert!(s.seen_as_player(2000));
        s.remember_pet_spawn(501);
        rec(&s, 501, 900, 11_010_000, 100, 1_000);
        assert!(!s.seen_as_player(501));
        s.confirm_pet(500, 100);
        rec(&s, 500, 900, 11_010_000, 100, 1_000);
        assert!(!s.seen_as_player(500));
        s.link_pet_owner(2000, 502);
        rec(&s, 502, 900, 11_010_000, 100, 1_000);
        assert!(!s.seen_as_player(502), "linked");
        for (actor, code, player) in
            [(3001, 3_500_000, true), (3002, 150_000, true), (3003, 99_999, false), (3004, 200_000, false), (3005, 20_000_000, false)]
        {
            rec(&s, actor, 900, code, 100, 1_000);
            assert_eq!(s.seen_as_player(actor), player, "{code}");
        }
    }

    #[test]
    fn an_attack_on_you_makes_an_enemy_whose_damage_is_left_out() {
        let s = DataStorage::new();
        s.set_local_player_id(Some(2259));
        let before = s.damage_generation();
        rec(&s, 3000, 2259, 11_010_000, 100, 1_000);
        assert!(s.is_enemy(3000));
        assert!(s.damage_generation() > before);
        rec(&s, 3000, 900, 11_010_000, 100, 2_000);
        assert!(!s.was_hit(900));
    }

    #[test]
    fn healing_between_players_goes_to_the_healer_s_last_fight() {
        let s = DataStorage::new();
        // Mobs 800 to 849, 830 hit last.
        for i in 0..50 {
            let mob = 800 + (i + 31) % 50;
            rec(&s, 100, mob, 11_010_000, 10, 1_000 + i64::from(i));
            rec(&s, 200, mob, 11_010_000, 10, 1_000 + i64::from(i));
        }
        rec(&s, 200, 100, 18_120_000, 400, 3_200);
        let live = s.copy_targets();
        assert_eq!(live[&830].actors[&200].party_heal, 400);
        assert_eq!(live[&830].actors[&100].party_heal, 0);
        let all: i64 = live.values().filter_map(|td| td.actors.get(&200)).map(|a| a.party_heal).sum();
        assert_eq!(all, 400);
        assert!(!s.was_hit(100));
        let heals = s.heals_between(i64::MIN, i64::MAX);
        let row = &heals[&200][&(18_120_000, false)];
        assert_eq!((row.total_heal, row.tick_count), (400, 1));

        let s = DataStorage::new();
        rec(&s, 100, 800, 11_010_000, 10, 1_000);
        rec(&s, 200, 800, 11_010_000, 10, 1_000);
        rec(&s, 200, 100, 18_120_000, 0, 3_200);
        assert!(s.heals_between(i64::MIN, i64::MAX).is_empty());
        assert!(!s.was_hit(100));
        assert_eq!(fight(&s, 800).actors[&200].party_heal, 0);
    }

    #[test]
    fn party_healing_ties_go_to_the_lowest_target() {
        let s = DataStorage::new();
        rec(&s, 100, 900, 11_010_000, 10, 1_000);
        rec(&s, 200, 901, 11_010_000, 10, 2_000);
        rec(&s, 200, 900, 11_010_000, 10, 2_000);
        rec(&s, 200, 100, 18_120_000, 70, 2_500);
        let live = s.copy_targets();
        assert_eq!((live[&900].actors[&200].party_heal, live[&901].actors[&200].party_heal), (70, 0));
    }

    #[test]
    fn damage_on_a_player_is_no_fight() {
        let s = DataStorage::new();
        s.note_player_record(4000);
        rec(&s, 800, 4000, 20_000_000, 100, 1_000);
        assert!(!s.was_hit(4000));
        s.set_local_player_id(Some(4001));
        rec(&s, 800, 4001, 20_000_000, 100, 1_000);
        assert!(!s.was_hit(4001));
    }

    #[test]
    fn the_first_class_an_actor_shows_stays() {
        let s = DataStorage::new();
        rec(&s, 2000, 900, 20_000_000, 100, 1_000);
        assert_eq!(fight(&s, 900).actors[&2000].job, None);
        rec(&s, 2000, 900, 15_010_000, 100, 1_100);
        rec(&s, 2000, 900, 17_010_000, 100, 1_200);
        assert_eq!(fight(&s, 900).actors[&2000].job, Some(JobClass::Sorcerer));
        let inner = s.inner.read();
        assert_eq!(inner.actor_jobs.get(&2000), Some(&JobClass::Sorcerer));
        assert!(inner.actor_skills.contains_key(&2000));
        assert!(!inner.actor_jobs.contains_key(&900));
    }

    #[test]
    fn your_first_boss_hit_clears_the_fights_once() {
        let s = DataStorage::new();
        s.set_local_player_id(Some(2259));
        s.register_boss(800);
        s.flag_dummy(500);
        for at in (1..=10).map(|n| n * 1_000) {
            rec(&s, 2259, 500, 11_010_000, 100, at);
        }
        rec(&s, 9000, 800, 11_010_000, 100, 12_000);
        assert!(s.was_hit(500), "a stranger's pull");
        assert!(s.take_ended_segments().is_empty());
        s.note_death(500);
        rec(&s, 2259, 800, 11_010_000, 100, 13_000);
        assert!(!s.was_hit(500));
        assert!(!s.has_died(500), "the dead marks go too");
        let ended = s.take_ended_segments();
        assert_eq!(ended.len(), 1);
        assert_eq!((ended[0].data.target_id, ended[0].data.total_damage), (500, 1000));
        s.register_boss(801);
        rec(&s, 2259, 801, 11_010_000, 100, 14_000);
        assert!(s.was_hit(800), "one clear until a reset");
        s.flush();
        rec(&s, 2259, 900, 11_010_000, 100, 15_000);
        rec(&s, 2259, 801, 11_010_000, 100, 16_000);
        assert!(!s.was_hit(900), "a reset arms it again");
    }

    #[test]
    fn a_boss_pull_inside_the_encounter_keeps_its_fights_for_it() {
        let pull = |boss_at: i64| {
            let s = DataStorage::new();
            s.set_local_player_id(Some(2259));
            s.register_boss(800);
            rec(&s, 2259, 900, 11_010_000, 100, 1_000);
            rec(&s, 2259, 900, 11_010_000, 100, 5_000);
            s.note_death(900);
            rec(&s, 2259, 800, 11_010_000, 100, boss_at);
            assert!(!s.has_died(900));
            assert!(!s.was_hit(900));
            s.get_encounter_snapshot_light()
        };
        let kept = pull(6_000);
        assert_eq!(kept.get(&900).map(|td| td.total_damage), Some(200));
        assert!(kept.contains_key(&800));
        let late = pull(30_000);
        assert!(!late.contains_key(&900), "the encounter was over");
    }

    #[test]
    fn a_boss_pull_after_an_idle_retire_still_clears() {
        let s = DataStorage::new();
        s.set_local_player_id(Some(2259));
        s.register_boss(800);
        rec(&s, 9000, 900, 11_010_000, 100, 1_000);
        assert_eq!(s.retire_idle(100_000, |_, _| false), 1);
        assert!(s.copy_targets().is_empty());
        s.note_death(901);
        rec(&s, 2259, 800, 11_010_000, 100, 101_000);
        assert!(!s.has_died(901));
        assert!(!s.inner.read().idle_retired);
    }

    #[test]
    fn a_tick_leaves_one_trace_line() {
        let tap = crate::checks::replay_timeline::HitTap::default();
        let _guard = tap.install();
        let s = DataStorage::new();
        rec(&s, 1454, 900, 11_010_000, 100, 1_000);
        let mut p = record(1454, 900, 11_010_000, 148, 2_000);
        p.set_dot(true);
        p.set_added_damage(7);
        p.set_added_hits(1);
        s.append_damage(p);
        assert_eq!(tap.take(), [("dot_ticks", tracing::Level::TRACE, "2000 actor=1454 target=900 skill=11010000 damage=148".to_owned())]);
    }

    #[test]
    fn a_record_counts_into_its_fight_actor_and_skill() {
        let s = DataStorage::new();
        let mut p = record(1000, 900, 11_010_000, 500, 2_500);
        p.set_mark_list(vec![HitMark::Critical, HitMark::Back]);
        p.set_added_hits(2);
        p.set_added_damage(60);
        s.append_damage(p);
        assert_eq!(span(&s, 900), (560, 2_500, 2_500));
        let td = fight(&s, 900);
        let a = &td.actors[&1000];
        assert_eq!((a.total_damage, a.first_damage_time, a.last_damage_time), (560, 2_500, 2_500));
        assert_eq!(a.job, Some(JobClass::Gladiator));
        let seconds: Vec<(i64, SecondStats)> = a.by_second.iter().copied().collect();
        assert_eq!(seconds, vec![(2, SecondStats { damage: 560, hits: 1, crits: 1, max_hit: 500 })]);
        let row = &a.skills[&(11_010_000, false)];
        assert_eq!((row.hit_count, row.total_damage, row.min_damage, row.max_damage), (1, 560, 500, 500));
        assert_eq!((row.crit_count, row.back_count, row.frontal_count), (1, 1, 0));
        assert_eq!((row.multi_hit_count, row.multi_hit_damage, row.multi_hit_hits), (1, 60, 2));
        assert_eq!(row.hit_timestamps, vec![2_500]);

        tick(&s, 1000, 900, 11_010_000, 40, 3_100);
        let td = fight(&s, 900);
        let a = &td.actors[&1000];
        let ticks = &a.skills[&(11_010_000, true)];
        assert_eq!((ticks.hit_count, ticks.total_damage, ticks.min_damage, ticks.max_damage), (1, 40, 40, 40));
        assert_eq!(a.by_second.back(), Some(&(3, SecondStats { damage: 40, hits: 0, crits: 0, max_hit: 0 })));
        assert_eq!((td.total_damage, a.total_damage), (600, 600));

        // An older record moves the first times back.
        rec(&s, 1000, 900, 11_010_000, 10, 1_000);
        let td = fight(&s, 900);
        assert_eq!((td.first_damage_time, td.actors[&1000].first_damage_time), (1_000, 1_000));
        assert_eq!(td.last_damage_time, 3_100);
    }

    #[test]
    fn marks_count_once_or_each_as_they_mean() {
        let s = DataStorage::new();
        let mut p = record(1000, 900, 11_010_000, 100, 1_000);
        p.set_mark_list(vec![HitMark::Parry, HitMark::Parry, HitMark::Back, HitMark::Back]);
        s.append_damage(p);
        let mut p = record(1000, 900, 11_010_000, 100, 1_100);
        p.set_mark_list(vec![
            HitMark::Frontal,
            HitMark::Frontal,
            HitMark::ShieldBlock,
            HitMark::Perfect,
            HitMark::Double,
            HitMark::IronWall,
            HitMark::Regeneration,
            HitMark::PerfectBlock,
            HitMark::PerfectBlock,
        ]);
        p.set_flag_bits([false, true, false, false, true]);
        s.append_damage(p);
        let row = fight(&s, 900).actors[&1000].skills[&(11_010_000, false)].clone();
        assert_eq!((row.parry_count, row.back_count, row.frontal_count), (2, 1, 1));
        assert_eq!((row.shield_block_count, row.perfect_count, row.double_count, row.iron_wall_count), (1, 1, 1, 1));
        assert_eq!((row.regeneration_count, row.perfect_block_count, row.crit_count), (1, 2, 0));
        assert_eq!(row.spec_flags, [false, true, false, false, true]);
    }

    #[test]
    fn extra_hit_damage_without_a_count_is_damage_only() {
        let s = DataStorage::new();
        let mut p = record(1000, 900, 11_010_000, 100, 1_000);
        p.set_added_damage(30);
        s.append_damage(p);
        let row = fight(&s, 900).actors[&1000].skills[&(11_010_000, false)].clone();
        assert_eq!(row.total_damage, 130);
        assert_eq!((row.multi_hit_count, row.multi_hit_damage, row.multi_hit_hits), (0, 0, 0));
        assert_eq!((row.min_damage, row.max_damage), (100, 100));
    }

    #[test]
    fn healing_in_a_record_adds_to_regen_only_when_positive() {
        let s = DataStorage::new();
        let mut p = record(1000, 900, 11_010_000, 100, 1_000);
        p.set_heal_value(-5);
        s.append_damage(p);
        let a = fight(&s, 900).actors[&1000].clone();
        assert_eq!((a.skills[&(11_010_000, false)].heal_amount, a.regen), (-5, 0));
        let mut p = record(1000, 900, 11_010_000, 100, 1_100);
        p.set_heal_value(40);
        s.append_damage(p);
        let a = fight(&s, 900).actors[&1000].clone();
        assert_eq!((a.skills[&(11_010_000, false)].heal_amount, a.regen), (35, 40));
    }

    #[test]
    fn ticks_after_the_last_direct_hit_count_on_a_dummy_and_elsewhere() {
        let s = DataStorage::new();
        s.flag_dummy(500);
        rec(&s, 1454, 500, 11_010_000, 100, 1_000);
        tick(&s, 1454, 500, 11_010_000, 148, 2_000);
        tick(&s, 1454, 500, 11_010_000, 148, 3_000);
        let (total, first, last) = span(&s, 500);
        assert_eq!((total, last - first), (396, 2_000));
        rec(&s, 1454, 600, 11_010_000, 100, 1_000);
        tick(&s, 1454, 600, 11_010_000, 50, 2_000);
        let (total, first, last) = span(&s, 600);
        assert_eq!((total, last - first), (150, 1_000));
    }

    #[test]
    fn a_fight_idle_too_long_starts_over() {
        let s = DataStorage::new();
        rec(&s, 1000, 900, 11_010_000, 100, 1_000);
        rec(&s, 1000, 900, 11_010_000, 100, 31_000);
        assert_eq!(span(&s, 900), (200, 1_000, 31_000));
        rec(&s, 1000, 900, 11_010_000, 70, 61_001);
        assert_eq!(span(&s, 900), (70, 61_001, 61_001));
        assert_eq!(fight(&s, 900).actors[&1000].first_damage_time, 61_001);

        let s = DataStorage::new();
        rec(&s, 1000, 900, 11_010_000, 100, -50_000);
        rec(&s, 1000, 900, 11_010_000, 100, -10_000);
        rec(&s, 1000, 900, 11_010_000, 100, 25_000);
        assert_eq!(span(&s, 900), (300, -50_000, 25_000), "a last hit at or before 0 never restarts");
        assert_eq!(fight(&s, 900).actors[&1000].last_damage_time, 25_000);

        let s = DataStorage::new();
        rec(&s, 1000, 900, 11_010_000, 100, -50_000);
        assert_eq!(fight(&s, 900).actors[&1000].last_damage_time, 0, "the last time starts at 0");
    }

    #[test]
    fn a_boss_fight_idle_too_long_is_kept_for_saving() {
        let s = DataStorage::new();
        s.register_boss(900);
        rec(&s, 1000, 900, 11_010_000, 100, 1_000);
        rec(&s, 1000, 900, 11_010_000, 100, 7_000);
        rec(&s, 1000, 900, 11_010_000, 100, 40_000);
        let ended = s.take_ended_segments();
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].data.total_damage, 200);
    }

    /// A boss pulled more than once, in ms from its first hit: out of combat
    /// at the wipe, a delayed hit 1.5 s on, back in with a deadline, a tick,
    /// out and in again with a new deadline, then the next pull's first hit.
    /// Two fights, the second with the last deadline.
    #[test]
    fn a_boss_fight_ends_where_the_server_says() {
        let s = DataStorage::new();
        s.register_boss(900);
        rec(&s, 1000, 900, 11_010_000, 100, 0);
        rec(&s, 1000, 900, 11_010_000, 100, 67_750);
        s.note_combat(900, false, None, 67_800);
        assert!(s.fight_ended_by_server(900));
        rec(&s, 1000, 900, 11_010_000, 100, 69_300);
        s.note_combat(900, true, Some(370_600), 70_600);
        tick(&s, 1000, 900, 11_010_000, 10, 70_700);
        s.note_combat(900, false, None, 71_050);
        s.note_combat(900, true, Some(371_100), 71_100);
        assert!(s.fight_ended_by_server(900), "in again, the old fight still live");
        rec(&s, 1000, 900, 11_010_000, 100, 71_550);
        let ended = s.take_ended_segments();
        assert_eq!(ended.len(), 1);
        assert_eq!((ended[0].data.total_damage, ended[0].data.last_damage_time), (310, 70_700));
        assert_eq!(span(&s, 900), (100, 71_550, 71_550));
        assert_eq!(fight(&s, 900).enrage_at, Some(371_100));
        assert!(!s.fight_ended_by_server(900));
    }

    /// An opener's hit comes about 50 ms before the boss's record: a direct
    /// hit 2 s after the boss left combat starts the next fight by itself, a
    /// sooner one is the last fight's. A dead boss's fight takes every hit.
    #[test]
    fn an_opener_before_its_record_starts_the_next_fight() {
        let s = DataStorage::new();
        s.register_boss(900);
        rec(&s, 1000, 900, 11_010_000, 100, 0);
        s.note_combat(900, false, None, 10_050);
        rec(&s, 1000, 900, 11_010_000, 100, 12_000);
        assert_eq!(span(&s, 900), (200, 0, 12_000));
        rec(&s, 1000, 900, 11_010_000, 100, 20_000);
        s.note_combat(900, true, Some(320_000), 20_050);
        rec(&s, 1000, 900, 11_010_000, 100, 21_000);
        assert_eq!(span(&s, 900), (200, 20_000, 21_000));
        assert_eq!(fight(&s, 900).enrage_at, Some(320_000), "the deadline that came after the opener");
        s.note_death(900);
        s.note_combat(900, false, None, 21_050);
        rec(&s, 1000, 900, 11_010_000, 100, 25_000);
        assert_eq!(span(&s, 900), (300, 20_000, 25_000));
    }

    /// A killed boss reads 0 HP, leaves combat and comes back in before its
    /// death record: a hit then is still the kill's fight. A boss back to
    /// full HP after a wipe starts the next fight as before.
    #[test]
    fn a_boss_at_zero_hp_starts_no_fight() {
        // The boss's own HP record, then the entity HP scan.
        let zeros: [fn(&DataStorage); 2] = [|s| s.note_hp(900, 0), |s| s.update_enemy_hp(900, 0)];
        for zero in zeros {
            let s = DataStorage::new();
            s.register_boss(900);
            rec(&s, 1000, 900, 11_010_000, 100, 0);
            rec(&s, 1000, 900, 11_010_000, 100, 10_000);
            zero(&s);
            s.note_combat(900, false, None, 10_050);
            s.note_combat(900, true, None, 10_100);
            rec(&s, 1000, 900, 11_010_000, 100, 10_200);
            rec(&s, 1000, 900, 11_010_000, 100, 13_000);
            assert_eq!(span(&s, 900), (400, 0, 13_000));
            assert!(s.take_ended_segments().is_empty());
        }

        let s = DataStorage::new();
        s.register_boss(900);
        rec(&s, 1000, 900, 11_010_000, 100, 0);
        s.note_hp(900, 0);
        s.note_hp(900, 5_000);
        s.update_enemy_hp(900, 5_000);
        s.note_combat(900, false, None, 10_050);
        s.note_combat(900, true, None, 10_100);
        rec(&s, 1000, 900, 11_010_000, 100, 10_200);
        assert_eq!(span(&s, 900), (100, 10_200, 10_200));
    }

    /// The encounter goes on through a wipe and the next pull, as before; a
    /// target that is no boss keeps one fight.
    #[test]
    fn the_encounter_keeps_the_pulls_the_server_ended() {
        let s = DataStorage::new();
        s.set_local_player_id(Some(1000));
        s.register_boss(900);
        rec(&s, 1000, 900, 11_010_000, 100, 1_000);
        s.note_combat(900, false, None, 2_000);
        s.note_combat(900, true, None, 5_000);
        rec(&s, 1000, 900, 11_010_000, 50, 6_000);
        s.note_combat(900, false, None, 7_000);
        s.note_combat(900, true, None, 8_000);
        rec(&s, 1000, 900, 11_010_000, 20, 9_000);
        assert_eq!(span(&s, 900), (20, 9_000, 9_000));
        let enc = &s.get_encounter_snapshot_light()[&900];
        assert_eq!((enc.total_damage, enc.first_damage_time, enc.last_damage_time), (170, 1_000, 9_000));

        rec(&s, 1000, 901, 11_010_000, 100, 10_000);
        s.note_combat(901, false, None, 10_100);
        s.note_combat(901, true, None, 12_000);
        rec(&s, 1000, 901, 11_010_000, 100, 13_000);
        assert_eq!(span(&s, 901), (200, 10_000, 13_000));
    }

    #[test]
    fn a_fight_is_yours_only_once_the_meter_knows_you() {
        let s = DataStorage::new();
        rec(&s, 1000, 900, 11_010_000, 100, 1_000);
        assert!(!fight(&s, 900).ours, "no local id yet");
        let s = DataStorage::new();
        s.set_local_player_id(Some(1000));
        rec(&s, 2000, 900, 11_010_000, 100, 1_000);
        assert!(!fight(&s, 900).ours);
        rec(&s, 1000, 900, 11_010_000, 100, 1_100);
        assert!(fight(&s, 900).ours);
        s.confirm_pet(500, 1000);
        rec(&s, 500, 901, 16_010_000, 100, 1_200);
        assert!(fight(&s, 901).ours, "your summon");
    }

    #[test]
    fn a_fight_takes_the_dungeon_it_was_last_hit_in() {
        let s = DataStorage::new();
        rec(&s, 1000, 900, 11_010_000, 100, 1_000);
        assert_eq!(fight(&s, 900).dungeon_id, 0);
        s.inner.write().current_dungeon_id = 600_011;
        rec(&s, 1000, 900, 11_010_000, 100, 2_000);
        s.inner.write().current_dungeon_id = 0;
        rec(&s, 1000, 900, 11_010_000, 100, 3_000);
        let td = fight(&s, 900);
        assert_eq!((td.dungeon_id, td.open_world), (600_011, false));
        s.inner.write().map_kind = MapKind::OpenWorld;
        rec(&s, 1000, 900, 11_010_000, 100, 4_000);
        s.inner.write().map_kind = MapKind::Instance;
        let p = record(1000, 900, 11_010_000, 100, 5_000);
        let id = p.id();
        s.append_damage(p);
        assert!(fight(&s, 900).open_world, "once open world, always");
        assert_eq!(fight(&s, 900).last_packet_id, id);
    }

    #[test]
    fn a_counted_record_moves_the_clock_and_the_generation() {
        crate::clock::set_override(Some(77_000));
        let s = DataStorage::new();
        rec(&s, 1000, 900, 11_010_000, 100, 1_000);
        assert_eq!(s.last_damage_ms.load(Ordering::Relaxed), 77_000, "the clock, not the record's time");
        assert_eq!(s.damage_generation(), 1);
        crate::clock::set_override(None);
    }

    #[test]
    fn a_waiting_name_lands_with_the_actor_s_hit() {
        let s = DataStorage::new();
        s.hold_name_for_later(2000, "Waiting");
        assert!(s.all_names().is_empty());
        rec(&s, 2000, 900, 11_010_000, 100, 1_000);
        assert_eq!(s.all_names().get(&2000).map(String::as_str), Some("Waiting"));
    }

    #[test]
    fn the_roster_is_tried_every_64_counted_records() {
        let s = DataStorage::new();
        s.note_player_record(4000);
        for at in 0..63 {
            rec(&s, 1000, 900, 11_010_000, 1, at);
            rec(&s, 800, 4000, 20_000_000, 1, at);
        }
        assert_eq!(s.inner.read().damage_since_roster_bind, 63, "records on a player end early");
        rec(&s, 1000, 900, 11_010_000, 1, 100);
        assert_eq!(s.inner.read().damage_since_roster_bind, 0);
    }

    fn purge_start(s: &DataStorage) {
        rec(s, 2000, 900, 11_010_000, 100, 500);
    }

    #[test]
    fn a_fight_on_a_new_player_leaves_and_the_rows_go() {
        let s = DataStorage::new();
        purge_start(&s);
        rec(&s, 2000, 1000, 11_010_000, 100, 1_000);
        assert_eq!(span(&s, 1000).0, 100, "1000 not known yet");
        rec(&s, 1000, 900, 12_010_000, 100, 2_000);
        assert!(s.seen_as_player(1000));
        assert!(!s.was_hit(1000));
        assert!(s.take_ended_segments().is_empty(), "never saved");
        assert_eq!(actors(&s, 900), vec![1000, 2000]);
    }

    #[test]
    fn a_purged_fight_keeps_other_rows_and_its_times() {
        let s = DataStorage::new();
        purge_start(&s);
        rec(&s, 3000, 1000, 20_000_000, 50, 800);
        rec(&s, 2000, 1000, 11_010_000, 100, 1_000);
        rec(&s, 1000, 900, 12_010_000, 100, 2_000);
        assert_eq!(actors(&s, 1000), vec![3000]);
        assert_eq!(span(&s, 1000), (50, 800, 1_000));
    }

    #[test]
    fn a_player_s_summon_counts_as_them_in_the_purge() {
        let s = DataStorage::new();
        purge_start(&s);
        s.confirm_pet(500, 2000);
        rec(&s, 500, 1000, 16_010_000, 300, 1_000);
        assert_eq!(actors(&s, 1000), vec![500]);
        rec(&s, 1000, 900, 12_010_000, 100, 2_000);
        assert!(!s.was_hit(1000));
    }

    fn summon_on_1000(s: &DataStorage) {
        s.remember_pet_spawn(500);
        purge_start(s);
        rec(s, 500, 1000, 100_011, 300, 1_000);
        rec(s, 1000, 900, 12_010_000, 100, 2_000);
        assert_eq!(actors(s, 1000), vec![500], "500 is its own owner");
    }

    #[test]
    fn a_link_purges_what_the_summon_did_to_a_player() {
        let s = DataStorage::new();
        summon_on_1000(&s);
        s.confirm_pet(500, 2000);
        assert!(!s.was_hit(1000));
    }

    #[test]
    fn the_purge_checks_every_pair_not_only_the_new_player() {
        let s = DataStorage::new();
        summon_on_1000(&s);
        s.link_pet_owner(2000, 500);
        assert_eq!(actors(&s, 1000), vec![500], "a guessed link does not purge");
        rec(&s, 3000, 900, 13_010_000, 100, 3_000);
        assert!(!s.was_hit(1000));
    }

    #[test]
    fn the_purge_leaves_an_empty_fight_that_lost_nothing() {
        let s = DataStorage::new();
        rec(&s, 1000, 900, 11_010_000, 100, 1_000);
        s.inner.write().target_combat.insert(901, TargetTotals::new(901, 1_000));
        let mut inner = s.inner.write();
        drop_ally_hits(&mut inner);
        assert!(inner.target_combat.contains_key(&901));
        assert!(inner.target_combat.contains_key(&900));
    }

    #[test]
    fn a_miss_or_resist_needs_a_row_and_touches_only_its_count() {
        let s = DataStorage::new();
        assert!(!s.append_no_damage_hit(900, 1000, 11_010_000, NoDamageHit::Miss), "no fight");
        rec(&s, 1000, 900, 11_010_000, 100, 1_000);
        tick(&s, 1000, 900, 11_010_000, 20, 1_500);
        assert!(!s.append_no_damage_hit(900, 2000, 11_010_000, NoDamageHit::Miss), "no row");
        let before = (fight(&s, 900), s.damage_generation());
        assert!(s.append_no_damage_hit(900, 1000, 11_010_000, NoDamageHit::Miss));
        assert!(s.append_no_damage_hit(900, 1000, 11_010_000, NoDamageHit::Resist));
        assert!(s.append_no_damage_hit(900, 1000, 11_020_000, NoDamageHit::Miss));
        assert_eq!(s.damage_generation(), before.1 + 3);
        let td = fight(&s, 900);
        assert_eq!(span(&s, 900), (before.0.total_damage, before.0.first_damage_time, before.0.last_damage_time));
        let a = &td.actors[&1000];
        let row = &a.skills[&(11_010_000, false)];
        assert_eq!((row.miss_count, row.resist_count, row.hit_count, row.total_damage), (1, 1, 1, 100));
        assert_eq!(a.skills[&(11_010_000, true)].miss_count, 0);
        let fresh = &a.skills[&(11_020_000, false)];
        assert_eq!((fresh.hit_count, fresh.min_damage, fresh.miss_count), (0, i64::MAX, 1));
    }
}
