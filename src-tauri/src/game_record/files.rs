//! Finding the game's records on disk and matching them to saved fights.
//!
//! The game folder is only ever read. A record belongs to every saved fight
//! whose time overlaps the record's window and whose target has the record's
//! `TargetName`. The meter's numbers for the window come from the fights' kept
//! slices, replayed through the parser; a fight without one shows the game's
//! numbers alone.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use chrono::{FixedOffset, Local, TimeZone};
use serde::Serialize;

use super::{add_rows, compare, decode, replay_slice_window, GameRecord, SkillRow, SliceWindow, TakeStat, N_COUNTS, SLACK_MS};
use crate::capture::fight_tape;
use crate::entity::taken::TakenStats;
use crate::entity::fight_record::FightSummary;
use crate::i18n::lookup::{NpcLookup, SkillLookup};

/// Aion 2 on Steam.
const APP_ID: &str = "3393110";
/// Inside `%LOCALAPPDATA%`, or the Proton prefix's copy of it.
const RECORDS_IN_LOCAL: [&str; 4] = ["AION2", "Saved_Steam", "PersistentDownloadDir", "DamageAnalyzer"];

fn records_dir(local_app_data: PathBuf) -> PathBuf {
    RECORDS_IN_LOCAL.iter().fold(local_app_data, |p, part| p.join(part))
}

/// The library folders a Steam `libraryfolders.vdf` names.
pub fn library_paths(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|line| {
            let mut parts = line.split('"').filter(|p| !p.trim().is_empty());
            if parts.next()? != "path" {
                return None;
            }
            // Windows paths are written with doubled backslashes.
            parts.next().map(|p| PathBuf::from(p.replace("\\\\", "\\")))
        })
        .collect()
}

/// The game's record folders on this computer: one subfolder per account in each.
pub fn record_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if crate::platform::files::GAME_RECORDS_IN_LOCAL_APPDATA {
        if let Some(local) = dirs::data_local_dir() {
            roots.push(records_dir(local));
        }
    } else if let Some(home) = dirs::home_dir() {
        let mut libraries = Vec::new();
        for steam in [".local/share/Steam", ".steam/steam", ".steam/root", ".var/app/com.valvesoftware.Steam/.local/share/Steam"] {
            let root = home.join(steam);
            for vdf in [root.join("steamapps/libraryfolders.vdf"), root.join("config/libraryfolders.vdf")] {
                if let Ok(text) = std::fs::read_to_string(vdf) {
                    libraries.extend(library_paths(&text));
                }
            }
            libraries.push(root);
        }
        for library in libraries {
            let prefix = library.join("steamapps/compatdata").join(APP_ID).join("pfx/drive_c/users/steamuser/AppData/Local");
            roots.push(records_dir(prefix));
        }
    }
    // One folder reached by two paths counts once.
    let mut seen = HashSet::new();
    roots
        .into_iter()
        .filter(|r| r.is_dir())
        .filter(|r| seen.insert(r.canonicalize().unwrap_or_else(|_| r.clone())))
        .collect()
}

/// Every `record_*.dat` in the account folders under `roots`.
pub fn record_files(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in roots {
        let Ok(accounts) = std::fs::read_dir(root) else { continue };
        for account in accounts.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()) {
            let Ok(files) = std::fs::read_dir(&account) else { continue };
            for path in files.filter_map(|e| e.ok()).map(|e| e.path()) {
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if name.starts_with("record_") && name.ends_with(".dat") {
                    out.push(path);
                }
            }
        }
    }
    out.sort();
    out
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Decoded records, by file, kept while the file is unchanged.
static DECODED: LazyLock<Mutex<HashMap<PathBuf, (Option<SystemTime>, Option<Arc<GameRecord>>)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn read_record(path: &Path) -> Option<Arc<GameRecord>> {
    let stamp = modified(path);
    if let Some((s, r)) = DECODED.lock().ok()?.get(path) {
        if *s == stamp {
            return r.clone();
        }
    }
    let record = std::fs::read(path).ok().and_then(|b| decode(&b)).map(Arc::new);
    DECODED.lock().ok()?.insert(path.to_path_buf(), (stamp, record.clone()));
    record
}

/// Drops what the caches keep for files under `roots` that are no longer in
/// `files` (sorted): a record deleted or renamed on disk.
fn forget_gone(roots: &[PathBuf], files: &[PathBuf]) {
    let gone = |p: &PathBuf| roots.iter().any(|r| p.starts_with(r)) && files.binary_search(p).is_err();
    if let Ok(mut m) = DECODED.lock() {
        m.retain(|p, _| !gone(p));
    }
    if let Ok(mut m) = CHECKS.lock() {
        m.retain(|p, _| !gone(p));
    }
}

/// Mob codes that carry `name` in any language the meter has, so a record
/// from a game in one language matches fights saved in another.
static CODES_BY_NAME: LazyLock<Mutex<HashMap<String, Arc<HashSet<i32>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn codes_named(data_dir: Option<&Path>, name: &str) -> Arc<HashSet<i32>> {
    if let Some(codes) = CODES_BY_NAME.lock().ok().and_then(|m| m.get(name).cloned()) {
        return codes;
    }
    let mut codes = HashSet::new();
    let tables = data_dir.and_then(|d| std::fs::read_dir(d.join("i18n").join("npcs")).ok());
    for table in tables.into_iter().flatten().filter_map(|e| e.ok()) {
        let Ok(text) = std::fs::read_to_string(table.path()) else { continue };
        let Ok(map) = serde_json::from_str::<HashMap<String, serde_json::Value>>(&text) else { continue };
        for (code, v) in map {
            if v["name"].as_str().or(v.as_str()) == Some(name) {
                codes.extend(code.parse::<i32>().ok());
            }
        }
    }
    let codes = Arc::new(codes);
    if let Ok(mut m) = CODES_BY_NAME.lock() {
        m.insert(name.to_string(), codes.clone());
    }
    codes
}

fn fight_span(f: &FightSummary) -> (i64, i64) {
    (f.start_time_ms, f.start_time_ms + f.duration_ms.max(0))
}

/// Whether two spans of time share a moment.
pub fn overlaps(a: (i64, i64), b: (i64, i64)) -> bool {
    a.0 <= b.1 && b.0 <= a.1
}

/// Whether a record with this window and target belongs to `fight`.
pub fn belongs_to(window: (i64, i64), target: &str, fight: &FightSummary, codes: &HashSet<i32>) -> bool {
    !target.is_empty()
        && overlaps((window.0 - SLACK_MS, window.1 + SLACK_MS), fight_span(fight))
        && (fight.boss_name == target || codes.contains(&fight.mob_code))
}

/// One record file set beside the meter's replay of the fights it covers.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordCheck {
    /// The record's file name.
    pub file: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub target: String,
    /// The saved fights the record belongs to, oldest first.
    pub fights: Vec<String>,
    /// False when one of those fights kept no slice to replay.
    pub compared: bool,
    pub game_total: i64,
    pub meter_total: i64,
    pub rows: Vec<SkillRow>,
    /// The player's damage taken over the record's window. The meter's side
    /// stays zero without a comparison.
    pub game_taken: TakeStat,
    pub meter_taken: TakeStat,
}

impl RecordCheck {
    pub fn differing(&self) -> usize {
        if self.compared { self.rows.iter().filter(|r| !r.same).count() } else { 0 }
    }
}

/// What the checks need: where things are, and the meter's tables.
pub struct Checker {
    pub app_data_dir: PathBuf,
    /// The meter's `src/data`.
    pub data_dir: Option<PathBuf>,
    pub skills: Arc<SkillLookup>,
    pub npcs: Arc<NpcLookup>,
    pub fights: Vec<FightSummary>,
    /// Where the game keeps records (`record_roots()`).
    pub roots: Vec<PathBuf>,
    /// The zone the game's times are in; `None` is this computer's.
    pub zone: Option<FixedOffset>,
}

/// Checks already made, by record file, with what they were made from.
static CHECKS: LazyLock<Mutex<HashMap<PathBuf, (String, Arc<RecordCheck>)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

impl Checker {
    fn window(&self, record: &GameRecord) -> Option<(i64, i64)> {
        match self.zone {
            Some(zone) => record.window_in(&zone),
            None => record.window_in(&Local),
        }
    }

    fn dot_ids(&self) -> HashSet<i32> {
        self.data_dir
            .as_ref()
            .and_then(|d| std::fs::read_to_string(d.join("dot_skill_ids.json")).ok())
            .and_then(|t| serde_json::from_str::<Vec<i32>>(&t).ok())
            .unwrap_or_default()
            .into_iter()
            .collect()
    }

    /// Every record that belongs to a saved fight, checked.
    pub fn checks(&self) -> Vec<Arc<RecordCheck>> {
        let dot_ids = self.dot_ids();
        let mut out = Vec::new();
        let files = record_files(&self.roots);
        forget_gone(&self.roots, &files);
        for path in files {
            let Some(record) = read_record(&path) else { continue };
            let Some(window) = self.window(&record) else { continue };
            let codes = codes_named(self.data_dir.as_deref(), &record.target);
            let mut fights: Vec<&FightSummary> =
                self.fights.iter().filter(|f| !f.is_live && belongs_to(window, &record.target, f, &codes)).collect();
            if fights.is_empty() {
                continue;
            }
            fights.sort_by_key(|f| (f.start_time_ms, f.id.clone()));
            // A fight still being saved changes its length and its slice.
            let key = format!(
                "{:?}|{}",
                modified(&path),
                fights
                    .iter()
                    .map(|f| format!("{}:{}:{:?}", f.id, f.duration_ms, crate::share::slice_stamp(&self.app_data_dir, &f.id)))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            if let Some((k, check)) = CHECKS.lock().ok().and_then(|m| m.get(&path).cloned()) {
                if k == key {
                    out.push(check);
                    continue;
                }
            }
            let check = Arc::new(self.check(&path, &record, window, &fights, &dot_ids));
            if let Ok(mut m) = CHECKS.lock() {
                m.insert(path.clone(), (key, check.clone()));
            }
            out.push(check);
        }
        out
    }

    fn check(
        &self,
        path: &Path,
        record: &GameRecord,
        window: (i64, i64),
        fights: &[&FightSummary],
        dot_ids: &HashSet<i32>,
    ) -> RecordCheck {
        // The record's damage to its target, across every fight it covers:
        // each fight's slice counts only its own stretch of the window.
        let mut meter = BTreeMap::new();
        let mut taken = TakenStats::default();
        let mut compared = true;
        for (i, f) in fights.iter().enumerate() {
            let Some((bytes, owner)) = crate::share::read_slice(&self.app_data_dir, &f.id) else {
                compared = false;
                break;
            };
            let Some((records, _)) = fight_tape::decode(&bytes) else {
                compared = false;
                break;
            };
            let (start, end) = fight_span(f);
            // The record's window for damage taken, split where the next
            // fight starts.
            let taken_from = if i == 0 { window.0 } else { window.0.max(start) };
            let taken_until = fights.get(i + 1).map_or(window.1, |next| window.1.min(next.start_time_ms - 1));
            let replay = replay_slice_window(
                &SliceWindow {
                    records: &records,
                    fight_start_ms: f.start_time_ms,
                    target_id: f.target_id,
                    from: window.0.max(start) - SLACK_MS,
                    until: window.1.min(end) + SLACK_MS,
                    owner,
                    game_total: record.total,
                    taken: (taken_from, taken_until),
                },
                &self.skills,
                &self.npcs,
                dot_ids,
            );
            add_rows(&mut meter, &replay.rows);
            taken.absorb(&replay.taken);
        }
        if !compared {
            meter.clear();
            taken = TakenStats::default();
        }
        RecordCheck {
            file: path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            start_ms: window.0,
            end_ms: window.1,
            target: record.target.clone(),
            fights: fights.iter().map(|f| f.id.clone()).collect(),
            compared,
            game_total: record.total,
            meter_total: meter.values().map(|r| r.damage).sum(),
            rows: compare(&record.skills, &meter),
            game_taken: record.taken,
            meter_taken: TakeStat::of_meter(&taken),
        }
    }

    /// One line per fight with a record, for History.
    pub fn statuses(&self) -> HashMap<String, FightStatus> {
        let mut out: HashMap<String, FightStatus> = HashMap::new();
        for check in self.checks() {
            for id in &check.fights {
                let s = out.entry(id.clone()).or_default();
                s.records += 1;
                if check.compared {
                    s.compared += 1;
                    s.differing_rows += check.differing();
                }
            }
        }
        out
    }

    /// The records of one fight, for Details.
    pub fn views(&self, fight_id: &str) -> Vec<RecordView> {
        let Some(fight) = self.fights.iter().find(|f| f.id == fight_id) else { return Vec::new() };
        self.checks()
            .into_iter()
            .filter(|c| c.fights.iter().any(|id| id == fight_id))
            .map(|c| {
                let span = fight_span(fight);
                let names: BTreeMap<i32, String> =
                    c.rows.iter().map(|r| (r.skill_id, self.skills.skill_title(r.skill_id))).collect();
                RecordView {
                    actor_id: crate::share::slice_uploader(&self.app_data_dir, fight_id),
                    covered_ms: (c.end_ms.min(span.1) - c.start_ms.max(span.0)).max(0),
                    other_fights: c.fights.len() - 1,
                    report: report(fight, &c, &names, self.zone),
                    names,
                    check: (*c).clone(),
                }
            })
            .collect()
    }
}

/// A fight's records in History.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FightStatus {
    pub records: usize,
    /// Records with a comparison (the fight has its slice).
    pub compared: usize,
    pub differing_rows: usize,
}

/// A check as one fight's Details shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordView {
    #[serde(flatten)]
    pub check: RecordCheck,
    /// Whose numbers the meter side is, when the slice says.
    pub actor_id: Option<i32>,
    /// How much of the record's window this fight spans.
    pub covered_ms: i64,
    /// The other saved fights the record also covers.
    pub other_fights: usize,
    /// Skill names by row id, in the meter's language.
    pub names: BTreeMap<i32, String>,
    /// The text behind "Copy for a bug report".
    pub report: String,
}

const COUNT_LABELS: [&str; N_COUNTS] = [
    "hits", "crit", "perfect", "double", "front", "back", "additional hits",
    "block", "miss", "immune", "endurance", "regeneration",
];

fn clock(ms: i64, zone: Option<FixedOffset>, format: &str) -> String {
    match zone {
        Some(z) => z.timestamp_millis_opt(ms).single().map(|t| t.format(format).to_string()),
        None => Local.timestamp_millis_opt(ms).single().map(|t| t.format(format).to_string()),
    }
    .unwrap_or_default()
}

/// The bug report: the fight, the record's window, and the rows that differ
/// with both values. In English, for whoever fixes the parser.
pub fn report(fight: &FightSummary, check: &RecordCheck, names: &BTreeMap<i32, String>, zone: Option<FixedOffset>) -> String {
    let mut out = String::new();
    out.push_str(&format!("Daevalog DPS Meter {}\n", crate::version::display()));
    out.push_str(&format!(
        "Fight: {} (mob {}), {}, {:.1} s, id {}\n",
        fight.boss_name,
        fight.mob_code,
        clock(fight.start_time_ms, zone, "%Y-%m-%d %H:%M:%S"),
        fight.duration_ms as f64 / 1000.0,
        fight.id
    ));
    out.push_str(&format!(
        "Game record: {}, {} to {}, target {}\n",
        check.file,
        clock(check.start_ms, zone, "%H:%M:%S%.3f"),
        clock(check.end_ms, zone, "%H:%M:%S%.3f"),
        check.target
    ));
    if check.fights.len() > 1 {
        out.push_str(&format!("The record covers {} saved fights: {}\n", check.fights.len(), check.fights.join(", ")));
    }
    if !check.compared {
        out.push_str("No packets saved for this fight: no comparison.\n");
        return out;
    }
    out.push_str(&format!("Damage: meter {}, game {}\n", check.meter_total, check.game_total));
    out.push_str(&format!("Damage taken: meter {}, game {}\n", check.meter_taken.damage, check.game_taken.damage));
    let differ: Vec<&SkillRow> = check.rows.iter().filter(|r| !r.same).collect();
    out.push_str(&format!("Rows that differ: {} of {}\n", differ.len(), check.rows.len()));
    for r in differ {
        let name = names.get(&r.skill_id).filter(|n| !n.is_empty()).map(String::as_str).unwrap_or("?");
        let mut parts = Vec::new();
        if r.meter.damage != r.game.damage {
            parts.push(format!("damage meter {} game {}", r.meter.damage, r.game.damage));
        }
        for i in 0..N_COUNTS {
            if r.meter.counts[i] != r.game.counts[i] {
                parts.push(format!("{} meter {} game {}", COUNT_LABELS[i], r.meter.counts[i], r.game.counts[i]));
            }
        }
        out.push_str(&format!("  {} {}: {}\n", r.skill_id, name, parts.join("; ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_record::tests::{record_bytes, row};

    fn fight(id: &str, name: &str, mob: i32, start: i64, duration: i64) -> FightSummary {
        FightSummary {
            id: id.into(),
            boss_name: name.into(),
            target_id: 77,
            start_time_ms: start,
            duration_ms: duration,
            total_damage: 0,
            jobs: vec![],
            job_ids: vec![],
            is_train: true,
            is_live: false,
            app_version: String::new(),
            mob_code: mob,
            dungeon_id: 0,
            member_jobs: vec![],
        }
    }

    #[test]
    fn library_folders_are_read_from_the_vdf() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/home/a/.local/share/Steam\"\n\t\t\"label\"\t\t\"\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}\n";
        assert_eq!(
            library_paths(vdf),
            vec![PathBuf::from("/home/a/.local/share/Steam"), PathBuf::from("D:\\SteamLibrary")]
        );
    }

    #[test]
    fn a_record_goes_with_fights_on_its_target_that_share_its_time() {
        let window = (10_000, 60_000);
        let none = HashSet::new();
        let on = |name: &str, start: i64, duration: i64, codes: &HashSet<i32>| {
            belongs_to(window, "Training Scarecrow", &fight("f", name, 0, start, duration), codes)
        };
        let scarecrow = "Training Scarecrow";
        assert!(on(scarecrow, 5_000, 10_000, &none), "runs into the window");
        assert!(on(scarecrow, 55_000, 30_000, &none), "runs out of it");
        assert!(!on(scarecrow, 0, 9_000, &none), "ends before");
        assert!(!on(scarecrow, 61_000, 9_000, &none), "starts after");
        assert!(on(scarecrow, 60_400, 9_000, &none), "inside the slack");
        assert!(!on("Punching Bag", 20_000, 9_000, &none), "another target");
        let korean = fight("k", "허수아비", 2_400_032, 20_000, 9_000);
        assert!(belongs_to(window, "Training Scarecrow", &korean, &[2_400_032].into()), "same mob, other language");
        assert!(!belongs_to(window, "", &fight("e", "", 0, 20_000, 9_000), &none), "no target name");
    }

    #[test]
    fn records_on_disk_are_matched_to_fights_without_slices() {
        let tmp = crate::built::Scratch::new("record-files");
        tmp.put("records/acc1/notes.txt", "not a record");
        let skill: &[i64] = &[20, 1, 2, 0, 20, 0, 6];
        let first = record_bytes(
            "2026-01-02T12:00:00.500Z",
            "2026-01-02T12:00:52.400Z",
            "Training Scarecrow",
            &[(16_040_000, 20_000, skill)],
        );
        tmp.put("records/acc1/record_1.dat", first);
        tmp.put("records/acc1/record_2.dat", record_bytes("2026-01-02T17:00:00.000Z", "2026-01-02T17:01:00.000Z", "Training Scarecrow", &[]));
        std::fs::create_dir_all(tmp.join("app")).unwrap();
        let start = chrono::DateTime::parse_from_rfc3339("2026-01-02T11:59:52.500Z").unwrap().timestamp_millis();
        let roots = vec![tmp.join("records")];
        assert_eq!(record_files(&roots).len(), 2);
        let checker = Checker {
            app_data_dir: tmp.join("app"),
            data_dir: None,
            skills: Arc::new(SkillLookup::new()),
            npcs: Arc::new(NpcLookup::new()),
            fights: vec![
                fight("auto_77_1", "Training Scarecrow", 2_400_032, start, 70_000),
                fight("auto_78_1", "Punching Bag", 2_000_000, start, 70_000),
            ],
            roots,
            zone: FixedOffset::east_opt(0),
        };
        let statuses = checker.statuses();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses["auto_77_1"], FightStatus { records: 1, compared: 0, differing_rows: 0 });
        let views = checker.views("auto_77_1");
        assert_eq!(views.len(), 1);
        let view = &views[0];
        assert!(!view.check.compared);
        assert_eq!(view.check.game_total, 20_000);
        assert_eq!(view.check.rows[0].game, row(20_000, skill));
        assert_eq!((view.covered_ms, view.other_fights), (51_900, 0));
        assert!(view.report.contains("record_1.dat, 12:00:00.500 to 12:00:52.400"), "{}", view.report);
        assert!(checker.views("auto_78_1").is_empty());
    }

    #[test]
    fn records_gone_from_disk_leave_the_caches() {
        let tmp = crate::built::Scratch::new("record-gone");
        let skill: &[i64] = &[1, 1, 0, 0, 1, 0, 0];
        let bytes = |end: &str| record_bytes("2026-01-02T12:00:00.500Z", end, "Training Scarecrow", &[(16_040_000, 100, skill)]);
        let first = tmp.put("records/acc1/record_1.dat", bytes("2026-01-02T12:00:52.400Z"));
        let second = tmp.put("records/acc1/record_2.dat", bytes("2026-01-02T12:00:52.500Z"));
        std::fs::create_dir_all(tmp.join("app")).unwrap();
        let start = chrono::DateTime::parse_from_rfc3339("2026-01-02T11:59:52.500Z").unwrap().timestamp_millis();
        let checker = Checker {
            app_data_dir: tmp.join("app"),
            data_dir: None,
            skills: Arc::new(SkillLookup::new()),
            npcs: Arc::new(NpcLookup::new()),
            fights: vec![fight("auto_77_1", "Training Scarecrow", 2_400_032, start, 70_000)],
            roots: vec![tmp.join("records")],
            zone: FixedOffset::east_opt(0),
        };
        let cached = |p: &PathBuf| (DECODED.lock().unwrap().contains_key(p), CHECKS.lock().unwrap().contains_key(p));
        assert_eq!(checker.checks().len(), 2);
        assert_eq!((cached(&first), cached(&second)), ((true, true), (true, true)));

        std::fs::remove_file(&first).unwrap();
        let renamed = tmp.join("records/acc1/record_3.dat");
        std::fs::rename(&second, &renamed).unwrap();
        assert_eq!(checker.checks().len(), 1);
        assert_eq!((cached(&first), cached(&second)), ((false, false), (false, false)));
        assert_eq!(cached(&renamed), (true, true));
    }

    #[test]
    fn the_bug_report_names_each_differing_count_with_both_sides() {
        let hits = |damage: i64, n: i64| row(damage, &[n, 0, 0, 0, n, 0, 0]);
        let game: BTreeMap<i32, _> = [(1, hits(100, 2)), (2, hits(50, 1))].into();
        let meter: BTreeMap<i32, _> = [(1, hits(100, 2)), (2, hits(50, 0))].into();
        let taken = TakeStat { damage: 20_374, ..TakeStat::default() };
        let check = RecordCheck {
            file: "record_9.dat".into(),
            start_ms: 0,
            end_ms: 26_331,
            target: "Training Scarecrow".into(),
            fights: vec!["auto_1_2".into()],
            compared: true,
            game_total: 150,
            meter_total: 150,
            rows: compare(&game, &meter),
            game_taken: taken,
            meter_taken: taken,
        };
        let names: BTreeMap<i32, String> = [(2, "Water Bomb".to_string())].into();
        let text = report(&fight("auto_1_2", "Training Scarecrow", 2_400_032, 0, 26_300), &check, &names, FixedOffset::east_opt(0));
        let wanted = [
            "Rows that differ: 1 of 2",
            "Damage taken: meter 20374, game 20374",
            "  2 Water Bomb: hits meter 0 game 1; front meter 0 game 1",
        ];
        let missing: Vec<&str> = wanted.into_iter().filter(|w| !text.contains(w)).collect();
        assert!(missing.is_empty(), "{missing:?} not in\n{text}");
        assert!(!text.contains("  1 "), "skill 1 agrees, so it is not listed:\n{text}");
        assert_eq!(check.differing(), 1);
    }
}
