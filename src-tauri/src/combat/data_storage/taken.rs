//! Damage taken: monsters' hits on players, kept hit by hit so each fight
//! takes the hits taken during it, as healing does.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::Ordering;

use crate::entity::owner_chain;
use crate::entity::taken::{TakenHit, TakenStats};

use super::damage::is_player;
use super::entities::is_monster;
use super::{DataStorage, Inner, TargetTotals};

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
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TakenSkillData {
    pub stats: TakenStats,
    pub source_code: i32,
    /// Each hit with damage: when it landed and how much.
    pub hit_times: Vec<(i64, i64)>,
}

/// Damage taken per player, then per skill.
pub type TakenBy = HashMap<i32, HashMap<i32, TakenSkillData>>;

/// The damage taken over some fights per player, as `taken_on` adds it up,
/// kept between reads so a read counts only the hits since the last (see
/// `taken_per_player`). Who counts is judged at each read, so the hits from
/// the fights' targets stay apart from the rest.
#[derive(Default)]
pub struct TakenSums {
    /// The fights (target, first hit) and where counting starts: another
    /// start counts afresh.
    fights: Vec<(i32, i64)>,
    from: i64,
    /// The fights' last hit at the last read.
    until: i64,
    /// The newest hit looked at (`TakenTick::seq`).
    seen: u64,
    /// Hits after `until`, which a later hit on the fights may take in.
    later: Vec<TakenTick>,
    /// Per player: the hits from the fights' targets, then the rest.
    by: HashMap<i32, [Option<TakenStats>; 2]>,
}

impl TakenSums {
    /// Count the hits not looked at yet, and those waiting that `until` now
    /// takes in.
    fn count(&mut self, ticks: &VecDeque<TakenTick>, until: i64) {
        let start = ticks.partition_point(|t| t.seq <= self.seen);
        let waiting = std::mem::take(&mut self.later);
        for t in waiting.into_iter().chain(ticks.range(start..).copied()) {
            if t.hit.at > until {
                self.later.push(t);
            } else if t.hit.at >= self.from {
                let from_target = self.fights.iter().any(|&(target, _)| target == t.source);
                let part = &mut self.by.entry(t.hit.target).or_default()[usize::from(!from_target)];
                part.get_or_insert_default().add(&t.hit);
            }
        }
        self.seen = ticks.back().map_or(self.seen, |t| t.seq);
        self.until = until;
    }
}

impl DataStorage {
    /// Record a hit a player took. Hits on monsters and summons are left out:
    /// a player never spawns as either.
    pub fn append_taken(&self, hit: TakenHit) {
        if hit.target == hit.actor || !self.looks_like_entity(hit.target) {
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
    pub fn fight_taken(&self, fight: &TargetTotals) -> TakenBy {
        taken_in(&self.inner.read(), &[fight], i64::MIN)
    }

    /// The damage taken over the live fights against `targets` (with what a
    /// boss pull cleared of the encounter when `encounter`), from `since` on.
    pub fn taken_on(&self, targets: &[i32], encounter: bool, since: Option<i64>) -> TakenBy {
        let inner = self.inner.read();
        taken_in(&inner, &inner.fights_on(targets, encounter), since.unwrap_or(i64::MIN))
    }

    /// `taken_on`, summed over the skills per player, counting into `sums`
    /// only the hits since it was last read.
    pub fn taken_per_player(
        &self,
        sums: &mut TakenSums,
        targets: &[i32],
        encounter: bool,
        since: Option<i64>,
    ) -> HashMap<i32, TakenStats> {
        let inner = self.inner.read();
        let fights = inner.fights_on(targets, encounter);
        let Some((from, until)) = fights_span(&fights, since.unwrap_or(i64::MIN)) else { return HashMap::new() };
        let ids: Vec<(i32, i64)> = fights.iter().map(|f| (f.target_id, f.first_damage_time)).collect();
        if sums.fights != ids || sums.from != from || until < sums.until {
            *sums = TakenSums { fights: ids, from, ..TakenSums::default() };
        }
        sums.count(&inner.taken, until);
        let fighters = fighters_of(&inner, &fights);
        let counted = sums.by.iter().filter(|(player, _)| !inner.enemy_ids.contains(player));
        counted
            .filter_map(|(&player, [theirs, rest])| {
                let rest = rest.as_ref().filter(|_| takes_part(&inner, &fighters, player));
                let mut parts = theirs.iter().chain(rest);
                let mut all = *parts.next()?;
                parts.for_each(|more| all.absorb(more));
                Some((player, all))
            })
            .collect()
    }

    /// A `0x1C` skill-effect entity's spawn named `parent`. When that is a
    /// monster, what the effect hits is the monster's damage.
    pub fn note_effect_parent(&self, effect: i32, parent: i32) {
        let mut inner = self.inner.write();
        if parent == effect || !is_monster(&inner, parent) {
            inner.effect_parents.remove(&effect);
            return;
        }
        if inner.effect_parents.len() >= MAX_EFFECT_PARENTS {
            inner.effect_parents.clear();
        }
        inner.effect_parents.insert(effect, parent);
    }
}

/// Whether a hit on `id` is a player's damage taken: any sign of a player
/// wins; else anything that spawned as a monster or a summon, or has an
/// owner, is left out. An id seen nowhere counts.
fn takes_damage(inner: &Inner, id: i32) -> bool {
    if is_player(inner, id) || inner.nickname_storage.contains_key(&id) {
        return true;
    }
    let summon_or_npc = inner.summon_spawn_ids.contains(&id)
        || inner.confirmed_summon_ids.contains(&id)
        || inner.mob_storage.contains_key(&id)
        || inner.summon_storage.contains_key(&id);
    !summon_or_npc
}

/// The damage taken during `fights` (from the first one's first hit to the
/// last one's last, not before `since`) by the people in them: you, your
/// party, whoever hit one of the fights' targets, and whoever those targets
/// hit. Players nearby in another fight are left out, as for healing, and so
/// are enemies (see `enemies`).
pub(super) fn taken_in(inner: &Inner, fights: &[&TargetTotals], since: i64) -> TakenBy {
    let mut out = TakenBy::new();
    let Some((from, until)) = fights_span(fights, since) else { return out };
    let fighters = fighters_of(inner, fights);
    let targets: HashSet<i32> = fights.iter().map(|f| f.target_id).collect();
    for t in inner.taken.iter().filter(|t| (from..=until).contains(&t.hit.at)) {
        let player = t.hit.target;
        let in_it = targets.contains(&t.source) || takes_part(inner, &fighters, player);
        if in_it && !inner.enemy_ids.contains(&player) {
            add_tick(&mut out, t);
        }
    }
    out
}

/// From the fights' first hit, not before `since`, to their last.
fn fights_span(fights: &[&TargetTotals], since: i64) -> Option<(i64, i64)> {
    let from = fights.iter().map(|f| f.first_damage_time).min()?;
    let until = fights.iter().map(|f| f.last_damage_time).max().unwrap_or(from);
    Some((from.max(since), until))
}

/// Everyone who hit one of the fights' targets, summons as their owners.
fn fighters_of(inner: &Inner, fights: &[&TargetTotals]) -> HashSet<i32> {
    let actors = fights.iter().flat_map(|f| f.actors.keys());
    actors.map(|&a| owner_chain::resolve(a, &inner.summon_storage)).collect()
}

/// Whether the hits `player` takes count for fights `fighters` fought
/// whoever dealt them: one of the fighters, you, or your party.
fn takes_part(inner: &Inner, fighters: &HashSet<i32>, player: i32) -> bool {
    fighters.contains(&player)
        || inner.local_player_id.is_some_and(|l| l as i32 == player)
        || inner.nickname_storage.get(&player).is_some_and(|n| inner.party_members.contains_key(n.as_str()))
}

/// Count one hit into `out`.
pub fn add_tick(out: &mut TakenBy, t: &TakenTick) {
    let e = out.entry(t.hit.target).or_default().entry(t.hit.skill).or_default();
    e.stats.add(&t.hit);
    if t.hit.damage > 0 {
        e.hit_times.push((t.hit.at, t.hit.damage));
    }
    if e.source_code == 0 {
        e.source_code = t.source_code;
    }
}

/// Drop the hits from before every fight still open: a fight takes only the
/// hits taken during it, and ended fights took theirs when they were retired.
fn prune_taken(inner: &mut Inner) {
    let Some(newest) = inner.taken.back().map(|t| t.hit.at) else { return };
    let keep_from = inner.prune_cut(newest, TAKEN_SLACK_MS);
    while inner.taken.front().is_some_and(|t| t.hit.at < keep_from) {
        inner.taken.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::data_storage::damage::rec;
    use crate::entity::taken::TakenKind;

    fn hit(s: &DataStorage, actor: i32, target: i32, at: i64) {
        rec(s, actor, target, 11_010_000, 100, at);
    }

    fn taken(s: &DataStorage, actor: i32, target: i32, at: i64, damage: i64) {
        s.append_taken(TakenHit::plain(at, target, actor, 1_800_510, damage, TakenKind::Hit));
    }

    #[test]
    fn hits_on_monsters_and_summons_are_not_taken() {
        let s = DataStorage::new();
        s.append_mob(900, 2_310_403);
        s.remember_pet_spawn(950);
        taken(&s, 900, 950, 1_000, 10);
        taken(&s, 900, 901, 1_000, 10);
        s.append_mob(901, 2_310_403);
        taken(&s, 900, 901, 1_000, 10);
        let all: Vec<i32> = s.taken_since(0).iter().map(|t| t.hit.target).collect();
        assert_eq!(all, vec![901], "901 was no monster yet");
    }

    #[test]
    fn any_player_sign_outweighs_a_monster_or_summon_mark() {
        let s = DataStorage::new();
        s.append_mob(950, 1);
        s.note_player_record(950);
        s.confirm_pet(951, 1000);
        s.append_mob(952, 1);
        s.name_entity(952, "Named");
        s.append_mob(953, 1);
        s.set_local_player_id(Some(953));
        s.link_pet_owner(1000, 954);
        for id in [950, 951, 952, 953, 954, 955] {
            taken(&s, 900, id, 1_000, 10);
        }
        let all: Vec<i32> = s.taken_since(0).iter().map(|t| t.hit.target).collect();
        assert_eq!(all, vec![950, 952, 953, 955], "955 was never seen");
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
        s.remember_pet_spawn(24810);
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

    /// `taken_per_player` into `sums`, checked against `taken_on` added up.
    fn check_sums(s: &DataStorage, sums: &mut TakenSums, since: Option<i64>) {
        let mut kept: Vec<(i32, TakenStats)> = s.taken_per_player(sums, &[900], false, since).into_iter().collect();
        let mut full: Vec<(i32, TakenStats)> = s.taken_on(&[900], false, since)
            .into_iter()
            .map(|(player, skills)| {
                let mut all = TakenStats::default();
                skills.values().for_each(|d| all.absorb(&d.stats));
                (player, all)
            })
            .collect();
        kept.sort_by_key(|k| k.0);
        full.sort_by_key(|k| k.0);
        assert_eq!(kept, full);
    }

    #[test]
    fn sums_kept_between_reads_match_a_full_count() {
        let s = DataStorage::new();
        s.set_local_player_id(Some(1000));
        s.append_mob(900, 1);
        let mut sums = TakenSums::default();
        hit(&s, 1000, 900, 1_000);
        taken(&s, 900, 1000, 500, 1); // before the fight
        taken(&s, 900, 1000, 2_000, 10); // after its last hit, for now
        check_sums(&s, &mut sums, None);
        taken(&s, 900, 3000, 1_500, 40); // a stranger the boss hit, sent late
        hit(&s, 1000, 900, 2_500);
        check_sums(&s, &mut sums, None);
        taken(&s, 901, 2000, 2_400, 20); // nearby, another fight
        check_sums(&s, &mut sums, None);
        hit(&s, 2000, 900, 3_000); // and now in this one
        check_sums(&s, &mut sums, None);
        check_sums(&s, &mut sums, Some(2_000));
        check_sums(&s, &mut sums, None);
        assert_eq!(s.taken_per_player(&mut sums, &[900], false, None).len(), 3);
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
