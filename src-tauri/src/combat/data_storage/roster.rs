//! The party roster and the dungeon it names, and naming members by class.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;

use crate::entity::job_class::JobClass;

use super::names::append_nickname_inner;
use super::{now_ms, DataStorage, Inner, PartyMember, PARTY_PLACEHOLDER_MS};

impl DataStorage {
    /// Take a party roster from a `0x9702` packet.
    ///
    /// `complete` says whether every member the packet declared was decoded. A
    /// complete roster replaces what we had, so a member who left the party
    /// disappears; a partial one only updates the members it did decode, so a
    /// record this parser trips over costs that member a refresh rather than
    /// costing the whole party their combat power.
    pub fn set_party_roster(&self, members: Vec<(String, PartyMember)>, complete: bool) {
        if members.is_empty() {
            return;
        }
        self.touch();
        let mut inner = self.inner.write();
        inner.party_roster_at_ms = now_ms();
        inner.party_placeholders_hidden = false;

        // Leaving, being kicked, or the party disbanding all show up the same way:
        // the next complete roster has you on your own. Going from a real party
        // down to one member means the party is over, so drop the roster rows and
        // ask for a combat reset — otherwise the meter keeps showing teammates who
        // are no longer with you.
        let was_in_party = inner.party_members.len() >= 2;
        let now_alone = complete && members.len() <= 1;
        let local_name = inner.local_character_name.clone();
        let dropped_self = complete
            && local_name.as_ref().is_some_and(|n| {
                !n.trim().is_empty() && !members.iter().any(|(name, _)| name.trim() == n.trim())
            });

        if was_in_party && (now_alone || dropped_self) {
            tracing::info!(
                "Party ended ({} -> {} members) — clearing party rows",
                inner.party_members.len(),
                members.len()
            );
            inner.party_members.clear();
            inner.current_dungeon_id = 0;
            inner.queued_dungeon_id = 0;
            drop(inner);
            self.flush_combat_only();
            self.combat_reset_requested.store(true, Ordering::Relaxed);
            return;
        }

        if complete {
            inner.party_members.clear();
        }
        for (name, member) in members {
            inner.party_members.insert(name, member);
        }
        bind_roster_names_by_class(&mut inner);
    }

    /// A zone load named the map it loads (`21 36`). The roster never sends 0
    /// for the open world, so a load into an open-world map is what ends the
    /// last instance's dungeon id. A teleport inside an instance names the
    /// instance's own map, so it keeps the id. A load into an instance takes
    /// the one the party queued for, if a roster named one.
    pub fn note_map_load(&self, map_id: i32) {
        let mut inner = self.inner.write();
        inner.own_records.zone_loaded();
        inner.in_open_world = is_open_world_map(map_id);
        if !inner.in_open_world {
            let queued = std::mem::take(&mut inner.queued_dungeon_id);
            if queued != 0 {
                inner.current_dungeon_id = queued;
            }
            return;
        }
        if inner.current_dungeon_id != 0 {
            tracing::debug!(
                "Map {map_id} is open world: leaving dungeon {}",
                inner.current_dungeon_id
            );
            inner.current_dungeon_id = 0;
        }
    }

    /// The instance a party roster names. In the open world that is the one
    /// the party queued for, named up to minutes before the load into it
    /// (2026-10-04 captures: 46 seconds and 3 minutes), so it waits for that
    /// load.
    pub fn set_current_dungeon(&self, dungeon_id: i32) {
        if dungeon_id <= 0 {
            return;
        }
        let mut inner = self.inner.write();
        if inner.in_open_world {
            inner.queued_dungeon_id = dungeon_id;
        } else {
            inner.current_dungeon_id = dungeon_id;
        }
    }

    pub fn current_dungeon_id(&self) -> i32 {
        self.inner.read().current_dungeon_id
    }

    pub fn get_party_members(&self) -> HashMap<String, PartyMember> {
        self.inner.read().party_members.clone()
    }

    /// Whether party members who have not fought should still be shown, with
    /// 0 damage, as a reminder of who is in the party.
    ///
    /// The game sends a roster on every party change, but nothing reliable
    /// when you go off on your own afterwards, so a dungeon party could stay
    /// on the meter long after (a player saw theirs 15 minutes on, through
    /// resets). These rows are for the start of a run: they show for
    /// `PARTY_PLACEHOLDER_MS` after the last roster, and a manual reset clears
    /// them until the next one. Members who fight are shown regardless.
    pub fn party_placeholders_wanted(&self) -> bool {
        let inner = self.inner.read();
        !inner.party_placeholders_hidden
            && now_ms() - inner.party_roster_at_ms < PARTY_PLACEHOLDER_MS
    }

    /// The player reset the meter: stop showing party members who have not
    /// fought, until the game sends the next roster.
    pub fn hide_party_placeholders(&self) {
        self.inner.write().party_placeholders_hidden = true;
    }
}

/// Open-world map ids from the game's Map table: the overworld maps and their
/// world layers (the overworld itself, split off for quest scenes).
static OPEN_WORLD_MAPS: std::sync::LazyLock<HashSet<i32>> = std::sync::LazyLock::new(|| {
    #[derive(serde::Deserialize)]
    struct Table {
        maps: HashSet<i32>,
    }
    serde_json::from_str::<Table>(include_str!("../../../../src/data/open_world_maps.json"))
        .map(|t| t.maps)
        .unwrap_or_default()
});

/// True for a map of the open world. Unknown ids (a map added by a later
/// patch) count as instances, which keeps the dungeon id as before.
pub fn is_open_world_map(map_id: i32) -> bool {
    OPEN_WORLD_MAPS.contains(&map_id)
}

/// Name party members whose entity id no packet has tied to their name yet,
/// by class: the roster gives each member's class, and so does the damage of
/// each player on the meter.
///
/// A player's id and name arrive together in their spawn (`45 36`), which the
/// game sends when they come into view. Start the meter with the party
/// already together and those spawns have been and gone: a player's capture
/// started in a dungeon showed four of five members as bare ids through two
/// bosses, until a later area re-sent the spawns (2026-10-03). The roster
/// arrived within seconds.
///
/// Only a pairing nothing else could explain is used: exactly one roster
/// member of a class without an entity, and exactly one unnamed player of
/// that class fighting now. Two of a class on either side are left alone,
/// unless one of the players clearly runs a rotation and the others only
/// repeat a skill or two (an aura or a spirit the spawn never covered).
/// When a party member has just been named, match the rest of the roster at
/// once. Naming one of two Spiritmasters settles the other by elimination,
/// but the match ran only every 64 damage records of the current fight, so
/// between pulls the other waited: 30 and 105 seconds in a 2026-10-02 run
/// with two Gladiators and two Spiritmasters, the second being the player.
pub(super) fn rebind_roster_after_naming(inner: &mut Inner, nickname: &str) {
    if inner.party_members.contains_key(nickname.trim()) {
        bind_roster_names_by_class(inner);
    }
}

pub(super) fn bind_roster_names_by_class(inner: &mut Inner) {
    if inner.party_members.len() < 2 {
        return;
    }
    let named: HashSet<&str> = inner.nickname_storage.values().map(|n| n.trim()).collect();
    let mut open: HashMap<JobClass, Vec<String>> = HashMap::new();
    for (name, member) in &inner.party_members {
        if let Some(job) = member.job
            && !named.contains(name.trim())
        {
            open.entry(job).or_default().push(name.clone());
        }
    }
    if open.is_empty() {
        return;
    }
    // Unnamed players in the current fight, with how many distinct skills each used.
    let mut skills: HashMap<i32, HashSet<i32>> = HashMap::new();
    for target in inner.target_combat.values() {
        for (&actor, data) in &target.actors {
            if inner.known_player_ids.contains(&actor)
                && !inner.nickname_storage.contains_key(&actor)
                && !inner.summon_storage.contains_key(&actor)
            {
                skills.entry(actor).or_default().extend(data.skills.keys().map(|&(code, _)| code));
            }
        }
    }
    // More unnamed players than unbound members means someone fighting is not
    // in the party (open world), and a stranger of the right class could be
    // the one matched. A rotation-less extra (a stray aura) counts here too;
    // that only delays naming until its owner's spawn does it.
    let open_names: usize = open.values().map(Vec::len).sum();
    if skills.len() > open_names {
        return;
    }
    let mut binds = Vec::new();
    for (job, names) in open {
        let [name] = names.as_slice() else { continue };
        let mut players: Vec<(i32, usize)> = skills
            .iter()
            .filter(|(id, _)| inner.actor_jobs.get(id) == Some(&job))
            .map(|(&id, s)| (id, s.len()))
            .collect();
        players.sort_by_key(|&(id, n)| (std::cmp::Reverse(n), id));
        let chosen = match players.as_slice() {
            [(id, _)] => *id,
            [(id, top), (_, next), ..] if *top >= 3 * *next => *id,
            _ => continue,
        };
        binds.push((chosen, name.clone()));
    }
    for (id, name) in binds {
        tracing::info!("Roster: {} is entity {}, the one unnamed player of their class", name, id);
        append_nickname_inner(inner, id, &name);
    }
}
