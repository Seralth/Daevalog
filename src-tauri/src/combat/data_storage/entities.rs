//! Entities: mobs, bosses, dummies, the dead, summons and their owners, low ids.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;

use crate::entity::job_class::JobClass;
use crate::entity::summon_resolver;

use super::damage::purge_friendly_damage;
use super::{ActorCombatData, DataStorage, Inner, TargetCombatData};

impl DataStorage {
    pub fn append_mob(&self, mid: i32, code: i32) {
        let mut inner = self.inner.write();
        inner.mob_storage.insert(mid, code);

        // NPC unclassification: if this entity was previously classified as a player
        // (damage with player-band skills arrived before the 0x3640 spawn packet),
        // undo the classification and scrub ghost player damage from aggregates.
        if inner.known_player_ids.remove(&mid) {
            tracing::trace!("NPC unclassification: entity {} reclassified as mob (code {})", mid, code);
            // Subtract ghost player damage from target totals
            for target_data in inner.target_combat.values_mut() {
                if let Some(actor_data) = target_data.actors.remove(&mid) {
                    target_data.total_damage -= actor_data.total_damage;
                }
            }
            drop(inner);
            self.touch();
        }
    }

    pub fn append_mob_hp(&self, mid: i32, hp: i32) {
        if hp > 0 {
            self.inner.write().mob_hp_data.insert(mid, hp);
        }
    }

    pub fn mark_entity_dead(&self, entity_id: i32) {
        if self.inner.write().dead_entity_ids.insert(entity_id) {
            self.touch();
        }
    }

    /// `id` left the world. Only a linked summon is marked: see
    /// `Inner::despawned_summon_ids`.
    pub fn note_despawn(&self, id: i32) {
        let mut inner = self.inner.write();
        if inner.summon_storage.contains_key(&id) {
            inner.despawned_summon_ids.insert(id);
        }
    }

    pub fn is_entity_dead(&self, entity_id: i32) -> bool {
        self.inner.read().dead_entity_ids.contains(&entity_id)
    }

    pub fn register_boss(&self, entity_id: i32) {
        self.inner.write().boss_entity_ids.insert(entity_id);
    }

    /// An entity the NPC table calls a training dummy. See `held_dot_ticks`.
    pub fn register_training_dummy(&self, entity_id: i32) {
        self.inner.write().training_dummy_ids.insert(entity_id);
    }

    pub fn is_boss(&self, entity_id: i32) -> bool {
        self.inner.read().boss_entity_ids.contains(&entity_id)
    }

    pub fn is_mob(&self, id: i32) -> bool {
        self.inner.read().mob_storage.contains_key(&id)
    }

    pub fn is_damage_target(&self, id: i32) -> bool {
        self.inner.read().target_combat.contains_key(&id)
    }

    pub fn is_summon(&self, id: i32) -> bool {
        self.inner.read().summon_storage.contains_key(&id)
    }

    pub fn is_confirmed_summon(&self, id: i32) -> bool {
        self.inner.read().confirmed_summon_ids.contains(&id)
    }

    /// Record that `id` appeared in a `40/41 36` mob/summon spawn (never a player
    /// spawn). Used to keep summon / spell-effect entities out of the known-player
    /// set.
    ///
    /// The game reuses entity ids, so a spawn is a new entity: whatever owner,
    /// summon or player mark the id had belongs to the one before. Entity 65746
    /// was one player's pet and then, 14 minutes later, another Spiritmaster's
    /// spirit, whose 7,032 damage went to the first owner (2026-10-04).
    pub fn note_summon_spawn(&self, id: i32) {
        let mut inner = self.inner.write();
        forget_entity(&mut inner, id);
        // A name the game gave this id outranks a spawn read out of a scan.
        if !inner.authoritative_name_ids.contains(&id) {
            inner.known_player_ids.remove(&id);
        }
        inner.player_spawn_ids.remove(&id);
        inner.summon_spawn_ids.insert(id);
    }

    /// A `44/45 36` player spawn for `id`: a player, not anyone's summon.
    pub fn note_player_spawn(&self, id: i32) {
        let mut inner = self.inner.write();
        forget_entity(&mut inner, id);
        inner.player_spawn_ids.insert(id);
    }

    /// A `44/45 36` record about `id` with no name in it. Still a player: in
    /// the check kit's captures 103 unnamed actors had one, 92 of them using
    /// four or more class skills, and none of the 1,022 summons and effects
    /// linked to an owner did (2026-10-06). Read out of a scan, so it leaves
    /// the id's links alone.
    pub fn note_player_record(&self, id: i32) {
        self.inner.write().player_spawn_ids.insert(id);
    }

    pub fn get_summon_spawn_ids(&self) -> HashSet<i32> {
        self.inner.read().summon_spawn_ids.clone()
    }

    /// Confirm that a sub-100 entity id is a real entity (seen in a spawn or an
    /// identity record), so the damage parser's `>= 100` resync gate lets it
    /// through. See `Inner::low_id_entities`.
    pub fn note_low_id_entity(&self, id: i32) {
        if (1..100).contains(&id) {
            self.inner.write().low_id_entities.insert(id);
        }
    }

    /// True when `id` passes the entity-id sanity gate used while walking damage
    /// records: anything at or above the usual floor, plus tiny ids the game has
    /// explicitly announced.
    pub fn is_plausible_entity_id(&self, id: i32) -> bool {
        if id >= 100 {
            return true;
        }
        id >= 1 && self.inner.read().low_id_entities.contains(&id)
    }

    pub fn register_confirmed_summon_by_id(&self, summon_id: i32, owner_id: i32) {
        tracing::trace!("Summon confirmed (5F 00): {} owned by {}", summon_id, owner_id);
        if link_summon(&mut self.inner.write(), summon_id, owner_id) {
            self.damage_generation.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn append_summon(&self, summoner: i32, summon: i32) {
        let mut inner = self.inner.write();

        // Guards from Kotlin
        if inner.nickname_storage.contains_key(&summon) { return; }
        if inner.known_player_ids.contains(&summon) { return; }
        if inner.hostile_target_ids.contains(&summon) { return; }
        if inner.summon_storage.contains_key(&summoner) { return; }
        if inner.mob_storage.contains_key(&summoner) && !inner.summon_storage.contains_key(&summoner) { return; }

        // Job compatibility check
        let summon_job = inner.actor_jobs.get(&summon).copied();
        let owner_job = inner.actor_jobs.get(&summoner).copied();
        if let (Some(sj), Some(oj)) = (summon_job, owner_job) {
            if sj != oj { return; }
        }

        tracing::debug!("Summon linked: {} owned by {}", summon, summoner);
        inner.summon_storage.insert(summon, summoner);
    }

    /// Who owns each summon, for the views: the links, and the summons and
    /// effects no link names. See `owners`.
    pub fn get_summon_data(&self) -> HashMap<i32, i32> {
        owners(&self.inner.read())
    }

    pub fn get_mob_hp_data(&self) -> HashMap<i32, i32> {
        self.inner.read().mob_hp_data.clone()
    }

    pub fn get_mob_hp(&self, id: i32) -> Option<i32> {
        self.inner.read().mob_hp_data.get(&id).copied()
    }

    /// Record a live current-HP reading for an entity (from the `8D ... 02 01 00`
    /// feed). Also seeds/raises the entity's MAX HP from the observed peak, so a
    /// boss whose spawn packet was missed still gets a usable denominator (current
    /// HP never exceeds max in-game, so taking the max never overstates it).
    pub fn set_mob_current_hp(&self, id: i32, hp: i32) {
        if hp < 0 {
            return;
        }
        let mut inner = self.inner.write();
        inner.mob_current_hp.insert(id, hp);
        let max = inner.mob_hp_data.entry(id).or_insert(0);
        if hp > *max {
            *max = hp;
        }
    }

    pub fn get_mob_current_hp(&self, id: i32) -> Option<i32> {
        self.inner.read().mob_current_hp.get(&id).copied()
    }

    /// The NPC code entity `id` spawned as, if it is a known mob.
    pub fn mob_code(&self, id: i32) -> Option<i32> {
        self.inner.read().mob_storage.get(&id).copied()
    }

    pub fn get_mob_data(&self) -> HashMap<i32, i32> {
        self.inner.read().mob_storage.clone()
    }

    /// Drop every summon link, for a capture from another session (a replay),
    /// whose entity ids mean something else.
    pub fn forget_summon_links(&self) {
        let mut inner = self.inner.write();
        inner.summon_storage.clear();
        inner.confirmed_summon_ids.clear();
        inner.summon_spawn_ids.clear();
        inner.despawned_summon_ids.clear();
        inner.player_spawn_ids.clear();
        inner.actor_skills.clear();
    }
}

/// The row that summons and effects nobody could be tied to share. Above the
/// entity ids (they end at 9,999,999) and below the party placeholder rows.
pub const UNATTRIBUTED_ID: i32 = 80_000_000;

/// Distinct class skills that make an actor a player. Of 1,022 summons and
/// effects linked to an owner in the check kit's 11 captures, none dealt
/// damage with more than 4; 82 actors with no name and no player record
/// used 5 to 19, 59 of them in a world boss crowd (2026-10-06).
const PLAYER_SKILLS: usize = 5;

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
/// so the only packets naming them are their own damage records. In the
/// check kit's world boss capture they dealt 5.7% of the class-skill damage,
/// each shown as a player of its own named by its id, and a2tools.app
/// refused a log of that fight as over raid size (2026-10-05).
///
/// The owner is your party's one member of the effect's class when no
/// other player of that class fought where the effect hit (see
/// `party_owner`); else `UNATTRIBUTED_ID`, one row for them all.
pub(super) fn owners(inner: &Inner) -> HashMap<i32, i32> {
    owners_in(inner, inner.target_combat.values().chain(inner.encounter_carry.values()))
}

/// `owners`, judging who fought where an effect hit by `fights`.
pub(super) fn owners_in<'a>(inner: &Inner, fights: impl Iterator<Item = &'a TargetCombatData>) -> HashMap<i32, i32> {
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
/// of it, or a stranger nearby. Without that check, 114 summons and effects
/// linked to strangers in three of the check kit's captures would have gone
/// to the party's member of their class; with it, none would have, and 105
/// would have gone to their right owner (2026-10-06).
fn party_owner(inner: &Inner, job: JobClass, present: Option<&HashSet<i32>>) -> Option<i32> {
    if inner.party_members.len() < 2 {
        return None;
    }
    let mut of_class = inner.party_members.iter().filter(|(_, m)| m.job == Some(job));
    let (name, _) = of_class.next()?;
    if of_class.next().is_some() {
        return None;
    }
    let member = inner.nickname_storage.iter().find(|(_, n)| n.trim() == name.trim()).map(|(&id, _)| id)?;
    present.is_some_and(|ids| ids.len() == 1 && ids.contains(&member)).then_some(member)
}

/// For each effect, the players of each class in the fights it hit: players
/// by their own skills, and the owners of linked summons by theirs.
fn players_beside_effects<'a>(
    inner: &Inner,
    owners: &HashSet<i32>,
    fights: impl Iterator<Item = &'a TargetCombatData>,
) -> HashMap<i32, HashMap<JobClass, HashSet<i32>>> {
    let mut out: HashMap<i32, HashMap<JobClass, HashSet<i32>>> = HashMap::new();
    for fight in fights {
        let mut players: HashMap<JobClass, HashSet<i32>> = HashMap::new();
        let mut effects = Vec::new();
        for &actor in fight.actors.keys() {
            let Some(used) = inner.actor_skills.get(&actor) else { continue };
            let player = if inner.summon_storage.contains_key(&actor) {
                summon_resolver::resolve(actor, &inner.summon_storage)
            } else if is_effect(inner, owners, actor, used) {
                effects.push(actor);
                continue;
            } else {
                actor
            };
            for &job in &used.classes {
                players.entry(job).or_default().insert(player);
            }
        }
        for effect in effects {
            let present = out.entry(effect).or_default();
            for (job, ids) in &players {
                present.entry(*job).or_default().extend(ids);
            }
        }
    }
    out
}

/// Skill codes of the records a Spiritmaster's spirit sends its owner (about
/// once a second) and the owner sends its spirits. Damage-shaped `04 38`
/// records, but the amount is no damage, and the pair is the most reliable
/// owner link there is: across five captures (2026-10-04) 1,251 spirits were
/// linked this way and none to two owners in one lifetime. Seen: 16990002,
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
    if summon_resolver::resolve(owner, &inner.summon_storage) == summon {
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
    purge_friendly_damage(inner, summon);
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
    // first hit by 100 ms in a scarecrow capture (2026-10-05), most likely
    // the same entity, so the link that follows takes what it did.
    inner.actor_skills.remove(&id);
    let owner = summon_resolver::resolve(owner, &inner.summon_storage);
    if owner <= 0 || owner == id {
        return;
    }
    for target in inner.target_combat.values_mut() {
        if let Some(data) = target.actors.remove(&id) {
            target.actors.entry(owner).or_insert_with(ActorCombatData::new).absorb(data);
        }
    }
    for tick in inner.heal_ticks.iter_mut().filter(|t| t.actor == id) {
        tick.actor = owner;
    }
    let held: Vec<(i32, i32)> = inner.held_dot_ticks.keys().filter(|k| k.1 == id).copied().collect();
    for key in held {
        let mut ticks = inner.held_dot_ticks.remove(&key).unwrap_or_default();
        for t in &mut ticks {
            t.set_actor_id(owner);
        }
        inner.held_dot_ticks.entry((key.0, owner)).or_default().extend(ticks);
    }
}
