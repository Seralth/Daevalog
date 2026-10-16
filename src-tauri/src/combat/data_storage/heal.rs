//! Healing done, per healer and skill.

use std::collections::{HashMap, HashSet, VecDeque};

use parking_lot::Mutex;

use super::{DataStorage, HealTotals, HealTick, Inner, TargetTotals};
use crate::entity::detail_bundle::HealAt;
use crate::entity::owner_chain;

/// Healing per healer, then per (skill, heal over time).
pub type HealsBy = HashMap<i32, HashMap<(i32, bool), HealTotals>>;

/// Heal ticks kept at most, a memory backstop only: ticks go when no open
/// fight needs them (`prune_heal_ticks`).
const MAX_HEAL_TICKS: usize = 1_000_000;
/// How often, in ticks, the ones no fight needs are dropped.
const PRUNE_EVERY: usize = 4096;
/// Ticks this recent are kept even with no fight open: a fight's first hit
/// can arrive after healing that landed during it.
const HEAL_SLACK_MS: i64 = 60_000;
/// Span sets whose sums are kept: the Heal and Tank tabs read two, every
/// listed target and the one Details opens on.
const KEPT_SUMS: usize = 4;

/// Heal sums kept between reads, per set of fight spans, so a read walks
/// only the ticks since the last (`sums_within`). A copy starts empty.
#[derive(Default)]
pub(super) struct HealSums(Mutex<Vec<Summed>>);

impl Clone for HealSums {
    fn clone(&self) -> Self {
        HealSums::default()
    }
}

impl HealSums {
    /// The oldest tick dropped: out of every sum that counted it.
    fn forget(&mut self, t: &HealTick) {
        for sums in self.0.get_mut().iter_mut().filter(|s| s.counted > 0) {
            sums.forget(t);
        }
    }

    /// `from`'s healing moved onto `to`, as its ticks were.
    pub(super) fn moved(&mut self, from: i32, to: i32) {
        for sums in self.0.get_mut() {
            let Some(by_skill) = sums.by.remove(&from) else { continue };
            let into = sums.by.entry(to).or_default();
            for (key, e) in by_skill {
                let sum = into.entry(key).or_default();
                sum.total_heal += e.total_heal;
                sum.tick_count = sum.tick_count.saturating_add(e.tick_count);
            }
        }
    }
}

/// The healing of the first `counted` ticks inside `spans`, by every healer,
/// without the ticks: who counts is judged at each read.
struct Summed {
    spans: Vec<(i64, i64)>,
    counted: usize,
    /// The latest tick counted: spans that agree up to it count them alike.
    upto: i64,
    by: HealsBy,
}

impl Summed {
    /// The sums that count ticks as `spans` do, new ones when none does.
    fn of<'a>(kept: &'a mut Vec<Summed>, spans: &[(i64, i64)]) -> &'a mut Summed {
        let at = kept.iter().position(|s| s.fits(spans)).unwrap_or_else(|| {
            if kept.len() >= KEPT_SUMS {
                kept.remove(0);
            }
            kept.push(Summed { spans: Vec::new(), counted: 0, upto: i64::MIN, by: HashMap::new() });
            kept.len() - 1
        });
        &mut kept[at]
    }

    /// Whether `spans` take or leave every tick counted as these did.
    fn fits(&self, spans: &[(i64, i64)]) -> bool {
        let upto = |s: &[(i64, i64)]| {
            s.iter().take_while(|s| s.0 <= self.upto).map(|&(from, to)| (from, to.min(self.upto))).collect::<Vec<_>>()
        };
        upto(&self.spans) == upto(spans)
    }

    /// Count the ticks up to the spans' end: a later one may yet fall in a
    /// span as the fight goes on.
    fn catch_up(&mut self, ticks: &VecDeque<HealTick>, spans: &[(i64, i64)]) {
        spans.clone_into(&mut self.spans);
        let end = spans.last().map_or(i64::MIN, |s| s.1);
        while let Some(t) = ticks.get(self.counted).filter(|t| t.at <= end) {
            if inside(spans, t.at) {
                add(&mut self.by, t);
            }
            self.upto = self.upto.max(t.at);
            self.counted += 1;
        }
    }

    /// Take back `t`, the oldest tick counted.
    fn forget(&mut self, t: &HealTick) {
        self.counted -= 1;
        if !inside(&self.spans, t.at) {
            return;
        }
        let Some(by_skill) = self.by.get_mut(&t.actor) else { return };
        let key = (t.skill, t.is_hot);
        if let Some(e) = by_skill.get_mut(&key) {
            e.total_heal -= t.amount;
            e.tick_count -= 1;
            if e.tick_count == 0 {
                by_skill.remove(&key);
            }
        }
        if by_skill.is_empty() {
            self.by.remove(&t.actor);
        }
    }
}

impl DataStorage {
    /// A heal tick of `amount` by `actor_id` on `target_id`, at `at`.
    pub fn add_heal_event(&self, actor_id: i32, target_id: i32, skill_code: i32, amount: i64, is_hot: bool, at: i64) {
        if amount <= 0 || !self.looks_like_entity(actor_id) {
            return;
        }
        let tick = HealTick { at, actor: actor_id, target: target_id, skill: skill_code, is_hot, amount };
        record_heal(&mut self.inner.write(), tick);
    }

    /// Healing done from `from_ms` to `to_ms`, by anyone.
    pub fn heals_between(&self, from_ms: i64, to_ms: i64) -> HashMap<i32, HashMap<(i32, bool), HealTotals>> {
        heals_between(&self.inner.read(), from_ms, to_ms)
    }

    /// The healing of several fights at once, each tick once: see `fights_heals`.
    pub fn fights_heals(&self, fights: &[&TargetTotals]) -> HealsBy {
        fights_heals(&self.inner.read(), fights, true)
    }

    /// `fights_heals` without the ticks, from sums kept between reads.
    pub fn fights_heal_sums(&self, fights: &[&TargetTotals]) -> HealsBy {
        fights_heals(&self.inner.read(), fights, false)
    }
}

/// The healing done during fights by the people in them: you, your party, and
/// whoever hit one of the fights' targets, their summons included. Players
/// nearby who only healed are someone else's fight (strangers at the next
/// training dummy would fill a solo dummy fight's HEAL), and an
/// enemy's healing is no part of any (see `enemies`). Several
/// fights take the ticks while any of them was being fought, each tick once,
/// the gaps between pulls left out as the fight time leaves them out.
/// Without `ticks`, only the sums.
pub(super) fn fights_heals(inner: &Inner, fights: &[&TargetTotals], ticks: bool) -> HealsBy {
    let fighters: HashSet<i32> = fights.iter()
        .flat_map(|f| f.actors.keys())
        .map(|&a| owner_chain::resolve(a, &inner.summon_storage))
        .collect();
    let spans = merged_spans(fights.iter().map(|f| (f.first_damage_time, f.last_damage_time)));
    // Judged per healer before their ticks are gathered: in a 400 against
    // 400 fight most ticks are strangers' and enemies', which would be
    // gathered and then dropped every half second.
    let ours = |actor: i32| {
        let owner = owner_chain::resolve(actor, &inner.summon_storage);
        !inner.enemy_ids.contains(&owner)
            && (fighters.contains(&owner)
            || inner.local_player_id.is_some_and(|l| l as i32 == owner)
            || inner.nickname_storage.get(&owner).is_some_and(|n| inner.party_members.contains_key(n.as_str())))
    };
    if ticks { heals_within(inner, &spans, ours) } else { sums_within(inner, &spans, ours) }
}

pub(super) fn record_heal(inner: &mut Inner, tick: HealTick) {
    inner.heal_ticks.push_back(tick);
    if inner.heal_ticks.len() % PRUNE_EVERY == 0 {
        prune_heal_ticks(inner);
    }
    if inner.heal_ticks.len() > MAX_HEAL_TICKS {
        drop_oldest(inner);
    }
}

/// Drop the oldest tick, from the kept sums too.
fn drop_oldest(inner: &mut Inner) {
    if let Some(t) = inner.heal_ticks.pop_front() {
        inner.heal_sums.forget(&t);
    }
}

/// Drop the ticks from before every fight still open: a fight takes only the
/// healing done during it (`heals_between`), and ended fights took theirs
/// when they were retired.
fn prune_heal_ticks(inner: &mut Inner) {
    let Some(newest) = inner.heal_ticks.back().map(|t| t.at) else { return };
    let keep_from = inner.prune_cut(newest, HEAL_SLACK_MS);
    while inner.heal_ticks.front().is_some_and(|t| t.at < keep_from) {
        drop_oldest(inner);
    }
}

pub(super) fn heals_between(inner: &Inner, from_ms: i64, to_ms: i64) -> HashMap<i32, HashMap<(i32, bool), HealTotals>> {
    heals_within(inner, &[(from_ms, to_ms)], |_| true)
}

/// Spans that overlap or touch joined into one, in time order.
fn merged_spans(spans: impl Iterator<Item = (i64, i64)>) -> Vec<(i64, i64)> {
    let mut spans: Vec<(i64, i64)> = spans.collect();
    spans.sort_unstable();
    let mut out: Vec<(i64, i64)> = Vec::new();
    for (first, last) in spans {
        match out.last_mut() {
            Some((_, end)) if first <= *end => *end = (*end).max(last),
            _ => out.push((first, last)),
        }
    }
    out
}

/// Healing done inside any of these spans (each from..=to, in time order,
/// apart), by the healers `keep` takes, each judged once.
fn heals_within(inner: &Inner, spans: &[(i64, i64)], keep: impl Fn(i32) -> bool) -> HealsBy {
    let mut out: HealsBy = HashMap::new();
    let mut kept: HashMap<i32, bool> = HashMap::new();
    for t in inner.heal_ticks.iter().filter(|t| inside(spans, t.at)) {
        if !*kept.entry(t.actor).or_insert_with(|| !is_mob(inner, t.actor) && keep(t.actor)) {
            continue;
        }
        add(&mut out, t).ticks.push(HealAt { at: t.at, target: t.target, amount: t.amount });
    }
    out
}

/// `heals_within` without the ticks, from the sums kept for these spans:
/// only the ticks since the last read are walked. In a 400 against 400
/// fight walking all of them took most of a Heal tab read.
fn sums_within(inner: &Inner, spans: &[(i64, i64)], keep: impl Fn(i32) -> bool) -> HealsBy {
    let mut kept: HashMap<i32, bool> = HashMap::new();
    let mut ours = |actor: i32| *kept.entry(actor).or_insert_with(|| !is_mob(inner, actor) && keep(actor));
    let mut all = inner.heal_sums.0.lock();
    let sums = Summed::of(&mut all, spans);
    sums.catch_up(&inner.heal_ticks, spans);
    let mut out: HealsBy = sums.by.iter().filter(|&(&actor, _)| ours(actor)).map(|(&actor, s)| (actor, s.clone())).collect();
    for t in inner.heal_ticks.range(sums.counted..).filter(|t| inside(spans, t.at)) {
        if ours(t.actor) {
            add(&mut out, t);
        }
    }
    out
}

/// Whether `at` falls in one of these spans (each from..=to, in time order,
/// apart).
fn inside(spans: &[(i64, i64)], at: i64) -> bool {
    let after = spans.partition_point(|&(from, _)| from <= at);
    after > 0 && at <= spans[after - 1].1
}

/// `t` added to its healer's and skill's sums, which come back.
fn add<'a>(by: &'a mut HealsBy, t: &HealTick) -> &'a mut HealTotals {
    let e = by.entry(t.actor).or_default().entry((t.skill, t.is_hot)).or_default();
    e.total_heal += t.amount;
    e.tick_count = e.tick_count.saturating_add(1);
    e
}

/// A mob is no healer. Protection Circle HoT ticks on a player carry a mob
/// in the healer field (the boss), and saved boss fights listed
/// the boss as a healer. A summon spawns like a mob, so a linked one is kept;
/// so is an id the game named as a player since.
fn is_mob(inner: &Inner, id: i32) -> bool {
    inner.mob_storage.contains_key(&id)
        && !inner.summon_storage.contains_key(&id)
        && !inner.known_player_ids.contains(&id)
        && !inner.nickname_storage.contains_key(&id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::data_storage::damage::{rec, record};
    use crate::combat::data_storage::RosterEntry;

    fn hit(s: &DataStorage, at: i64) {
        rec(s, 1000, 900, 11_010_000, 100, at);
    }

    #[test]
    fn a_heal_needs_an_amount_and_a_plausible_healer() {
        let s = DataStorage::new();
        hit(&s, 0);
        s.add_heal_event(50, 50, 17_010_000, 100, false, 1_000);
        assert!(s.heals_between(i64::MIN, i64::MAX).is_empty(), "50 is no entity yet");
        s.remember_small_id(50);
        s.add_heal_event(50, 50, 17_010_000, 100, false, 1_000);
        s.add_heal_event(2000, 2000, 17_010_000, 0, false, 1_000);
        s.add_heal_event(2000, 2000, 17_010_000, -20, false, 1_000);
        let heals = s.heals_between(i64::MIN, i64::MAX);
        assert_eq!(heals.keys().copied().collect::<Vec<_>>(), vec![50]);
        assert_eq!(heals[&50][&(17_010_000, false)].total_heal, 100);
    }

    #[test]
    fn healing_in_a_fight_comes_from_its_fighters_party_and_summons() {
        let s = DataStorage::new();
        s.set_local_player_id(Some(1000));
        hit(&s, 0);
        rec(&s, 5000, 900, 11_010_000, 100, 500);
        hit(&s, 2_000);
        s.link_pet_owner(1000, 3000);
        s.name_entity(4000, "Cleric");
        s.replace_roster(vec![("Cleric".to_owned(), RosterEntry::default())], true);
        for healer in [1000, 3000, 4000, 5000, 6000] {
            s.add_heal_event(healer, healer, 17_010_000, 100, false, 1_000);
        }
        let fight = s.copy_targets()[&900].clone();
        let mut healers: Vec<i32> = s.fights_heals(&[&fight]).into_keys().collect();
        healers.sort();
        assert_eq!(healers, vec![1000, 3000, 4000, 5000], "6000 only healed nearby");
    }

    #[test]
    fn a_long_fight_keeps_the_healing_from_its_start() {
        let s = DataStorage::new();
        hit(&s, 0);
        for at in 0..150_000 {
            s.add_heal_event(2000, 2000, 17_010_000, 10, false, at);
            if at % 20_000 == 0 {
                hit(&s, at);
            }
        }
        let heals = s.heals_between(0, 150_000);
        assert_eq!(heals[&2000][&(17_010_000, false)].tick_count, 150_000);
    }

    #[test]
    fn only_players_and_their_summons_heal() {
        let s = DataStorage::new();
        hit(&s, 0);
        s.append_mob(22809, 2310171);
        s.append_mob(500, 1);
        s.link_pet_owner(14409, 500);
        s.name_entity(14274, "Templar");
        for (actor, skill) in [(22809, 18_730_003), (500, 16_770_000), (14274, 18_730_003), (14409, 2_011_101)] {
            s.add_heal_event(actor, actor, skill, 100, true, 1_000);
        }
        let mut healers: Vec<i32> = s.heals_between(0, 2_000).into_keys().collect();
        healers.sort();
        assert_eq!(healers, vec![500, 14274, 14409], "the boss is not one");
    }

    #[test]
    fn healing_from_before_every_open_fight_is_dropped() {
        let s = DataStorage::new();
        for at in 0..PRUNE_EVERY as i64 {
            s.add_heal_event(2000, 2000, 17_010_000, 10, false, at);
        }
        assert_eq!(s.inner.read().heal_ticks.len(), PRUNE_EVERY, "within the slack");
        hit(&s, 200_000);
        for at in 200_000..200_000 + PRUNE_EVERY as i64 {
            s.add_heal_event(2000, 2000, 17_010_000, 10, false, at);
        }
        let inner = s.inner.read();
        assert_eq!(inner.heal_ticks.len(), PRUNE_EVERY);
        assert_eq!(inner.heal_ticks.front().map(|t| t.at), Some(200_000));
    }

    #[test]
    fn fights_at_once_take_each_tick_once_and_leave_out_the_gaps() {
        let s = DataStorage::new();
        let on = |target: i32, at: i64| s.append_damage(record(1000, target, 11_010_000, 100, at));
        on(900, 0);
        on(900, 10_000); // a boss, 0 to 10 s
        on(901, 4_000);
        on(901, 6_000); // its add, 4 to 6 s
        on(902, 30_000);
        on(902, 35_000); // the next pull, 30 to 35 s
        for at in [2_000, 5_000, 20_000, 32_000] {
            s.add_heal_event(1000, 1000, 17_010_000, 100, false, at);
        }
        let inner = s.inner.read();
        let fights: Vec<&TargetTotals> = [900, 901, 902].iter().map(|t| &inner.target_combat[t]).collect();
        let each: i32 = fights.iter().map(|f| fights_heals(&inner, &[f], true)[&1000][&(17_010_000, false)].tick_count).sum();
        assert_eq!(each, 4, "the tick at 5 s is in the boss's span and the add's");
        let all = &fights_heals(&inner, &fights, true)[&1000][&(17_010_000, false)];
        assert_eq!((all.tick_count, all.total_heal), (3, 300), "once each; the tick at 20 s fell between pulls");
    }

    /// (healer, skill, HoT, healing, ticks), in order.
    type Flat = Vec<(i32, i32, bool, i64, i32)>;

    fn flat(heals: HealsBy) -> Flat {
        let mut out: Flat = heals
            .into_iter()
            .flat_map(|(actor, by)| by.into_iter().map(move |((skill, hot), e)| (actor, skill, hot, e.total_heal, e.tick_count)))
            .collect();
        out.sort_unstable();
        out
    }

    /// The kept sums of these fights, which must be the full walk's.
    fn sums(s: &DataStorage, fights: &[&TargetTotals]) -> Flat {
        let kept = s.fights_heal_sums(fights);
        assert!(kept.values().flat_map(|by| by.values()).all(|e| e.ticks.is_empty()));
        let kept = flat(kept);
        assert_eq!(kept, flat(s.fights_heals(fights)));
        kept
    }

    fn live(s: &DataStorage, target: i32) -> TargetTotals {
        s.copy_targets()[&target].clone()
    }

    #[test]
    fn kept_heal_sums_follow_the_fight_its_people_and_their_summons() {
        let s = DataStorage::new();
        s.set_local_player_id(Some(1000));
        s.confirm_pet(3000, 1000);
        let heal = |at: i64| {
            for healer in [1000, 3000, 4000, 5000] {
                s.add_heal_event(healer, healer, 17_010_000, 100, false, at);
            }
        };
        hit(&s, 0);
        heal(500);
        hit(&s, 1_000);
        heal(1_500);
        let healers = |fights: &[&TargetTotals]| sums(&s, fights).iter().map(|e| (e.0, e.4)).collect::<Vec<_>>();
        assert_eq!(healers(&[&live(&s, 900)]), [(1000, 1), (3000, 1)], "the ticks at 1.5 s came after the fight");
        hit(&s, 2_000);
        assert_eq!(healers(&[&live(&s, 900)]), [(1000, 2), (3000, 2)]);
        rec(&s, 5000, 900, 11_010_000, 100, 2_000);
        s.name_entity(4000, "Cleric");
        s.replace_roster(vec![("Cleric".to_owned(), RosterEntry::default())], true);
        assert_eq!(healers(&[&live(&s, 900)]), [(1000, 2), (3000, 2), (4000, 2), (5000, 2)], "a new fighter and party member");
        s.remember_pet_spawn(3000);
        assert_eq!(healers(&[&live(&s, 900)]), [(1000, 4), (4000, 2), (5000, 2)], "the old summon's healing went to its owner");

        // Two fights read apart and together keep a sum each.
        let on_901 = |at: i64| rec(&s, 1000, 901, 11_010_000, 100, at);
        on_901(3_000);
        for at in (3_000..10_000).step_by(1_000) {
            heal(at + 500);
            on_901(at + 1_000);
            let (boss, add) = (live(&s, 900), live(&s, 901));
            for fights in [&[&boss][..], &[&add], &[&boss, &add]] {
                sums(&s, fights);
            }
            assert!(s.inner.read().heal_sums.0.lock().len() <= 3);
        }
    }

    #[test]
    fn kept_heal_sums_lose_the_ticks_a_prune_drops() {
        let s = DataStorage::new();
        hit(&s, 0);
        hit(&s, 1_000);
        for at in 0..1_000 {
            s.add_heal_event(1000, 1000, 17_010_000, 10, false, at);
        }
        let old = live(&s, 900);
        assert_eq!(sums(&s, &[&old])[0].4, 1_000);
        rec(&s, 1000, 901, 11_010_000, 100, 200_000);
        s.retire_idle(200_000, |_, _| false);
        for at in 200_000..200_000 + PRUNE_EVERY as i64 {
            s.add_heal_event(1000, 1000, 17_010_000, 10, false, at);
        }
        assert_eq!(s.inner.read().heal_ticks.front().map(|t| t.at), Some(200_000), "pruned");
        assert!(sums(&s, &[&old]).is_empty());
        rec(&s, 1000, 901, 11_010_000, 100, 205_000);
        assert_eq!(sums(&s, &[&live(&s, 901)])[0].4, PRUNE_EVERY as i32);
    }

    #[test]
    fn kept_heal_sums_lose_the_ticks_the_cap_drops() {
        let s = DataStorage::new();
        let tick = |at: i64| HealTick { at, actor: 1000, target: 1000, skill: 17_010_000 + (at % 3) as i32, is_hot: false, amount: 1 + at % 7 };
        // A tick every ms, a hit every 20 s.
        let fight = |from: i64, to: i64| {
            for start in (from..to).step_by(20_000) {
                hit(&s, start);
                let mut inner = s.inner.write();
                (start..start + 20_000).for_each(|at| record_heal(&mut inner, tick(at)));
            }
            hit(&s, to);
        };
        fight(0, 600_000);
        sums(&s, &[&live(&s, 900)]);
        fight(600_000, MAX_HEAL_TICKS as i64 + 100_000);
        assert_eq!(s.inner.read().heal_ticks.front().map(|t| t.at), Some(100_000), "capped");
        sums(&s, &[&live(&s, 900)]);
    }
}
