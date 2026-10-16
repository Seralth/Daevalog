//! Nicknames: binding names to entity ids, and evicting a name from an old id.

use std::collections::{HashMap, HashSet};

use crate::capture::names::in_cjk_blocks;

use super::damage::drop_ally_hits;
use super::roster::match_roster_by_job;
use super::{ActorTotals, DataStorage, Inner};

/// How sure a bind is of its name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Source {
    /// A loose scan, a held name, a class match: an old name may be better.
    Guess,
    /// The protocol's word, or yours.
    Stated,
}

impl DataStorage {
    /// A name from a loose scan (an actor name field, a loot record, a
    /// summon's owner): the field read may belong to someone else, so it
    /// never overrides what the protocol stated.
    pub fn append_nickname(&self, id: i32, name: &str) {
        let s = &mut *self.inner.write();
        if scan_may_name(s, id, name) {
            give_name(s, id, name, Source::Guess);
            settle_roster_after(s, name);
        }
    }

    pub fn knows_name(&self, id: i32) -> bool {
        self.inner.read().nickname_storage.contains_key(&id)
    }

    /// A name the protocol states for this id: a spawn or a self record.
    pub fn name_entity(&self, id: i32, name: &str) {
        let s = &mut *self.inner.write();
        state_name(s, id, name);
        settle_roster_after(s, name);
    }

    pub fn stored_name(&self, id: i32) -> Option<String> {
        self.inner.read().nickname_storage.get(&id).cloned()
    }

    /// A name that comes back on its id after `forget_names`.
    pub fn fix_name(&self, id: i32, name: &str) {
        let s = &mut *self.inner.write();
        s.permanent_nicknames.retain(|_, kept| kept != name);
        s.permanent_nicknames.insert(id, name.to_owned());
        state_name(s, id, name);
    }

    pub fn all_names(&self) -> HashMap<i32, String> {
        self.inner.read().nickname_storage.clone()
    }

    /// Your configured name, put on your entity.
    pub fn set_local_nickname(&self, id: i32, name: &str) {
        state_name(&mut self.inner.write(), id, name);
    }

    /// The id holding exactly `name`. A bind keeps a name on one id; were it
    /// on two, the lowest id answers, the same on every run.
    pub fn id_for_name(&self, name: &str) -> Option<i32> {
        holders_of(&self.inner.read(), name).first().copied()
    }

    /// A name for an id not seen fighting yet. It goes on at the id's first
    /// damage record, unless something names the id before.
    pub fn hold_name_for_later(&self, id: i32, name: &str) {
        let s = &mut *self.inner.write();
        if s.nickname_storage.get(&id).is_none() {
            s.pending_nicknames.insert(id, name.to_owned());
        }
    }

    /// Whether the id hit or was hit in a live fight, or is linked to an
    /// owner.
    pub fn seen_in_fight(&self, actor_id: i32) -> bool {
        let s = self.inner.read();
        in_a_fight(&s, actor_id) || s.summon_storage.contains_key(&actor_id)
    }

    pub fn seen_as_player(&self, id: i32) -> bool {
        self.inner.read().known_player_ids.contains(&id)
    }

    /// Start the names over: only the permanent ones, and yours on your id,
    /// stay. Who is a player and who owns which summon is kept.
    pub fn forget_names(&self) {
        let s = &mut *self.inner.write();
        s.pending_nicknames.clear();
        s.nickname_storage.clone_from(&s.permanent_nicknames);
        s.authoritative_name_ids = s.permanent_nicknames.keys().copied().collect();
        let Some(local) = s.local_player_id.map(|id| id as i32) else { return };
        let Some(me) = my_name(s).map(str::to_owned) else { return };
        s.nickname_storage.retain(|&id, held| id == local || held.trim() != me);
        s.nickname_storage.insert(local, me);
        s.authoritative_name_ids.insert(local);
    }

    pub fn known_player_set(&self) -> HashSet<i32> {
        self.inner.read().known_player_ids.clone()
    }
}

/// Your configured name, trimmed; `None` when unset or blank.
pub(super) fn my_name(s: &Inner) -> Option<&str> {
    Some(s.local_character_name.as_deref()?.trim()).filter(|me| !me.is_empty())
}

/// Give a name held back for `uid` (`hold_name_for_later`), now that it
/// has fought, unless it got one meanwhile.
pub(super) fn promote_held_name(s: &mut Inner, uid: i32) {
    if !s.nickname_storage.contains_key(&uid)
        && let Some(name) = s.pending_nicknames.remove(&uid)
    {
        give_name(s, uid, &name, Source::Guess);
    }
}

/// Put `name` on `id`. A stated name skips the check that keeps a better
/// old one.
pub(super) fn give_name(s: &mut Inner, id: i32, name: &str, source: Source) {
    let old = s.nickname_storage.get(&id).map(String::as_str);
    if old != Some(name) {
        if source == Source::Guess && old.is_some_and(|old| old_name_stands(old, name)) {
            return;
        }
        evict(s, id, name, source);
        s.nickname_storage.insert(id, name.to_owned());
        now_a_player(s, id);
    }
    if my_name(s) == Some(name.trim()) {
        s.local_player_id = Some(i64::from(id));
    }
}

/// The protocol's word, or yours: marked, so no scan renames the id.
fn state_name(s: &mut Inner, id: i32, name: &str) {
    s.authoritative_name_ids.insert(id);
    give_name(s, id, name, Source::Stated);
}

/// Whether a loose scan may put `name` on `id`. Scans read ids from fields
/// that may hold anything (a record counter, an owner's legion), so only an
/// id the meter has met is named, and never against the protocol's word.
fn scan_may_name(s: &Inner, id: i32, name: &str) -> bool {
    let stated = |who: i32| s.authoritative_name_ids.contains(&who);
    let met = s.nickname_storage.contains_key(&id)
        || stated(id)
        || s.known_player_ids.contains(&id)
        || s.summon_storage.contains_key(&id)
        || in_a_fight(s, id);
    let renames_stated = stated(id) && s.nickname_storage.get(&id).is_some_and(|held| held != name);
    let held_by_stated = s.nickname_storage.iter().any(|(&other, held)| other != id && held == name && stated(other));
    met && !renames_stated && !held_by_stated
}

/// Every id holding exactly `name`, lowest first.
fn holders_of(s: &Inner, name: &str) -> Vec<i32> {
    let mut ids: Vec<i32> = s.nickname_storage.iter().filter(|e| e.1 == name).map(|e| *e.0).collect();
    ids.sort_unstable();
    ids
}

fn in_a_fight(s: &Inner, id: i32) -> bool {
    s.target_combat.iter().any(|(&target, fight)| target == id || fight.actors.contains_key(&id))
}

/// Whether a guessed bind keeps `old` over `new`. A scan that loses the CJK
/// part of a name, or picks up a short fragment or a two-letter word, gives
/// a shorter name than the real one.
fn old_name_stands(old: &str, new: &str) -> bool {
    let new_cjk = new.chars().any(in_cjk_blocks);
    let lost_script = old.chars().any(in_cjk_blocks) && !new_cjk && new.len() < old.len();
    let fragment = !new_cjk && new.len() <= 5 && old.len() > new.len();
    lost_script || fragment
}

/// Take `name` off every id but `keeper`. An id the protocol named, losing
/// its name to a stated one, was the same character under the id of an
/// earlier zone. Any other holder had the name by mistake.
fn evict(s: &mut Inner, keeper: i32, name: &str, source: Source) {
    let stated_bind = source == Source::Stated;
    for old in holders_of(s, name).into_iter().filter(|&old| old != keeper) {
        let was_stated = s.authoritative_name_ids.remove(&old);
        s.known_player_ids.remove(&old);
        s.nickname_storage.remove(&old);
        s.pending_nicknames.remove(&old);
        if stated_bind {
            s.permanent_nicknames.remove(&old);
        }
        if stated_bind && was_stated {
            carry_over(s, old, keeper);
        } else {
            drop_what_it_did(s, old);
        }
    }
}

/// `old` and `keeper` are one character: its summons and what it did go
/// over to `keeper`, totals unchanged.
fn carry_over(s: &mut Inner, old: i32, keeper: i32) {
    s.summon_storage.values_mut().filter(|owner| **owner == old).for_each(|owner| *owner = keeper);
    let seq = s.next_seq();
    for fight in s.target_combat.values_mut() {
        let Some(row) = fight.actors.remove(&old) else { continue };
        let kept = fight.actors.entry(keeper).or_insert_with(ActorTotals::new);
        kept.absorb(row);
        kept.seq = seq;
    }
}

/// `old` held the name by mistake: what it did under it and its summons'
/// links go. Its targets stay, empty or not.
fn drop_what_it_did(s: &mut Inner, old: i32) {
    s.summon_storage.retain(|_, owner| *owner != old);
    for fight in s.target_combat.values_mut() {
        let lost = fight.actors.remove(&old).map_or(0, |row| row.total_damage);
        fight.total_damage -= lost;
    }
}

/// A named id is a player. A confirmed summon is a named pet: it keeps its
/// owner and is none.
fn now_a_player(s: &mut Inner, id: i32) {
    if s.confirmed_summon_ids.contains(&id) {
        return;
    }
    s.summon_storage.remove(&id);
    let first_time = s.known_player_ids.insert(id);
    if first_time {
        drop_ally_hits(s);
    }
}

/// A roster member just got an id: with two of a class, naming one leaves
/// the other as the only fit.
fn settle_roster_after(s: &mut Inner, name: &str) {
    let member = s.party_members.contains_key(name.trim());
    if member {
        match_roster_by_job(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::data_storage::damage::rec;
    use crate::combat::data_storage::roster::{name_of, AT, G_A, ME, T_B};
    use crate::combat::data_storage::tests::{game_says, member_as, seat};
    use crate::entity::job_class::JobClass::{Gladiator, Templar};

    /// No class's skill: the actor does not become a player by it.
    const PLAIN: i32 = 20_010_000;
    /// A Gladiator and a Templar skill.
    const GLAD: i32 = 11_010_000;
    const TEMP: i32 = 12_010_000;

    const CLERIC: &str = "Cl_x";
    const FOE: &str = "Foe_x";
    const LONG: &str = "Long_name";
    const SHORT: &str = "Ab_c";
    const G_B: &str = "G_b";

    fn row(db: &DataStorage, target: i32, actor: i32) -> Option<i64> {
        db.copy_targets().get(&target)?.actors.get(&actor).map(|a| a.total_damage)
    }

    fn total(db: &DataStorage, target: i32) -> Option<i64> {
        db.copy_targets().get(&target).map(|t| t.total_damage)
    }

    fn owner(db: &DataStorage, summon: i32) -> Option<i32> {
        db.inner.read().summon_storage.get(&summon).copied()
    }

    /// Two unnamed Gladiators, 101 and 102, and a roster of two Gladiators:
    /// neither can be told apart yet.
    fn twin_gladiators() -> DataStorage {
        let db = DataStorage::new();
        for k in 0..4 {
            rec(&db, 101, 900, GLAD + k * 10_000, 100, AT);
            rec(&db, 102, 900, GLAD + k * 10_000, 100, AT);
        }
        seat(&db, &[(G_A, member_as(1, Gladiator)), (G_B, member_as(2, Gladiator))]);
        assert_eq!([name_of(&db, 101), name_of(&db, 102)], ["-", "-"]);
        db
    }

    #[test]
    fn the_held_name_again_touches_nothing() {
        let db = DataStorage::new();
        rec(&db, 5, 900, PLAIN, 10, AT);
        db.append_nickname(5, CLERIC);
        let marked = db.seen_as_player(5);
        // An NPC spawn takes the player mark back.
        db.append_mob(5, 1);
        let unmarked = db.seen_as_player(5);
        db.append_nickname(5, CLERIC);
        assert_eq!([marked, unmarked, db.seen_as_player(5)], [true, false, false]);
        assert_eq!(db.all_names().len(), 1);
    }

    #[test]
    fn scans_never_trade_a_name_for_a_shorter_or_scriptless_one() {
        let second_wins = |old: &str, new: &str| {
            let db = DataStorage::new();
            rec(&db, 5, 900, PLAIN, 10, AT);
            db.append_nickname(5, old);
            db.append_nickname(5, new);
            name_of(&db, 5) == new
        };
        let han3 = "王_甲乙";
        let han2 = "王_甲";
        assert_eq!((han3.len(), han2.len(), "カ_".len(), "ᄀ_".len()), (10, 7, 4, 4));
        // CJK lost and fewer bytes, a fragment of 5 bytes or fewer, and kana,
        // which is no CJK.
        assert!(!second_wins(han3, "Abc_defg"));
        assert!(!second_wins(LONG, SHORT));
        assert!(!second_wins("Abc_def", "カ_"));
        assert!(second_wins(han2, "Abc_defg"));
        assert!(second_wins(SHORT, "Abc_d"));
        assert!(second_wins(LONG, "Abc_de"));
        assert!(second_wins("Abc_def", "王_"));
        assert!(second_wins("Abc_def", "ᄀ_"));

        let db = DataStorage::new();
        rec(&db, 5, 900, PLAIN, 10, AT);
        db.append_nickname(5, han3);
        db.name_entity(5, SHORT);
        assert_eq!(name_of(&db, 5), SHORT, "the protocol's word skips the guard");
    }

    #[test]
    fn a_stated_name_moving_ids_carries_damage_and_summons() {
        let db = DataStorage::new();
        db.name_entity(101, CLERIC);
        rec(&db, 101, 900, PLAIN, 500, AT);
        db.confirm_pet(600, 101);
        db.name_entity(202, CLERIC);
        assert_eq!(name_of(&db, 101), "-");
        assert!(!db.seen_as_player(101));
        assert_eq!([row(&db, 900, 101), row(&db, 900, 202), total(&db, 900)], [None, Some(500), Some(500)]);
        assert_eq!(owner(&db, 600), Some(202));
        rec(&db, 202, 900, PLAIN, 300, AT);
        assert_eq!([row(&db, 900, 202), total(&db, 900)], [Some(800); 2]);
    }

    #[test]
    fn returning_character_takes_old_damage() {
        let db = DataStorage::new();
        db.name_entity(101, CLERIC);
        rec(&db, 101, 900, GLAD, 500, 1_000);
        db.name_entity(202, CLERIC);
        rec(&db, 202, 900, GLAD, 300, 2_000);
        assert_eq!([row(&db, 900, 101), row(&db, 900, 202), total(&db, 900)], [None, Some(800), Some(800)]);

        let db = DataStorage::new();
        rec(&db, 303, 900, GLAD, 500, 1_000);
        db.append_nickname(303, CLERIC);
        db.name_entity(404, CLERIC);
        assert_eq!((total(&db, 900), row(&db, 900, 404)), (Some(0), None));
    }

    #[test]
    fn kept_name_follows_authoritative_move() {
        let db = DataStorage::new();
        db.fix_name(6925, CLERIC);
        db.name_entity(7577, CLERIC);
        db.forget_names();
        assert_eq!(name_of(&db, 6925), "-");
    }

    #[test]
    fn local_name_survives_name_reset_and_flush() {
        let db = DataStorage::new();
        db.name_entity(13600, ME);
        game_says(&db, 13600, ME);
        db.name_entity(2001, CLERIC);
        db.forget_names();
        assert_eq!(db.local_player_id(), Some(13600));
        assert_eq!([name_of(&db, 13600), name_of(&db, 2001)], [ME, "-"]);
        db.flush();
        assert_eq!(name_of(&db, 13600), ME);
    }

    #[test]
    fn a_scanned_name_moving_ids_drops_what_the_old_id_did() {
        let db = DataStorage::new();
        rec(&db, 303, 900, GLAD, 500, AT);
        db.confirm_pet(601, 303);
        db.append_nickname(303, CLERIC);
        db.name_entity(404, CLERIC);
        assert_eq!([row(&db, 900, 303), row(&db, 900, 404)], [None; 2]);
        assert_eq!(total(&db, 900), Some(0), "the target stays, empty");
        assert_eq!(owner(&db, 601), None);
        assert_eq!(name_of(&db, 303), "-");
    }

    #[test]
    fn an_unforced_eviction_leaves_the_kept_name_in_the_list() {
        let db = DataStorage::new();
        db.fix_name(6, CLERIC);
        rec(&db, 6, 900, PLAIN, 200, AT);
        // A held name skips the scan gate, and binds as a guess.
        db.hold_name_for_later(7, CLERIC);
        rec(&db, 7, 900, PLAIN, 50, AT);
        assert_eq!([name_of(&db, 6), name_of(&db, 7)], ["-", CLERIC]);
        assert_eq!((row(&db, 900, 6), total(&db, 900)), (None, Some(50)));
        db.forget_names();
        assert_eq!([name_of(&db, 6), name_of(&db, 7)], [CLERIC, "-"]);
    }

    #[test]
    fn a_named_pvp_stranger_is_a_player_and_their_target_goes() {
        let db = DataStorage::new();
        rec(&db, 100, 200, GLAD, 300, AT);
        assert!(db.seen_as_player(100) && db.was_hit(200));
        db.name_entity(200, FOE);
        assert!(db.seen_as_player(200));
        assert!(!db.was_hit(200), "its one actor was a player");

        // A guessed link gives way to a name.
        db.link_pet_owner(100, 501);
        let linked = owner(&db, 501);
        db.name_entity(501, "Pl_x");
        assert_eq!((linked, owner(&db, 501), db.seen_as_player(501)), (Some(100), None, true));

        // A confirmed summon keeps its owner and is no player.
        db.confirm_pet(500, 100);
        db.name_entity(500, "Pet_x");
        assert_eq!((name_of(&db, 500), owner(&db, 500), db.seen_as_player(500)), ("Pet_x".into(), Some(100), false));
    }

    #[test]
    fn lookups_match_exactly_and_ties_go_to_the_lowest_id() {
        let db = DataStorage::new();
        db.name_entity(7, CLERIC);
        assert_eq!([db.knows_name(7), db.knows_name(8)], [true, false]);
        assert_eq!([db.id_for_name(CLERIC), db.id_for_name(" Cl_x")], [Some(7), None]);

        // Each store's maps have their own order; the answer may not.
        let answers: HashSet<Option<i32>> = (0..32)
            .map(|_| {
                let db = DataStorage::new();
                let mut s = db.inner.write();
                for id in [9300, 9100, 9200] {
                    s.nickname_storage.insert(id, "Twin_x".to_owned());
                }
                drop(s);
                db.id_for_name("Twin_x")
            })
            .collect();
        assert_eq!(answers, HashSet::from([Some(9100)]));
    }

    #[test]
    fn appearing_in_combat_is_fighting_or_a_link() {
        let db = DataStorage::new();
        rec(&db, 10, 900, PLAIN, 1, AT);
        db.confirm_pet(11, 10);
        db.name_entity(12, "Idle_x");
        let seen = [10, 900, 11, 12].map(|id| db.seen_in_fight(id));
        assert_eq!(seen, [true, true, true, false], "actor, target, summon, a player who never fought");
        assert!(db.known_player_set().contains(&12));
    }

    #[test]
    fn your_name_on_an_id_makes_it_you() {
        let db = DataStorage::new();
        db.set_local_character_name(Some(format!(" {ME} ")));
        db.name_entity(100, ME);
        let first = db.local_player_id();
        db.set_local_player_id(Some(5));
        db.append_nickname(100, ME);
        assert_eq!([first, db.local_player_id()], [Some(100); 2], "the same name, found again");

        // Not when the guard keeps the old name.
        let db = DataStorage::new();
        db.set_local_character_name(Some(SHORT.into()));
        rec(&db, 5, 900, PLAIN, 1, AT);
        db.append_nickname(5, LONG);
        db.append_nickname(5, SHORT);
        assert_eq!((name_of(&db, 5), db.local_player_id()), (LONG.into(), None));
    }

    #[test]
    fn scans_name_only_met_ids_and_never_against_a_stated_name() {
        let db = DataStorage::new();
        // An id read from a record counter, never seen.
        db.append_nickname(777, ME);
        let unseen = name_of(&db, 777);
        rec(&db, 777, 900, PLAIN, 1, AT);
        db.append_nickname(777, ME);
        db.append_nickname(900, FOE);
        db.confirm_pet(800, 777);
        db.append_nickname(800, "Pet_y");
        let names = [unseen, name_of(&db, 777), name_of(&db, 900), name_of(&db, 800)];
        assert_eq!(names, ["-", ME, FOE, "Pet_y"], "an actor, a target and a summon are met");

        let db = DataStorage::new();
        db.name_entity(100, ME);
        rec(&db, 200, 900, PLAIN, 1, AT);
        db.append_nickname(200, ME);
        // A legion name read beside the player's.
        db.append_nickname(100, "Legion_y");
        let after_both = [name_of(&db, 100), name_of(&db, 200)];
        db.append_nickname(100, ME);
        assert_eq!(after_both, [ME, "-"]);
        assert_eq!(name_of(&db, 100), ME);
    }

    #[test]
    fn a_permanent_name_moves_between_ids_and_comes_back_after_a_reset() {
        let db = DataStorage::new();
        db.fix_name(30, "Kept_x");
        db.fix_name(31, "Kept_x");
        db.append_nickname(31, FOE);
        let before = name_of(&db, 31);
        db.forget_names();
        db.append_nickname(31, FOE);
        assert_eq!([before, name_of(&db, 30), name_of(&db, 31)], ["Kept_x", "-", "Kept_x"], "stated before and after");

        let db = twin_gladiators();
        db.fix_name(101, G_A);
        assert_eq!(name_of(&db, 102), "-", "no roster pass");
    }

    #[test]
    fn your_hand_set_name_is_stated_but_not_kept_through_a_reset() {
        let db = DataStorage::new();
        db.set_local_nickname(40, ME);
        db.append_nickname(40, "Me_long_x");
        let before = name_of(&db, 40);
        db.forget_names();
        assert_eq!([before, name_of(&db, 40)], [ME, "-"]);

        let db = twin_gladiators();
        db.set_local_nickname(101, G_A);
        assert_eq!(name_of(&db, 102), "-", "no roster pass");
    }

    #[test]
    fn a_held_name_waits_for_the_first_hit() {
        let db = DataStorage::new();
        db.hold_name_for_later(300, "P_x");
        let before = name_of(&db, 300);
        rec(&db, 300, 900, PLAIN, 1, AT);
        assert_eq!([before, name_of(&db, 300)], ["-", "P_x"]);

        let db = DataStorage::new();
        db.hold_name_for_later(300, "P_x");
        db.hold_name_for_later(300, "P_y");
        rec(&db, 300, 900, PLAIN, 1, AT);
        assert_eq!(name_of(&db, 300), "P_y", "the later one");

        let db = DataStorage::new();
        db.name_entity(300, ME);
        db.hold_name_for_later(300, "P_x");
        rec(&db, 300, 900, PLAIN, 1, AT);
        assert_eq!(name_of(&db, 300), ME);
        assert!(db.inner.read().pending_nicknames.is_empty(), "named already: not held");

        let db = DataStorage::new();
        db.hold_name_for_later(300, "P_x");
        db.name_entity(300, ME);
        rec(&db, 300, 900, PLAIN, 1, AT);
        assert_eq!(name_of(&db, 300), ME);
        assert!(db.inner.read().pending_nicknames.contains_key(&300), "named in between: still held");
    }

    #[test]
    fn a_held_name_skips_the_scan_gate() {
        let db = DataStorage::new();
        db.name_entity(311, FOE);
        db.hold_name_for_later(310, FOE);
        rec(&db, 310, 900, PLAIN, 1, AT);
        assert_eq!([name_of(&db, 310), name_of(&db, 311)], [FOE, "-"]);
    }

    #[test]
    fn a_reset_keeps_permanent_names_and_yours_and_nothing_else() {
        let db = DataStorage::new();
        rec(&db, 8, 900, GLAD, 1, AT);
        db.confirm_pet(70, 8);
        db.fix_name(60, "Perm_x");
        // Kept too, but trimmed it is your name, on another id.
        db.fix_name(61, &format!("{ME} "));
        db.append_nickname(8, "Scan_x");
        db.hold_name_for_later(50, "Later_x");
        db.set_local_player_id(Some(9));
        db.set_local_character_name(Some(format!(" {ME} ")));
        db.forget_names();
        let mut left: Vec<(i32, String)> = db.all_names().into_iter().collect();
        left.sort();
        assert_eq!(left, [(9, ME.to_owned()), (60, "Perm_x".to_owned())]);
        rec(&db, 50, 900, PLAIN, 1, AT);
        assert_eq!(name_of(&db, 50), "-", "the held name is gone");
        assert_eq!((db.seen_as_player(8), owner(&db, 70)), (true, Some(8)));
        db.append_nickname(9, "Me_y");
        db.append_nickname(60, "Perm_y");
        assert_eq!([name_of(&db, 9), name_of(&db, 60)], [ME, "Perm_x"], "both stated");
        assert_eq!(db.local_player_id(), Some(9));
    }

    #[test]
    fn a_roster_name_found_by_a_scan_runs_the_roster_pass_even_if_kept_out() {
        let db = DataStorage::new();
        rec(&db, 50, 900, PLAIN, 1, AT);
        db.append_nickname(50, "Long_name_x");
        seat(&db, &[(G_A, member_as(1, Gladiator)), (T_B, member_as(2, Templar))]);
        for k in 0..3 {
            rec(&db, 60, 900, TEMP + k * 10_000, 100, AT);
        }
        let before = name_of(&db, 60);
        db.append_nickname(50, G_A);
        assert_eq!([before, name_of(&db, 50), name_of(&db, 60)], ["-", "Long_name_x", T_B]);
    }
}
