//! Entities: mobs, bosses, dummies, the dead, summons and their owners, low ids.

use std::collections::{HashMap, HashSet};

use crate::entity::job_class::JobClass;
use crate::entity::owner_chain;

use super::damage::drop_ally_hits;
use super::enemies::after_link;
use super::{ActorTotals, DataStorage, Inner, TargetTotals};

/// The row that summons and effects nobody could be tied to share. Above the
/// entity ids (they end at 9,999,999) and below the party placeholder rows.
pub const UNATTRIBUTED_ID: i32 = 80_000_000;

/// Distinct class skills that make an actor a player. Summons and effects
/// linked to an owner deal damage with at most 4; unnamed players without a
/// player record, as in a world boss crowd, use 5 or more.
const PLAYER_SKILLS: usize = 5;

/// Ids below this are the damage parser's resync guard; the game hands some
/// out all the same, and those count once a spawn or identity record names them.
const LOW_IDS: std::ops::RangeInclusive<i32> = 1..=99;

/// Whether `id` is a monster: it spawned as an NPC and is no summon.
pub(super) fn is_monster(inner: &Inner, id: i32) -> bool {
    inner.mob_storage.contains_key(&id) && !inner.summon_storage.contains_key(&id)
}

// Monsters, bosses, dummies and the dead.
impl DataStorage {
    /// `mid` spawned as the NPC `code`. An id taken for a player until now
    /// was a monster all along: its damage leaves the fights. A fight left
    /// with no rows stays, unlike in the friendly-damage purge: it was a
    /// fight on a monster all the same (spec M1).
    pub fn append_mob(&self, mid: i32, code: i32) {
        let mut guard = self.inner.write();
        guard.mob_storage.insert(mid, code);
        if guard.known_player_ids.remove(&mid) {
            for fight in guard.target_combat.values_mut() {
                fight.remove_actors(|who| who == mid);
            }
            drop(guard);
            self.touch();
        }
    }

    /// The max HP of `mid`, when the game sends one.
    pub fn remember_enemy_hp(&self, mid: i32, hp: i32) {
        if hp <= 0 {
            return;
        }
        self.inner.write().mob_hp_data.insert(mid, hp);
    }

    /// The NPC code `id` spawned as.
    pub fn enemy_code(&self, id: i32) -> Option<i32> {
        self.inner.read().mob_storage.get(&id).copied()
    }

    /// Whether `id` spawned as an NPC.
    pub fn is_mob(&self, id: i32) -> bool {
        self.enemy_code(id).is_some()
    }

    /// Entity id to NPC code, for every NPC seen spawning.
    pub fn enemy_code_map(&self) -> HashMap<i32, i32> {
        self.inner.read().mob_storage.clone()
    }

    /// The HP `id` has now. Its max HP is raised to it when lower or unknown.
    pub fn update_enemy_hp(&self, id: i32, hp: i32) {
        if hp.is_negative() {
            return;
        }
        let mut guard = self.inner.write();
        let max = guard.mob_hp_data.get(&id).map_or(hp, |&before| before.max(hp));
        guard.mob_hp_data.insert(id, max);
        guard.mob_current_hp.insert(id, hp);
    }

    pub fn enemy_max_hp(&self, id: i32) -> Option<i32> {
        self.inner.read().mob_hp_data.get(&id).copied()
    }

    pub fn enemy_hp_now(&self, id: i32) -> Option<i32> {
        self.inner.read().mob_current_hp.get(&id).copied()
    }

    /// Whether `id` has a live fight (not one kept for the encounter).
    pub fn was_hit(&self, id: i32) -> bool {
        self.inner.read().target_combat.contains_key(&id)
    }

    pub fn register_boss(&self, entity_id: i32) {
        self.inner.write().boss_entity_ids.insert(entity_id);
    }

    pub fn is_boss(&self, entity_id: i32) -> bool {
        self.inner.read().boss_entity_ids.contains(&entity_id)
    }

    pub fn note_death(&self, entity_id: i32) {
        let first_time = self.inner.write().dead_entity_ids.insert(entity_id);
        if first_time {
            self.touch();
        }
    }

    pub fn has_died(&self, entity_id: i32) -> bool {
        self.inner.read().dead_entity_ids.contains(&entity_id)
    }

    pub fn flag_dummy(&self, entity_id: i32) {
        self.inner.write().training_dummy_ids.insert(entity_id);
    }
}

// Summons, players and their ids.
impl DataStorage {
    /// Whether `id` has an owner.
    pub fn is_pet(&self, id: i32) -> bool {
        self.inner.read().summon_storage.contains_key(&id)
    }

    /// `id` left the world. Only a summon's leaving matters: see
    /// `despawned_summon_ids`.
    pub fn note_despawn(&self, id: i32) {
        let mut guard = self.inner.write();
        if guard.summon_storage.contains_key(&id) {
            guard.despawned_summon_ids.insert(id);
        }
    }

    pub fn pet_is_confirmed(&self, id: i32) -> bool {
        self.inner.read().confirmed_summon_ids.contains(&id)
    }

    /// A summon spawned under `id`: a new entity, whatever had the id before.
    /// It stays a player only when the game itself named it one.
    pub fn remember_pet_spawn(&self, id: i32) {
        let mut guard = self.inner.write();
        let state = &mut *guard;
        forget_entity(state, id);
        let named_by_game = state.authoritative_name_ids.contains(&id);
        for set in [&mut state.player_spawn_ids, &mut state.enemy_ids] {
            set.remove(&id);
        }
        if !named_by_game {
            state.known_player_ids.remove(&id);
        }
        state.summon_spawn_ids.insert(id);
    }

    /// A player spawned under `id`: a new entity.
    pub fn note_player_spawn(&self, id: i32) {
        let mut guard = self.inner.write();
        forget_entity(&mut guard, id);
        guard.player_spawn_ids.insert(id);
    }

    /// A record only players get named `id`. Unlike a spawn it says nothing
    /// of a new entity, so links stay.
    pub fn note_player_record(&self, id: i32) {
        self.inner.write().player_spawn_ids.insert(id);
    }

    pub fn pet_spawn_ids(&self) -> HashSet<i32> {
        let state = self.inner.read();
        state.summon_spawn_ids.iter().copied().collect()
    }

    /// A spawn or identity record named `id`; under 100 it counts from now on.
    pub fn remember_small_id(&self, id: i32) {
        if LOW_IDS.contains(&id) {
            self.inner.write().low_id_entities.insert(id);
        }
    }

    /// Whether `id` can be an entity: 100 and up, or a low id seen named.
    pub fn looks_like_entity(&self, id: i32) -> bool {
        match id {
            100.. => true,
            low if LOW_IDS.contains(&low) => self.inner.read().low_id_entities.contains(&low),
            _ => false,
        }
    }

    /// A record tied `summon_id` to `owner_id` for certain.
    pub fn confirm_pet(&self, summon_id: i32, owner_id: i32) {
        let mut guard = self.inner.write();
        if link_summon(&mut guard, summon_id, owner_id) {
            drop(guard);
            self.touch();
        }
    }

    /// Give `summon` to `summoner` on weaker grounds than a link record. The
    /// parser calls it for confirmed summons only, so it moves a link at
    /// most. Refused when either side looks like something else: the summon
    /// a player or someone's target, the summoner a summon or a monster, or
    /// the two of different classes.
    pub fn link_pet_owner(&self, summoner: i32, summon: i32) {
        let mut guard = self.inner.write();
        if !may_own(&guard, summoner, summon) {
            return;
        }
        let moved = guard.summon_storage.insert(summon, summoner) != Some(summoner);
        after_link(&mut guard, summoner);
        if moved {
            drop(guard);
            self.touch();
        }
    }

    /// Every actor's owner: see `owners`.
    pub fn pet_owners(&self) -> HashMap<i32, i32> {
        let state = self.inner.read();
        owners(&state)
    }
}

/// Whether `summoner` may own `summon` on the parser's guess. Not when the
/// summon shows as a player (a name, a class skill) or was hit by one, the
/// summoner is itself a summon or an NPC, or both used class skills of
/// different classes.
fn may_own(state: &Inner, summoner: i32, summon: i32) -> bool {
    let summon_is_player = state.nickname_storage.contains_key(&summon) || state.known_player_ids.contains(&summon);
    let summoner_is_pet_or_mob =
        state.summon_storage.contains_key(&summoner) || state.mob_storage.contains_key(&summoner);
    let classes = (state.actor_jobs.get(&summoner), state.actor_jobs.get(&summon));
    let classes_differ = matches!(classes, (Some(mine), Some(its)) if mine != its);
    !(summon_is_player || summoner_is_pet_or_mob || classes_differ || state.hostile_target_ids.contains(&summon))
}

/// The class skills an actor used.
#[derive(Debug, Clone, Default)]
pub(super) struct SkillUse {
    classes: Vec<JobClass>,
    /// The first `PLAYER_SKILLS` distinct ones.
    skills: Vec<i32>,
}

impl SkillUse {
    pub(super) fn add(&mut self, job: JobClass, skill: i32) {
        if !self.classes.contains(&job) {
            self.classes.push(job);
        }
        if self.skills.len() < PLAYER_SKILLS && !self.skills.contains(&skill) {
            self.skills.push(skill);
        }
    }

    /// The class, while every skill was of one.
    fn class(&self) -> Option<JobClass> {
        match self.classes.as_slice() {
            [job] => Some(*job),
            _ => None,
        }
    }
}

/// The summon links, plus an owner for every actor that dealt damage with
/// class skills and is neither linked nor a player. Those are summons and
/// effects: the server sends no spawn for other players' effects in a crowd,
/// so the only packets naming them are their own damage records. Without
/// an owner, each would show as a player of its own named by its id, and
/// a2tools.app refuses such a log as over raid size.
///
/// The owner is your party's one member of the effect's class when no
/// other player of that class fought where the effect hit (see
/// `party_owner`); else `UNATTRIBUTED_ID`, one row for them all.
pub(super) fn owners(inner: &Inner) -> HashMap<i32, i32> {
    owners_in(inner, inner.target_combat.values().chain(inner.encounter_carry.values()))
}

/// `owners`, judging who fought where an effect hit by `fights`.
pub(super) fn owners_in<'a>(inner: &Inner, fights: impl Iterator<Item = &'a TargetTotals>) -> HashMap<i32, i32> {
    let mut out = inner.summon_storage.clone();
    let owners: HashSet<i32> = inner.summon_storage.values().copied().collect();
    let present = players_beside_effects(inner, &owners, fights);
    for (&id, used) in &inner.actor_skills {
        if is_effect(inner, &owners, id, used) {
            let owner = used
                .class()
                .and_then(|job| party_owner(inner, job, present.get(&id).and_then(|p| p.get(&job))))
                .filter(|&owner| owner != id);
            out.insert(id, owner.unwrap_or(UNATTRIBUTED_ID));
        }
    }
    out
}

/// Linked to no one, and no sign of a player: a name, a player record, your
/// own id, a summon of its own (`owners`), or a player's rotation.
fn is_effect(inner: &Inner, owners: &HashSet<i32>, id: i32, used: &SkillUse) -> bool {
    !inner.summon_storage.contains_key(&id)
        && !inner.nickname_storage.contains_key(&id)
        && !inner.player_spawn_ids.contains(&id)
        && inner.local_player_id != Some(id as i64)
        && used.skills.len() < PLAYER_SKILLS
        && !owners.contains(&id)
}

/// The entity of your party's one member of class `job`, when the players of
/// that class who fought where the effect hit (`present`) are that member
/// alone. Anyone else of the class could be the caster: two party members
/// of it, or a stranger nearby. Without that check, strangers' summons and
/// effects go to the party's member of their class.
fn party_owner(inner: &Inner, job: JobClass, present: Option<&Beside>) -> Option<i32> {
    if inner.party_members.len() < 2 {
        return None;
    }
    let mut of_class = inner.party_members.iter().filter(|(_, m)| m.job == Some(job));
    let (name, _) = of_class.next()?;
    if of_class.next().is_some() {
        return None;
    }
    // The lowest id of the name, not whichever the map yields first.
    let member = inner.nickname_storage.iter().filter(|(_, n)| n.trim() == name.trim()).map(|(&id, _)| id).min()?;
    (present == Some(&Beside::One(member))).then_some(member)
}

/// The players of one class who fought where an effect hit: one, or more.
#[derive(Clone, Copy, PartialEq)]
enum Beside {
    One(i32),
    Many,
}

/// Adds `more` to the players of `job` in `present`.
fn add_beside(present: &mut HashMap<JobClass, Beside>, job: JobClass, more: Beside) {
    present.entry(job).and_modify(|was| if *was != more { *was = Beside::Many }).or_insert(more);
}

/// For each effect, the players of each class in the fights it hit: players
/// by their own skills, and the owners of linked summons by theirs.
fn players_beside_effects<'a>(
    inner: &Inner,
    owners: &HashSet<i32>,
    fights: impl Iterator<Item = &'a TargetTotals>,
) -> HashMap<i32, HashMap<JobClass, Beside>> {
    let mut out: HashMap<i32, HashMap<JobClass, Beside>> = HashMap::new();
    for fight in fights {
        let mut players: HashMap<JobClass, Beside> = HashMap::new();
        let mut effects = Vec::new();
        for &actor in fight.actors.keys() {
            let Some(used) = inner.actor_skills.get(&actor) else { continue };
            let player = if inner.summon_storage.contains_key(&actor) {
                owner_chain::resolve(actor, &inner.summon_storage)
            } else if is_effect(inner, owners, actor, used) {
                effects.push(actor);
                continue;
            } else {
                actor
            };
            for &job in &used.classes {
                add_beside(&mut players, job, Beside::One(player));
            }
        }
        for effect in effects {
            let present = out.entry(effect).or_default();
            for (&job, &more) in &players {
                add_beside(present, job, more);
            }
        }
    }
    out
}

/// Skill codes of the records a Spiritmaster's spirit sends its owner (about
/// once a second) and the owner sends its spirits. Damage-shaped `04 38`
/// records, but the amount is no damage, and the pair is the most reliable
/// owner link there is: no spirit linked this way has been seen with two
/// owners in one lifetime. Seen: 16990002,
/// 16990003 and 16770000.
const SPIRIT_TO_OWNER: std::ops::RangeInclusive<i32> = 16_990_000..=16_999_999;
const OWNER_TO_SPIRIT: std::ops::RangeInclusive<i32> = 16_770_000..=16_779_999;

/// `(summon, owner)` if this record is a link record.
pub(super) fn owner_link(skill_code: i32, actor_id: i32, target_id: i32) -> Option<(i32, i32)> {
    if SPIRIT_TO_OWNER.contains(&skill_code) {
        Some((actor_id, target_id))
    } else if OWNER_TO_SPIRIT.contains(&skill_code) {
        Some((target_id, actor_id))
    } else {
        None
    }
}

/// Link `summon` to `owner` as a confirmed summon. Returns whether anything
/// changed: the link records repeat every second.
pub(super) fn link_summon(inner: &mut Inner, summon: i32, owner: i32) -> bool {
    if summon <= 0 || owner <= 0 || summon == owner {
        return false;
    }
    if inner.summon_storage.get(&summon) == Some(&owner) && inner.confirmed_summon_ids.contains(&summon) {
        return false;
    }
    // A link through the summon back to itself would hide both.
    if owner_chain::resolve(owner, &inner.summon_storage) == summon {
        return false;
    }
    // Another owner means another entity under a reused id whose spawn went
    // unseen: what the old one did stays with its owner.
    if inner.summon_storage.get(&summon).is_some_and(|&old| old != owner) {
        forget_entity(inner, summon);
    }
    inner.confirmed_summon_ids.insert(summon);
    inner.known_player_ids.remove(&summon);
    inner.summon_storage.insert(summon, owner);
    drop_ally_hits(inner);
    after_link(inner, owner);
    true
}

/// A new entity under `id`: drop the old one's owner link and summon marks.
/// What a linked summon did moves onto its owner, where it was shown anyway,
/// so the new entity's owner does not inherit it.
fn forget_entity(inner: &mut Inner, id: i32) {
    inner.despawned_summon_ids.remove(&id);
    inner.confirmed_summon_ids.remove(&id);
    inner.summon_spawn_ids.remove(&id);
    inner.actor_jobs.remove(&id);
    inner.hostile_target_ids.remove(&id);
    inner.effect_parents.remove(&id);
    let Some(owner) = inner.summon_storage.remove(&id) else { return };
    // Only a linked actor's skills go. A spawn followed an unlinked actor's
    // first hit by 100 ms on a scarecrow, most likely
    // the same entity, so the link that follows takes what it did.
    inner.actor_skills.remove(&id);
    let owner = owner_chain::resolve(owner, &inner.summon_storage);
    if owner <= 0 || owner == id {
        return;
    }
    let seq = inner.next_seq();
    for target in inner.target_combat.values_mut() {
        if let Some(data) = target.actors.remove(&id) {
            let kept = target.actors.entry(owner).or_insert_with(ActorTotals::new);
            kept.absorb(data);
            kept.seq = seq;
        }
    }
    for tick in inner.heal_ticks.iter_mut().filter(|t| t.actor == id) {
        tick.actor = owner;
    }
    inner.heal_sums.moved(id, owner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::data_storage::damage::{rec, tick};
    use crate::combat::data_storage::tests::{dealt, member_as, seat};
    use crate::entity::job_class::JobClass::{Cleric, Sorcerer};

    const N1: &str = "Namone";
    const N2: &str = "Nametwo";
    const N3: &str = "Namethree";

    #[test]
    fn a_player_who_spawns_as_a_monster_loses_their_rows() {
        let s = DataStorage::new();
        rec(&s, 700, 900, 11_010_000, 300, 1_000);
        rec(&s, 700, 901, 11_010_000, 200, 1_000);
        rec(&s, 2000, 900, 11_010_000, 100, 1_000);
        let before = s.damage_generation();
        s.append_mob(700, 2_310_403);
        assert!(!s.seen_as_player(700));
        assert_eq!(s.damage_generation(), before + 1);
        let live = s.copy_targets();
        assert_eq!(live[&900].total_damage, 100);
        assert_eq!(live[&900].actors.keys().copied().collect::<Vec<_>>(), vec![2000]);
        assert_eq!((live[&901].total_damage, live[&901].actors.len()), (0, 0));
        assert_eq!((live[&901].first_damage_time, live[&901].last_damage_time), (1_000, 1_000));
        assert!(s.was_hit(901));
        assert_eq!(s.enemy_code(700), Some(2_310_403));
        assert!(s.is_mob(700));
        s.append_mob(701, 1);
        assert_eq!(s.damage_generation(), before + 1, "an id nobody knew");
        s.append_mob(701, 2);
        assert_eq!(s.enemy_code(701), Some(2));
        assert_eq!(s.enemy_code_map(), HashMap::from([(700, 2_310_403), (701, 2)]));
    }

    #[test]
    fn max_hp_takes_any_positive_value() {
        let s = DataStorage::new();
        s.remember_enemy_hp(900, 5000);
        s.remember_enemy_hp(900, 3000);
        assert_eq!(s.enemy_max_hp(900), Some(3000));
        s.remember_enemy_hp(900, 0);
        s.remember_enemy_hp(900, -4);
        s.remember_enemy_hp(901, 0);
        assert_eq!((s.enemy_max_hp(900), s.enemy_max_hp(901)), (Some(3000), None));
    }

    #[test]
    fn current_hp_raises_the_max_hp() {
        let s = DataStorage::new();
        s.update_enemy_hp(900, -1);
        assert_eq!((s.enemy_hp_now(900), s.enemy_max_hp(900)), (None, None));
        s.update_enemy_hp(901, 0);
        assert_eq!((s.enemy_hp_now(901), s.enemy_max_hp(901)), (Some(0), Some(0)));
        s.remember_enemy_hp(902, 5000);
        s.update_enemy_hp(902, 7000);
        assert_eq!(s.enemy_max_hp(902), Some(7000));
        s.update_enemy_hp(902, 3000);
        assert_eq!((s.enemy_hp_now(902), s.enemy_max_hp(902)), (Some(3000), Some(7000)));
    }

    #[test]
    fn a_death_moves_the_generation_once() {
        let s = DataStorage::new();
        s.note_death(900);
        s.note_death(900);
        assert!(s.has_died(900) && !s.has_died(901));
        assert_eq!(s.damage_generation(), 1);
    }

    #[test]
    fn only_a_linked_summon_is_marked_gone() {
        let s = DataStorage::new();
        s.note_despawn(600);
        tick(&s, 600, 900, 16_010_000, 50, 1_000);
        assert_eq!(dealt(&s, 900, 600), 50);
    }

    #[test]
    fn boss_dummy_and_summon_spawn_marks_read_back() {
        let s = DataStorage::new();
        s.register_boss(800);
        s.flag_dummy(500);
        assert!(s.is_boss(800) && !s.is_boss(801) && !s.is_boss(500));
        assert_eq!(s.damage_generation(), 0);
        assert!(s.inner.read().training_dummy_ids.contains(&500));
        s.remember_pet_spawn(502);
        assert_eq!(s.pet_spawn_ids(), HashSet::from([502]));
        assert!(!s.is_pet(502) && !s.pet_is_confirmed(502));
        rec(&s, 1000, 900, 11_010_000, 100, 1_000);
        assert!(s.was_hit(900) && !s.was_hit(1000));
        assert!(!s.is_mob(900));
    }

    #[test]
    fn a_summon_spawn_is_a_new_entity_that_is_no_player() {
        let s = DataStorage::new();
        rec(&s, 700, 900, 11_010_000, 100, 1_000);
        s.remember_pet_spawn(700);
        assert!(!s.seen_as_player(700));
        assert!(s.pet_spawn_ids().contains(&700));

        s.name_entity(701, "Named");
        s.remember_pet_spawn(701);
        assert!(s.seen_as_player(701), "the game named it");

        s.note_player_record(702);
        rec(&s, 2000, 702, 11_010_000, 100, 1_000);
        assert!(!s.was_hit(702));
        s.remember_pet_spawn(702);
        rec(&s, 2000, 702, 11_010_000, 100, 1_100);
        assert!(s.was_hit(702));

        s.confirm_pet(500, 100);
        rec(&s, 500, 901, 16_010_000, 300, 1_000);
        s.remember_pet_spawn(500);
        assert!(!s.is_pet(500) && !s.pet_is_confirmed(500));
        assert_eq!((dealt(&s, 901, 100), dealt(&s, 901, 500)), (300, 0));
    }

    #[test]
    fn a_summon_spawn_ends_an_enemy_mark() {
        let s = DataStorage::new();
        s.set_local_player_id(Some(2259));
        rec(&s, 3000, 2259, 11_010_000, 100, 1_000);
        assert!(s.is_enemy(3000));
        s.remember_pet_spawn(3000);
        assert!(!s.is_enemy(3000));
        s.note_player_spawn(3001);
        s.remember_pet_spawn(3001);
        assert!(!s.inner.read().player_spawn_ids.contains(&3001));
    }

    #[test]
    fn a_player_spawn_drops_the_old_link() {
        let s = DataStorage::new();
        s.confirm_pet(500, 200);
        rec(&s, 500, 900, 16_010_000, 50, 1_000);
        s.note_player_spawn(500);
        assert!(!s.is_pet(500));
        assert_eq!(dealt(&s, 900, 200), 50);
        assert!(s.inner.read().player_spawn_ids.contains(&500));
    }

    #[test]
    fn a_player_record_keeps_links() {
        let s = DataStorage::new();
        s.confirm_pet(500, 100);
        s.note_player_record(500);
        assert_eq!(s.pet_owners().get(&500), Some(&100));
    }

    #[test]
    fn low_ids_are_entities_once_named() {
        let s = DataStorage::new();
        assert!(s.looks_like_entity(100));
        assert!(!s.looks_like_entity(99));
        s.remember_small_id(99);
        assert!(s.looks_like_entity(99));
        s.remember_small_id(0);
        assert!(!s.looks_like_entity(0));
        s.remember_small_id(-5);
        assert!(!s.looks_like_entity(-5));
        s.remember_small_id(100);
        assert!(!s.inner.read().low_id_entities.contains(&100));
    }

    #[test]
    fn a_confirmed_link_moves_the_generation_when_new() {
        let s = DataStorage::new();
        s.confirm_pet(500, 100);
        assert_eq!(s.damage_generation(), 1);
        s.confirm_pet(500, 100);
        s.confirm_pet(501, 501);
        assert_eq!(s.damage_generation(), 1);
    }

    /// A storage where 2000 is a known Gladiator, then `setup`, then the
    /// guessed link of 600 to 2000. Whether it took.
    fn guessed_link(setup: impl Fn(&DataStorage)) -> bool {
        let s = DataStorage::new();
        rec(&s, 2000, 900, 11_010_000, 100, 500);
        setup(&s);
        s.link_pet_owner(2000, 600);
        s.is_pet(600)
    }

    #[test]
    fn a_guessed_link_is_refused_on_any_sign_against_it() {
        assert!(!guessed_link(|s| s.name_entity(600, "Named")));
        assert!(!guessed_link(|s| rec(s, 600, 900, 11_010_000, 100, 600)), "600 is a player");
        assert!(!guessed_link(|s| rec(s, 2000, 600, 11_010_000, 100, 600)), "a player hit 600");
        assert!(!guessed_link(|s| s.confirm_pet(2000, 3000)), "2000 is a summon");
        assert!(!guessed_link(|s| s.append_mob(2000, 1)), "2000 is a monster");
        assert!(!guessed_link(|s| {
            s.remember_pet_spawn(600);
            rec(s, 600, 900, 15_010_000, 100, 600);
        }), "a Sorcerer's summon for a Gladiator");
        assert!(guessed_link(|_| {}));
    }

    #[test]
    fn a_guessed_link_of_the_same_class_takes() {
        let s = DataStorage::new();
        rec(&s, 2000, 900, 15_020_000, 100, 500);
        s.remember_pet_spawn(600);
        rec(&s, 600, 900, 15_010_000, 100, 600);
        s.link_pet_owner(2000, 600);
        assert_eq!(s.pet_owners().get(&600), Some(&2000));
    }

    #[test]
    fn a_guessed_link_moves_nothing_else() {
        let s = DataStorage::new();
        rec(&s, 2000, 900, 11_010_000, 100, 500);
        let before = s.damage_generation();
        s.link_pet_owner(2000, 600);
        assert!(s.is_pet(600) && !s.pet_is_confirmed(600));
        assert_eq!(s.pet_owners().get(&600), Some(&2000));
        assert_eq!(s.damage_generation(), before + 1, "a new link");
        s.link_pet_owner(2000, 600);
        assert_eq!(s.damage_generation(), before + 1, "the same link again");

        let s = DataStorage::new();
        rec(&s, 2000, 900, 11_010_000, 100, 500);
        s.confirm_pet(600, 3000);
        s.link_pet_owner(2000, 600);
        assert_eq!(s.pet_owners().get(&600), Some(&2000));
        assert!(s.pet_is_confirmed(600), "still confirmed");
    }

    #[test]
    fn a_guessed_link_to_an_enemy_takes_the_summon_s_damage_out() {
        let s = DataStorage::new();
        s.set_local_player_id(Some(2259));
        rec(&s, 2000, 900, 11_010_000, 100, 500);
        s.remember_pet_spawn(600);
        rec(&s, 600, 900, 100_011, 300, 1_000);
        rec(&s, 3000, 2259, 11_010_000, 100, 2_000);
        assert!(s.is_enemy(3000));
        assert_eq!(dealt(&s, 900, 600), 300);
        s.link_pet_owner(3000, 600);
        assert_eq!((dealt(&s, 900, 600), dealt(&s, 900, 2000)), (0, 100));
        assert_eq!(s.copy_targets()[&900].total_damage, 100);
    }

    #[test]
    fn an_effect_goes_to_the_lowest_id_under_its_member_s_name() {
        let s = DataStorage::new();
        seat(&s, &[(N1, member_as(1, Sorcerer)), (N2, member_as(2, Cleric))]);
        for id in [150, 101] {
            s.inner.write().nickname_storage.insert(id, N2.to_owned());
        }
        rec(&s, 101, 900, 17_010_000, 500, 1_000);
        rec(&s, 700, 900, 17_020_000, 300, 1_100);
        assert_eq!(s.pet_owners().get(&700), Some(&101));
    }

    fn owner_of(s: &DataStorage, id: i32) -> Option<i32> {
        s.pet_owners().get(&id).copied()
    }

    /// A Sorcerer (100, you) and a Cleric (101), both on target 900.
    fn mixed_party() -> DataStorage {
        let s = DataStorage::new();
        s.set_local_player_id(Some(100));
        s.name_entity(100, N1);
        s.name_entity(101, N2);
        seat(&s, &[(N1, member_as(1, Sorcerer)), (N2, member_as(2, Cleric))]);
        rec(&s, 100, 900, 15_010_000, 500, 1_000);
        rec(&s, 101, 900, 17_010_000, 500, 1_000);
        s
    }

    #[test]
    fn link_hits_link_and_add_no_damage() {
        let s = DataStorage::new();
        s.name_entity(100, N1);
        rec(&s, 500, 900, 16_010_000, 300, 1_000);
        assert!(!s.is_pet(500));
        rec(&s, 500, 100, 16_990_002, 20, 1_500);
        assert!(s.pet_is_confirmed(500) && owner_of(&s, 500) == Some(100));
        rec(&s, 100, 501, 16_770_000, 197, 1_600);
        assert_eq!(Some(100), owner_of(&s, 501));
        let all = s.copy_targets();
        let mut hit: Vec<i32> = all.keys().copied().collect();
        hit.sort();
        assert_eq!(hit, [900]);
        assert_eq!(300, all[&900].total_damage);
        assert_eq!(s.heals_between(i64::MIN, i64::MAX).len(), 0);
        assert!(!s.seen_as_player(500));
    }

    #[test]
    fn effect_left_unattributed_when_ambiguous() {
        let effect_hit = |s: &DataStorage| {
            rec(s, 700, 900, 17_020_000, 300, 1_100);
            assert_eq!(Some(UNATTRIBUTED_ID), owner_of(s, 700));
        };

        // A second Cleric outside the party.
        let s = mixed_party();
        s.name_entity(202, N3);
        rec(&s, 202, 900, 17_010_000, 400, 1_050);
        effect_hit(&s);

        // A second Cleric on another mob the effect hit too.
        let s = mixed_party();
        s.name_entity(202, N3);
        rec(&s, 202, 901, 17_010_000, 400, 1_050);
        rec(&s, 700, 901, 17_020_000, 300, 1_060);
        effect_hit(&s);

        // A Cleric skill used by someone's summon.
        let s = mixed_party();
        s.confirm_pet(600, 303);
        rec(&s, 600, 900, 17_030_000, 400, 1_050);
        effect_hit(&s);

        // Two Clerics on the roster.
        let s = mixed_party();
        seat(&s, &[(N1, member_as(1, Sorcerer)), (N2, member_as(2, Cleric)), (N3, member_as(3, Cleric))]);
        effect_hit(&s);

        // A roster Cleric nobody has seen yet.
        let s = DataStorage::new();
        seat(&s, &[(N1, member_as(1, Sorcerer)), (N2, member_as(2, Cleric))]);
        effect_hit(&s);
    }

    #[test]
    fn a_new_spawn_under_an_id_starts_without_the_old_owner() {
        let s = DataStorage::new();
        rec(&s, 500, 100, 16_990_002, 20, 1_000);
        rec(&s, 500, 900, 16_010_000, 300, 1_100);
        // The id comes back as someone else's spirit.
        s.remember_pet_spawn(500);
        assert!(!s.is_pet(500));
        assert!(!s.pet_is_confirmed(500));
        assert_eq!(dealt(&s, 900, 100), 300, "the old spirit's damage stays its owner's");
        assert_eq!(dealt(&s, 900, 500), 0);
        rec(&s, 500, 200, 16_990_002, 20, 2_000);
        rec(&s, 500, 900, 16_010_000, 50, 2_100);
        assert_eq!(owner_of(&s, 500), Some(200));
        assert_eq!(dealt(&s, 900, 500), 50);
        assert_eq!(dealt(&s, 900, 100), 300);

        // A player spawn under a summon's id ends the summon too.
        s.note_player_spawn(500);
        assert!(!s.is_pet(500));
        assert_eq!(dealt(&s, 900, 200), 50);
    }

    #[test]
    fn a_link_to_another_owner_keeps_what_the_old_entity_did() {
        // The spawn between the two went unseen.
        let s = DataStorage::new();
        rec(&s, 500, 100, 16_990_002, 20, 1_000);
        rec(&s, 500, 900, 16_010_000, 300, 1_100);
        rec(&s, 200, 500, 16_770_000, 197, 5_000);
        assert_eq!(owner_of(&s, 500), Some(200));
        assert_eq!(dealt(&s, 900, 100), 300);
        assert_eq!(dealt(&s, 900, 500), 0);
    }

    #[test]
    fn a_summon_using_a_class_skill_is_no_player() {
        let s = DataStorage::new();
        s.confirm_pet(500, 100);
        rec(&s, 500, 900, 16_010_000, 300, 1_000);
        assert!(!s.seen_as_player(500));
        assert_eq!(owner_of(&s, 500), Some(100));

        // Spawned as a summon, owner not known yet.
        s.remember_pet_spawn(501);
        rec(&s, 501, 900, 16_010_000, 300, 1_000);
        assert!(!s.seen_as_player(501));

        // A link through the summon back to itself is refused.
        rec(&s, 100, 500, 16_990_002, 20, 1_000);
        assert_eq!(owner_of(&s, 100), None);
    }

    #[test]
    fn summon_links_survive_a_reset() {
        let s = DataStorage::new();
        rec(&s, 500, 100, 16_990_002, 20, 1_000);
        s.confirm_pet(501, 100);
        s.remember_pet_spawn(502);
        s.flush();
        assert_eq!([owner_of(&s, 500), owner_of(&s, 501)], [Some(100); 2]);
        assert!(s.pet_spawn_ids().contains(&502));
        rec(&s, 502, 900, 16_010_000, 300, 2_000);
        assert!(!s.seen_as_player(502), "still a summon after the reset");
    }

    #[test]
    fn an_effect_goes_to_the_partys_one_member_of_its_class() {
        let s = mixed_party();
        // Never spawned, never linked: only its damage names it.
        rec(&s, 700, 900, 17_020_000, 300, 1_100);
        assert_eq!(owner_of(&s, 700), Some(101));
        assert_eq!([owner_of(&s, 100), owner_of(&s, 101)], [None; 2]);

        // Another Cleric on another mob says nothing about this one.
        s.name_entity(202, "Stranger");
        rec(&s, 202, 901, 17_010_000, 400, 1_150);
        assert_eq!(owner_of(&s, 700), Some(101));

        // Skills of another class too: it could be anyone's.
        rec(&s, 700, 900, 15_020_000, 300, 1_200);
        assert_eq!(owner_of(&s, 700), Some(UNATTRIBUTED_ID));
    }

    #[test]
    fn a_player_is_no_effect_without_a_name() {
        let s = DataStorage::new();
        // Named by a player spawn, and still a player once a reset forgets the name.
        s.note_player_spawn(201);
        s.name_entity(201, "Spawned");
        // A player record with no name in it.
        s.note_player_record(202);
        rec(&s, 201, 900, 11_010_000, 100, 1_000);
        rec(&s, 202, 900, 11_010_000, 100, 1_000);
        // A player's rotation: five distinct skills.
        for k in 0..5 {
            rec(&s, 203, 900, 11_010_000 + k * 10_000, 100, 1_000);
        }
        // The owner of a linked summon, with one skill.
        s.confirm_pet(600, 204);
        rec(&s, 204, 900, 16_010_000, 100, 1_000);
        // Four skills and nothing else: an effect.
        for k in 0..4 {
            rec(&s, 700, 900, 11_010_000 + k * 10_000, 100, 1_000);
        }
        s.forget_names();
        let owners = s.pet_owners();
        for player in [201, 202, 203, 204] {
            assert_eq!(owners.get(&player), None, "{player} is a player");
        }
        assert_eq!(owners.get(&700), Some(&UNATTRIBUTED_ID));
    }

    #[test]
    fn an_effects_spawn_after_its_first_hits_leaves_them_for_its_link() {
        let s = DataStorage::new();
        rec(&s, 700, 900, 15_020_000, 300, 1_000);
        s.remember_pet_spawn(700);
        assert_eq!(owner_of(&s, 700), Some(UNATTRIBUTED_ID), "no link yet");
        s.confirm_pet(700, 100);
        rec(&s, 700, 900, 15_020_000, 50, 1_100);
        assert_eq!(owner_of(&s, 700), Some(100));
        assert_eq!(dealt(&s, 900, 700), 350, "all of it is the owner's");
    }
}
