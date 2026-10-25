// SPDX-License-Identifier: AGPL-3.0-only

//! What the timed notices share, the spawn notices (world_spawns.rs) and the
//! field boss notices (field_bosses.rs): when a lead comes and how long a
//! notice stays, the notice and its move on the tray, and the game's words
//! for a title and a boss. The live loop's tick for both is countdowns.rs.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use chrono::{Local, TimeZone};
use serde_json::Value;

use crate::i18n::{fill, text};

/// "{name} at {time}": the notice's title.
pub const AT: &str = "notices.spawn.at";

/// The keys this file looks up, for the strings check.
#[cfg(test)]
pub const KEYS: [&str; 1] = [AT];

/// Minutes before a boss that a notice comes.
pub const BOSS_LEADS: [i64; 3] = [30, 15, 5];
/// A notice still comes this long after its time, for a meter that was busy
/// or asleep then; later than that it is left out.
pub const LATE: i64 = 3 * 60;
/// A notice goes this long after its time.
pub const KEEP_AFTER: i64 = 10 * 60;
pub const DAY: i64 = 24 * 60 * 60;

/// How long before its time a notice with these leads (minutes) first
/// comes, in seconds.
pub fn reach(leads: &[i64]) -> i64 {
    leads.iter().max().copied().unwrap_or(0) * 60
}

/// Whether a notice for a spawn at `at` belongs on the tray at `now`: from
/// its first lead, `reach` seconds before, until it goes.
pub fn on_tray(at: i64, now: i64, reach: i64) -> bool {
    at - reach <= now && now < at + KEEP_AFTER
}

/// The leads posted, so each comes once: (what, time, lead), `what` being
/// what the notice is for, such as a spawn's kind or a field boss's row.
pub struct Posted<K>(HashSet<(K, i64, i64)>);

impl<K> Default for Posted<K> {
    fn default() -> Self {
        Posted(HashSet::new())
    }
}

impl<K: Copy + Eq + Hash> Posted<K> {
    /// The lead of `what` at `at` due at `now`: the smallest of `leads`
    /// (minutes) whose time has come, no more than LATE ago, and that has not
    /// been posted. It and the leads before it count as posted: one notice
    /// per lead at most.
    pub fn due_lead(&mut self, what: K, at: i64, now: i64, leads: &[i64]) -> Option<i64> {
        let lead = leads
            .iter()
            .copied()
            .filter(|&lead| {
                let when = at - lead * 60;
                when <= now && now < when + LATE && !self.0.contains(&(what, at, lead))
            })
            .min()?;
        for &earlier in leads.iter().filter(|&&l| l >= lead) {
            self.0.insert((what, at, earlier));
        }
        Some(lead)
    }

    /// Forgets the times more than a day past.
    pub fn forget_old(&mut self, now: i64) {
        self.0.retain(|&(_, at, _)| at > now - DAY);
    }
}

/// The keys of the notices in `shown` whose time (`at` of each) is
/// KEEP_AFTER past; they leave `shown`.
pub fn expire<K: Clone, V>(shown: &mut HashMap<K, V>, now: i64, at: impl Fn(&V) -> i64) -> Vec<K> {
    let mut gone = Vec::new();
    shown.retain(|key, value| {
        let keep = now < at(value) + KEEP_AFTER;
        if !keep {
            gone.push(key.clone());
        }
        keep
    });
    gone
}

/// Every notice in `shown`, for when the setting goes off; the leads posted
/// are forgotten, so they come again once it is back on.
pub fn take_all<K, V, P>(posted: &mut Posted<P>, shown: &mut HashMap<K, V>) -> Vec<K> {
    posted.0.clear();
    shown.drain().map(|(key, _)| key).collect()
}

/// A notice on the tray whose spawn time moved: its new title and time, or
/// off the tray when the new time is outside the stretch it shows in.
#[derive(Clone, Debug, PartialEq)]
pub enum Move {
    Retime { key: String, title: String, until: i64 },
    Remove(String),
}

impl Move {
    pub fn key(&self) -> &str {
        match self {
            Move::Retime { key, .. } | Move::Remove(key) => key,
        }
    }

    pub fn apply(self) {
        match self {
            Move::Retime { key, title, until } => crate::notices::retime(&key, &title, until),
            Move::Remove(key) => crate::notices::remove(&key),
        }
    }
}

/// A notice for the tray: its key (one per spawn time and kind), title, text
/// and the unix second it counts down to.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub key: String,
    /// "abyss", "rift" or a field boss's tier, which picks the icon.
    pub kind: &'static str,
    pub title: String,
    pub text: String,
    pub until: i64,
}

/// The game's words for one language: "Spacetime Rift", the maps' names
/// (src/data/i18n/spawns/<lang>.json) and the bosses' names
/// (src/data/i18n/npcs/<lang>.json).
#[derive(Default)]
pub struct SpawnNames {
    pub rift: String,
    pub maps: HashMap<u32, String>,
    pub npcs: HashMap<u32, String>,
}

impl SpawnNames {
    pub fn from_json(spawns: &Value, npcs: &Value) -> SpawnNames {
        SpawnNames {
            rift: spawns["rift"].as_str().unwrap_or_default().to_string(),
            maps: spawns["maps"]
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(id, name)| Some((id.parse().ok()?, name.as_str()?.to_string())))
                .collect(),
            npcs: npcs
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(id, npc)| Some((id.parse().ok()?, npc["name"].as_str()?.to_string())))
                .collect(),
        }
    }

    /// The words in `language`, or in English where it has none.
    pub fn in_language(language: &str) -> SpawnNames {
        let data = crate::data_dir();
        let read = |folder: &str, lang: &str| data.as_ref().and_then(|dir| crate::read_json(&dir.join("i18n").join(folder).join(format!("{lang}.json"))));
        let spawns = read("spawns", language).or_else(|| read("spawns", "en")).unwrap_or(Value::Null);
        let npcs = read("npcs", language).or_else(|| read("npcs", "en")).unwrap_or(Value::Null);
        SpawnNames::from_json(&spawns, &npcs)
    }
}

/// The words one language gives, with the clock's format and zone (the
/// computer's: the times in a notice are the player's own clock).
pub struct Words<'a> {
    pub language: &'a str,
    pub names: &'a SpawnNames,
    pub hour24: bool,
    pub local_offset: &'a dyn Fn(i64) -> i64,
}

impl Words<'_> {
    /// "{name} at {time}", the time on the player's clock.
    pub fn title_named(&self, name: &str, at: i64) -> String {
        let local = at + (self.local_offset)(at);
        let (hour, minute) = (local.rem_euclid(DAY) / 3600, local.rem_euclid(3600) / 60);
        let time = crate::i18n::clock(self.language, hour as u32, minute as u32, self.hour24);
        fill(&text(self.language, AT), &[("name", name), ("time", &time)])
    }

    /// A boss's name, or its NPC number when the words have none.
    pub fn npc(&self, npc: u32) -> String {
        self.names.npcs.get(&npc).cloned().unwrap_or_else(|| npc.to_string())
    }

    /// One boss and its map: "Boss A (Map One)".
    pub fn boss_line(&self, npc: u32, map: u32) -> String {
        let name = self.npc(npc);
        match self.names.maps.get(&map) {
            Some(place) => format!("{name} ({place})"),
            None => name,
        }
    }
}

/// This computer's offset from UTC at a unix second.
pub fn local_offset(at: i64) -> i64 {
    Local.timestamp_opt(at, 0).single().map(|t| t.offset().local_minus_utc() as i64).unwrap_or(0)
}

/// The words both notices' tests use: maps 20 and 22 and bosses 1 to 4 for
/// the spawn schedule's, maps 1010 and 1110 and Gartua and a commander for
/// the field bosses'.
#[cfg(test)]
pub mod test_words {
    use super::*;
    use serde_json::json;

    pub fn names() -> SpawnNames {
        SpawnNames::from_json(
            &json!({ "rift": "Spacetime Rift", "maps": { "20": "Map One", "22": "Map Two", "1010": "Map One", "1110": "Map Two" } }),
            &json!({
                "1": { "name": "Boss A" }, "2": { "name": "Boss B" }, "3": { "name": "Boss C" }, "4": { "name": "Boss D" },
                "2101074": { "name": "Green Golem" }, "2101120": { "name": "Quiet One" }, "2400800": { "name": "Other Golem" }
            }),
        )
    }

    /// In English, on the 24 hour clock, with the player's clock `local`.
    pub fn words<'a>(names: &'a SpawnNames, local: &'a dyn Fn(i64) -> i64) -> Words<'a> {
        Words { language: "en", names, hour24: true, local_offset: local }
    }

    /// A player on UTC.
    pub fn utc(_: i64) -> i64 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_due_lead_is_the_smallest_come_within_late_and_comes_once() {
        let mut posted = Posted::default();
        let at = 10_000;
        assert_eq!(posted.due_lead(1, at, at - 31 * 60, &BOSS_LEADS), None, "before the first lead");
        assert_eq!(posted.due_lead(1, at, at - 30 * 60, &BOSS_LEADS), Some(30));
        assert_eq!(posted.due_lead(1, at, at - 30 * 60 + 20, &BOSS_LEADS), None, "once per lead");
        // Another boss of the same time has its own leads.
        assert_eq!(posted.due_lead(2, at, at - 30 * 60 + 20, &BOSS_LEADS), Some(30));
        assert_eq!(posted.due_lead(1, at, at - 15 * 60 + LATE - 1, &BOSS_LEADS), Some(15));
        assert_eq!(posted.due_lead(1, at, at - 5 * 60 + LATE, &BOSS_LEADS), None, "more than LATE after");
    }

    #[test]
    fn a_late_lead_counts_the_ones_before_it_posted() {
        // Leads within LATE of each other: the smallest comes, the larger
        // ones with it.
        let leads = [6, 5, 4];
        let mut posted = Posted::default();
        let at = 10_000;
        assert_eq!(posted.due_lead('a', at, at - 4 * 60, &leads), Some(4));
        assert_eq!(posted.due_lead('a', at, at - 4 * 60 + 1, &leads), None);
        let mut posted = Posted::default();
        assert_eq!(posted.due_lead('a', at, at - 5 * 60, &leads), Some(5));
        assert_eq!(posted.due_lead('a', at, at - 4 * 60, &leads), Some(4), "a smaller lead still comes");
    }

    #[test]
    fn leads_a_day_past_are_forgotten_and_all_go_with_take_all() {
        let mut posted = Posted::default();
        let at = 10_000;
        posted.due_lead(1, at, at - 5 * 60, &BOSS_LEADS);
        posted.forget_old(at + DAY - 1);
        assert_eq!(posted.due_lead(1, at, at - 5 * 60, &BOSS_LEADS), None, "kept for a day");
        posted.forget_old(at + DAY);
        assert_eq!(posted.due_lead(1, at, at - 5 * 60, &BOSS_LEADS), Some(5));
        let mut shown = HashMap::from([("a", 1)]);
        assert_eq!(take_all(&mut posted, &mut shown), ["a"]);
        assert!(shown.is_empty());
        assert_eq!(posted.due_lead(1, at, at - 5 * 60, &BOSS_LEADS), Some(5), "posted again once back on");
    }

    #[test]
    fn a_notice_expires_keep_after_its_time() {
        let mut shown = HashMap::from([("early", 100), ("late", 200)]);
        assert!(expire(&mut shown, 100 + KEEP_AFTER - 1, |&at| at).is_empty());
        assert_eq!(expire(&mut shown, 100 + KEEP_AFTER, |&at| at), ["early"]);
        assert_eq!(shown, HashMap::from([("late", 200)]));
    }
}
