//! What History shows: the saved fights by the 1.x History page's rules
//! (public/src/js/history.js `populateDropdowns`, `applyFilters`,
//! `renderGrouped`, `buildRow`), one saved fight's numbers as 1.x Details
//! shows a fight opened from History (`details_view::saved_view`), and the
//! fight's game records as 1.x shows them (gameRecord.js, details.js
//! `gameDetails`), so a front end shows them as they come.
//!
//! It reads only what 1.x reads: the saved fights (`get_fight_history`,
//! `load_fight`), which have a link (`share_status`) and the game records
//! (`game_record_status`, `game_record_details`). `checks/history_report.rs`
//! prints these answers and the view for a folder of saved fights.
//!
//! New in 2.0 and made here too: each fight's party damage per second, your
//! damage per second and place, and per player the damage received, its
//! blocks and parries, and deaths.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use chrono::{FixedOffset, Local, NaiveDateTime, TimeZone, Timelike};
use serde::Serialize;

use crate::combat::data_storage::UNATTRIBUTED_ID;
use crate::combat::details_view::{self, Names, PlayerEntry, Received};
use crate::entity::fight_record::{FightRecord, FightSummary};
use crate::game_record::files::{FightStatus, RecordView};
use crate::game_record::Row;
use crate::i18n::lookup::NpcLookup;

/// How the list is laid out; the 1.x setting `historyViewMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    /// A section per dungeon and difficulty, the open world one more.
    #[default]
    Dungeon,
    /// A section per boss.
    Boss,
    /// Newest first.
    List,
}

impl Layout {
    pub fn from_setting(value: &str) -> Self {
        match value {
            "grouped" => Layout::Boss,
            "list" => Layout::List,
            _ => Layout::Dungeon,
        }
    }

    pub fn setting(self) -> &'static str {
        match self {
            Layout::Dungeon => "dungeon",
            Layout::Boss => "grouped",
            Layout::List => "list",
        }
    }
}

/// What the list shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Filters {
    pub layout: Layout,
    /// A boss as saved (`FightSummary::boss_name`); empty for every boss.
    pub boss: String,
    /// A class as saved ("정령성"); empty for every class.
    pub job: String,
    /// A day, "YYYY-MM-DD"; empty for every day.
    pub date: String,
    /// The 1.x setting `historyShowTraining`, on unless saved as "0".
    pub show_training: bool,
    pub only_records: bool,
}

impl Default for Filters {
    fn default() -> Self {
        Self { layout: Layout::Dungeon, boss: String::new(), job: String::new(), date: String::new(), show_training: true, only_records: false }
    }
}

/// The game's dungeon names and difficulties in one language
/// (`src/data/i18n/dungeons/<lang>.json`), as i18n.js reads them.
#[derive(Debug, Clone, Default)]
pub struct Dungeons(HashMap<i32, (String, String, String)>);

/// A dungeon's difficulty in the game's words, and its kind ("easy",
/// "normal", ... or "tier").
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tier {
    pub key: String,
    pub label: String,
}

impl Dungeons {
    /// The table of `language`; English when that one has no entries.
    pub fn load(data_dir: &Path, language: &str) -> Self {
        let read = |lang: &str| -> Self {
            let path = data_dir.join("i18n").join("dungeons").join(format!("{lang}.json"));
            let Some(map) = std::fs::read_to_string(path)
                .ok()
                .and_then(|text| serde_json::from_str::<HashMap<String, serde_json::Value>>(&text).ok())
            else {
                return Self::default();
            };
            let text = |v: &serde_json::Value, k: &str| v.get(k).and_then(|s| s.as_str()).unwrap_or_default().to_string();
            Self(
                map.iter()
                    .filter_map(|(id, v)| Some((id.parse().ok()?, (text(v, "name"), text(v, "label"), text(v, "difficulty")))))
                    .collect(),
            )
        };
        let table = read(language);
        if table.0.is_empty() && language != "en" { read("en") } else { table }
    }

    /// i18n.js `getDungeonDifficulty`.
    pub fn tier(&self, id: i32) -> Option<Tier> {
        let (_, label, difficulty) = self.0.get(&id)?;
        (!label.is_empty()).then(|| Tier {
            key: if difficulty.is_empty() { "tier".into() } else { difficulty.clone() },
            label: label.clone(),
        })
    }

    /// i18n.js `getDungeonLabel`: the name and the difficulty in brackets
    /// ("Name (Exploration)"), the name alone without a difficulty, empty
    /// when the table has none.
    pub fn label(&self, id: i32) -> String {
        let Some((name, _, _)) = self.0.get(&id).filter(|e| !e.0.is_empty()) else { return String::new() };
        match self.tier(id) {
            Some(tier) => format!("{name} ({})", tier.label),
            None => name.clone(),
        }
    }
}

/// The words History builds names with, in the window's language.
#[derive(Debug, Clone, Default)]
pub struct ListWords {
    /// `history.openWorld`.
    pub open_world: String,
    /// Each class's name by the class as saved ("정령성"); a class missing
    /// here goes by its saved name (history.js `getJobLabel`).
    pub classes: HashMap<String, String>,
}

/// Saved class name to its UI text key under `classes.`, in game order.
pub const CLASS_KEYS: [(&str, &str); 9] = [
    ("검성", "GLADIATOR"),
    ("수호성", "TEMPLAR"),
    ("궁성", "RANGER"),
    ("살성", "ASSASSIN"),
    ("마도성", "SORCERER"),
    ("치유성", "CLERIC"),
    ("정령성", "ELEMENTALIST"),
    ("호법성", "CHANTER"),
    ("권성", "FIGHTER"),
];

/// How a time of day is written: Settings' Display Time, in the reader's
/// language (shared/format.js `formatClock`). 24-hour: 14:05, 00:30.
/// 12-hour: 2:05 PM, 12:30 AM; in Korean 오후 2:05.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Clock {
    pub hour24: bool,
    marks: DayMarks,
}

/// The words for the two halves of a 12-hour day, and where they go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum DayMarks {
    /// After the time: 2:05 PM.
    #[default]
    After,
    /// 오후 2:05.
    Korean,
}

impl Clock {
    pub fn new(hour24: bool, language: &str) -> Self {
        let marks = match language {
            "ko" => DayMarks::Korean,
            _ => DayMarks::After,
        };
        Self { hour24, marks }
    }

    /// A time of day, to the minute, or with `seconds` to the second.
    pub fn text(&self, time: &impl Timelike, seconds: bool) -> String {
        let rest = if seconds { format!("{:02}:{:02}", time.minute(), time.second()) } else { format!("{:02}", time.minute()) };
        let hour = time.hour();
        if self.hour24 {
            return format!("{hour:02}:{rest}");
        }
        let h12 = format!("{}:{rest}", if hour % 12 == 0 { 12 } else { hour % 12 });
        let pm = hour >= 12;
        match self.marks {
            DayMarks::After => format!("{h12} {}", if pm { "PM" } else { "AM" }),
            DayMarks::Korean => format!("{} {h12}", if pm { "오후" } else { "오전" }),
        }
    }
}

/// Names in the reader's language, the zone dates are written in and the
/// clock times of day are written on.
pub struct ListNames<'a> {
    pub npcs: &'a NpcLookup,
    pub dungeons: &'a Dungeons,
    pub words: &'a ListWords,
    /// `None`: this computer's.
    pub zone: Option<FixedOffset>,
    pub clock: Clock,
}

impl ListNames<'_> {
    /// A fight's boss in the reader's language: saved fights keep the name
    /// as it was then, the mob code names it in any (history.js `bossLabel`).
    pub fn boss(&self, mob_code: i32, boss_name: &str, target_id: i32) -> String {
        let name = if mob_code > 0 { self.npcs.npc_title(mob_code) } else { String::new() };
        if !name.trim().is_empty() {
            return name;
        }
        if !boss_name.is_empty() {
            return boss_name.to_string();
        }
        format!("Boss #{target_id}")
    }

    /// A dungeon section's title: the dungeon and its difficulty, or the
    /// open world (history.js `dungeonTitle`).
    pub fn dungeon(&self, dungeon_id: i32) -> String {
        if dungeon_id == 0 {
            return self.words.open_world.clone();
        }
        let label = self.dungeons.label(dungeon_id);
        if label.is_empty() { format!("#{dungeon_id}") } else { label }
    }

    pub fn class(&self, job: &str) -> String {
        self.words.classes.get(job).cloned().unwrap_or_else(|| job.to_string())
    }

    fn local(&self, ms: i64) -> Option<NaiveDateTime> {
        match self.zone {
            Some(zone) => zone.timestamp_millis_opt(ms).single().map(|t| t.naive_local()),
            None => Local.timestamp_millis_opt(ms).single().map(|t| t.naive_local()),
        }
    }

    /// The day, "YYYY-MM-DD", in local time: what the date filter lists
    /// (history.js `formatDay`).
    pub fn day(&self, ms: i64) -> String {
        self.local(ms).map_or_else(|| "-".into(), |t| t.format("%Y-%m-%d").to_string())
    }

    /// The day and the time of day on the reader's clock, "YYYY-MM-DD
    /// h:mm PM" or "YYYY-MM-DD HH:mm" (history.js `formatDate`).
    pub fn date(&self, ms: i64) -> String {
        self.local(ms).map_or_else(|| "-".into(), |t| format!("{} {}", t.format("%Y-%m-%d"), self.clock.text(&t, false)))
    }

    /// The time of day to the second, "h:mm:ss PM" on a 12-hour clock
    /// (gameRecord.js `clock`).
    fn time(&self, ms: i64) -> String {
        self.local(ms).map(|t| self.clock.text(&t, true)).unwrap_or_default()
    }
}

/// A name's letters as a collation compares them first: accents and the
/// kana voicing marks set aside, case folded, hiragana as katakana.
fn base_letters(text: &str) -> String {
    icu_normalizer::DecomposingNormalizerBorrowed::new_nfd()
        .normalize(text)
        .chars()
        .filter(|c| !matches!(*c, '\u{0300}'..='\u{036F}' | '\u{3099}' | '\u{309A}'))
        .map(|c| match c {
            '\u{3041}'..='\u{3096}' => char::from_u32(c as u32 + 0x60).unwrap_or(c),
            _ => c,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

/// Names in the order the 1.x page's `localeCompare` puts them: by their
/// letters, then accents and voicing, then lowercase first.
fn label_order(a: &str, b: &str) -> std::cmp::Ordering {
    base_letters(a).cmp(&base_letters(b)).then_with(|| a.to_lowercase().cmp(&b.to_lowercase())).then_with(|| b.cmp(a))
}

/// Each player's damage in a saved fight and the time it is a rate over, as
/// the fight's DPS table has them: kept for every fight with the list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FightPlayers {
    pub ms: i64,
    /// (name, damage), most first; the unattributed row left out.
    pub players: Vec<(String, i64)>,
}

/// The time a saved fight's party table counts rates over: its own time on
/// its target, else its Details time (details_view `build`, `bar_ms`).
fn rate_ms(record: &FightRecord) -> i64 {
    let end = record.start_time_ms + record.duration_ms;
    let ms = if record.target_id > 0 && end > 0 { record.duration_ms } else { 0 };
    if ms > 0 { ms } else { record.details.battle_time }
}

pub fn fight_players(record: &FightRecord) -> FightPlayers {
    let name = |id: i32| record.actors.iter().find(|a| a.actor_id == id).map_or_else(|| id.to_string(), |a| a.nickname.clone());
    let mut players: Vec<(i32, String, i64)> = details_view::saved_actor_damage(record)
        .into_iter()
        .filter(|&(id, _)| id != UNATTRIBUTED_ID)
        .map(|(id, damage)| (id, name(id), damage))
        .collect();
    players.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    FightPlayers { ms: rate_ms(record), players: players.into_iter().map(|(_, n, d)| (n, d)).collect() }
}

/// Your damage per second in a fight and your place in its DPS table.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub per_second: Option<f64>,
    pub rank: usize,
}

/// Your place, by your name: a saved fight's ids are from its own session.
pub fn your_place(players: &FightPlayers, you: &str) -> Option<Place> {
    if you.trim().is_empty() {
        return None;
    }
    let is_you = |name: &str| details_view::saved_is_you(name, you);
    let mine = players.players.iter().filter(|(n, _)| is_you(n)).map(|(_, d)| *d).max()?;
    let rank = 1 + players.players.iter().filter(|(n, d)| !is_you(n) && *d > mine).count();
    Some(Place { per_second: (players.ms > 0).then(|| mine as f64 / players.ms as f64 * 1000.0), rank })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Choice {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListFight {
    pub id: String,
    /// The boss in the reader's language.
    pub boss: String,
    /// When it started, the day and the time as `ListNames::date` writes
    /// them.
    pub date: String,
    pub start_ms: i64,
    pub duration_ms: i64,
    pub total_damage: i64,
    /// Everyone's damage per second over the fight's time.
    pub per_second: Option<f64>,
    /// Its dungeon and difficulty, or the open world.
    pub dungeon: String,
    pub tier: Option<Tier>,
    pub train: bool,
    pub live: bool,
    /// One class per party member, at most 12 (history.js `buildRow`).
    pub jobs: Vec<String>,
    /// Its game records.
    pub record: Option<FightStatus>,
    /// It has a link on a2tools.app.
    pub uploaded: bool,
    pub you: Option<Place>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    /// Empty in the list layout.
    pub title: String,
    pub fights: Vec<ListFight>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryList {
    pub groups: Vec<Group>,
    /// Fights the filters let through, and all of them.
    pub shown: usize,
    pub total: usize,
    /// The filters' choices, from every saved fight.
    pub bosses: Vec<Choice>,
    pub jobs: Vec<Choice>,
    pub dates: Vec<String>,
    /// Any fight has a game record.
    pub any_records: bool,
}

/// What the list is built from: the backend's answers 1.x reads.
pub struct ListInput<'a> {
    /// `fight_summaries`, newest first.
    pub fights: &'a [FightSummary],
    /// `fight_players` of each fight, by id.
    pub players: &'a HashMap<String, FightPlayers>,
    /// `Checker::statuses`.
    pub records: &'a HashMap<String, FightStatus>,
    /// Fights with a link (`share_status`).
    pub uploaded: &'a HashSet<String>,
    /// Your name; empty when not known.
    pub you: &'a str,
}

/// The list as 1.x History lists it with these filters.
pub fn list(input: &ListInput, filters: &Filters, names: &ListNames) -> HistoryList {
    // The choices, from every fight (history.js `populateDropdowns`).
    let mut bosses: Vec<Choice> = Vec::new();
    for f in input.fights.iter().filter(|f| !f.boss_name.is_empty()) {
        if !bosses.iter().any(|b| b.value == f.boss_name) {
            bosses.push(Choice { value: f.boss_name.clone(), label: names.boss(f.mob_code, &f.boss_name, f.target_id) });
        }
    }
    bosses.sort_by(|a, b| label_order(&a.label, &b.label));
    let mut jobs: Vec<Choice> = Vec::new();
    for job in input.fights.iter().flat_map(|f| &f.jobs).filter(|j| !j.is_empty()) {
        if !jobs.iter().any(|c| &c.value == job) {
            jobs.push(Choice { value: job.clone(), label: names.class(job) });
        }
    }
    jobs.sort_by(|a, b| label_order(&a.label, &b.label));
    let mut dates: Vec<String> = input.fights.iter().map(|f| names.day(f.start_time_ms)).filter(|d| d != "-").collect();
    dates.sort();
    dates.dedup();
    dates.reverse();

    // history.js `applyFilters`.
    let shown: Vec<&FightSummary> = input
        .fights
        .iter()
        .filter(|f| {
            !(f.is_train && !filters.show_training)
                && (filters.boss.is_empty() || f.boss_name == filters.boss)
                && (filters.job.is_empty() || f.jobs.contains(&filters.job))
                && (filters.date.is_empty() || names.day(f.start_time_ms) == filters.date)
                && (!filters.only_records || input.records.contains_key(&f.id))
        })
        .collect();

    let entry = |f: &FightSummary| {
        let jobs = if f.member_jobs.is_empty() { &f.jobs } else { &f.member_jobs };
        ListFight {
            id: f.id.clone(),
            boss: names.boss(f.mob_code, &f.boss_name, f.target_id),
            date: names.date(f.start_time_ms),
            start_ms: f.start_time_ms,
            duration_ms: f.duration_ms,
            total_damage: f.total_damage,
            per_second: (f.duration_ms > 0).then(|| f.total_damage as f64 / f.duration_ms as f64 * 1000.0),
            dungeon: names.dungeon(f.dungeon_id),
            tier: if f.dungeon_id != 0 { names.dungeons.tier(f.dungeon_id) } else { None },
            train: f.is_train,
            live: f.is_live,
            jobs: jobs.iter().take(12).filter(|j| !j.is_empty()).cloned().collect(),
            record: input.records.get(&f.id).cloned(),
            uploaded: input.uploaded.contains(&f.id),
            you: input.players.get(&f.id).and_then(|p| your_place(p, input.you)),
        }
    };

    // history.js `renderGrouped`: sections keyed by what their title says,
    // newest run first inside, sections by their newest run.
    let groups = match filters.layout {
        Layout::List => vec![Group { title: String::new(), fights: shown.iter().map(|f| entry(f)).collect() }],
        layout => {
            let mut groups: Vec<Group> = Vec::new();
            for f in &shown {
                let title = if layout == Layout::Dungeon { names.dungeon(f.dungeon_id) } else { names.boss(f.mob_code, &f.boss_name, f.target_id) };
                match groups.iter_mut().find(|g| g.title == title) {
                    Some(g) => g.fights.push(entry(f)),
                    None => groups.push(Group { title, fights: vec![entry(f)] }),
                }
            }
            for g in &mut groups {
                g.fights.sort_by(|a, b| b.start_ms.cmp(&a.start_ms));
            }
            let newest = |g: &Group| g.fights.iter().map(|f| f.start_ms).max().unwrap_or(0);
            groups.sort_by(|a, b| newest(b).cmp(&newest(a)));
            groups
        }
    };
    HistoryList {
        groups,
        shown: shown.len(),
        total: input.fights.len(),
        bosses,
        jobs,
        dates,
        any_records: !input.records.is_empty(),
    }
}

/// A row of the DPS table: a player's damage on the fight's target (1.x's
/// party bar), with their crit rate and hits (1.x's overview of that bar).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DpsRow {
    #[serde(flatten)]
    pub player: PlayerEntry,
    /// Bar length, 0 to 1.
    pub fill: f64,
    pub crit_rate: f64,
    pub hits: i64,
}

/// A row of the Heal table: 1.x's party bar under HEAL.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealRow {
    #[serde(flatten)]
    pub player: PlayerEntry,
    pub fill: f64,
}

/// A row of the Tank table: the damage a player received (`amount`), per
/// second, as a share of everyone's, with the attacks, blocks and parries
/// behind it, and their deaths (1.x's overview of that player).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TankRow {
    #[serde(flatten)]
    pub player: PlayerEntry,
    pub fill: f64,
    pub attacks: i64,
    pub shield_block: i64,
    pub parry: i64,
    /// `None` when the game sent no HP for the player.
    pub deaths: Option<i64>,
}

/// One skill row of a game record: the game's numbers and the meter's.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordRow {
    pub code: i32,
    /// "#code" when the tables have no name.
    pub name: String,
    pub game: Row,
    pub meter: Row,
    pub same: bool,
    /// The game has a number on it, so the game's view lists it.
    pub in_game: bool,
    /// The same for the meter.
    pub in_meter: bool,
}

/// A game record of the fight as 1.x shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordEntry {
    pub file: String,
    /// The record's window, to the second (`ListNames::time`).
    pub from: String,
    pub to: String,
    pub target: String,
    /// False: no packets were kept for the fight, only the game's numbers.
    pub compared: bool,
    pub game_total: i64,
    pub meter_total: i64,
    /// Damage received over the window.
    pub game_taken: i64,
    pub meter_taken: i64,
    /// Seconds of this fight the window covers.
    pub covers: i64,
    pub other_fights: usize,
    /// Rows the game and the meter agree on, of `rows`.
    pub matching: usize,
    pub rows: Vec<RecordRow>,
    /// "Copy for a bug report".
    pub report: String,
}

/// One saved fight.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FightView {
    pub id: String,
    pub boss: String,
    pub dungeon: String,
    pub tier: Option<Tier>,
    pub date: String,
    pub train: bool,
    pub live: bool,
    pub uploaded: bool,
    /// Players in the DPS table, the unattributed row left out.
    pub players: usize,
    /// Its overview, every player: 1.x Details on a saved fight.
    pub battle_ms: i64,
    pub damage: i64,
    pub per_second: Option<f64>,
    pub heal: i64,
    pub received: Option<Received>,
    pub deaths: Option<i64>,
    pub dps: Vec<DpsRow>,
    pub heal_rows: Vec<HealRow>,
    pub tank: Vec<TankRow>,
    /// Its game records, oldest first; 1.x opens on the last.
    pub records: Vec<RecordEntry>,
}

fn fills<T>(rows: &mut [T], amount: impl Fn(&T) -> i64, set: impl Fn(&mut T, f64)) {
    let top = rows.iter().map(&amount).max().unwrap_or(0).max(1);
    for row in rows.iter_mut() {
        let a = amount(row);
        set(row, (a as f64 / top as f64).clamp(0.0, 1.0));
    }
}

/// What 1.x shows of a game record (gameRecord.js `summary`, `coverage`;
/// details.js `gameDetails`).
pub fn record_entry(view: &RecordView, names: &ListNames) -> RecordEntry {
    let c = &view.check;
    let rows: Vec<RecordRow> = c
        .rows
        .iter()
        .map(|r| RecordRow {
            code: r.skill_id,
            name: view.names.get(&r.skill_id).filter(|n| !n.is_empty()).cloned().unwrap_or_else(|| format!("#{}", r.skill_id)),
            game: r.game,
            meter: r.meter,
            same: r.same,
            in_game: r.game.damage > 0 || r.game.counts.iter().any(|&n| n != 0),
            in_meter: r.meter.damage > 0 || r.meter.counts.iter().any(|&n| n != 0),
        })
        .collect();
    RecordEntry {
        file: c.file.clone(),
        from: names.time(c.start_ms),
        to: names.time(c.end_ms),
        target: c.target.clone(),
        compared: c.compared,
        game_total: c.game_total,
        meter_total: c.meter_total,
        game_taken: c.game_taken.damage,
        meter_taken: c.meter_taken.damage,
        covers: (view.covered_ms as f64 / 1000.0 + 0.5).floor() as i64,
        other_fights: view.other_fights,
        matching: rows.iter().filter(|r| r.same).count(),
        rows,
        report: view.report.clone(),
    }
}

/// One saved fight as History shows it. `you`: your name; `uploaded`: it has
/// a link; `live`: the meter is still in it; `records`: its game records
/// (`Checker::views`).
pub fn fight(record: &FightRecord, you: &str, uploaded: bool, live: bool, records: &[RecordView], details: &dyn Names, names: &ListNames) -> FightView {
    let view = details_view::saved_view(record, you, details);
    let all = &view.damage.stats;
    let overview = |id: i32| details_view::saved_overview(record, &[id], details);

    let mut dps: Vec<DpsRow> = view.players[..view.bars]
        .iter()
        .map(|p| {
            let (stats, _) = overview(p.id);
            DpsRow { player: p.clone(), fill: 0.0, crit_rate: stats.crit_rate, hits: stats.hits }
        })
        .collect();
    fills(&mut dps, |r| r.player.amount, |r, f| r.fill = f);
    let mut heal_rows: Vec<HealRow> = view.healers.iter().map(|p| HealRow { player: p.player.clone(), fill: 0.0 }).collect();
    fills(&mut heal_rows, |r| r.player.amount, |r, f| r.fill = f);

    // Everyone who received damage or has a death count, most received first.
    let mut ids: Vec<i32> = record.details.taken_skills.iter().map(|e| e.actor_id).collect();
    ids.extend(record.details.deaths.iter().flatten().map(|e| e.actor_id));
    let mut seen = HashSet::new();
    ids.retain(|id| *id > 0 && seen.insert(*id));
    let received_all = all.received.map_or(0, |r| r.damage);
    let ctx = details_view::saved_context(record, you);
    let you_named = details_view::You { id: None, name: you.trim() };
    let mut tank: Vec<TankRow> = ids
        .into_iter()
        .map(|id| {
            let (stats, taken) = overview(id);
            let damage = stats.received.map_or(0, |r| r.damage);
            let share = if received_all > 0 { damage as f64 / received_all as f64 * 100.0 } else { 0.0 };
            TankRow {
                player: details_view::player_entry(&ctx, id, &you_named, damage, taken.per_second, share),
                fill: 0.0,
                attacks: stats.received.map_or(0, |r| r.attacks),
                shield_block: taken.shield_block,
                parry: taken.parry,
                deaths: stats.deaths,
            }
        })
        .collect();
    tank.sort_by(|a, b| b.player.amount.cmp(&a.player.amount).then(a.player.id.cmp(&b.player.id)));
    fills(&mut tank, |r| r.player.amount, |r, f| r.fill = f);

    FightView {
        id: record.id.clone(),
        boss: names.boss(record.mob_code, &record.boss_name, record.target_id),
        dungeon: names.dungeon(record.dungeon_id),
        tier: if record.dungeon_id != 0 { names.dungeons.tier(record.dungeon_id) } else { None },
        date: names.date(record.start_time_ms),
        train: record.is_train,
        live,
        uploaded,
        players: view.players[..view.bars].iter().filter(|p| !p.unattributed).count(),
        battle_ms: all.battle_ms,
        damage: all.damage,
        per_second: all.per_second,
        heal: view.heal.stats.heal,
        received: all.received,
        deaths: all.deaths,
        dps,
        heal_rows,
        tank,
        records: records.iter().map(|r| record_entry(r, names)).collect(),
    }
}

/// Deletes a saved fight and the packets kept for it, as 1.x deletes one
/// from History. The id must be a plain file name.
pub fn delete_fight(app_data_dir: &Path, history: &super::fight_history::FightHistoryManager, id: &str) -> Result<(), String> {
    if !super::fight_history::is_plain_name(id) {
        return Err(format!("Invalid fight id: {id:?}"));
    }
    crate::share::delete_slice_files(app_data_dir, id);
    history.delete_fight(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_sort_by_their_letters_first() {
        let mut names = vec!["クレリック", "グラディエーター", "Zed", "écho", "Echo", "alpha"];
        names.sort_by(|a, b| label_order(a, b));
        assert_eq!(names, ["alpha", "Echo", "écho", "Zed", "グラディエーター", "クレリック"]);
        assert_eq!(base_letters("ガード"), base_letters("かーと"), "hiragana as katakana, voicing set aside");
    }

    #[test]
    fn a_time_of_day_on_each_clock() {
        let at = |h, m, s| chrono::NaiveTime::from_hms_opt(h, m, s).unwrap();
        let (h12, h24) = (Clock::new(false, "en"), Clock::new(true, "en"));
        assert_eq!(h12.text(&at(14, 5, 3), false), "2:05 PM");
        assert_eq!(h12.text(&at(0, 30, 0), false), "12:30 AM", "midnight");
        assert_eq!(h12.text(&at(12, 0, 9), true), "12:00:09 PM", "noon");
        assert_eq!(h12.text(&at(11, 59, 0), false), "11:59 AM");
        assert_eq!(h24.text(&at(14, 5, 3), false), "14:05");
        assert_eq!(h24.text(&at(0, 30, 7), true), "00:30:07", "midnight");
        assert_eq!(h24.text(&at(12, 0, 0), false), "12:00", "noon");
        // Korean puts the half of the day first, in its words.
        assert_eq!(Clock::new(false, "ko").text(&at(15, 30, 0), false), "오후 3:30");
        assert_eq!(Clock::new(false, "ko").text(&at(0, 5, 0), false), "오전 12:05");
        assert_eq!(Clock::new(true, "ko").text(&at(15, 30, 0), false), "15:30");
        assert_eq!(Clock::new(false, "ja").text(&at(15, 30, 0), false), "3:30 PM");
        assert_eq!(Clock::default(), Clock::new(false, "en"), "12-hour unless set");
    }

    #[test]
    fn dates_and_record_times_follow_the_clock() {
        let (npcs, dungeons, words) = (NpcLookup::new(), Dungeons::default(), ListWords::default());
        let names = |hour24, language| ListNames {
            npcs: &npcs,
            dungeons: &dungeons,
            words: &words,
            zone: FixedOffset::east_opt(0),
            clock: Clock::new(hour24, language),
        };
        let ms = |h, m, s| chrono::Utc.with_ymd_and_hms(2026, 1, 2, h, m, s).unwrap().timestamp_millis();
        assert_eq!(names(false, "en").date(ms(0, 5, 9)), "2026-01-02 12:05 AM");
        assert_eq!(names(true, "en").date(ms(0, 5, 9)), "2026-01-02 00:05");
        assert_eq!(names(false, "ko").date(ms(12, 30, 0)), "2026-01-02 오후 12:30");
        assert_eq!(names(false, "en").time(ms(0, 5, 9)), "12:05:09 AM");
        assert_eq!(names(true, "en").time(ms(13, 5, 9)), "13:05:09");
        for (hour24, language) in [(false, "en"), (true, "en"), (false, "ko")] {
            assert_eq!(names(hour24, language).day(ms(23, 59, 0)), "2026-01-02");
        }
        let never = i64::MAX;
        assert_eq!((names(false, "en").day(never), names(false, "en").date(never), names(false, "en").time(never)), ("-".into(), "-".into(), String::new()));
    }
}
