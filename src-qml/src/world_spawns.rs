// SPDX-License-Identifier: AGPL-3.0-only

//! Spawn notices: a countdown in the notice tray before each Field Boss (the
//! game's word for world bosses) and each Spacetime Rift. The times come from
//! the game's own schedule (src/data/world_spawns.json), which runs on the
//! server's clock: the Server Time Zone setting says which zone that is
//! (empty: this computer's). A Field Boss gets its notice 30, 15 and 5
//! minutes before, a rift 5 minutes before; each spawn time is one notice of
//! its kind, updated in place, that the tray counts down to and that goes
//! 10 minutes after the spawn.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{Local, TimeZone};
use cxx_qt_lib::{QByteArray, QDateTime, QTimeZone};
use serde_json::Value;

use crate::i18n::{fill, text};

/// "{name} at {time}": the notice's title.
pub const AT: &str = "notices.spawn.at";

/// The keys this file looks up, for the strings check.
#[cfg(test)]
pub const KEYS: [&str; 1] = [AT];

/// Minutes before the spawn that a notice comes.
pub const BOSS_LEADS: [i64; 3] = [30, 15, 5];
pub const RIFT_LEADS: [i64; 1] = [5];
/// A notice still comes this long after its time, for a meter that was busy
/// or asleep then; later than that it is left out.
const LATE: i64 = 3 * 60;
/// A spawn's notice goes this long after the spawn.
const KEEP_AFTER: i64 = 10 * 60;
const DAY: i64 = 24 * 60 * 60;
/// How often the live loop looks.
const EVERY: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Boss,
    Rift,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Boss => "boss",
            Kind::Rift => "rift",
        }
    }

    fn leads(self) -> &'static [i64] {
        match self {
            Kind::Boss => &BOSS_LEADS,
            Kind::Rift => &RIFT_LEADS,
        }
    }
}

/// One row of the schedule: at `minute` after midnight on the server's clock
/// on each of its weekdays, then every `cycle` minutes for the rest of that
/// day (0: once).
#[derive(Clone, Debug, PartialEq)]
pub struct Spawn {
    pub kind: Kind,
    /// The boss; a rift has none.
    pub npc: Option<u32>,
    pub map: u32,
    pub minute: i64,
    pub cycle: i64,
    /// Monday first.
    pub weekdays: [bool; 7],
}

#[derive(Default)]
pub struct Schedule {
    spawns: Vec<Spawn>,
}

impl Schedule {
    pub fn from_json(json: &Value) -> Schedule {
        let spawns = json["spawns"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| {
                let kind = match s["kind"].as_str()? {
                    "boss" => Kind::Boss,
                    "rift" => Kind::Rift,
                    _ => return None,
                };
                let mut weekdays = [false; 7];
                for day in s["weekdays"].as_array()?.iter().filter_map(Value::as_u64) {
                    *weekdays.get_mut(day as usize)? = true;
                }
                Some(Spawn {
                    kind,
                    npc: s["npc"].as_u64().map(|n| n as u32),
                    map: s["map"].as_u64()? as u32,
                    minute: s["minute"].as_i64()?,
                    cycle: s["cycle"].as_i64().unwrap_or(0),
                    weekdays,
                })
            })
            .collect();
        Schedule { spawns }
    }

    /// Each spawn at a unix second from `from` to `to`, in time order.
    /// `offset` gives the server clock's offset from UTC in seconds at a
    /// unix second.
    pub fn between(&self, from: i64, to: i64, offset: &dyn Fn(i64) -> i64) -> Vec<(i64, &Spawn)> {
        let mut out = Vec::new();
        let first = (from + offset(from)).div_euclid(DAY) - 1;
        let last = (to + offset(to)).div_euclid(DAY) + 1;
        for day in first..=last {
            // Day 0 of the Unix clock is day 3 of a week that starts on Monday.
            let weekday = (day + 3).rem_euclid(7) as usize;
            for spawn in self.spawns.iter().filter(|s| s.weekdays[weekday]) {
                let mut minute = spawn.minute;
                while minute < 24 * 60 {
                    let at = to_utc(day * DAY + minute * 60, offset);
                    if (from..=to).contains(&at) {
                        out.push((at, spawn));
                    }
                    if spawn.cycle <= 0 {
                        break;
                    }
                    minute += spawn.cycle;
                }
            }
        }
        out.sort_by_key(|(at, s)| (*at, s.kind, s.map, s.npc));
        out
    }
}

/// A time on the server's clock (seconds as if it were UTC) as a unix second.
fn to_utc(local: i64, offset: &dyn Fn(i64) -> i64) -> i64 {
    let guess = local - offset(local);
    local - offset(guess)
}

/// The game's words for one language: "Field Boss", "Spacetime Rift", the
/// maps' names (src/data/i18n/spawns/<lang>.json) and the bosses' names
/// (src/data/i18n/npcs/<lang>.json).
#[derive(Default)]
pub struct SpawnNames {
    pub field_boss: String,
    pub rift: String,
    pub maps: HashMap<u32, String>,
    pub npcs: HashMap<u32, String>,
}

impl SpawnNames {
    pub fn from_json(spawns: &Value, npcs: &Value) -> SpawnNames {
        SpawnNames {
            field_boss: spawns["fieldBoss"].as_str().unwrap_or_default().to_string(),
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
}

/// A notice for the tray: its key (one per spawn time and kind), title, text
/// and the unix second it counts down to.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub key: String,
    /// "boss" or "rift", which picks the icon.
    pub kind: &'static str,
    pub title: String,
    pub text: String,
    pub until: i64,
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
    fn title(&self, kind: Kind, at: i64) -> String {
        let local = at + (self.local_offset)(at);
        let (hour, minute) = (local.rem_euclid(DAY) / 3600, local.rem_euclid(3600) / 60);
        let time = crate::i18n::clock(self.language, hour as u32, minute as u32, self.hour24);
        let name = match kind {
            Kind::Boss => &self.names.field_boss,
            Kind::Rift => &self.names.rift,
        };
        fill(&text(self.language, AT), &[("name", name), ("time", &time)])
    }

    /// The bosses of one spawn time, a line per map: "Boss B, Boss C (Map
    /// One)", then "Boss D (Map Two)". The game holds some bosses twice;
    /// each name comes once.
    fn bosses(&self, spawns: &[&Spawn]) -> String {
        let mut by_map: BTreeMap<u32, Vec<String>> = BTreeMap::new();
        for spawn in spawns {
            let Some(npc) = spawn.npc else { continue };
            let name = self.names.npcs.get(&npc).cloned().unwrap_or_else(|| npc.to_string());
            let names = by_map.entry(spawn.map).or_default();
            if !names.contains(&name) {
                names.push(name);
            }
        }
        by_map
            .into_iter()
            .map(|(map, names)| match self.names.maps.get(&map) {
                Some(place) => format!("{} ({place})", names.join(", ")),
                None => names.join(", "),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// What has been posted, so each lead comes once and each notice goes after
/// its spawn.
#[derive(Default)]
pub struct SpawnNotices {
    posted: HashSet<(i64, Kind, i64)>,
    /// Notices on the tray now, by key: the spawn time.
    shown: HashMap<String, i64>,
}

impl SpawnNotices {
    /// The notices due at `now`; rifts list no maps, since they open on every
    /// faction's maps at once.
    pub fn due(&mut self, schedule: &Schedule, words: &Words, now: i64, offset: &dyn Fn(i64) -> i64) -> Vec<Notice> {
        let reach = BOSS_LEADS.iter().chain(&RIFT_LEADS).max().copied().unwrap_or(0) * 60;
        let mut groups: BTreeMap<(i64, Kind), (i64, Vec<&Spawn>)> = BTreeMap::new();
        for (at, spawn) in schedule.between(now - LATE, now + reach, offset) {
            for &lead in spawn.kind.leads() {
                let when = at - lead * 60;
                if when <= now && now < when + LATE && !self.posted.contains(&(at, spawn.kind, lead)) {
                    let group = groups.entry((at, spawn.kind)).or_insert((lead, Vec::new()));
                    group.0 = group.0.min(lead);
                    group.1.push(spawn);
                }
            }
        }
        let mut out = Vec::new();
        for ((at, kind), (lead, spawns)) in groups {
            // Leads of this spawn already past go too: one notice per lead at most.
            for &earlier in kind.leads().iter().filter(|&&l| l >= lead) {
                self.posted.insert((at, kind, earlier));
            }
            let key = format!("spawn:{}:{at}", kind.name());
            let text = if kind == Kind::Boss { words.bosses(&spawns) } else { String::new() };
            self.shown.insert(key.clone(), at);
            out.push(Notice { key, kind: kind.name(), title: words.title(kind, at), text, until: at });
        }
        self.posted.retain(|(at, _, _)| *at > now - DAY);
        out
    }

    /// The keys of notices whose spawn is long enough past; they leave the list.
    pub fn expired(&mut self, now: i64) -> Vec<String> {
        let gone: Vec<String> = self.shown.iter().filter(|&(_, &at)| now >= at + KEEP_AFTER).map(|(key, _)| key.clone()).collect();
        for key in &gone {
            self.shown.remove(key);
        }
        gone
    }

    /// Every notice this has on the tray, for when the setting goes off.
    pub fn take_all(&mut self) -> Vec<String> {
        self.posted.clear();
        self.shown.drain().map(|(key, _)| key).collect()
    }
}

/// The offset from UTC of `zone` (an IANA name; empty: this computer's) at a
/// unix second, by Qt's time zone data; an unknown name counts as empty.
fn zone_offset(zone: &str) -> impl Fn(i64) -> i64 {
    let named = if zone.is_empty() { None } else { Some(QTimeZone::from_iana(&QByteArray::from(zone))) };
    let tz = match named {
        Some(tz) if tz.is_valid() => tz,
        _ => QTimeZone::system_time_zone(),
    };
    let utc = QTimeZone::utc();
    move |at: i64| tz.offset_from_utc(&QDateTime::from_secs_since_epoch(at, &utc)) as i64
}

/// The server regions in the order the setting lists them, by the game's
/// code for each (its name is in src/data/i18n/spawns), with the time zone
/// it starts the setting at. Only North America - West is checked against
/// the game's own server clock; the others are the usual zone of the
/// region, which the player checks against the in-game clock (ST).
pub const REGIONS: [(&str, &str); 6] = [
    ("NAW", "America/Los_Angeles"),
    ("NAE", "America/New_York"),
    ("EU", "Europe/Berlin"),
    ("LA", "America/Sao_Paulo"),
    ("AS", "Asia/Singapore"),
    ("JP", "Asia/Tokyo"),
];

/// The regions for the setting's list, in the game's words for `language`:
/// [{"label", "value"}], the value being the time zone.
pub fn regions_json(language: &str) -> String {
    let data = crate::data_dir();
    let read = |lang: &str| data.as_ref().and_then(|dir| crate::read_json(&dir.join("i18n/spawns").join(format!("{lang}.json"))));
    let words = read(language).or_else(|| read("en")).unwrap_or(Value::Null);
    let list: Vec<Value> = REGIONS
        .iter()
        .map(|(code, zone)| serde_json::json!({ "label": words["regions"][code].as_str().unwrap_or(code), "value": zone }))
        .collect();
    Value::Array(list).to_string()
}

/// Whether `zone` is an IANA time zone name Qt knows.
pub fn valid_zone(zone: &str) -> bool {
    QTimeZone::from_iana(&QByteArray::from(zone)).is_valid()
}

fn local_offset(at: i64) -> i64 {
    Local.timestamp_opt(at, 0).single().map(|t| t.offset().local_minus_utc() as i64).unwrap_or(0)
}

fn names_in(language: &str) -> SpawnNames {
    let data = crate::data_dir();
    let read = |folder: &str, lang: &str| data.as_ref().and_then(|dir| crate::read_json(&dir.join("i18n").join(folder).join(format!("{lang}.json"))));
    let spawns = read("spawns", language).or_else(|| read("spawns", "en")).unwrap_or(Value::Null);
    let npcs = read("npcs", language).or_else(|| read("npcs", "en")).unwrap_or(Value::Null);
    SpawnNames::from_json(&spawns, &npcs)
}

struct Live {
    schedule: Schedule,
    names: HashMap<String, SpawnNames>,
    notices: SpawnNotices,
    looked: Option<Instant>,
}

/// From the live loop: posts the notices due, takes off the ones past, and
/// all of them when `enabled` goes off.
pub fn tick(language: &str, hour24: bool, enabled: bool, zone: &str) {
    static LIVE: OnceLock<Mutex<Live>> = OnceLock::new();
    let live = LIVE.get_or_init(|| {
        let schedule = crate::data_dir().and_then(|dir| crate::read_json(&dir.join("world_spawns.json"))).map(|j| Schedule::from_json(&j)).unwrap_or_default();
        Mutex::new(Live { schedule, names: HashMap::new(), notices: SpawnNotices::default(), looked: None })
    });
    let mut live = live.lock().unwrap_or_else(|e| e.into_inner());
    let live = &mut *live;
    if !enabled {
        for key in live.notices.take_all() {
            crate::notices::remove(&key);
        }
        live.looked = None;
        return;
    }
    if live.looked.is_some_and(|at| at.elapsed() < EVERY) {
        return;
    }
    live.looked = Some(Instant::now());
    let now = Local::now().timestamp();
    let offset = zone_offset(zone);
    let names = live.names.entry(language.to_string()).or_insert_with(|| names_in(language));
    let words = Words { language, names, hour24, local_offset: &local_offset };
    for notice in live.notices.due(&live.schedule, &words, now, &offset) {
        crate::notices::post_until(&notice.key, notice.kind, &notice.title, &notice.text, notice.until);
    }
    for key in live.notices.expired(now) {
        crate::notices::remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Boss A every 3 hours from 01:00; bosses B and C on days 0, 3 and 5
    /// (Monday first) at 21:30 on map 20, and C again on map 22, held twice
    /// as the game holds some bosses; a rift every 3 hours from 02:00, held
    /// twice too.
    fn schedule() -> Schedule {
        Schedule::from_json(&json!({ "spawns": [
            { "kind": "boss", "map": 20, "npc": 1, "race": "All", "minute": 60, "cycle": 180, "weekdays": [0,1,2,3,4,5,6] },
            { "kind": "boss", "map": 20, "npc": 2, "race": "All", "minute": 1290, "cycle": 0, "weekdays": [0,3,5] },
            { "kind": "boss", "map": 20, "npc": 3, "race": "All", "minute": 1290, "cycle": 0, "weekdays": [0,3,5] },
            { "kind": "boss", "map": 22, "npc": 4, "race": "All", "minute": 1290, "cycle": 0, "weekdays": [0,3,5] },
            { "kind": "boss", "map": 22, "npc": 4, "race": "All", "minute": 1290, "cycle": 0, "weekdays": [0,3,5] },
            { "kind": "rift", "map": 30, "race": "Light", "minute": 120, "cycle": 180, "weekdays": [0,1,2,3,4,5,6] },
            { "kind": "rift", "map": 30, "race": "Light", "minute": 120, "cycle": 180, "weekdays": [0,1,2,3,4,5,6] },
            { "kind": "unknown", "map": 1, "minute": 0, "weekdays": [0] }
        ]}))
    }

    fn names() -> SpawnNames {
        SpawnNames::from_json(
            &json!({ "fieldBoss": "Field Boss", "rift": "Spacetime Rift", "maps": { "20": "Map One", "22": "Map Two" } }),
            &json!({ "1": { "name": "Boss A" }, "2": { "name": "Boss B" }, "3": { "name": "Boss C" }, "4": { "name": "Boss D" } }),
        )
    }

    /// A Monday, midnight in `zone`'s clock (UTC-7).
    const START: i64 = 1_792_393_200;
    /// 13 days later, at 02:00, the zone's clock goes back an hour, to UTC-8.
    const CHANGE: i64 = START + 13 * DAY + 2 * 3600;

    fn zone(at: i64) -> i64 {
        if at < CHANGE {
            -7 * 3600
        } else {
            -8 * 3600
        }
    }

    fn words<'a>(names: &'a SpawnNames, local: &'a dyn Fn(i64) -> i64) -> Words<'a> {
        Words { language: "en", names, hour24: true, local_offset: local }
    }

    fn same_zone(_: i64) -> i64 {
        -7 * 3600
    }

    #[test]
    fn the_unknown_kind_is_left_out() {
        assert_eq!(schedule().spawns.len(), 7);
    }

    #[test]
    fn cycles_run_from_their_first_time_to_the_end_of_the_day() {
        let s = schedule();
        let hours: Vec<i64> =
            s.between(START, START + DAY - 1, &zone).into_iter().filter(|(_, sp)| sp.npc == Some(1)).map(|(at, _)| (at - START) / 3600).collect();
        assert_eq!(hours, [1, 4, 7, 10, 13, 16, 19, 22]);
    }

    #[test]
    fn weekdays_pick_the_days() {
        let s = schedule();
        let week = s.between(START, START + 7 * DAY - 1, &zone);
        let days: Vec<i64> = week.iter().filter(|(_, sp)| sp.npc == Some(2)).map(|(at, _)| (at - START) / DAY).collect();
        assert_eq!(days, [0, 3, 5]);
        let first = week.iter().find(|(_, sp)| sp.npc == Some(2)).unwrap().0;
        assert_eq!(first - START, 21 * 3600 + 30 * 60);
    }

    #[test]
    fn server_clock_times_hold_when_the_zone_changes_its_offset() {
        let s = schedule();
        let day = START + 13 * DAY;
        let times: Vec<i64> = s.between(day, day + DAY + 3600, &zone).into_iter().filter(|(_, sp)| sp.npc == Some(1)).map(|(at, _)| at).collect();
        // 01:00 still at UTC-7; 04:00 at UTC-8, an hour later in UTC.
        assert!(times.contains(&(day + 3600)), "{times:?}");
        assert!(times.contains(&(day + 4 * 3600 + 3600)), "{times:?}");
    }

    #[test]
    fn a_boss_notice_comes_30_15_and_5_minutes_before_and_updates_in_place() {
        let n = names();
        let s = schedule();
        let w = words(&n, &same_zone);
        let mut notices = SpawnNotices::default();
        let spawn = START + 3600;
        let mut keys = Vec::new();
        for lead in [30, 15, 5] {
            let got = notices.due(&s, &w, spawn - lead * 60, &zone);
            assert_eq!(got.len(), 1, "lead {lead}");
            assert_eq!(got[0].title, "Field Boss at 01:00");
            assert_eq!(got[0].text, "Boss A (Map One)");
            assert_eq!(got[0].kind, "boss");
            assert_eq!(got[0].until, spawn);
            keys.push(got[0].key.clone());
            assert!(notices.due(&s, &w, spawn - lead * 60 + 20, &zone).is_empty(), "once per lead");
        }
        assert!(keys.iter().all(|k| k == &keys[0]), "one notice per spawn");
    }

    #[test]
    fn bosses_of_one_time_share_a_notice_a_line_per_map_and_each_name_comes_once() {
        let n = names();
        let s = schedule();
        let w = words(&n, &same_zone);
        let spawn = START + 21 * 3600 + 30 * 60;
        let got = SpawnNotices::default().due(&s, &w, spawn - 30 * 60, &zone);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, "Boss B, Boss C (Map One)\nBoss D (Map Two)");
    }

    #[test]
    fn a_rift_notice_comes_5_minutes_before_without_maps() {
        let n = names();
        let s = schedule();
        let w = words(&n, &same_zone);
        let mut notices = SpawnNotices::default();
        let rift = START + 2 * 3600;
        assert!(notices.due(&s, &w, rift - 30 * 60, &zone).iter().all(|n| n.kind != "rift"));
        let got = notices.due(&s, &w, rift - 5 * 60, &zone);
        let notice = got.iter().find(|n| n.kind == "rift").expect("rift notice");
        assert_eq!(notice.title, "Spacetime Rift at 02:00");
        assert_eq!(notice.text, "");
    }

    #[test]
    fn a_late_meter_posts_once_within_three_minutes_and_not_after() {
        let n = names();
        let s = schedule();
        let w = words(&n, &same_zone);
        let spawn = START + 3600;
        // Asleep through the 30 and 15 minute times: only the 5 minute one comes.
        assert_eq!(SpawnNotices::default().due(&s, &w, spawn - 5 * 60 + 120, &zone).len(), 1);
        assert!(SpawnNotices::default().due(&s, &w, spawn - 5 * 60 + LATE + 1, &zone).is_empty());
    }

    #[test]
    fn the_title_uses_the_players_own_clock_and_format() {
        let n = names();
        let s = schedule();
        let ahead = |_: i64| 2 * 3600; // a player at UTC+2, on a server at UTC-7
        let mut w = words(&n, &ahead);
        w.hour24 = false;
        let got = SpawnNotices::default().due(&s, &w, START + 3600 - 30 * 60, &zone);
        assert_eq!(got[0].title, "Field Boss at 10:00 AM");
    }

    #[test]
    fn a_notice_goes_ten_minutes_after_its_spawn_and_all_go_when_switched_off() {
        let n = names();
        let s = schedule();
        let w = words(&n, &same_zone);
        let mut notices = SpawnNotices::default();
        let spawn = START + 3600;
        let key = notices.due(&s, &w, spawn - 5 * 60, &zone)[0].key.clone();
        assert!(notices.expired(spawn + KEEP_AFTER - 1).is_empty());
        assert_eq!(notices.expired(spawn + KEEP_AFTER), [key]);
        notices.due(&s, &w, spawn + 3 * 3600 - 5 * 60, &zone);
        assert_eq!(notices.take_all().len(), 1);
    }

    #[test]
    fn qt_knows_the_region_zones_and_gives_offsets() {
        for (_, zone) in REGIONS {
            assert!(valid_zone(zone), "{zone}");
        }
        assert!(!valid_zone("Not/AZone"));
        // Etc/GMT+5 is UTC-5 all year (the sign is POSIX's).
        assert_eq!(zone_offset("Etc/GMT+5")(START), -5 * 3600);
        assert_eq!(zone_offset("Not/AZone")(START), zone_offset("")(START), "an unknown zone is this computer's");
    }
}
