//! Enemies: players fighting you or your party. The PvE meter leaves them
//! out, with everything they did, damage on monsters too.

use crate::entity::summon_resolver;

use super::damage::{is_ours, is_player};
use super::{DataStorage, Inner, TargetCombatData};

impl DataStorage {
    /// Whether `id`, or the player whose summon it is, is an enemy.
    pub fn is_enemy(&self, id: i32) -> bool {
        is_enemy(&self.inner.read(), id)
    }
}

/// Whether `id`, or the player whose summon it is, is an enemy.
pub(super) fn is_enemy(inner: &Inner, id: i32) -> bool {
    !inner.enemy_ids.is_empty() && inner.enemy_ids.contains(&summon_resolver::resolve(id, &inner.summon_storage))
}

/// An attack by `actor` on `target`. When one side is you or your party and
/// the other a player who is not, that player is an enemy until you load
/// into another map, and what they did before goes too. Another map can hand
/// out ids again; a reset, a teleport or a respawn in the same map keeps the
/// mark (see `note_map_load`). Until the meter knows you everyone
/// counts as yours, so no one is marked. Returns whether a new enemy was
/// marked.
pub(super) fn note_attack(inner: &mut Inner, actor: i32, target: i32) -> bool {
    if inner.local_player_id.is_none() {
        return false;
    }
    let actor = summon_resolver::resolve(actor, &inner.summon_storage);
    let target = summon_resolver::resolve(target, &inner.summon_storage);
    if actor == target || !is_player(inner, actor) || !is_player(inner, target) {
        return false;
    }
    let enemy = match (is_ours(inner, actor), is_ours(inner, target)) {
        (true, false) => target,
        (false, true) => actor,
        _ => return false,
    };
    if !inner.enemy_ids.insert(enemy) {
        return false;
    }
    tracing::debug!("Enemy player {enemy}: left out of the meter");
    purge(inner, enemy);
    true
}

/// A summon just linked to `owner`: when that is an enemy, what the summon
/// did before the link goes too.
pub(super) fn after_link(inner: &mut Inner, owner: i32) {
    let owner = summon_resolver::resolve(owner, &inner.summon_storage);
    if inner.enemy_ids.contains(&owner) {
        purge(inner, owner);
    }
}

/// Take what `enemy` and their summons did out of the fights: the live ones,
/// what a boss pull kept for the encounter, and the ended ones waiting for
/// the auto-save.
fn purge(inner: &mut Inner, enemy: i32) {
    let Inner { ref summon_storage, ref mut target_combat, ref mut encounter_carry, ref mut ended_segments, .. } = *inner;
    let theirs = |a: i32| summon_resolver::resolve(a, summon_storage) == enemy;
    target_combat.retain(|_, td| drop_actors(td, theirs));
    encounter_carry.retain(|_, td| drop_actors(td, theirs));
    ended_segments.retain_mut(|seg| {
        seg.heals.retain(|&a, _| !theirs(a));
        seg.taken.remove(&enemy);
        drop_actors(&mut seg.data, theirs)
    });
}

/// Drop the actors `theirs` takes from a fight, and their part of its total
/// and its time. False when no one is left.
fn drop_actors(td: &mut TargetCombatData, theirs: impl Fn(i32) -> bool) -> bool {
    let gone: Vec<i32> = td.actors.keys().copied().filter(|&a| theirs(a)).collect();
    if gone.is_empty() {
        return true;
    }
    for a in gone {
        if let Some(ad) = td.actors.remove(&a) {
            td.total_damage -= ad.total_damage;
        }
    }
    let hit = td.actors.values().filter(|a| a.first_damage_time <= a.last_damage_time);
    let (first, last) = hit.fold((i64::MAX, i64::MIN), |(f, l), a| (f.min(a.first_damage_time), l.max(a.last_damage_time)));
    if first <= last {
        td.first_damage_time = first;
        td.last_damage_time = last;
    }
    !td.actors.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::damage_packet::ParsedDamagePacket;

    /// Rending Blow, which attacks, and Healing Light, which heals.
    const ATTACK: i32 = 11_010_000;
    const HEAL: i32 = 17_120_000;

    fn hit(s: &DataStorage, actor: i32, target: i32, skill: i32, at: i64) {
        let mut p = ParsedDamagePacket::new();
        p.set_actor_id(actor);
        p.set_target_id(target);
        p.set_skill_code(skill);
        p.set_damage(100);
        p.set_timestamp(at);
        s.append_damage(p);
    }

    #[test]
    fn an_enemy_stays_one_until_you_leave_the_map() {
        let s = DataStorage::new();
        s.set_local_player_id(Some(2259));
        s.note_map_load(20);
        hit(&s, 3000, 2259, ATTACK, 1_000);
        assert!(s.is_enemy(3000));
        s.flush();
        assert!(s.is_enemy(3000), "a reset leaves the enemy there");
        s.note_map_load(20);
        assert!(s.is_enemy(3000), "a respawn in the same map");
        s.note_map_load(1010);
        assert!(!s.is_enemy(3000), "the id can be someone else's now");

        hit(&s, 3000, 2259, ATTACK, 2_000);
        s.note_summon_spawn(3000);
        assert!(!s.is_enemy(3000), "the id is a summon's now");
    }

    #[test]
    fn a_heal_strangers_fighting_or_a_blind_meter_make_no_enemy() {
        let s = DataStorage::new();
        hit(&s, 3000, 2259, ATTACK, 1_000);
        assert!(!s.is_enemy(3000), "before the meter knows you, everyone is yours");
        s.set_local_player_id(Some(2259));
        hit(&s, 3000, 2259, HEAL, 2_000);
        assert!(!s.is_enemy(3000), "a stranger's heal on you");
        s.note_player_spawn(4000);
        hit(&s, 3000, 4000, ATTACK, 3_000);
        assert!(!s.is_enemy(3000) && !s.is_enemy(4000), "two strangers fighting");
    }

    #[test]
    fn an_enemys_summon_counts_as_them() {
        let s = DataStorage::new();
        s.set_local_player_id(Some(2259));
        // A spirit's hits before its link.
        s.note_summon_spawn(500);
        hit(&s, 500, 900, 100_011, 1_000);
        hit(&s, 3000, 2259, ATTACK, 2_000);
        assert!(s.get_combat_snapshot().contains_key(&900));
        s.register_confirmed_summon_by_id(500, 3000);
        assert!(!s.get_combat_snapshot().contains_key(&900), "what it did before the link goes");
        hit(&s, 500, 901, 100_011, 3_000);
        assert!(!s.get_combat_snapshot().contains_key(&901));

        // A linked spirit's attack on you makes its owner an enemy.
        s.note_player_spawn(3001);
        s.register_confirmed_summon_by_id(600, 3001);
        hit(&s, 600, 2259, 100_011, 4_000);
        assert!(s.is_enemy(3001));
    }
}
