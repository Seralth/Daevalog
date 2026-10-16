//! The party roster and the dungeon it names, and naming members by class.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::Ordering;

use crate::entity::job_class::JobClass;

use super::deaths::end_hp_known;
use super::names::{give_name, my_name, Source};
use super::{now_ms, DataStorage, Inner, RosterEntry, GHOST_ROW_MS};

impl DataStorage {
    /// A party roster (`0x9702`): every member when `complete`, else some.
    pub fn replace_roster(&self, members: Vec<(String, RosterEntry)>, complete: bool) {
        if members.is_empty() {
            return;
        }
        let at = now_ms();
        let mut guard = self.inner.write();
        guard.party_roster_at_ms = at;
        guard.party_placeholders_hidden = false;
        let ended = complete && party_ended(&guard, &members);
        if ended {
            break_up(&mut guard, at);
        } else {
            seat(&mut guard, members, complete, at);
        }
        drop(guard);
        if ended {
            self.clear_fight_totals();
            self.combat_reset_requested.store(true, Ordering::Relaxed);
        }
        self.touch();
    }

    /// The key of the roster that comes next (`02 97 <key>`): the one a
    /// leave has to name. A party that ends forgets it.
    pub fn note_party_key(&self, key: u32) {
        self.inner.write().party_key = Some(key);
    }

    /// `1b 97 <key>` for the party seated now: that party ends, and only the
    /// party. During a fight it waits for the fights live now to be over and
    /// saved, so they keep the party's rows, heals, hits taken and names
    /// (`settle_leave`). On purpose: if the record means something else (the
    /// decoder's `party_left`), the party's rows drop until the next roster,
    /// about 10 s, the least visible way to be wrong. Another key, or the
    /// same leave again, is ignored.
    pub fn leave_party(&self, key: u32) {
        let mut guard = self.inner.write();
        if guard.party_key != Some(key) || guard.party_left_during.is_some() {
            return;
        }
        let live = guard.target_combat.values().map(|f| (f.target_id, f.first_damage_time)).collect();
        guard.party_left_during = Some(live);
        let gone = settle_leave(&mut guard, now_ms());
        drop(guard);
        if gone {
            self.touch();
        }
    }

    pub fn current_dungeon_id(&self) -> i32 {
        self.inner.read().current_dungeon_id
    }

    /// The `21 36` map load. Ids, HP readings and the records about you
    /// belong to the zone before; the map says which dungeon you are in.
    pub fn note_map_load(&self, map_id: i32) {
        self.forget_player_ids();
        let at = now_ms();
        let s = &mut *self.inner.write();
        s.own_records.zone_loaded();
        end_hp_known(s, at, |_, _| true);
        if s.map_id != map_id {
            s.enemy_ids.clear();
        }
        s.map_id = map_id;
        let own = own_dungeon(map_id);
        s.map_kind = match (is_open_world_map(map_id), own) {
            (true, _) => MapKind::OpenWorld,
            (false, 0) => MapKind::Instance,
            (false, _) => MapKind::Own,
        };
        if s.map_kind != MapKind::Instance {
            // The open world is no dungeon; an own map is its own.
            s.current_dungeon_id = if s.map_kind == MapKind::Own { own } else { 0 };
        } else if s.queued_dungeon_id != 0 && map_of_dungeon(s.queued_dungeon_id) == map_id {
            s.current_dungeon_id = std::mem::take(&mut s.queued_dungeon_id);
        } else if map_of_dungeon(s.current_dungeon_id) != map_id {
            s.current_dungeon_id = 0;
        }
    }

    pub fn roster_entries(&self) -> HashMap<String, RosterEntry> {
        self.inner.read().party_members.clone()
    }

    /// The dungeon a roster names. A queue pops minutes before the load into
    /// the dungeon, so it waits for that load unless you are on its map.
    pub fn enter_dungeon(&self, dungeon_id: i32) {
        if dungeon_id < 1 {
            return;
        }
        let s = &mut *self.inner.write();
        let here = s.map_kind == MapKind::Unknown || s.map_id == map_of_dungeon(dungeon_id);
        if here {
            s.current_dungeon_id = dungeon_id;
        } else {
            s.queued_dungeon_id = dungeon_id;
        }
    }

    /// Rows for roster members who have not fought: for a while after each
    /// roster, unless a manual reset hid them.
    pub fn ghost_rows_shown(&self) -> bool {
        let at = now_ms();
        let s = self.inner.read();
        !s.party_placeholders_hidden && at.saturating_sub(s.party_roster_at_ms) < GHOST_ROW_MS
    }

    pub fn drop_ghost_rows(&self) {
        self.inner.write().party_placeholders_hidden = true;
    }
}

/// A whole roster that leaves out you, or holds one member, after one of
/// two or more: you left the party, or it broke up.
fn party_ended(s: &Inner, members: &[(String, RosterEntry)]) -> bool {
    if s.party_members.len() < 2 {
        return false;
    }
    let without_you = my_name(s).is_some_and(|me| !members.iter().any(|(name, _)| name.trim() == me));
    members.len() < 2 || without_you
}

/// The party is over: no roster, no party HP, and no dungeon but an own map.
fn break_up(s: &mut Inner, at: i64) {
    drop_party(s, at);
    if s.map_kind != MapKind::Own {
        s.current_dungeon_id = 0;
    }
}

/// You are out of the party: no roster, party HP or queue. The dungeon you
/// are in stays until the next map load.
fn drop_party(s: &mut Inner, at: i64) {
    s.party_members.clear();
    s.party_key = None;
    s.party_left_during = None;
    s.party_hp_ids.clear();
    end_hp_known(s, at, |s, id| s.local_player_id != Some(i64::from(id)));
    s.queued_dungeon_id = 0;
}

/// The party you left goes once none of the fights live at the leave is
/// live (each ended, by a quiet spell, a reset, a zone change or a new
/// pull) and no ended fight waits for the save, which reads the party. True
/// when it went.
pub(super) fn settle_leave(s: &mut Inner, at: i64) -> bool {
    let Some(fights) = &s.party_left_during else { return false };
    let live = |&(target, first): &(i32, i64)| s.target_combat.get(&target).is_some_and(|f| f.first_damage_time == first);
    if fights.iter().any(live) || !s.ended_segments.is_empty() {
        return false;
    }
    drop_party(s, at);
    true
}

/// Store the roster; after a whole one, named ids it leaves out have left.
/// A roster after a leave seats its party, ending any wait to drop one.
fn seat(s: &mut Inner, members: Vec<(String, RosterEntry)>, complete: bool, at: i64) {
    s.party_left_during = None;
    if complete {
        s.party_members.clear();
    }
    s.party_members.extend(members);
    if complete {
        let gone: Vec<i32> = s.party_hp_ids.iter().copied().filter(|&id| left_party(s, id)).collect();
        for id in &gone {
            s.party_hp_ids.remove(id);
        }
        end_hp_known(s, at, left_party);
    }
    match_roster_by_job(s);
}

/// A named id, not you, whose name the roster no longer holds. An id
/// without a name cannot be told to have left.
fn left_party(s: &Inner, id: i32) -> bool {
    let named = s.nickname_storage.get(&id);
    s.local_player_id != Some(i64::from(id)) && named.is_some_and(|name| !s.party_members.contains_key(name))
}

/// What the last map load entered. Unknown until the first one.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum MapKind {
    #[default]
    Unknown,
    OpenWorld,
    /// An instance filed under its own map: a sealed, quest, daily, Ascension
    /// or Ascension Trial dungeon, Nightmare, the Abyss or a Daeva Hunter
    /// recon site. No party roster names it.
    Own,
    Instance,
}

/// Open-world map ids from the game's Map table: the overworld maps and their
/// world layers (the overworld itself, split off for quest scenes).
static OPEN_WORLD_MAPS: std::sync::LazyLock<HashSet<i32>> =
    std::sync::LazyLock::new(|| map_list(include_str!("../../../../src/data/open_world_maps.json")));

#[derive(Default, serde::Deserialize)]
struct InstanceMaps {
    /// The dungeon of each instance map a fight is filed under without a
    /// roster. Most share the map's id; the Abyss's do not.
    #[serde(rename = "ownMaps")]
    own_maps: HashMap<i32, i32>,
    /// The map of each dungeon whose id is not its map's.
    #[serde(rename = "dungeonMaps")]
    dungeon_maps: HashMap<i32, i32>,
}

static INSTANCE_MAPS: std::sync::LazyLock<InstanceMaps> = std::sync::LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../../src/data/instance_maps.json")).unwrap_or_default()
});

/// The dungeon a fight on `map_id` is filed under without a roster; 0 for
/// none.
fn own_dungeon(map_id: i32) -> i32 {
    INSTANCE_MAPS.own_maps.get(&map_id).copied().unwrap_or(0)
}

/// The map a dungeon id is played on: most share the id, a few do not
/// (Citadel of the Fallen Daeva 600151 is map 600144).
fn map_of_dungeon(dungeon_id: i32) -> i32 {
    INSTANCE_MAPS.dungeon_maps.get(&dungeon_id).copied().unwrap_or(dungeon_id)
}

/// True for a map of the open world. Unknown ids (a map added by a later
/// patch) count as instances, which keeps the dungeon id as before.
pub fn is_open_world_map(map_id: i32) -> bool {
    OPEN_WORLD_MAPS.contains(&map_id)
}

/// The ids of a `{"maps": [...]}` table; none when it does not read.
fn map_list(json: &str) -> HashSet<i32> {
    #[derive(serde::Deserialize)]
    struct Table {
        maps: HashSet<i32>,
    }
    serde_json::from_str::<Table>(json).map(|t| t.maps).unwrap_or_default()
}

/// Name roster members from their class, before their spawns: a meter
/// started with the party already together sees no spawn until a later
/// area. Only where the fit cannot be a stranger: in an instance, with no
/// more unnamed players fighting than roster names left open, and one open
/// name of the class.
pub(super) fn match_roster_by_job(s: &mut Inner) {
    if s.party_members.len() < 2 || s.map_kind == MapKind::OpenWorld {
        return;
    }
    let open = open_roster_names(s);
    let unnamed = unnamed_players(s);
    if open.is_empty() || unnamed.len() > open.len() {
        return;
    }
    let only_of_class = |job: JobClass| open.iter().filter(|o| o.0 == job).count() == 1;
    let picks: Vec<(i32, String)> = open
        .iter()
        .filter(|o| only_of_class(o.0))
        .filter_map(|(job, name)| Some((fit_of_class(s, &unnamed, *job)?, name.clone())))
        .collect();
    for (id, name) in picks {
        give_name(s, id, &name, Source::Guess);
    }
}

/// Roster members with a class whose name no id holds (both trimmed), as
/// (class, name), by name.
fn open_roster_names(s: &Inner) -> Vec<(JobClass, String)> {
    let on_an_id = |name: &str| s.nickname_storage.values().any(|held| held.trim() == name.trim());
    let mut open: Vec<(JobClass, String)> = s
        .party_members
        .iter()
        .filter(|(name, _)| !on_an_id(name))
        .filter_map(|(name, member)| Some((member.job?, name.clone())))
        .collect();
    open.sort_by(|a, b| a.1.cmp(&b.1));
    open
}

/// Players in a live fight with no name and no owner, by id, each with how
/// many distinct skills it used there (a skill and its ticks are one).
fn unnamed_players(s: &Inner) -> Vec<(i32, usize)> {
    let free = |id: &i32| {
        s.known_player_ids.contains(id) && !s.nickname_storage.contains_key(id) && !s.summon_storage.contains_key(id)
    };
    let mut used: BTreeMap<i32, HashSet<i32>> = Default::default();
    let rows = s.target_combat.values().flat_map(|fight| fight.actors.iter());
    for (id, row) in rows.filter(|(id, _)| free(id)) {
        used.entry(*id).or_default().extend(row.skills.keys().map(|key| key.0));
    }
    used.into_iter().map(|(id, codes)| (id, codes.len())).collect()
}

/// The one unnamed player of `job`; of two or more, the one with at least
/// three times the skills of the next (a player beside a summon-like few).
fn fit_of_class(s: &Inner, unnamed: &[(i32, usize)], job: JobClass) -> Option<i32> {
    let mut fits: Vec<(usize, i32)> =
        unnamed.iter().filter(|(id, _)| s.actor_jobs.get(id) == Some(&job)).map(|&(id, n)| (n, id)).collect();
    // Most skills first, the lower id of equals.
    fits.sort_by_key(|&(n, id)| (std::cmp::Reverse(n), id));
    match fits[..] {
        [] => None,
        [(_, only)] => Some(only),
        [(most, first), (next, _), ..] => (most >= 3 * next).then_some(first),
    }
}

/// Names the storage tests share. Each has a `_`, which no game name has.
#[cfg(test)]
pub(super) const ME: &str = "Me_x";
#[cfg(test)]
pub(super) const A: &str = "A_x";
#[cfg(test)]
pub(super) const B: &str = "B_x";
#[cfg(test)]
pub(super) const G_A: &str = "G_a";
#[cfg(test)]
pub(super) const T_B: &str = "T_b";

/// The time on the damage records the naming tests send.
#[cfg(test)]
pub(super) const AT: i64 = 1_000;

/// Send a roster of `members`: the whole party when `complete`, else some.
#[cfg(test)]
pub(super) fn party(db: &DataStorage, members: &[(&str, RosterEntry)], complete: bool) {
    let list = members.iter().cloned().map(|(name, m)| (name.to_owned(), m)).collect();
    db.replace_roster(list, complete);
}

/// The name on `id`, or "-" without one.
#[cfg(test)]
pub(super) fn name_of(db: &DataStorage, id: i32) -> String {
    db.stored_name(id).unwrap_or_else(|| "-".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Clock;
    use crate::combat::data_storage::damage::{rec, tick};
    use crate::combat::data_storage::tests::{member, member_as};
    use crate::entity::job_class::JobClass::{Elementalist, Gladiator, Templar};

    const C: &str = "C_x";

    /// The first `kinds` Gladiator skills, one hit each on 900; with
    /// `ticks`, a tick of each too.
    fn gladiator(db: &DataStorage, actor: i32, kinds: i32, ticks: bool) {
        for skill in (0..kinds).map(|k| 11_010_000 + k * 10_000) {
            rec(db, actor, 900, skill, 100, AT);
            if ticks {
                tick(db, actor, 900, skill, 40, AT);
            }
        }
    }

    fn members(db: &DataStorage) -> Vec<String> {
        let mut out: Vec<String> = db.roster_entries().into_keys().collect();
        out.sort();
        out
    }

    fn last_span(db: &DataStorage, id: i32) -> Option<(i64, i64)> {
        db.hp_known().get(&id)?.back().copied()
    }

    #[test]
    fn an_empty_roster_is_ignored() {
        let db = DataStorage::new();
        party(&db, &[(A, member(1)), (B, member(2))], true);
        db.drop_ghost_rows();
        let generation = db.damage_generation();
        db.replace_roster(Vec::new(), true);
        assert_eq!((db.damage_generation(), members(&db)), (generation, vec![A.to_owned(), B.to_owned()]));
        assert!(!db.ghost_rows_shown(), "still hidden");
    }

    #[test]
    fn a_lone_roster_ends_the_party_and_the_queue() {
        let db = DataStorage::new();
        db.note_map_load(1010);
        db.enter_dungeon(600002);
        party(&db, &[(A, member(1)), (B, member(2))], true);
        rec(&db, 1454, 900, 20_010_000, 100, AT);
        let before = db.reset_was_asked();
        party(&db, &[(A, member(1))], true);
        let after = db.reset_was_asked();
        assert_eq!((before, after, members(&db).len()), (false, true, 0), "the lone member is not stored");
        assert!(db.copy_targets().is_empty());
        db.note_map_load(600002);
        assert_eq!(db.current_dungeon_id(), 0, "the queue went with the party");
    }

    #[test]
    fn the_party_ending_keeps_an_own_dungeon_only() {
        let sealed = DataStorage::new();
        sealed.note_map_load(310051);
        let queued = DataStorage::new();
        queued.enter_dungeon(600002);
        for db in [&sealed, &queued] {
            party(db, &[(A, member(1)), (B, member(2))], true);
            party(db, &[(A, member(1))], true);
        }
        assert_eq!([sealed.current_dungeon_id(), queued.current_dungeon_id()], [310051, 0]);
    }

    #[test]
    fn a_whole_roster_without_you_ends_the_party() {
        let db = DataStorage::new();
        db.set_local_character_name(Some(format!(" {ME} ")));
        let (you_and_a, b_and_c) = ([(ME, member(1)), (A, member(2))], [(B, member(1)), (C, member(2))]);
        party(&db, &you_and_a, true);
        party(&db, &b_and_c, false);
        let grown = members(&db);
        let quiet = db.reset_was_asked();
        party(&db, &b_and_c, true);
        assert_eq!((grown.len(), quiet, members(&db).len(), db.reset_was_asked()), (4, false, 0, true));

        // Without a name of yours only the size tells.
        let db = DataStorage::new();
        party(&db, &you_and_a, true);
        party(&db, &b_and_c, true);
        assert_eq!((members(&db), db.reset_was_asked()), (vec![B.to_owned(), C.to_owned()], false));
    }

    #[test]
    fn the_party_ending_closes_hp_but_yours() {
        let db = DataStorage::new();
        let clock = Clock::at(1_000);
        db.set_local_player_id(Some(1000));
        party(&db, &[(A, member(1)), (B, member(2))], true);
        db.note_party_hp(2000, 500);
        db.note_hp(1000, 900);
        clock.to(5_000);
        party(&db, &[(A, member(1))], true);
        assert_eq!([last_span(&db, 2000), last_span(&db, 1000)], [Some((1_000, 5_000)), Some((1_000, i64::MAX))]);
        assert!(db.inner.read().party_hp_ids.is_empty());
    }

    #[test]
    fn leaving_ends_the_party_only() {
        let db = DataStorage::new();
        let clock = Clock::at(1_000);
        db.set_local_player_id(Some(1000));
        db.note_map_load(600002);
        db.enter_dungeon(600002);
        db.enter_dungeon(600021);
        db.note_party_key(7);
        party(&db, &[(A, member(1)), (B, member(2))], true);
        db.note_party_hp(2000, 500);
        db.note_hp(1000, 900);
        clock.to(5_000);
        db.leave_party(8);
        assert_eq!(members(&db).len(), 2, "another party's key");
        db.leave_party(7);
        assert!(members(&db).is_empty() && db.inner.read().party_hp_ids.is_empty());
        assert_eq!([last_span(&db, 2000), last_span(&db, 1000)], [Some((1_000, 5_000)), Some((1_000, i64::MAX))]);
        assert_eq!((db.reset_was_asked(), db.current_dungeon_id()), (false, 600002));
        db.note_map_load(600021);
        assert_eq!(db.current_dungeon_id(), 0, "the queue went with the party");
    }

    #[test]
    fn a_party_left_during_a_fight_goes_once_the_fight_is_over_and_saved() {
        let db = DataStorage::new();
        db.register_boss(900);
        db.note_party_key(7);
        party(&db, &[(A, member(1)), (B, member(2))], true);
        rec(&db, 1454, 900, 20_010_000, 100, 1_000);
        rec(&db, 1454, 900, 20_010_000, 100, 7_000);
        db.leave_party(7);
        db.leave_party(7);
        let seated = |now: i64| {
            db.retire_idle(now, |_, _| false);
            members(&db).len()
        };
        assert_eq!(seated(40_000), 2, "the fight is live");
        db.clear_fight_totals();
        assert_eq!(seated(41_000), 2, "over; the save reads the party first");
        assert_eq!(db.take_ended_segments().len(), 1);
        assert_eq!((seated(42_000), db.reset_was_asked()), (0, false));
    }

    #[test]
    fn a_roster_after_a_leave_during_a_fight_seats_its_party() {
        let db = DataStorage::new();
        db.note_party_key(7);
        party(&db, &[(A, member(1)), (B, member(2))], true);
        rec(&db, 1454, 900, 20_010_000, 100, AT);
        db.leave_party(7);
        db.note_party_key(8);
        party(&db, &[(A, member(1)), (C, member(2))], true);
        db.clear_fight_totals();
        db.retire_idle(AT + 60_000, |_, _| false);
        assert_eq!(members(&db), [A.to_owned(), C.to_owned()]);
    }

    #[test]
    fn named_ids_left_out_of_a_whole_roster_have_left() {
        let db = DataStorage::new();
        let clock = Clock::at(1_000);
        db.set_local_player_id(Some(1000));
        for (id, who) in [(1000, ME), (2000, A), (3000, "Gone_x")] {
            db.name_entity(id, who);
        }
        party(&db, &[(A, member(1)), ("Gone_x", member(2))], true);
        [1000, 2000, 3000, 4000].iter().for_each(|&id| db.note_party_hp(id, 500));
        clock.to(2_000);
        party(&db, &[(A, member(1)), ("New_x", member(2))], true);
        let spans = [1000, 2000, 3000, 4000].map(|id| last_span(&db, id).map(|span| span.1));
        assert_eq!(spans, [Some(i64::MAX), Some(i64::MAX), Some(2_000), Some(i64::MAX)], "you, a member, gone, unnamed");
        let mut hp_ids: Vec<i32> = db.inner.read().party_hp_ids.iter().copied().collect();
        hp_ids.sort();
        assert_eq!(hp_ids, [1000, 2000, 4000]);

        // Some members only: updated by name, nobody leaves.
        let busy = RosterEntry { slot: 1, level: 50, ..Default::default() };
        db.replace_roster(vec![(A.to_owned(), busy)], false);
        assert_eq!((members(&db), db.roster_entries()[A].level), (vec![A.to_owned(), "New_x".to_owned()], 50));
    }

    #[test]
    fn a_roster_of_one_names_nobody() {
        let db = DataStorage::new();
        party(&db, &[(G_A, member_as(1, Gladiator))], true);
        gladiator(&db, 101, 6, false);
        assert_eq!(name_of(&db, 101), "-");
    }

    #[test]
    fn of_two_unnamed_players_of_a_class_only_a_clear_lead_names_one() {
        let pair = [(G_A, member_as(1, Gladiator)), (T_B, member_as(2, Templar))];
        let named = |first: i32, second: i32| {
            let db = DataStorage::new();
            gladiator(&db, 101, first, false);
            gladiator(&db, 102, second, false);
            party(&db, &pair, true);
            [name_of(&db, 101), name_of(&db, 102)]
        };
        assert_eq!(named(6, 2), [G_A, "-"]);
        assert_eq!(named(5, 2), ["-", "-"], "5 is under 3 times 2");
    }

    #[test]
    fn a_skill_with_and_without_its_ticks_is_one_skill() {
        let db = DataStorage::new();
        gladiator(&db, 101, 2, true);
        gladiator(&db, 102, 6, false);
        party(&db, &[(G_A, member_as(1, Gladiator)), (T_B, member_as(2, Templar))], true);
        assert_eq!([name_of(&db, 101), name_of(&db, 102)], ["-", G_A]);
    }

    #[test]
    fn a_linked_actor_is_never_named_from_the_roster() {
        let db = DataStorage::new();
        gladiator(&db, 101, 6, false);
        db.inner.write().summon_storage.insert(101, 5);
        party(&db, &[(G_A, member_as(1, Gladiator)), (T_B, member_as(2, Templar))], true);
        assert_eq!(name_of(&db, 101), "-");
    }

    /// 64 rounds on target 900; in each, every actor in turn uses one of
    /// `kinds` skills of class `class_no`.
    fn cast_cycle(db: &DataStorage, who: &[i32], class_no: i32, kinds: i32) {
        for round in 0..64i32 {
            let skill = class_no * 1_000_000 + 10_000 + 10 * (round % kinds);
            for &one in who {
                rec(db, one, 900, skill, 100, i64::from(round));
            }
        }
    }

    #[test]
    fn second_same_class_member_named_by_elimination() {
        let db = DataStorage::new();
        let elementalist = |slot| RosterEntry { job: Some(Elementalist), slot, ..RosterEntry::default() };
        party(&db, &[(A, elementalist(1)), (B, elementalist(2))], true);
        for who in [1792, 13520] {
            for nth in 1..=4 {
                rec(&db, who, 900, 16_000_000 + nth * 10_000, 100, AT);
            }
        }
        assert_eq!(name_of(&db, 13520), "-");
        db.name_entity(1792, A);
        assert_eq!(name_of(&db, 13520), B);
    }

    #[test]
    fn roster_binds_unique_class() {
        let db = DataStorage::new();
        party(&db, &[(A, member_as(1, Gladiator)), (B, member_as(2, Templar))], true);
        cast_cycle(&db, &[101], 11, 6);
        cast_cycle(&db, &[102], 12, 6);
        assert_eq!([name_of(&db, 101), name_of(&db, 102)], [A, B]);
    }

    #[test]
    fn ambiguous_class_stays_unbound() {
        let db = DataStorage::new();
        party(&db, &[(A, member_as(1, Gladiator)), (B, member_as(2, Gladiator))], true);
        cast_cycle(&db, &[101], 11, 6);
        assert_eq!(name_of(&db, 101), "-");

        let db = DataStorage::new();
        party(&db, &[(A, member_as(1, Gladiator)), (B, member_as(2, Templar))], true);
        cast_cycle(&db, &[101, 103], 11, 6);
        assert_eq!([name_of(&db, 101), name_of(&db, 103)], ["-", "-"]);
        db.name_entity(103, C);
        cast_cycle(&db, &[101], 11, 6);
        assert_eq!(name_of(&db, 101), A);
    }

    #[test]
    fn outside_players_block_class_binding() {
        let db = DataStorage::new();
        party(&db, &[(A, member_as(1, Gladiator)), (B, member_as(2, Templar))], true);
        db.name_entity(100, B);
        for (who, class_no) in [(100, 12), (102, 14), (104, 17), (101, 11)] {
            cast_cycle(&db, &[who], class_no, 6);
        }
        // More unnamed players than open roster names.
        assert_eq!(name_of(&db, 101), "-");
    }

    #[test]
    fn named_member_not_bound_twice() {
        let db = DataStorage::new();
        db.name_entity(101, A);
        party(&db, &[(A, member_as(1, Gladiator)), (B, member_as(2, Templar))], true);
        cast_cycle(&db, &[101], 11, 6);
        cast_cycle(&db, &[105], 11, 6);
        assert_eq!([name_of(&db, 101), name_of(&db, 105)], [A, "-"]);
    }

    #[test]
    fn roster_placeholders_expire_and_hide() {
        let db = DataStorage::new();
        let start = 1_700_000_000_000;
        let clock = Clock::at(start);
        let wanted = || db.ghost_rows_shown();
        assert!(!wanted());
        let pair = [(A, member(2)), (B, member(3))];
        party(&db, &pair, true);
        assert!(wanted());
        db.drop_ghost_rows();
        assert!(!wanted());
        party(&db, &pair, true);
        assert!(wanted());
        clock.to(start + 15 * 60_000);
        assert!(!wanted());
        assert_eq!(2, db.roster_entries().len());
    }

    #[test]
    fn map_tables_read_their_ids() {
        let ids = map_list(r#"{"source": "test", "maps": [1010, 101021]}"#);
        assert_eq!(ids, HashSet::from([1010, 101021]));
        assert!(map_list("not json").is_empty() && map_list(r#"{"maps": "1010"}"#).is_empty());
        assert_eq!([1010, 101021, 20].map(is_open_world_map), [true, true, false]);
    }

    #[test]
    fn a_queue_waits_through_an_own_dungeon() {
        let db = DataStorage::new();
        db.note_map_load(1010);
        db.enter_dungeon(600021);
        let mut seen = Vec::new();
        for map in [310051, 600021] {
            db.note_map_load(map);
            seen.push(db.current_dungeon_id());
        }
        assert_eq!(seen, [310051, 600021]);
    }

    #[test]
    fn a_dungeon_id_of_zero_or_less_is_no_dungeon() {
        let db = DataStorage::new();
        db.enter_dungeon(600002);
        let first = db.current_dungeon_id();
        db.enter_dungeon(0);
        db.enter_dungeon(-1);
        assert_eq!([first, db.current_dungeon_id()], [600002; 2], "set before any map load, then kept");
    }
}
