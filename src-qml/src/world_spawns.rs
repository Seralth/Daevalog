// SPDX-License-Identifier: AGPL-3.0-only

//! Spawn notices: a countdown in the notice tray before each Abyss Boss (the
//! world bosses of Reshanta) and each Spacetime Rift. The times come from
//! the game's own schedule (src/data/world_spawns.json), which runs on the
//! server's clock: the Server Time Zone setting says which zone that is
//! (empty: this computer's). An Abyss Boss gets its notice 30, 15 and 5
//! minutes before, a rift 5 minutes before; each spawn time is one notice of
//! its kind, updated in place, that the tray counts down to and that goes
//! 10 minutes after the spawn; a new Server Time Zone moves the notices on
//! the tray at once. Gartua and the commanders get the same countdown from
//! the world map's times (field_bosses.rs).

use std::collections::{BTreeMap, HashMap};

use cxx_qt_lib::{QByteArray, QDateTime, QTimeZone};
use serde_json::Value;

use crate::i18n::text;
use crate::timed_notices::{expire, on_tray, reach, take_all, Move, Notice, Posted, Words, BOSS_LEADS, DAY, LATE};

/// The game's words for an Abyss Boss ("Abyss boss monster" in its
/// achievements): the name in an Abyss Boss notice's title.
pub const ABYSS_BOSS: &str = "notices.spawn.abyssBoss";

/// The keys this file looks up, for the strings check.
#[cfg(test)]
pub const KEYS: [&str; 1] = [ABYSS_BOSS];

/// Minutes before a rift that its notice comes; an Abyss Boss's are
/// BOSS_LEADS.
pub const RIFT_LEADS: [i64; 1] = [5];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Abyss,
    Rift,
}

impl Kind {
    /// The notice's kind, which picks its icon (NoticeIcon.qml).
    fn name(self) -> &'static str {
        match self {
            Kind::Abyss => "abyss",
            Kind::Rift => "rift",
        }
    }

    fn leads(self) -> &'static [i64] {
        match self {
            Kind::Abyss => &BOSS_LEADS,
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
                // The schedule's bosses are the Abyss Bosses.
                let kind = match s["kind"].as_str()? {
                    "boss" => Kind::Abyss,
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

/// "Abyss Boss at 01:00" or "Spacetime Rift at 02:00".
fn title(words: &Words, kind: Kind, at: i64) -> String {
    let name = match kind {
        Kind::Abyss => text(words.language, ABYSS_BOSS),
        Kind::Rift => words.names.rift.clone(),
    };
    words.title_named(&name, at)
}

/// The bosses of one spawn time, a line per map: "Boss B, Boss C (Map One)",
/// then "Boss D (Map Two)". The game holds some bosses twice; each name
/// comes once.
fn bosses(words: &Words, spawns: &[&Spawn]) -> String {
    let mut by_map: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for spawn in spawns {
        let Some(npc) = spawn.npc else { continue };
        let name = words.npc(npc);
        let names = by_map.entry(spawn.map).or_default();
        if !names.contains(&name) {
            names.push(name);
        }
    }
    by_map
        .into_iter()
        .map(|(map, names)| match words.names.maps.get(&map) {
            Some(place) => format!("{} ({place})", names.join(", ")),
            None => names.join(", "),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A notice on the tray: its kind, its spawn time on the server's clock
/// (seconds as if it were UTC), which the Server Time Zone setting does not
/// move, and that time as a unix second in the zone it was posted in.
struct Shown {
    kind: Kind,
    server: i64,
    at: i64,
}

/// What has been posted, so each lead comes once and each notice goes after
/// its spawn.
#[derive(Default)]
pub struct SpawnNotices {
    posted: Posted<Kind>,
    /// Notices on the tray now, by key.
    shown: HashMap<String, Shown>,
}

impl SpawnNotices {
    /// The notices on the tray whose spawn a new Server Time Zone moved: each
    /// to its new time at once, or off the tray when the new time is outside
    /// the stretch its notice shows in (a lead of the new time brings it
    /// back).
    pub fn moved(&mut self, words: &Words, now: i64, offset: &dyn Fn(i64) -> i64) -> Vec<Move> {
        let mut moves = Vec::new();
        self.shown.retain(|key, shown| {
            let at = to_utc(shown.server, offset);
            if at == shown.at {
                return true;
            }
            shown.at = at;
            let keep = on_tray(at, now, reach(shown.kind.leads()));
            moves.push(if keep {
                Move::Retime { key: key.clone(), title: title(words, shown.kind, at), until: at }
            } else {
                Move::Remove(key.clone())
            });
            keep
        });
        moves.sort_by(|a, b| a.key().cmp(b.key()));
        moves
    }

    /// The notices due at `now`; rifts list no maps, since they open on every
    /// faction's maps at once.
    pub fn due(&mut self, schedule: &Schedule, words: &Words, now: i64, offset: &dyn Fn(i64) -> i64) -> Vec<Notice> {
        let mut groups: BTreeMap<(i64, Kind), Vec<&Spawn>> = BTreeMap::new();
        for (at, spawn) in schedule.between(now - LATE, now + reach(&BOSS_LEADS).max(reach(&RIFT_LEADS)), offset) {
            groups.entry((at, spawn.kind)).or_default().push(spawn);
        }
        let mut out = Vec::new();
        for ((at, kind), spawns) in groups {
            if self.posted.due_lead(kind, at, now, kind.leads()).is_none() {
                continue;
            }
            // Keyed by the time on the server's clock, which the Server Time
            // Zone setting does not move: a change of zone updates the notice.
            let server = at + offset(at);
            let key = format!("spawn:{}:{server}", kind.name());
            let text = if kind == Kind::Abyss { bosses(words, &spawns) } else { String::new() };
            self.shown.insert(key.clone(), Shown { kind, server, at });
            out.push(Notice { key, kind: kind.name(), title: title(words, kind, at), text, until: at });
        }
        self.posted.forget_old(now);
        out
    }

    /// The keys of notices whose spawn is long enough past; they leave the list.
    pub fn expired(&mut self, now: i64) -> Vec<String> {
        expire(&mut self.shown, now, |shown| shown.at)
    }

    /// Every notice this has on the tray, for when the setting goes off.
    pub fn take_all(&mut self) -> Vec<String> {
        take_all(&mut self.posted, &mut self.shown)
    }
}

/// The offset from UTC of `zone` (an IANA name; empty: this computer's) at a
/// unix second, by Qt's time zone data; an unknown name counts as empty.
pub fn zone_offset(zone: &str) -> impl Fn(i64) -> i64 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timed_notices::test_words::{names, words};
    use crate::timed_notices::KEEP_AFTER;
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
            assert_eq!(got[0].title, "Abyss Boss at 01:00");
            assert_eq!(got[0].text, "Boss A (Map One)");
            assert_eq!(got[0].kind, "abyss");
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
        assert_eq!(got[0].title, "Abyss Boss at 10:00 AM");
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
    fn a_new_server_zone_updates_the_notice_of_the_same_spawn() {
        let n = names();
        let s = schedule();
        let w = words(&n, &same_zone);
        let west = |_: i64| -8 * 3600;
        let mut notices = SpawnNotices::default();
        // Boss A at 01:00 on the server's clock: an hour later in UTC on the zone to the west.
        let before = notices.due(&s, &w, START + 3600 - 30 * 60, &zone);
        let after = notices.due(&s, &w, START + 2 * 3600 - 15 * 60, &west);
        assert_eq!(before.len(), 1);
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].key, before[0].key);
        assert_eq!(after[0].until, START + 2 * 3600);
        assert!(notices.expired(START + 2 * 3600 + KEEP_AFTER - 1).is_empty(), "goes after the new time");
        assert_eq!(notices.expired(START + 2 * 3600 + KEEP_AFTER), [before[0].key.clone()]);
        // The 02:00 rift, on one zone and then the other, is one notice too.
        notices.due(&s, &w, START + 2 * 3600 - 5 * 60, &zone);
        notices.due(&s, &w, START + 3 * 3600 - 5 * 60, &west);
        assert_eq!(notices.take_all().len(), 1);
    }

    /// The boss words with the game string that holds each (an Abyss Boss
    /// achievement; Eternal Gartua's), and other strings for languages where
    /// that one does not: the game has them only inside longer text. Russian
    /// declines the Abyss Boss there ("босса Бездны"); the title takes the
    /// nominative.
    const BOSS_WORDS: [(&str, &str, &[(&str, &str)]); 2] = [
        (ABYSS_BOSS, "AchievementString_Archieve_STR_Group_Season_G1_L_Thema_234211501_obj_desc", &[]),
        (
            crate::field_bosses::FIELD_BOSS,
            "AchievementString_Archieve_STR_Group_L_Challenge_Boss_312102501_obj_desc",
            &[
                ("fr", "AchievementString_Archieve_STR_Onboarding_L_S4_Board3_Tab_730431021_obj_desc"),
                ("ru", "String_STR_ITEM_MATERIAL_FIELDBOSS_FRAME_A_C_03B_B_body"),
            ],
        ),
    ];
    /// (language, key, ours, as the game declines it).
    const DECLINED: [(&str, &str, &str, &str); 1] = [("ru", ABYSS_BOSS, "Босс Бездны", "босса Бездны")];

    /// Needs the game's strings, as i18n.rs's check of the game's words:
    /// DAEVALOG_GAME_L10N, a folder with one folder per locale.
    #[test]
    fn the_boss_words_are_the_games() {
        let Some(folder) = std::env::var_os("DAEVALOG_GAME_L10N") else {
            eprintln!("DAEVALOG_GAME_L10N is not set: the game's words are not checked");
            return;
        };
        let mut wrong = Vec::new();
        for (language, locale) in [("de", "de-DE"), ("es", "es-ES"), ("fr", "fr-FR"), ("ja", "ja-JP"), ("ko", "ko-KR"), ("pt", "pt-BR"), ("ru", "ru-RU")] {
            let dir = std::path::Path::new(&folder).join(locale);
            let file = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "json")).unwrap();
            let game: Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
            for (key, source, other) in BOSS_WORDS {
                let source = other.iter().find(|(l, _)| *l == language).map_or(source, |(_, s)| s);
                let said = game["Entries"][source].as_str().unwrap_or_else(|| panic!("{locale}: no {source}")).to_lowercase();
                let mut ours = text(language, key);
                if let Some((_, _, nominative, declined)) = DECLINED.iter().find(|(l, k, _, _)| *l == language && *k == key) {
                    assert_eq!(ours, *nominative, "{language} {key}");
                    ours = declined.to_string();
                }
                if !said.contains(&ours.to_lowercase()) {
                    wrong.push(format!("{language} {key}: {ours:?} is not in {source} {said:?}"));
                }
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    #[test]
    fn a_new_server_zone_moves_a_notice_on_the_tray_at_once() {
        let n = names();
        let s = schedule();
        let w = words(&n, &same_zone);
        let mut notices = SpawnNotices::default();
        // Boss A at 01:00 on the server's clock, its notice up since the 30
        // minute lead; 20 minutes before, the zone moves 10 minutes east.
        let spawn = START + 3600;
        let key = notices.due(&s, &w, spawn - 30 * 60, &zone)[0].key.clone();
        let now = spawn - 20 * 60;
        assert!(notices.moved(&w, now, &zone).is_empty(), "the same zone moves nothing");
        let east = |at: i64| zone(at) + 600;
        let moved = notices.moved(&w, now, &east);
        assert_eq!(moved, [Move::Retime { key: key.clone(), title: "Abyss Boss at 00:50".into(), until: spawn - 600 }]);
        assert!(notices.moved(&w, now, &east).is_empty(), "moved once");
        assert!(notices.expired(spawn - 600 + KEEP_AFTER - 1).is_empty());
        assert_eq!(notices.expired(spawn - 600 + KEEP_AFTER), [key]);
    }

    #[test]
    fn a_notice_whose_new_time_is_past_or_far_leaves_the_tray_at_once() {
        let n = names();
        let s = schedule();
        let w = words(&n, &same_zone);
        let spawn = START + 3600;
        let now = spawn - 20 * 60;
        // Two hours east: the spawn was 100 minutes ago, past its 10 minutes.
        let mut notices = SpawnNotices::default();
        let key = notices.due(&s, &w, spawn - 30 * 60, &zone)[0].key.clone();
        let east = |at: i64| zone(at) + 2 * 3600;
        assert_eq!(notices.moved(&w, now, &east), [Move::Remove(key.clone())]);
        assert!(notices.expired(spawn + KEEP_AFTER).is_empty(), "gone already");
        // An hour west: the new time is 80 minutes on, before its first
        // lead, which brings it back.
        let mut notices = SpawnNotices::default();
        notices.due(&s, &w, spawn - 30 * 60, &zone);
        let west = |at: i64| zone(at) - 3600;
        assert_eq!(notices.moved(&w, now, &west), [Move::Remove(key.clone())]);
        let back = notices.due(&s, &w, spawn + 3600 - 30 * 60, &west);
        assert_eq!(back.iter().map(|n| (n.key.as_str(), n.until)).collect::<Vec<_>>(), [(key.as_str(), spawn + 3600)]);
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
