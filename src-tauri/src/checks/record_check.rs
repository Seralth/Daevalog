//! Sets a game Damage Analyzer record beside a replay of the same fight, skill
//! by skill: the game's numbers are the truth the meter is checked against.
//! The decoding and the comparison are `crate::game_record`'s; this replays a
//! whole capture instead of a saved fight's slice.
//!
//! ```text
//! A2_RECORD=record_<ticks>.dat     the game's record (required); Aion 2 saves
//!                                  them under .../PersistentDownloadDir/DamageAnalyzer/
//! A2_REPLAY_FILE=packets_x.txt     a capture that covers the fight (required)
//! A2_SLACK_MS=500                  widen the record's window by this much on each
//!                                  side (its clock and the capture's differ a little)
//! A2_TAKEN_SLACK_MS=0              the same for the damage taken, which matches the
//!                                  game with the record's own window
//! cargo test --lib record_check -- --ignored --nocapture
//! ```
//!
//! After the skill rows it sets the record's damage taken (`TakeStatData`)
//! beside what the meter read of the hits on the same player in the window.
//!
//! `saved_fights_match_the_game` runs the app's own path end to end: it
//! replays the capture as the live meter does, saves fights and their slices
//! to a scratch folder, and checks every record in `A2_RECORDS` against them.
//!
//! ```text
//! A2_RECORDS=/tmp/a2-records       a folder of record_*.dat files
//! A2_REPLAY_FILE=packets_x.txt     a capture that covers them
//! A2_KEEP_APP=<dir>                save every fight, not only those near a
//!                                  record, into the data folder <dir> (its
//!                                  history/ and slices/) and keep it, for
//!                                  History; then A2_RECORDS may be left out
//! cargo test --lib saved_fights_match_the_game -- --ignored --nocapture
//! ```

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::capture::fight_tape::{self, SliceRecord};
use crate::capture::reassembly::Reassembler;
use crate::capture::decoder::PacketDecoder;
use crate::combat::data_storage::{DataStorage, TargetTotals};
use crate::entity::fight_record::FightRecord;
use crate::entity::taken::TakenStats;
use crate::game_record::files::{overlaps, Checker, RecordCheck};
use crate::game_record::{self, Row, SkillRow, TakeStat, WindowTap, COUNTS, TAKEN_COUNTS};
use crate::history::fight_history::FightHistoryManager;
use crate::i18n::lookup::{NpcLookup, SkillLookup};

use super::{data_dir, lookups};

fn env(n: &str) -> Option<String> {
    std::env::var(n).ok().filter(|v| !v.is_empty())
}

/// One capture line: time (ms since the epoch, and the zone it was written
/// in), stream, bytes.
fn capture_line(line: &str) -> Option<(i64, chrono::FixedOffset, &str, Vec<u8>)> {
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let mut parts = line.splitn(3, '|');
    let (ts, key, hex) = (parts.next()?, parts.next()?, parts.next()?);
    let when = chrono::DateTime::parse_from_rfc3339(ts).ok()?;
    let bytes = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()).collect::<Option<Vec<u8>>>()?;
    Some((when.timestamp_millis(), *when.offset(), key, bytes))
}

/// The live meter's parsers, one per stream, on one storage.
// Not `PacketLogReplay`: that drops repeated payloads, and the record check must count every one.
struct Feeder {
    storage: Arc<DataStorage>,
    skills: Arc<SkillLookup>,
    npcs: Arc<NpcLookup>,
    dot_ids: HashSet<i32>,
    streams: HashMap<String, (Reassembler, PacketDecoder)>,
}

impl Feeder {
    fn new() -> Feeder {
        let (skills, npcs, dot_ids) = lookups();
        Feeder { storage: Arc::new(DataStorage::new()), skills, npcs, dot_ids, streams: HashMap::new() }
    }

    fn feed(&mut self, stream: &str, at: i64, bytes: &[u8]) {
        if !self.streams.contains_key(stream) {
            let fresh = (Reassembler::new(), self.parser());
            self.streams.insert(stream.to_string(), fresh);
        }
        let Some((joiner, parser)) = self.streams.get_mut(stream) else { return };
        parser.pin_clock(Some(at));
        joiner.push_bytes(bytes, parser);
    }

    fn parser(&self) -> PacketDecoder {
        let tables = (Arc::clone(&self.skills), Arc::clone(&self.npcs));
        let mut out = PacketDecoder::new(Arc::clone(&self.storage), tables.0, tables.1);
        out.use_tick_skills(self.dot_ids.clone());
        out
    }
}

/// A target, the player the record is about, and that player's rows on it.
type Pick = (i32, i32, BTreeMap<i32, Row>);

/// The target in the window whose damage, by the player it is about, comes
/// nearest the record: (target, owner, rows). The record's target name, when
/// a target carries it, counts for a whole record total.
fn pick_target(
    record: &game_record::GameRecord,
    feeder: &Feeder,
    before: &HashMap<i32, TargetTotals>,
    after: &HashMap<i32, TargetTotals>,
) -> Option<Pick> {
    let links = feeder.storage.pet_owners();
    let local = feeder.storage.local_player_id().map(|id| id as i32);
    let mobs = feeder.storage.enemy_code_map();
    let mut targets: Vec<i32> = after.keys().copied().collect();
    targets.sort();
    let mut best: Option<(i64, Pick)> = None;
    for target in targets {
        let rows = game_record::rows_between(before.get(&target), &after[&target], &links, &feeder.skills);
        let owner = match local {
            Some(me) if rows.keys().any(|(o, _)| *o == me) => me,
            Some(_) => continue,
            None => match game_record::closest_owner(&rows, record.total) {
                Some(o) => o,
                None => continue,
            },
        };
        let mine = game_record::rows_of(&rows, owner);
        let damage: i64 = mine.values().map(|r| r.damage).sum();
        let named = feeder.npcs.npc_title(mobs.get(&target).copied().unwrap_or(0)) == record.target;
        let gap = (damage - record.total).abs() + if named { 0 } else { record.total };
        if best.as_ref().is_none_or(|(g, _)| gap < *g) {
            best = Some((gap, (target, owner, mine)));
        }
    }
    best.map(|(_, pick)| pick)
}

/// A game record beside a whole capture's replay over the record's window.
#[test]
#[ignore]
fn record_check() {
    let (Some(record_path), Some(capture)) = (env("A2_RECORD"), env("A2_REPLAY_FILE")) else {
        eprintln!("set A2_RECORD and A2_REPLAY_FILE");
        return;
    };
    let slack = env("A2_SLACK_MS").and_then(|v| v.parse().ok()).unwrap_or(game_record::SLACK_MS);
    let record = game_record::decode(&std::fs::read(record_path).expect("record readable")).expect("a record");
    println!(
        "record: {} {} .. {}, {} damage, {} skills",
        record.target,
        record.start,
        record.end,
        record.total,
        record.skills.len()
    );
    let text = std::fs::read_to_string(capture).expect("capture readable");
    let mut feeder = Feeder::new();
    let mut taken = super::replay_taken::Gather::default();
    let mut window = (0, 0);
    let mut tap: Option<WindowTap> = None;
    for (at, zone, stream, bytes) in text.lines().filter_map(capture_line) {
        let tap = tap.get_or_insert_with(|| {
            window = record.window_in(&zone).expect("record window");
            WindowTap::new(window.0 - slack, window.1 + slack)
        });
        if !tap.step(&feeder.storage, at) {
            break;
        }
        feeder.feed(stream, at, &bytes);
        taken.after_line(&feeder.storage);
    }
    crate::clock::set_override(None);
    let storage = feeder.storage.clone();
    let (before, after) = tap.expect("the capture has packets").finish(&storage);
    let local = storage.local_player_id();
    let local32 = local.map(|id| id as i32);
    let (skills, npcs) = (&feeder.skills, &feeder.npcs);
    let Some((target, owner, rows)) = pick_target(&record, &feeder, &before, &after) else {
        println!("no damage in the window");
        if let Some(me) = local32 {
            compare_taken(&record, &taken, window, me, skills, npcs);
        }
        return;
    };
    let damage: i64 = rows.values().map(|r| r.damage).sum();
    println!("meter: target {target}, actor {owner} (local player {local:?}), {damage} damage, {} skills", rows.len());
    print_rows(&game_record::compare(&record.skills, &rows), skills);
    println!("total: game {}, meter {damage}", record.total);
    compare_taken(&record, &taken, window, owner, skills, npcs);
}

/// How often the live meter saves its open fights.
const AUTO_SAVE_MS: i64 = 30_000;
/// How far a saved fight may sit from a record and still get its slice.
const NEAR_MS: i64 = 60_000;

/// A capture replayed as the live meter runs: its zone, every packet, and
/// the fights the meter saved, by id.
struct Replayed {
    zone: chrono::FixedOffset,
    kept: Vec<SliceRecord>,
    saved: BTreeMap<String, FightRecord>,
}

/// Replays the capture's text into `feeder` and saves fights as the live
/// meter does: every `AUTO_SAVE_MS`, then all open ones at the end.
fn replay_and_save(feeder: &mut Feeder, text: &str) -> Replayed {
    let mut meter = crate::combat::dps_calculator::DpsCalculator::new(
        feeder.storage.clone(),
        feeder.skills.clone(),
        feeder.npcs.clone(),
        Arc::new(crate::capture::ping_tracker::PingTracker::new()),
    );
    let mut zone = None;
    let mut kept: Vec<SliceRecord> = Vec::new();
    let mut saved: BTreeMap<String, FightRecord> = BTreeMap::new();
    let mut last_save: Option<i64> = None;
    for (at, offset, stream, bytes) in text.lines().filter_map(capture_line) {
        zone.get_or_insert(offset);
        feeder.feed(stream, at, &bytes);
        kept.push(SliceRecord { captured_at_ms: at, stream: stream.to_string(), bytes });
        if last_save.is_none_or(|t| at >= t + AUTO_SAVE_MS) {
            last_save = Some(at);
            saved.extend(meter.close_boss_fights().into_iter().map(|f| (f.id.clone(), f)));
        }
    }
    let zone = zone.expect("the capture has packets");
    let end = kept.last().map_or(0, |p| p.captured_at_ms);
    crate::clock::set_override(Some(end + 60_000));
    saved.extend(meter.close_boss_fights_now().into_iter().map(|f| (f.id.clone(), f)));
    crate::clock::set_override(None);
    Replayed { zone, kept, saved }
}

/// The windows, in `zone`, of every record in the folder.
fn record_windows(records: Option<&Path>, zone: &chrono::FixedOffset) -> Vec<(i64, i64)> {
    let mut windows = Vec::new();
    let listing = records.and_then(|r| std::fs::read_dir(r).ok());
    for entry in listing.into_iter().flatten().filter_map(|e| e.ok()) {
        let decoded = std::fs::read(entry.path()).ok().and_then(|b| game_record::decode(&b));
        windows.extend(decoded.and_then(|r| r.window_in(zone)));
    }
    windows
}

/// Saves the fights near a record (every fight with `keep_all`) to the data
/// folder `app`, each near one with its slice.
fn save_with_slices(history: &FightHistoryManager, app: &Path, storage: &DataStorage, replayed: &Replayed, windows: &[(i64, i64)], keep_all: bool) {
    for (id, fight) in &replayed.saved {
        let (start, stop) = (fight.start_time_ms, fight.start_time_ms + fight.duration_ms);
        let near = windows.iter().any(|w| overlaps((w.0 - NEAR_MS, w.1 + NEAR_MS), (start, stop)));
        if !near && !keep_all {
            continue;
        }
        history.save_fight(fight).expect("fight saved");
        let head = format!("saved {id} {} {:.1} s, {} damage", fight.boss_name, fight.duration_ms as f64 / 1000.0, fight.total_damage);
        if !near {
            println!("{head}, no slice");
            continue;
        }
        let from = start - fight_tape::SLICE_HEAD_MS - fight_tape::SLICE_LOOKBACK_MS;
        let until = stop + fight_tape::TAIL_MS;
        let around: Vec<SliceRecord> =
            replayed.kept.iter().filter(|p| (from..=until).contains(&p.captured_at_ms)).cloned().collect();
        let slice = fight_tape::build(&around, start, stop, &crate::share::names_from(storage)).expect("slice");
        crate::share::write_slice(app, id, slice, crate::share::uploader_in(fight, storage)).expect("slice written");
        println!("{head}");
    }
}

/// Each record's check: its fights, its totals, its rows, its damage taken.
fn print_checks(checks: &[Arc<RecordCheck>], skills: &SkillLookup) {
    for c in checks {
        println!(
            "\n{}: {} fights {:?}, compared {}, damage game {} meter {}, {} of {} rows differ",
            c.file,
            c.target,
            c.fights,
            c.compared,
            c.game_total,
            c.meter_total,
            c.differing(),
            c.rows.len()
        );
        print_rows(&c.rows, skills);
        println!(
            "damage taken: game {} {:?}, meter {} {:?}",
            c.game_taken.damage, c.game_taken.counts, c.meter_taken.damage, c.meter_taken.counts
        );
    }
}

/// The app's own save path, end to end: replay as the live meter does, save
/// its fights and their slices, then check every record against them.
#[test]
#[ignore]
fn saved_fights_match_the_game() {
    let scratch = crate::built::Scratch::new("saved-fights");
    let keep: Option<PathBuf> = env("A2_KEEP_APP").map(Into::into);
    let records: Option<PathBuf> = match (env("A2_RECORDS"), &keep) {
        (Some(folder), _) => Some(folder.into()),
        (None, Some(_)) => None,
        (None, None) => Some("/tmp/a2-records".into()),
    };
    let capture = env("A2_REPLAY_FILE");
    let capture = capture.as_deref().unwrap_or("/tmp/a2-caps/packets.txt");
    let app = keep.clone().unwrap_or_else(|| scratch.join("app"));
    std::fs::create_dir_all(&app).expect("data folder");
    let records_root = scratch.join("records");
    std::fs::create_dir_all(&records_root).expect("scratch folder");
    if let Some(real) = records.as_ref().and_then(|r| r.canonicalize().ok()) {
        // The checker reads account folders: give it one.
        crate::platform::files::symlink(&real, &records_root.join("account")).expect("records link");
    }

    let text = std::fs::read_to_string(capture).expect("capture readable");
    let mut feeder = Feeder::new();
    let replayed = replay_and_save(&mut feeder, &text);
    let windows = record_windows(records.as_deref(), &replayed.zone);
    let history = FightHistoryManager::new(app.clone());
    save_with_slices(&history, &app, &feeder.storage, &replayed, &windows, keep.is_some());

    let checker = Checker {
        app_data_dir: app.clone(),
        data_dir: Some(data_dir()),
        skills: feeder.skills.clone(),
        npcs: feeder.npcs.clone(),
        fights: history.fight_summaries(),
        roots: vec![records_root],
        zone: Some(replayed.zone),
    };
    let checks = checker.checks();
    let statuses = checker.statuses();
    drop(scratch);
    if keep.is_some() && windows.is_empty() {
        return;
    }
    assert!(!checks.is_empty(), "no record belongs to a saved fight");
    print_checks(&checks, &feeder.skills);
    println!("\nHistory: {statuses:?}");
    for c in &checks {
        assert!(c.compared, "{} was not compared", c.file);
        assert_eq!(c.meter_total, c.game_total, "{}: damage differs", c.file);
    }
}

fn print_rows(rows: &[SkillRow], skills: &SkillLookup) {
    println!("\n{:>9} {:<34} {:>16}  {}", "skill", "name", "damage game/meter",
             COUNTS.map(|c| format!("{c:>9}")).join(""));
    for r in rows {
        let (g, m) = (r.game, r.meter);
        let cell = |a: i64, b: i64| if a == b { format!("{a:>9}") } else { format!("{:>9}", format!("{a}/{b}*")) };
        let counts: String = (0..COUNTS.len()).map(|i| cell(g.counts[i], m.counts[i])).collect();
        println!("{:>9} {:<34.34} {:>16}  {counts}", r.skill_id, skills.skill_title(r.skill_id),
                 if g.damage == m.damage { g.damage.to_string() } else { format!("{}/{}*", g.damage, m.damage) });
    }
    // The hit results over all rows: most are zero on every row.
    let sum = |side: fn(&SkillRow) -> &Row, i: usize| rows.iter().map(|r| side(r).counts[i]).sum::<i64>();
    let results: Vec<String> = (7..COUNTS.len())
        .map(|i| format!("{} {}/{}", COUNTS[i], sum(|r| &r.game, i), sum(|r| &r.meter, i)))
        .collect();
    println!("\nhit results, game/meter: {}", results.join(", "));
    let damage_same = rows.iter().filter(|r| r.game.damage == r.meter.damage).count();
    let same = rows.iter().filter(|r| r.same).count();
    println!("\nrows: {}; damage equal on {damage_same}; every count equal on {same}. * = game/meter differ", rows.len());
}

/// The record's damage taken beside the meter's for `owner` over the
/// record's window, then the meter's by skill.
fn compare_taken(
    record: &game_record::GameRecord,
    taken: &super::replay_taken::Gather,
    window: (i64, i64),
    owner: i32,
    skills: &SkillLookup,
    npcs: &NpcLookup,
) {
    let taken_slack: i64 = env("A2_TAKEN_SLACK_MS").and_then(|v| v.parse().ok()).unwrap_or(0);
    let (by, _) = taken.between(window.0 - taken_slack, window.1 + taken_slack);
    let mut sum = TakenStats::default();
    let mut rows: Vec<_> = by.get(&owner).into_iter().flatten().collect();
    rows.sort_by_key(|(code, d)| (-d.stats.damage, **code));
    for (_, d) in &rows {
        sum.absorb(&d.stats);
    }
    print_taken(record.taken, TakeStat::of_meter(&sum), owner, taken_slack);
    for (code, d) in rows {
        let from = if d.source_code == 0 { "?".to_string() } else { npcs.npc_title(d.source_code) };
        println!("  {code} {} from {from}: {}", skills.skill_title(*code), super::replay_taken::line(&d.stats));
    }
}

/// The record's damage taken beside the meter's, number by number.
fn print_taken(game: TakeStat, meter: TakeStat, owner: i32, slack: i64) {
    println!("\ndamage taken by actor {owner}, the record's window widened by {slack} ms:");
    let head: String = TAKEN_COUNTS.iter().map(|c| format!("{c:>9}")).collect();
    println!("{:>6} {:>9}{head}", "", "damage");
    for (side, t) in [("game", game), ("meter", meter)] {
        let counts: String = t.counts.iter().map(|n| format!("{n:>9}")).collect();
        println!("{side:>6} {:>9}{counts}", t.damage);
    }
    let differ: Vec<&str> = std::iter::once(("damage", game.damage != meter.damage))
        .chain(TAKEN_COUNTS.iter().enumerate().map(|(i, c)| (*c, game.counts[i] != meter.counts[i])))
        .filter(|(_, d)| *d)
        .map(|(c, _)| c)
        .collect();
    if differ.is_empty() {
        println!("damage taken: game and meter agree on every number");
    } else {
        println!("damage taken: game and meter differ on {}", differ.join(", "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hit by a named player: a nameless actor reads as an effect.
    fn hit(storage: &DataStorage, actor: i32, target: i32, damage: i64) {
        storage.name_entity(actor, &format!("Player{actor}"));
        let mut p = crate::combat::data_storage::record(actor, target, 11_020_000, damage, 5_000);
        p.set_hit_kind(2);
        storage.append_damage(p);
    }

    fn record(target: &str, total: i64) -> game_record::GameRecord {
        let bytes = game_record::tests::record_bytes("2026-01-02T12:00:00.500Z", "2026-01-02T12:00:52.400Z", target, &[(1, total, &[1])]);
        game_record::decode(&bytes).expect("a record")
    }

    /// A feeder with empty tables.
    fn feeder() -> Feeder {
        let (skills, npcs) = (Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
        Feeder { storage: Arc::new(DataStorage::new()), skills, npcs, dot_ids: HashSet::new(), streams: HashMap::new() }
    }

    /// (target, owner, damage) as the record check would pick them.
    fn pick(feeder: &Feeder, record: &game_record::GameRecord, local: Option<i32>) -> Option<(i32, i32, i64)> {
        feeder.storage.set_local_player_id(local.map(i64::from));
        let after: HashMap<i32, TargetTotals> = feeder.storage.copy_targets_brief();
        let found = pick_target(record, feeder, &HashMap::new(), &after);
        found.map(|(t, o, rows)| (t, o, rows.values().map(|r| r.damage).sum()))
    }

    #[test]
    fn without_a_local_player_the_nearest_damage_picks_target_and_owner() {
        let feeder = feeder();
        hit(&feeder.storage, 3_001, 900, 400);
        hit(&feeder.storage, 3_002, 900, 1_000);
        hit(&feeder.storage, 3_003, 901, 2_000);
        assert_eq!(pick(&feeder, &record("", 950), None), Some((900, 3_002, 1_000)));
        assert_eq!(pick(&feeder, &record("", 1_900), None), Some((901, 3_003, 2_000)));
    }

    #[test]
    fn a_known_player_without_rows_on_a_target_passes_it_over() {
        let feeder = feeder();
        hit(&feeder.storage, 3_001, 900, 400);
        hit(&feeder.storage, 3_002, 901, 9_000);
        assert_eq!(pick(&feeder, &record("", 9_000), Some(3_001)), Some((900, 3_001, 400)));
        assert_eq!(pick(&feeder, &record("", 9_000), Some(4_444)), None, "no damage in the window");
    }
}
