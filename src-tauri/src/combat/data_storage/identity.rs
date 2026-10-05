//! Who the local player is: the game's self record, the loot vote, server and profile.

use std::collections::{HashMap, HashSet};

use crate::entity::job_class::JobClass;

use super::{DataStorage, Inner, LocalProfile};

impl DataStorage {
    pub fn set_local_character_name(&self, name: Option<String>) {
        self.inner.write().local_character_name = name;
    }

    pub fn local_character_name(&self) -> Option<String> {
        self.inner.read().local_character_name.clone()
    }

    /// Record who the game says the local player is. `name` is `None` for a
    /// tutorial character. Returns whether anything changed.
    pub fn set_local_identity_from_game(&self, id: i64, name: Option<String>) -> bool {
        let mut inner = self.inner.write();
        let changed = !inner.local_identity_from_game
            || inner.local_player_id != Some(id)
            || inner.local_character_name != name
            || inner.loot_identity.applied;
        inner.loot_identity.applied = false;
        set_game_identity(&mut inner, id, name);
        changed
    }

    /// The loot from a mob that just died belongs to `owner_id`, named `name`.
    ///
    /// The self record that names you arrives on login and zone loads, so a
    /// meter started mid-session can go a long while without it. Loot records
    /// fill that gap: they name the player whose kill it was, and mostly that
    /// is you. Not always: a kill by someone nearby reaches you too, inside
    /// another packet (2026-10-04, "Deityclaire" a second into a capture whose
    /// player was Naicha). So they are a vote. Until the self record arrives,
    /// you are the owner named most often, while that owner leads outright;
    /// a tie means not knowing. A configured name already matched to a player
    /// in the world is kept. Returns whether the local identity changed.
    ///
    /// Taking the first name and ignoring loot records for good once a second
    /// appeared left that capture with no local player at all.
    pub fn note_loot_owner(&self, mob_id: i32, owner_id: i32, name: &str) -> bool {
        let mut inner = self.inner.write();
        let loot = &mut inner.loot_identity;
        let entry = loot.owners.entry(name.to_string()).or_insert_with(|| (owner_id, HashSet::new()));
        entry.0 = owner_id;
        if entry.1.len() < 10_000 {
            entry.1.insert(mob_id);
        }
        // Only you and your party: a stranger farming nearby out-killed the
        // player in one capture, 3 to 2, and took over as "you".
        let scope = &loot.party_scope;
        let mut ranked: Vec<(&String, i32, usize)> = loot
            .owners
            .iter()
            .filter(|(_, (id, _))| scope.contains(id))
            .map(|(n, (id, kills))| (n, *id, kills.len()))
            .collect();
        ranked.sort_by(|a, b| b.2.cmp(&a.2));
        let leader = match ranked.as_slice() {
            [] => return false, // nobody of yours yet: not evidence either way
            [first] => Some((first.0.clone(), first.1)),
            [first, second, ..] if first.2 > second.2 => Some((first.0.clone(), first.1)),
            _ => None,
        };
        let Some((name, owner_id)) = leader else {
            let was_applied = std::mem::take(&mut loot.applied);
            tracing::info!("loot records name several players equally; not using them to identify you");
            if was_applied {
                // Back to not knowing: the UI's name and the self record decide.
                inner.local_identity_from_game = false;
                inner.local_player_id = None;
                return true;
            }
            return false;
        };
        let name = name.as_str();
        if inner.local_identity_from_game && !inner.loot_identity.applied {
            return false; // the self record has spoken
        }
        let configured_and_found = inner.local_player_id.is_some_and(|id| {
            let configured = inner.local_character_name.as_deref().map(str::trim);
            configured.is_some() && inner.nickname_storage.get(&(id as i32)).map(String::as_str) == configured
        });
        if configured_and_found && inner.local_character_name.as_deref().map(str::trim) != Some(name) {
            return false;
        }
        if inner.local_identity_from_game
            && inner.local_player_id == Some(owner_id as i64)
            && inner.local_character_name.as_deref() == Some(name)
        {
            return false;
        }
        inner.loot_identity.applied = true;
        set_game_identity(&mut inner, owner_id as i64, Some(name.to_string()));
        true
    }

    /// The server sent a `06 38` record about `entity_id`.
    ///
    /// Measured on every capture at hand: in the Global ones it names the
    /// local player hundreds of times and players nearby never; in older
    /// Korean/Taiwanese ones, party members too. So it marks you and your
    /// party, which is what tells your loot from a stranger's.
    pub fn note_party_scope(&self, entity_id: i32) {
        if !(100..=9_999_999).contains(&entity_id) {
            return;
        }
        let mut inner = self.inner.write();
        let scope = &mut inner.loot_identity.party_scope;
        if scope.len() < 10_000 {
            scope.insert(entity_id);
        }
    }

    /// `name`'s home server, as a self or loot record states it.
    pub fn note_player_server(&self, name: &str, server_id: u16) {
        if !(1000..3000).contains(&server_id) {
            return;
        }
        let mut inner = self.inner.write();
        // Bounded: one entry per character met, and a session meets hundreds.
        if inner.player_servers.len() < 10_000 || inner.player_servers.contains_key(name) {
            inner.player_servers.insert(name.to_string(), server_id);
        }
    }

    /// Your class and level, as your own self record states them.
    ///
    /// A record whose level did not read (a partial copy, the scan having met
    /// one in still-compressed bytes) keeps the level already known for that
    /// character rather than erasing it.
    pub fn note_self_profile(&self, name: &str, class: Option<JobClass>, level: Option<u32>) {
        let mut inner = self.inner.write();
        let (old_class, old_level) = match &inner.self_profile {
            Some((n, c, l)) if n == name => (*c, *l),
            _ => (None, None),
        };
        inner.self_profile = Some((name.to_string(), class.or(old_class), level.or(old_level)));
    }

    /// Who you are playing, as far as the game has said: name, server, class
    /// and level. Class falls back to the one your skills show, for a meter
    /// started before the self record came; level has no such fallback.
    pub fn local_profile(&self) -> LocalProfile {
        let server = self.fight_server_id();
        let inner = self.inner.read();
        let name = inner.local_character_name.clone();
        let (mut class, mut level) = (None, None);
        if let (Some(n), Some((pn, pc, pl))) = (name.as_deref(), inner.self_profile.as_ref()) {
            if n.trim() == pn.trim() {
                class = *pc;
                level = *pl;
            }
        }
        if class.is_none() {
            class = inner.local_player_id.and_then(|id| inner.actor_jobs.get(&(id as i32)).copied());
        }
        LocalProfile { name, server_id: server, class, level }
    }

    /// The server the fights being recorded are on: the local player's, else
    /// the party's. 0 when nothing has said.
    ///
    /// A server id names its region (`1304` is Europe), which is what this is
    /// for: uploaded logs are grouped by region. The local player's own server
    /// comes from the self record (or a loot record naming them); the roster's
    /// is the fallback, by majority, since in a cross-server party each member
    /// keeps their own server but all share the region.
    pub fn fight_server_id(&self) -> u16 {
        let inner = self.inner.read();
        let local = inner.local_character_name.as_deref().map(str::trim);
        if let Some(&server) = local.and_then(|n| inner.player_servers.get(n)) {
            return server;
        }
        if let Some(member) = local.and_then(|n| inner.party_members.get(n)) {
            if member.server_id != 0 {
                return member.server_id;
            }
        }
        let mut counts: HashMap<u16, usize> = HashMap::new();
        for member in inner.party_members.values() {
            if member.server_id != 0 {
                *counts.entry(member.server_id).or_default() += 1;
            }
        }
        counts.into_iter().max_by_key(|&(server, n)| (n, std::cmp::Reverse(server))).map_or(0, |(s, _)| s)
    }

    /// Whether the game itself named the local player (the self record or the
    /// character list), not a guess from loot records. Only this may stand
    /// over a name the player typed (issue #13): a loot guess can be a
    /// bystander's kill.
    pub fn local_identity_from_self_record(&self) -> bool {
        let inner = self.inner.read();
        inner.local_identity_from_game && !inner.loot_identity.applied
    }

    /// Whether the local player's identity came from the game rather than from
    /// the UI (window title, settings, a remembered name).
    pub fn local_identity_from_game(&self) -> bool {
        self.inner.read().local_identity_from_game
    }

    /// Replace the supporter roster. Called after each download.
    pub fn set_supporters(&self, roster: crate::supporters::Roster) {
        self.inner.write().supporters = std::sync::Arc::new(roster);
    }

    pub fn supporters(&self) -> std::sync::Arc<crate::supporters::Roster> {
        self.inner.read().supporters.clone()
    }

    pub fn set_local_player_id(&self, id: Option<i64>) {
        self.inner.write().local_player_id = id;
    }

    pub fn local_player_id(&self) -> Option<i64> {
        self.inner.read().local_player_id
    }
}

#[derive(Default)]
pub(super) struct LootIdentity {
    /// Each owner the loot records have named this session: the entity id
    /// they gave, and the kills (mob ids) they named them for. Kills, not
    /// records: the embedded scan sees one record again on every read of the
    /// buffer it sits in, which counted one kill dozens of times.
    owners: HashMap<String, (i32, HashSet<i32>)>,
    /// Entities the server has sent a `06 38` record about. Those go to you
    /// and your party, never to strangers fighting nearby, so a loot owner
    /// outside this set is someone else's kill. See `note_party_scope`.
    party_scope: HashSet<i32>,
    /// The local identity currently in force came from them, not from the
    /// self record.
    applied: bool,
}

fn set_game_identity(inner: &mut Inner, id: i64, name: Option<String>) {
    // An older id kept with your name would come back on the next
    // `reset_nicknames` and be taken for you again.
    if let Some(name) = name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        inner.permanent_nicknames.retain(|&k, v| k as i64 == id || v.trim() != name);
    }
    inner.local_identity_from_game = true;
    inner.local_player_id = Some(id);
    inner.local_character_name = name;
}
