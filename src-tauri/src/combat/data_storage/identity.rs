//! Who the local player is: the game's self record, the loot vote, server and profile.

use std::collections::{HashMap, HashSet};

use crate::entity::job_class::JobClass;

use super::names::{give_name, my_name, Source};
use super::{DataStorage, Inner, SelfCard};

/// Entity ids run from 1 to this.
const TOP_ENTITY_ID: i32 = 9_999_999;
/// The lowest id the party scope takes.
const SCOPE_FLOOR: i32 = 100;
/// Ids the party scope holds at most.
const SCOPE_CAP: usize = 10_000;
/// Mobs one name's loot counts at most.
const KILLS_CAP: usize = 10_000;
/// Names whose loot is counted at most; past it, only a name looting under
/// an id in the party scope joins, as only those vote.
const LOOTERS_CAP: usize = 10_000;
/// Names whose server is kept at most.
const SERVERS_CAP: usize = 10_000;
/// Ids the records about you count at a time.
const COUNTED_CAP: usize = 64;
/// Records about one other id in a row that outvote a standing self record.
const RUN_TO_MOVE: u32 = 3;

impl DataStorage {
    /// The configured character name, as given. Nothing else changes.
    pub fn set_local_character_name(&self, name: Option<String>) {
        self.inner.write().local_character_name = name;
    }

    /// The self record named you (`name` is `None` for a tutorial
    /// character). True when this changes who you are, or replaces a guess
    /// from loot.
    pub fn set_local_identity_from_game(&self, id: i64, name: Option<String>) -> bool {
        let s = &mut *self.inner.write();
        let known = s.self_known_from_game
            && !s.loot_identity.guessed
            && s.local_player_id == Some(id)
            && s.local_character_name == name;
        s.loot_identity.guessed = false;
        take_identity(s, id, name);
        !known
    }

    pub fn local_character_name(&self) -> Option<String> {
        self.inner.read().local_character_name.clone()
    }

    /// `owner_id`, under `name`, looted `mob_id`. Loot names its owner, and
    /// the name with the most kills among the ids the game reports on to you
    /// alone is you. True when that changed who you are.
    pub fn claim_loot(&self, mob_id: i32, owner_id: i32, name: &str) -> bool {
        let s = &mut *self.inner.write();
        s.loot_identity.count(mob_id, owner_id, name);
        match s.loot_identity.vote() {
            Vote::Nobody => false,
            Vote::Tie => withdraw_guess(s),
            Vote::Leads(leader, id) => loot_names_you(s, leader, id),
        }
    }

    /// The game named you, and no loot guess stands in for it.
    pub fn local_identity_from_self_record(&self) -> bool {
        let s = self.inner.read();
        s.self_known_from_game && !s.loot_identity.guessed
    }

    /// A record of a kind the game sends about you alone (`OWN_RECORDS`)
    /// named `entity_id`. True when that changed who you are.
    pub fn note_own_record(&self, entity_id: i32) -> bool {
        if !(1..=TOP_ENTITY_ID).contains(&entity_id) {
            return false;
        }
        let s = &mut *self.inner.write();
        if s.self_known_from_game {
            record_against_the_game(s, entity_id)
        } else {
            record_alone(s, entity_id)
        }
    }

    pub fn self_known_from_game(&self) -> bool {
        self.inner.read().self_known_from_game
    }

    /// The game sent a `06 38` about `entity_id`: you, in some regions your
    /// party too, never a stranger. Only loot by these ids votes.
    pub fn mark_in_party(&self, entity_id: i32) {
        if !(SCOPE_FLOOR..=TOP_ENTITY_ID).contains(&entity_id) {
            return;
        }
        let s = &mut *self.inner.write();
        let scope = &mut s.loot_identity.scope;
        if scope.len() < SCOPE_CAP {
            scope.insert(entity_id);
        }
    }

    /// Another id set as you ends the records' move past the game's word.
    pub fn set_local_player_id(&self, id: Option<i64>) {
        let s = &mut *self.inner.write();
        let moved = s.local_player_id != id;
        s.own_records.past_game &= !moved;
        s.local_player_id = id;
    }

    /// A player's server, from their name's record. Server ids run from 1000
    /// to 2999; anything else is a misread.
    pub fn remember_home_server(&self, name: &str, server_id: u16) {
        if !(1000..3000).contains(&server_id) {
            return;
        }
        let s = &mut *self.inner.write();
        let room = s.player_servers.len() < SERVERS_CAP;
        if let Some(known) = s.player_servers.get_mut(name) {
            *known = server_id;
        } else if room {
            s.player_servers.insert(name.to_owned(), server_id);
        }
    }

    pub fn local_player_id(&self) -> Option<i64> {
        self.inner.read().local_player_id
    }

    /// Every player's server a name's record gave (loot and summon records), by name.
    pub fn player_servers(&self) -> HashMap<String, u16> {
        self.inner.read().player_servers.clone()
    }

    /// Class and level from the self record. A field that did not read
    /// keeps what an earlier record of the same character said.
    pub fn store_self_card(&self, name: &str, class: Option<JobClass>, level: Option<u32>) {
        let s = &mut *self.inner.write();
        let earlier = s.self_profile.take().filter(|p| p.0 == name).map_or((None, None), |p| (p.1, p.2));
        s.self_profile = Some((name.to_owned(), class.or(earlier.0), level.or(earlier.1)));
    }

    /// Who you are playing: name, server, class and level as far as known.
    pub fn self_card(&self) -> SelfCard {
        let s = self.inner.read();
        let (class, level) = match &s.self_profile {
            Some((said, class, level)) if Some(said.trim()) == configured_name(&s) => (*class, *level),
            _ => (None, None),
        };
        let from_hits = || s.local_player_id.and_then(|id| s.actor_jobs.get(&(id as i32)).copied());
        SelfCard {
            name: s.local_character_name.clone(),
            server_id: fight_server(&s),
            class: class.or_else(from_hits),
            level,
        }
    }

    /// The server a saved fight is filed under; 0 when not known.
    pub fn home_server(&self) -> u16 {
        fight_server(&self.inner.read())
    }
}

/// What loot has said about who you are.
#[derive(Default, Clone)]
pub(super) struct LootClaim {
    looters: HashMap<String, Looter>,
    /// Ids from `mark_in_party`. Kept all session.
    scope: HashSet<i32>,
    /// The identity in force is this vote's guess.
    guessed: bool,
}

#[derive(Default, Clone)]
struct Looter {
    /// The id the name looted under last.
    id: i32,
    kills: HashSet<i32>,
}

enum Vote {
    Nobody,
    Tie,
    Leads(String, i64),
}

impl LootClaim {
    fn count(&mut self, mob: i32, owner: i32, name: &str) {
        let full = self.looters.len() >= LOOTERS_CAP && !self.looters.contains_key(name);
        if full && !self.scope.contains(&owner) {
            return;
        }
        let looter = self.looters.entry(name.to_owned()).or_default();
        looter.id = owner;
        if looter.kills.len() < KILLS_CAP {
            looter.kills.insert(mob);
        }
    }

    /// The name with the most kills among those whose last id is in scope;
    /// a tie at the top leads nobody.
    fn vote(&self) -> Vote {
        let in_scope = self.looters.iter().filter(|(_, l)| self.scope.contains(&l.id));
        // Every looter has a kill: a top of 0 means no looter in scope.
        match leading(in_scope.map(|(name, l)| ((name, l.id), l.kills.len()))) {
            (Some((name, id)), _) => Vote::Leads(name.clone(), i64::from(id)),
            (None, 0) => Vote::Nobody,
            (None, _) => Vote::Tie,
        }
    }
}

/// You are (id, name) by the game's word or the loot's. The caller puts the
/// name in the table.
fn take_identity(s: &mut Inner, id: i64, name: Option<String>) {
    s.own_records.streak = None;
    s.own_records.past_game = false;
    if let Some(me) = name.as_deref().map(str::trim).filter(|me| !me.is_empty()) {
        s.permanent_nicknames.retain(|&other, kept| i64::from(other) == id || kept.trim() != me);
    }
    s.self_known_from_game = true;
    s.local_player_id = Some(id);
    s.local_character_name = name;
}

/// Two names tied in loot: a guess from loot no longer holds.
fn withdraw_guess(s: &mut Inner) -> bool {
    let had_guess = std::mem::take(&mut s.loot_identity.guessed);
    if had_guess {
        s.self_known_from_game = false;
        s.local_player_id = None;
    }
    had_guess
}

/// The loot leader becomes you, unless something surer has spoken or it is
/// you already.
fn loot_names_you(s: &mut Inner, leader: String, id: i64) -> bool {
    let self_record_spoke = s.self_known_from_game && !s.loot_identity.guessed;
    let records_moved_on = s.own_records.past_game && s.local_player_id != Some(id);
    let you_already = s.self_known_from_game
        && s.local_player_id == Some(id)
        && s.local_character_name.as_deref() == Some(leader.as_str());
    if self_record_spoke || records_moved_on || you_already || configured_name_seen(s, &leader) {
        return false;
    }
    s.loot_identity.guessed = true;
    take_identity(s, id, Some(leader));
    true
}

/// Your configured name is on your id already, and `leader` is someone
/// else: a party member's loot cannot outvote it.
fn configured_name_seen(s: &Inner, leader: &str) -> bool {
    let Some(me) = configured_name(s) else { return false };
    let on_your_id = s.local_player_id.and_then(|id| s.nickname_storage.get(&(id as i32)));
    me != leader && on_your_id.is_some_and(|held| held == me)
}

/// While the self record's word stands: only an unbroken run about one
/// other id overrules it (a missed self record after a zone load kept the
/// old id through a boss fight).
fn record_against_the_game(s: &mut Inner, id: i32) -> bool {
    let records = &mut s.own_records;
    if s.local_player_id == Some(i64::from(id)) {
        records.streak = None;
        return false;
    }
    let run = match records.streak {
        Some((same, n)) if same == id => n + 1,
        _ => 1,
    };
    if run < RUN_TO_MOVE {
        records.streak = Some((id, run));
        return false;
    }
    records.streak = None;
    records.counts = HashMap::from([(id, run)]);
    records.bound = Some(i64::from(id));
    records.past_game = true;
    s.loot_identity.guessed = false;
    s.self_known_from_game = false;
    // The configured name may be another character's.
    s.local_character_name = None;
    s.local_player_id = Some(i64::from(id));
    true
}

/// Without the self record: the id named most, by two or more, is you; a
/// tie at the top means not knowing.
fn record_alone(s: &mut Inner, id: i32) -> bool {
    let records = &mut s.own_records;
    let room = records.counts.len() < COUNTED_CAP;
    if let Some(n) = records.counts.get_mut(&id) {
        *n += 1;
    } else if room {
        records.counts.insert(id, 1);
    }
    let choice = match leading(records.counts.iter().map(|(&id, &n)| (id, n))) {
        (None, _) => None,
        (Some(top), most) if most >= 2 => Some(i64::from(top)),
        _ => return false,
    };
    // An id set some other way is not these records' to replace.
    if s.local_player_id.is_some() && s.local_player_id != records.bound {
        return false;
    }
    records.bound = choice;
    if s.local_player_id == choice {
        return false;
    }
    s.local_player_id = choice;
    let unnamed = choice.map(|id| id as i32).filter(|id| !s.nickname_storage.contains_key(id));
    if let (Some(id), Some(me)) = (unnamed, my_name(s).map(str::to_owned)) {
        give_name(s, id, &me, Source::Stated);
    }
    true
}

/// The key counted most and its count; `None` when two share the top.
fn leading<K, N: Copy + Default + Ord>(counts: impl IntoIterator<Item = (K, N)>) -> (Option<K>, N) {
    let mut top = (None, N::default());
    for (key, n) in counts {
        if n > top.1 {
            top = (Some(key), n);
        } else if n == top.1 {
            top.0 = None;
        }
    }
    top
}

/// Your configured name, trimmed, kept even when blank: the profile, the
/// server and the loot vote compare it as set, where binding it (`my_name`)
/// skips a blank one.
fn configured_name(s: &Inner) -> Option<&str> {
    s.local_character_name.as_deref().map(str::trim)
}

/// The server of your name's record, else of your roster entry, else the
/// one most roster members are on.
fn fight_server(s: &Inner) -> u16 {
    let me = configured_name(s);
    let recorded = me.and_then(|me| s.player_servers.get(me)).copied();
    let your_entry = me.and_then(|me| s.party_members.get(me)).map(|m| m.server_id).filter(|&id| id != 0);
    recorded.or(your_entry).unwrap_or_else(|| roster_server(s))
}

/// The server most roster members are on, the lowest of a tie; 0 for none.
fn roster_server(s: &Inner) -> u16 {
    let mut servers: Vec<u16> = s.party_members.values().map(|m| m.server_id).filter(|&id| id != 0).collect();
    servers.sort_unstable();
    let mut best: &[u16] = &[];
    for run in servers.chunk_by(|a, b| a == b) {
        if run.len() > best.len() {
            best = run;
        }
    }
    best.first().copied().unwrap_or(0)
}

/// What the records about you alone have said since the last zone load. See
/// `note_own_record`.
#[derive(Default, Clone)]
pub(super) struct OwnRecords {
    /// How many each id got. Entity ids are handed out per zone load, so a
    /// zone load starts this over.
    counts: HashMap<i32, u32>,
    /// The local player these records put in force.
    bound: Option<i64>,
    /// The last entity they named in a row while the game's word stood,
    /// other than the id in force, and how many times.
    streak: Option<(i32, u32)>,
    /// They moved you past the game's word, and nothing has spoken since.
    past_game: bool,
}

impl OwnRecords {
    pub(super) fn zone_loaded(&mut self) {
        self.counts.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::data_storage::RosterEntry;
    use crate::combat::data_storage::damage::rec;
    use crate::combat::data_storage::roster::{name_of, A, AT, B, ME};
    use crate::combat::data_storage::tests::{call_me, game_says, me, member, seat, whoami};

    const OTHER: &str = "Other_x";

    fn scope(ids: &[i32]) -> DataStorage {
        let db = DataStorage::new();
        ids.iter().for_each(|&id| db.mark_in_party(id));
        db
    }

    /// Own records about each id in turn; what each call returned.
    fn own(db: &DataStorage, ids: &[i32]) -> Vec<bool> {
        ids.iter().map(|&id| db.note_own_record(id)).collect()
    }

    #[test]
    fn a_configured_name_alone_says_nothing_else() {
        let db = DataStorage::new();
        call_me(&db, " Me_x ");
        assert_eq!(whoami(&db), me(None, " Me_x ", false));
        assert!(!db.local_identity_from_self_record());
    }

    #[test]
    fn a_tutorial_character_is_you_without_a_name() {
        let db = DataStorage::new();
        let first = db.set_local_identity_from_game(3877, None);
        assert_eq!((first, whoami(&db)), (true, (Some(3877), None, true)));
        assert!(db.local_identity_from_self_record());
        let again = db.set_local_identity_from_game(3877, None);
        let changes = [again, game_says(&db, 3877, "Tut_x"), game_says(&db, 3878, "Tut_x"), game_says(&db, 3878, "Tut_x")];
        assert_eq!(changes, [false, true, true, false], "the same, then a name, another id, the same");
        assert_eq!(name_of(&db, 3878), "-", "the caller binds the name");
    }

    #[test]
    fn the_self_record_takes_over_a_loot_guess() {
        let db = scope(&[1454]);
        let guessed = db.claim_loot(900, 1454, A);
        assert_eq!([guessed, db.self_known_from_game(), db.local_identity_from_self_record()], [true, true, false]);
        assert!(game_says(&db, 1454, A), "the same id and name, now stated");
        assert!(db.local_identity_from_self_record());
    }

    #[test]
    fn the_game_naming_you_drops_your_name_kept_on_other_ids() {
        let db = DataStorage::new();
        for (id, kept) in [(5, format!(" {ME}")), (6, OTHER.to_owned())] {
            db.fix_name(id, &kept);
        }
        game_says(&db, 9, "Me_x ");
        db.forget_names();
        assert_eq!([5, 6, 9].map(|id| name_of(&db, id)), ["-", OTHER, ME]);
    }

    #[test]
    fn another_id_set_by_hand_ends_the_records_move() {
        let db = scope(&[7001, 8123]);
        game_says(&db, 4294, ME);
        assert_eq!(own(&db, &[7001, 7001, 7001]), [false, false, true]);
        let blocked = db.claim_loot(900, 8123, B);
        db.set_local_player_id(Some(7001));
        let still = db.claim_loot(901, 8123, B);
        db.set_local_player_id(Some(5));
        let free = db.claim_loot(902, 8123, B);
        assert_eq!([blocked, still, free], [false, false, true], "moved; the same id keeps it; another ends it");
        assert_eq!(whoami(&db), me(Some(8123), B, true));
    }

    #[test]
    fn the_name_with_more_kills_leads() {
        let db = scope(&[1454, 3583]);
        let votes: Vec<bool> = (1..=3).map(|mob| db.claim_loot(mob, 1454, A)).collect();
        let late = db.claim_loot(10, 3583, B);
        assert_eq!((votes, late), (vec![true, false, false], false), "three kills to one");
        assert_eq!(whoami(&db), me(Some(1454), A, true));
    }

    #[test]
    fn a_tie_without_a_guess_in_force_changes_nothing() {
        let db = scope(&[1454, 3583]);
        game_says(&db, 42, ME);
        let votes = [db.claim_loot(1, 1454, A), db.claim_loot(2, 3583, B)];
        assert_eq!(votes, [false; 2]);
        assert_eq!(whoami(&db), me(Some(42), ME, true));
    }

    #[test]
    fn a_name_looting_from_outside_the_scope_stops_voting() {
        let db = scope(&[1454, 3583]);
        let first = [db.claim_loot(1, 1454, A), db.claim_loot(2, 3583, B)];
        let withdrawn = whoami(&db);
        // B_x's latest id is out of scope.
        let back = db.claim_loot(3, 9999, B);
        assert_eq!((first, withdrawn, back), ([true, true], (None, Some(A.into()), false), true));
        assert_eq!(whoami(&db), me(Some(1454), A, true));
    }

    #[test]
    fn a_mob_counts_once_and_kills_stop_at_the_cap() {
        let db = scope(&[1454, 3583]);
        db.claim_loot(1, 1454, A);
        db.claim_loot(1, 1454, A);
        assert!(db.claim_loot(2, 3583, B), "one kill each");

        let db = scope(&[1454, 3583]);
        (0..10_005).for_each(|mob| _ = db.claim_loot(mob, 1454, A));
        let changed: Vec<i32> = (0..10_000).filter(|&mob| db.claim_loot(mob, 3583, B)).collect();
        assert_eq!(changed, [9_999], "a tie at 10,000 each");
        assert_eq!(db.local_player_id(), None);
    }

    #[test]
    fn past_the_looter_cap_only_names_in_scope_join() {
        let db = scope(&[1454]);
        for n in 0..LOOTERS_CAP {
            db.inner.write().loot_identity.looters.insert(format!("P_{n}"), Looter::default());
        }
        let late = db.claim_loot(1, 9999, "Late_x");
        let ours = db.claim_loot(2, 1454, A);
        let s = db.inner.read();
        let looters = &s.loot_identity.looters;
        assert_eq!((late, ours, looters.len(), looters.contains_key("Late_x")), (false, true, LOOTERS_CAP + 1, false));
    }

    #[test]
    fn loot_guess_names_you_until_self_record() {
        let db = scope(&[1454]);
        call_me(&db, A);
        assert!(db.claim_loot(900, 1454, B));
        assert_eq!(whoami(&db), me(Some(1454), B, true));
        assert!(!db.claim_loot(900, 1454, B), "the same call again");
        assert!(game_says(&db, 2001, B));
        assert!(!db.claim_loot(900, 1454, B));
        assert_eq!(whoami(&db), me(Some(2001), B, true));
    }

    #[test]
    fn tied_loot_votes_withdraw_the_guess() {
        let db = scope(&[1454, 3583]);
        db.claim_loot(900, 1454, A);
        assert!(db.claim_loot(901, 3583, B));
        assert_eq!(whoami(&db), me(None, A, false));
        // The same kill again is no new vote.
        assert!(!db.claim_loot(900, 1454, A));
        assert_eq!(db.local_player_id(), None);
        assert!(db.claim_loot(902, 1454, A));
        assert_eq!(whoami(&db), me(Some(1454), A, true));
    }

    #[test]
    fn bystander_loot_never_wins_in_either_order() {
        let fresh = || {
            let db = scope(&[13978]);
            call_me(&db, A);
            db
        };
        let db = fresh();
        let votes = [db.claim_loot(1, 13978, A), db.claim_loot(2, 15855, B)];
        assert_eq!((votes, db.local_player_id()), ([true, false], Some(13978)));

        let db = fresh();
        let votes = [db.claim_loot(1, 15855, B), db.claim_loot(2, 13978, A)];
        assert_eq!((votes, db.local_player_id()), ([false, true], Some(13978)));
        assert!(!db.local_identity_from_self_record());
    }

    #[test]
    fn loot_outside_party_scope_is_ignored() {
        let db = scope(&[3001]);
        assert!(!db.claim_loot(50_101, 3100, A));
        assert!(db.claim_loot(50_102, 3001, B));
        assert!((50_103..=50_105).all(|mob| !db.claim_loot(mob, 3200, OTHER)));
        assert_eq!(whoami(&db), me(Some(3001), B, true));
    }

    #[test]
    fn own_records_name_you_after_two() {
        let db = DataStorage::new();
        call_me(&db, A);
        assert!(!db.note_own_record(4321));
        assert_eq!(db.local_player_id(), None);
        assert!(db.note_own_record(4321));
        assert_eq!(whoami(&db), me(Some(4321), A, false));
        assert_eq!(name_of(&db, 4321), A);
        assert!(!db.local_identity_from_self_record());

        game_says(&db, 5678, A);
        assert_eq!(own(&db, &[4321, 4321, 5678, 4321, 4321]), [false; 5]);
        assert_eq!(db.local_player_id(), Some(5678));
    }

    #[test]
    fn own_records_follow_a_new_entity_after_three() {
        let db = scope(&[7001, 8123]);
        game_says(&db, 4294, A);
        db.name_entity(4294, A);
        assert_eq!(own(&db, &[7001, 7001, 7001]), [false, false, true]);
        assert_eq!(whoami(&db), (Some(7001), None, false));
        assert_eq!([name_of(&db, 7001), name_of(&db, 4294)], ["-", A]);
        assert!(!db.note_own_record(7001));

        assert!(!db.claim_loot(900, 8123, B));
        assert_eq!(db.local_player_id(), Some(7001));
        db.claim_loot(901, 7001, A);
        assert!(db.claim_loot(902, 7001, A));
        assert_eq!(whoami(&db).1.as_deref(), Some(A));
        assert_eq!(db.local_player_id(), Some(7001));
        game_says(&db, 7001, A);
        assert!(db.local_identity_from_self_record());
    }

    #[test]
    fn own_records_tie_clears_the_id() {
        let db = DataStorage::new();
        own(&db, &[4321, 4321, 8765]);
        assert_eq!(db.local_player_id(), Some(4321));
        assert!(db.note_own_record(8765));
        assert_eq!(db.local_player_id(), None);
        assert!(db.note_own_record(8765));
        assert_eq!(db.local_player_id(), Some(8765));
    }

    #[test]
    fn own_records_do_not_override_a_set_id() {
        let db = DataStorage::new();
        db.set_local_player_id(Some(1111));
        assert_eq!(own(&db, &[4321; 3]), [false; 3]);
        assert_eq!(db.local_player_id(), Some(1111));

        let db = scope(&[4321]);
        own(&db, &[4321, 4321]);
        assert!(db.claim_loot(900, 4321, A));
        assert_eq!(whoami(&db), me(Some(4321), A, true));
    }

    #[test]
    fn records_about_you_start_over_at_a_zone_load() {
        let db = DataStorage::new();
        own(&db, &[4321; 5]);
        db.note_map_load(1010);
        db.note_own_record(8765);
        assert_eq!(db.local_player_id(), Some(4321), "kept until the new id has two");
        db.note_own_record(8765);
        assert_eq!(db.local_player_id(), Some(8765));
    }

    #[test]
    fn self_profile_level_kept_when_unreadable() {
        let db = DataStorage::new();
        let level = |db: &DataStorage| db.self_card().level;
        game_says(&db, 3001, A);
        db.store_self_card(A, Some(JobClass::Cleric), Some(29));
        db.store_self_card(A, Some(JobClass::Cleric), None);
        assert_eq!(level(&db), Some(29));
        db.store_self_card(A, Some(JobClass::Cleric), Some(30));
        assert_eq!(level(&db), Some(30));
        db.store_self_card(B, None, None);
        game_says(&db, 1, B);
        assert_eq!(level(&db), None);
    }

    #[test]
    fn fight_server_from_self_then_roster() {
        let db = DataStorage::new();
        assert_eq!(0, db.home_server());
        let on = |server_id| RosterEntry { server_id, ..member(1) };
        seat(&db, &[(A, on(2304)), (B, on(2304)), (OTHER, on(1307))]);
        // The majority.
        assert_eq!(2304, db.home_server());
        game_says(&db, 7, OTHER);
        assert_eq!(1307, db.home_server());
        db.remember_home_server(OTHER, 1304);
        assert_eq!(1304, db.home_server());
        // No server has the number 99.
        db.remember_home_server(OTHER, 99);
        assert_eq!(1304, db.home_server());
    }

    #[test]
    fn hand_set_name_drops_old_ids_after_self_record() {
        let db = DataStorage::new();
        db.fix_name(6925, A);
        db.fix_name(6926, A);
        db.forget_names();
        assert_eq!(name_of(&db, 6925), "-");
        game_says(&db, 7577, A);
        db.forget_names();
        assert_eq!(name_of(&db, 6926), "-");
        assert_eq!(Some(7577), db.id_for_name(A));
    }

    #[test]
    fn your_configured_name_on_your_id_outvotes_loot_in_scope() {
        let db = scope(&[1454]);
        call_me(&db, " Te_x ");
        db.name_entity(4099, "Te_x");
        let found = db.local_player_id();
        let vote = db.claim_loot(900, 1454, "Tr_x");
        assert_eq!((found, vote), (Some(4099), false));
        assert_eq!(whoami(&db), me(Some(4099), " Te_x ", false));
    }

    #[test]
    fn own_records_outside_the_id_range_count_for_nothing() {
        let db = DataStorage::new();
        assert_eq!(own(&db, &[0, -5, 10_000_000, 0, -5, 10_000_000]), [false; 6]);
        assert!(db.inner.read().own_records.counts.is_empty());
        assert_eq!(own(&db, &[9_999_999, 9_999_999]), [false, true]);
        assert_eq!(db.local_player_id(), Some(9_999_999));
    }

    #[test]
    fn only_an_unbroken_run_overrules_the_self_record() {
        let db = DataStorage::new();
        game_says(&db, 5100, ME);
        assert_eq!(own(&db, &[6200, 6200, 6300, 6200, 6200, 6200]), [false, false, false, false, false, true]);
        assert_eq!(whoami(&db), (Some(6200), None, false));

        let db = DataStorage::new();
        game_says(&db, 5100, ME);
        assert_eq!(own(&db, &[6200, 6200, 5100, 6200]), [false; 4], "a record about you breaks the run");
        assert_eq!(db.local_player_id(), Some(5100));
    }

    #[test]
    fn a_zone_load_does_not_break_a_run() {
        let db = DataStorage::new();
        game_says(&db, 5100, ME);
        own(&db, &[6200, 6200]);
        db.note_map_load(1010);
        assert!(db.note_own_record(6200));
    }

    #[test]
    fn records_count_at_most_64_ids() {
        let db = DataStorage::new();
        let many: Vec<i32> = (1001..=1064).collect();
        assert!(own(&db, &many).iter().all(|&changed| !changed));
        assert_eq!(own(&db, &[2000, 2000]), [false; 2], "not counted");
        let held = db.local_player_id();
        assert_eq!((held, db.note_own_record(1001), db.local_player_id()), (None, true, Some(1001)));
    }

    #[test]
    fn the_id_records_choose_takes_your_trimmed_name_only_if_unme() {
        let db = DataStorage::new();
        call_me(&db, " Me_x ");
        assert_eq!(own(&db, &[4321, 4321]), [false, true]);
        let given = name_of(&db, 4321);
        db.append_nickname(4321, "Me_long_x");
        assert_eq!([given, name_of(&db, 4321)], [ME, "Me_long_x"], "trimmed, and not marked stated");

        let db = DataStorage::new();
        db.name_entity(4321, OTHER);
        call_me(&db, ME);
        assert_eq!(own(&db, &[4321, 4321]), [false, true]);
        assert_eq!(name_of(&db, 4321), OTHER);

        let db = DataStorage::new();
        call_me(&db, "  ");
        assert_eq!(own(&db, &[4321, 4321]), [false, true]);
        assert_eq!(name_of(&db, 4321), "-");
    }

    #[test]
    fn party_scope_holds_entity_ids_only() {
        let db = scope(&[99, 10_000_000, 100, 9_999_999]);
        assert_eq!(db.inner.read().loot_identity.scope, HashSet::from([100, 9_999_999]));

        let db = scope(&(100..10_100).collect::<Vec<i32>>());
        db.mark_in_party(20_000);
        let s = db.inner.read();
        assert_eq!((s.loot_identity.scope.len(), s.loot_identity.scope.contains(&20_000)), (10_000, false));
    }

    #[test]
    fn servers_outside_the_range_are_misreads() {
        let db = DataStorage::new();
        for (who, server) in [(A, 999), (B, 3000), (ME, 1000), (OTHER, 2999)] {
            db.remember_home_server(who, server);
        }
        let mut kept: Vec<(String, u16)> = db.player_servers().into_iter().collect();
        kept.sort();
        assert_eq!(kept, [(ME.to_owned(), 1000), (OTHER.to_owned(), 2999)]);

        let db = DataStorage::new();
        (0..10_000).for_each(|n| db.remember_home_server(&format!("P_{n}"), 1001));
        db.remember_home_server("Late_x", 1002);
        db.remember_home_server("P_7", 1003);
        let servers = db.player_servers();
        assert_eq!((servers.len(), servers.get("Late_x"), servers.get("P_7")), (10_000, None, Some(&1003)));
    }

    #[test]
    fn your_profile_by_trimmed_name_with_your_class_from_your_hits() {
        let db = DataStorage::new();
        game_says(&db, 3001, ME);
        rec(&db, 3001, 900, 12_010_000, 100, AT);
        let p = db.self_card();
        assert_eq!((p.name, p.class, p.level), (Some(ME.to_owned()), Some(JobClass::Templar), None));

        call_me(&db, "Me_x ");
        db.store_self_card(" Me_x", Some(JobClass::Cleric), Some(29));
        db.store_self_card(" Me_x", None, None);
        let p = db.self_card();
        // Another exact name replaces the profile whole.
        db.store_self_card(ME, None, Some(30));
        let q = db.self_card();
        assert_eq!([(p.class, p.level), (q.class, q.level)], [(Some(JobClass::Cleric), Some(29)), (Some(JobClass::Templar), Some(30))]);
    }

    #[test]
    fn the_fight_server_falls_back_through_your_roster_entry_to_the_majority() {
        let on = |server_id| RosterEntry { server_id, ..member(1) };
        let db = DataStorage::new();
        seat(&db, &[(A, on(2304)), (B, on(1307))]);
        let tie = db.home_server();
        call_me(&db, " A_x ");
        assert_eq!([tie, db.home_server()], [1307, 2304], "a tie to the lower id, then your own entry");

        let db = DataStorage::new();
        seat(&db, &[(A, on(2304)), (B, on(2304)), (ME, on(0)), (OTHER, on(1307))]);
        call_me(&db, ME);
        let majority = db.home_server();
        db.remember_home_server(ME, 1500);
        assert_eq!([majority, db.home_server(), db.self_card().server_id], [2304, 1500, 1500]);
    }
}
