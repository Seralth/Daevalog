//! Damage taken: monsters' hits on players, kept hit by hit so each fight
//! takes the hits taken during it, as healing does.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;

use crate::entity::summon_resolver;
use crate::entity::taken::{TakenHit, TakenStats};

use super::{DataStorage, Inner, TargetCombatData};

/// Hits kept at most, a memory backstop only: hits go when no open fight
/// needs them (`prune_taken`).
const MAX_TAKEN: usize = 1_000_000;
/// How often, in hits, the ones no fight needs are dropped.
const PRUNE_EVERY: usize = 4096;
/// Hits this recent are kept even with no fight open: a fight's first hit
/// can come after the hits taken during it (the boss hits the tank first).
pub(super) const TAKEN_SLACK_MS: i64 = 60_000;
/// Skill-effect entities whose monster is known, at most. They live for
/// seconds; the map starts over when full.
const MAX_EFFECT_PARENTS: usize = 16_384;

/// One hit taken, with who dealt it as far as the meter can tell.
#[derive(Debug, Clone, Copy)]
pub struct TakenTick {
    /// Counts up from 1, one per hit. See `taken_since`.
    pub seq: u64,
    pub hit: TakenHit,
    /// The attacker, or the monster whose skill-effect entity it is.
    pub source: i32,
    /// The source's NPC code; 0 when it was never seen spawning.
    pub source_code: i32,
}

/// One skill's damage on one player, and the NPC that used it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TakenSkillData {
    pub stats: TakenStats,
    pub source_code: i32,
}

/// Damage taken per player, then per skill.
pub type TakenBy = HashMap<i32, HashMap<i32, TakenSkillData>>;

impl DataStorage {
    /// Record a hit a player took. Hits on monsters and summons are left out:
    /// a player never spawns as either.
    pub fn append_taken(&self, hit: TakenHit) {
        if hit.target == hit.actor || !self.is_plausible_entity_id(hit.target) {
            return;
        }
        let mut inner = self.inner.write();
        if !takes_damage(&inner, hit.target) {
            return;
        }
        let source = inner.effect_parents.get(&hit.actor).copied().unwrap_or(hit.actor);
        let source_code = inner.mob_storage.get(&source).copied().unwrap_or(0);
        inner.taken_seq += 1;
        let seq = inner.taken_seq;
        inner.taken.push_back(TakenTick { seq, hit, source, source_code });
        if inner.taken.len() % PRUNE_EVERY == 0 {
            prune_taken(&mut inner);
        }
        if inner.taken.len() > MAX_TAKEN {
            inner.taken.pop_front();
        }
        drop(inner);
        self.taken_generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Changes whenever a hit is taken.
    pub fn taken_generation(&self) -> i64 {
        self.taken_generation.load(Ordering::Relaxed)
    }

    /// The hits taken after the one numbered `seq`, oldest first. A reader
    /// that asks after each packet sees every hit, pruned or not.
    pub fn taken_since(&self, seq: u64) -> Vec<TakenTick> {
        let inner = self.inner.read();
        let start = inner.taken.partition_point(|t| t.seq <= seq);
        inner.taken.range(start..).copied().collect()
    }

    /// One fight's damage taken: from its first hit to its last, by the
    /// people in it (see `taken_in`).
    pub fn fight_taken(&self, fight: &TargetCombatData) -> TakenBy {
        taken_in(&self.inner.read(), &[fight], i64::MIN)
    }

    /// The damage taken over the live fights against `targets` (with what a
    /// boss pull cleared of the encounter when `encounter`), from `since` on.
    pub fn taken_on(&self, targets: &[i32], encounter: bool, since: Option<i64>) -> TakenBy {
        let inner = self.inner.read();
        let carry = targets.iter().filter_map(|t| inner.encounter_carry.get(t)).filter(|_| encounter);
        let fights: Vec<&TargetCombatData> = targets.iter().filter_map(|t| inner.target_combat.get(t)).chain(carry).collect();
        taken_in(&inner, &fights, since.unwrap_or(i64::MIN))
    }

    /// A `0x1C` skill-effect entity's spawn named `parent`. When that is a
    /// monster, what the effect hits is the monster's damage.
    pub fn note_effect_parent(&self, effect: i32, parent: i32) {
        let mut inner = self.inner.write();
        let monster = inner.mob_storage.contains_key(&parent) && !inner.summon_storage.contains_key(&parent);
        if parent == effect || !monster {
            inner.effect_parents.remove(&effect);
            return;
        }
        if inner.effect_parents.len() >= MAX_EFFECT_PARENTS {
            inner.effect_parents.clear();
        }
        inner.effect_parents.insert(effect, parent);
    }
}

/// Whether `id` can be a player taking damage: a player by any sign, or at
/// least never a monster or a summon.
fn takes_damage(inner: &Inner, id: i32) -> bool {
    let player = inner.player_spawn_ids.contains(&id)
        || inner.known_player_ids.contains(&id)
        || inner.nickname_storage.contains_key(&id)
        || inner.local_player_id == Some(id as i64);
    player
        || !(inner.mob_storage.contains_key(&id)
            || inner.summon_spawn_ids.contains(&id)
            || inner.summon_storage.contains_key(&id)
            || inner.confirmed_summon_ids.contains(&id))
}

/// The damage taken during `fights` (from the first one's first hit to the
/// last one's last, not before `since`) by the people in them: you, your
/// party, whoever hit one of the fights' targets, and whoever those targets
/// hit. Players nearby in another fight are left out, as for healing.
pub(super) fn taken_in(inner: &Inner, fights: &[&TargetCombatData], since: i64) -> TakenBy {
    let mut out = TakenBy::new();
    let Some(from) = fights.iter().map(|f| f.first_damage_time).min() else { return out };
    let until = fights.iter().map(|f| f.last_damage_time).max().unwrap_or(from);
    let from = from.max(since);
    let fighters: HashSet<i32> = fights
        .iter()
        .flat_map(|f| f.actors.keys())
        .map(|&a| summon_resolver::resolve(a, &inner.summon_storage))
        .collect();
    let targets: HashSet<i32> = fights.iter().map(|f| f.target_id).collect();
    for t in inner.taken.iter().filter(|t| (from..=until).contains(&t.hit.at)) {
        let player = t.hit.target;
        let in_it = fighters.contains(&player)
            || targets.contains(&t.source)
            || inner.local_player_id.is_some_and(|l| l as i32 == player)
            || inner.nickname_storage.get(&player).is_some_and(|n| inner.party_members.contains_key(n.as_str()));
        if in_it {
            add_tick(&mut out, t);
        }
    }
    out
}

/// Count one hit into `out`.
pub fn add_tick(out: &mut TakenBy, t: &TakenTick) {
    let e = out.entry(t.hit.target).or_default().entry(t.hit.skill).or_default();
    e.stats.add(&t.hit);
    if e.source_code == 0 {
        e.source_code = t.source_code;
    }
}

/// Drop the hits from before every fight still open: a fight takes only the
/// hits taken during it, and ended fights took theirs when they were retired.
fn prune_taken(inner: &mut Inner) {
    let Some(newest) = inner.taken.back().map(|t| t.hit.at) else { return };
    let oldest_fight = inner.target_combat.values()
        .chain(inner.encounter_carry.values())
        .map(|td| td.first_damage_time)
        .min()
        .unwrap_or(i64::MAX);
    let keep_from = oldest_fight.min(newest.saturating_sub(TAKEN_SLACK_MS));
    while inner.taken.front().is_some_and(|t| t.hit.at < keep_from) {
        inner.taken.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::damage_packet::ParsedDamagePacket;
    use crate::entity::taken::TakenKind;

    fn hit(s: &DataStorage, actor: i32, target: i32, at: i64) {
        let mut p = ParsedDamagePacket::new();
        p.set_actor_id(actor);
        p.set_target_id(target);
        p.set_skill_code(11_010_000);
        p.set_damage(100);
        p.set_timestamp(at);
        s.append_damage(p);
    }

    fn taken(s: &DataStorage, actor: i32, target: i32, at: i64, damage: i64) {
        s.append_taken(TakenHit::plain(at, target, actor, 1_800_510, damage, TakenKind::Hit));
    }

    #[test]
    fn hits_on_monsters_and_summons_are_not_taken() {
        let s = DataStorage::new();
        s.append_mob(900, 2_310_403);
        s.note_summon_spawn(950);
        taken(&s, 900, 950, 1_000, 10);
        taken(&s, 900, 901, 1_000, 10);
        s.append_mob(901, 2_310_403);
        taken(&s, 900, 901, 1_000, 10);
        let all: Vec<i32> = s.taken_since(0).iter().map(|t| t.hit.target).collect();
        assert_eq!(all, vec![901], "901 was no monster yet");
    }

    #[test]
    fn a_skill_effect_entity_hits_for_its_monster() {
        let s = DataStorage::new();
        s.append_mob(27249, 2_310_403);
        s.append_mob(24810, 2_920_048);
        s.note_effect_parent(24810, 27249);
        s.note_effect_parent(24811, 9200);
        taken(&s, 24810, 9200, 1_000, 607);
        taken(&s, 24811, 9200, 1_000, 5);
        let ticks = s.taken_since(0);
        assert_eq!((ticks[0].source, ticks[0].source_code), (27249, 2_310_403));
        assert_eq!(ticks[1].source, 24811, "a player is no effect's monster");
        // A new spawn under the id is another entity.
        s.note_summon_spawn(24810);
        taken(&s, 24810, 9200, 2_000, 1);
        assert_eq!(s.taken_since(2)[0].source, 24810);
    }

    #[test]
    fn a_fight_takes_the_hits_on_the_people_in_it_during_it() {
        let s = DataStorage::new();
        s.set_local_player_id(Some(1000));
        s.append_mob(900, 1);
        hit(&s, 1000, 900, 1_000);
        hit(&s, 2000, 900, 1_500);
        hit(&s, 1000, 900, 9_000);
        taken(&s, 900, 1000, 500, 1); // before the fight
        taken(&s, 900, 1000, 2_000, 10);
        taken(&s, 900, 2000, 3_000, 20); // a stranger who hit the boss
        taken(&s, 900, 3000, 4_000, 40); // a stranger the boss hit
        taken(&s, 901, 4000, 5_000, 80); // nearby, another fight
        taken(&s, 900, 1000, 9_500, 1); // after its last hit
        let fight = s.inner.read().target_combat[&900].clone();
        let by = s.fight_taken(&fight);
        let mut players: Vec<(i32, i64)> = by.iter().map(|(&p, k)| (p, k.values().map(|d| d.stats.damage).sum())).collect();
        players.sort();
        assert_eq!(players, vec![(1000, 10), (2000, 20), (3000, 40)]);
        assert_eq!(s.taken_on(&[900], false, Some(2_500)).len(), 2, "from the window on");
    }

    #[test]
    fn an_ended_fight_keeps_its_damage_taken_through_a_reset() {
        let s = DataStorage::new();
        s.set_local_player_id(Some(1000));
        s.append_mob(900, 1);
        s.register_boss(900);
        hit(&s, 1000, 900, 1_000);
        taken(&s, 900, 1000, 3_000, 700);
        hit(&s, 1000, 900, 7_000);
        s.flush();
        assert!(s.taken_since(0).is_empty());
        let ended = s.take_ended_segments();
        assert_eq!(ended[0].taken[&1000][&1_800_510].stats.damage, 700);
    }

    #[test]
    fn hits_no_open_fight_needs_are_dropped() {
        let s = DataStorage::new();
        for at in 0..PRUNE_EVERY as i64 {
            taken(&s, 900, 1000, at, 1);
        }
        assert_eq!(s.taken_since(0).len(), PRUNE_EVERY, "within the slack");
        hit(&s, 1000, 900, 200_000);
        for at in 200_000..200_000 + PRUNE_EVERY as i64 {
            taken(&s, 900, 1000, at, 1);
        }
        let left = s.taken_since(0);
        assert_eq!(left.len(), PRUNE_EVERY);
        assert_eq!(left[0].hit.at, 200_000);
        assert_eq!(s.taken_since(left[10].seq).len(), PRUNE_EVERY - 11);
    }
}
