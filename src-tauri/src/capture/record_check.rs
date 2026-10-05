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
//! cargo test --lib record_check -- --ignored --nocapture
//! ```
//!
//! `saved_fights_match_the_game` runs the app's own path end to end: it
//! replays the capture as the live meter does, saves fights and their slices
//! to a scratch folder, and checks every record in `A2_RECORDS` against them.
//!
//! ```text
//! A2_RECORDS=/tmp/a2-records       a folder of record_*.dat files
//! A2_REPLAY_FILE=packets_x.txt     a capture that covers them
//! cargo test --lib saved_fights_match_the_game -- --ignored --nocapture
//! ```

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use super::stream_assembler::StreamAssembler;
use super::stream_processor::StreamProcessor;
use crate::combat::data_storage::DataStorage;
use crate::game_record::{self, Row, SkillRow, WindowTap, COUNTS};
use crate::i18n::lookup::{NpcLookup, SkillLookup};

fn env(n: &str) -> Option<String> {
    std::env::var(n).ok().filter(|v| !v.is_empty())
}

fn data_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data")
}

fn lookups() -> (Arc<SkillLookup>, Arc<NpcLookup>, HashSet<i32>) {
    let skills = Arc::new(SkillLookup::new());
    let npcs = Arc::new(NpcLookup::new());
    crate::i18n::lookup::load_language(&skills, &npcs, &data_dir(), "en");
    let dot_ids: HashSet<i32> = std::fs::read_to_string(data_dir().join("dot_skill_ids.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<i32>>(&t).ok())
        .unwrap_or_default()
        .into_iter()
        .collect();
    (skills, npcs, dot_ids)
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

fn print_rows(rows: &[SkillRow], skills: &SkillLookup) {
    println!("\n{:>9} {:<34} {:>16}  {}", "skill", "name", "damage game/meter",
             COUNTS.map(|c| format!("{c:>11}")).join(""));
    for r in rows {
        let (g, m) = (r.game, r.meter);
        let cell = |a: i64, b: i64| if a == b { format!("{a:>11}") } else { format!("{:>11}", format!("{a}/{b}*")) };
        let counts: String = (0..7).map(|i| cell(g.counts[i], m.counts[i])).collect();
        println!("{:>9} {:<34.34} {:>16}  {counts}", r.skill_id, skills.get_skill_name(r.skill_id),
                 if g.damage == m.damage { g.damage.to_string() } else { format!("{}/{}*", g.damage, m.damage) });
    }
    let damage_same = rows.iter().filter(|r| r.game.damage == r.meter.damage).count();
    let same = rows.iter().filter(|r| r.same).count();
    println!("\nrows: {}; damage equal on {damage_same}; every count equal on {same}. * = game/meter differ", rows.len());
}

#[test]
#[ignore]
fn record_check() {
    let (Some(record_path), Some(capture)) = (env("A2_RECORD"), env("A2_REPLAY_FILE")) else {
        eprintln!("set A2_RECORD and A2_REPLAY_FILE");
        return;
    };
    let slack: i64 = env("A2_SLACK_MS").and_then(|v| v.parse().ok()).unwrap_or(game_record::SLACK_MS);
    let record = game_record::decode(&std::fs::read(&record_path).expect("record readable")).expect("a record");
    println!("record: {} {} .. {}, {} damage, {} skills", record.target, record.start, record.end,
             record.total, record.skills.len());

    let (skills, npcs, dot_ids) = lookups();
    let storage = Arc::new(DataStorage::new());
    let mut streams: HashMap<String, (StreamAssembler, StreamProcessor)> = HashMap::new();
    let mut tap: Option<WindowTap> = None;
    for line in std::fs::read_to_string(&capture).expect("capture readable").lines() {
        let Some((when, zone, key, bytes)) = capture_line(line) else { continue };
        // The record's times are local, so read them in the capture's zone.
        let tap = tap.get_or_insert_with(|| {
            let (from, until) = record.window_in(&zone).expect("record window");
            WindowTap::new(from - slack, until + slack)
        });
        if !tap.step(&storage, when) {
            break;
        }
        let (assembler, processor) = streams.entry(key.to_string()).or_insert_with(|| {
            let mut p = StreamProcessor::new(storage.clone(), skills.clone(), npcs.clone());
            p.set_dot_skill_ids(dot_ids.clone());
            (StreamAssembler::new(), p)
        });
        processor.set_override_timestamp(Some(when));
        assembler.process_chunk(&bytes, processor);
    }
    let (before, after) = tap.expect("the capture has packets").finish(&storage);
    let summons = storage.get_summon_data();

    // The fight: the target named as in the record whose damage from one
    // owner in the window comes closest to the record's total.
    let mut best: Option<(i64, i32, i32, BTreeMap<i32, Row>)> = None;
    for (&target, t) in &after {
        let rows = game_record::rows_between(before.get(&target), t, &summons, &skills);
        let named = npcs.get_npc_name(storage.get_mob_data().get(&target).copied().unwrap_or(0)) == record.target;
        let Some(owner) = game_record::closest_owner(&rows, record.total) else { continue };
        let mine = game_record::rows_of(&rows, owner);
        let d: i64 = mine.values().map(|r| r.damage).sum();
        let gap = (d - record.total).abs() + if named { 0 } else { record.total };
        if best.as_ref().is_none_or(|b| gap < b.0) {
            best = Some((gap, target, owner, mine));
        }
    }
    let Some((_, target, owner, meter)) = best else {
        println!("no damage in the window");
        return;
    };
    let total: i64 = meter.values().map(|r| r.damage).sum();
    println!("meter: target {target}, actor {owner} (local player {:?}), {total} damage, {} skills",
             storage.local_player_id(), meter.len());
    print_rows(&game_record::compare(&record.skills, &meter), &skills);
    println!("total: game {}, meter {total}", record.total);
}

#[test]
#[ignore]
fn saved_fights_match_the_game() {
    use crate::capture::evidence_slice::{self, CapturedPacket};
    use crate::combat::dps_calculator::DpsCalculator;
    use crate::combat::ping_tracker::PingTracker;
    use crate::game_record::files::Checker;
    use crate::history::fight_history::FightHistoryManager;

    let records_dir = env("A2_RECORDS").unwrap_or_else(|| "/tmp/a2-records".into());
    let capture = env("A2_REPLAY_FILE").unwrap_or_else(|| "/tmp/a2-caps/packets_20261004_042115.txt".into());
    let scratch = std::env::temp_dir().join(format!("a2t-saved-fights-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let app_dir = scratch.join("app");
    // The checker looks one folder down, where the game keeps one per account.
    std::fs::create_dir_all(scratch.join("records")).unwrap();
    std::os::unix::fs::symlink(std::fs::canonicalize(&records_dir).unwrap(), scratch.join("records/account")).unwrap();

    // Replay as the live meter runs: the auto-save every 30 s, a slice per saved fight.
    let (skills, npcs, dot_ids) = lookups();
    let storage = Arc::new(DataStorage::new());
    let mut calc = DpsCalculator::new(storage.clone(), skills.clone(), npcs.clone(), Arc::new(PingTracker::new()));
    let mut streams: HashMap<String, (StreamAssembler, StreamProcessor)> = HashMap::new();
    let mut packets: Vec<CapturedPacket> = Vec::new();
    let mut saved = BTreeMap::new();
    let mut zone = None;
    let mut next_save = 0;
    for line in std::fs::read_to_string(&capture).expect("capture readable").lines() {
        let Some((when, z, key, bytes)) = capture_line(line) else { continue };
        zone.get_or_insert(z);
        let (assembler, processor) = streams.entry(key.to_string()).or_insert_with(|| {
            let mut p = StreamProcessor::new(storage.clone(), skills.clone(), npcs.clone());
            p.set_dot_skill_ids(dot_ids.clone());
            (StreamAssembler::new(), p)
        });
        processor.set_override_timestamp(Some(when));
        assembler.process_chunk(&bytes, processor);
        packets.push(CapturedPacket { captured_at_ms: when, stream: key.to_string(), bytes });
        if when >= next_save {
            for r in calc.snapshot_boss_fights() {
                saved.insert(r.id.clone(), r);
            }
            next_save = when + 30_000;
        }
    }
    let last = packets.last().map(|p| p.captured_at_ms).unwrap_or(0);
    crate::clock::set_override(Some(last + 60_000));
    for r in calc.snapshot_boss_fights_force() {
        saved.insert(r.id.clone(), r);
    }
    crate::clock::set_override(None);

    // Only the fights near a record: a slice per fight is slow to cut.
    let zone = zone.expect("the capture has packets");
    let windows: Vec<(i64, i64)> = std::fs::read_dir(&records_dir)
        .unwrap()
        .filter_map(|e| std::fs::read(e.ok()?.path()).ok())
        .filter_map(|b| game_record::decode(&b)?.window_in(&zone))
        .collect();
    let history = FightHistoryManager::new(app_dir.clone());
    let names = crate::share::names_from(&storage);
    for r in saved.values() {
        let span = (r.start_time_ms, r.start_time_ms + r.duration_ms);
        if !windows.iter().any(|w| crate::game_record::files::overlaps((w.0 - 60_000, w.1 + 60_000), span)) {
            continue;
        }
        history.save_fight(r).unwrap();
        let from = span.0 - evidence_slice::LEAD_IN_MS - evidence_slice::PRELUDE_MS;
        let window: Vec<CapturedPacket> = packets
            .iter()
            .filter(|p| p.captured_at_ms >= from && p.captured_at_ms <= span.1 + evidence_slice::TAIL_MS)
            .cloned()
            .collect();
        let slice = evidence_slice::build(&window, span.0, span.1, &names).expect("slice");
        crate::share::write_slice(&app_dir, &r.id, slice, crate::share::uploader_in(r, &storage)).unwrap();
        println!("saved {} {} {:.1} s, {} damage", r.id, r.boss_name, r.duration_ms as f64 / 1000.0, r.total_damage);
    }

    let checker = Checker {
        app_data_dir: app_dir.clone(),
        data_dir: Some(data_dir()),
        skills: skills.clone(),
        npcs: npcs.clone(),
        fights: history.list_fights(),
        roots: vec![scratch.join("records")],
        zone: Some(zone),
    };
    let checks = checker.checks();
    let statuses = checker.statuses();
    let _ = std::fs::remove_dir_all(&scratch);
    assert!(!checks.is_empty(), "no record belongs to a saved fight");
    for c in &checks {
        println!("\n{}: {} fights {:?}, compared {}, damage game {} meter {}, {} of {} rows differ",
                 c.file, c.target, c.fights, c.compared, c.game_total, c.meter_total, c.differing(), c.rows.len());
        print_rows(&c.rows, &skills);
    }
    println!("\nHistory: {statuses:?}");
    // Rows can differ where the capture replay above matches: the slice's
    // blinder rewrites short byte runs that equal a name, and some sit inside
    // the skill ids of spirit hits (2026-10-04: d3 86 01 00, skill 100051,
    // came back as 32 63 01 00). Damage is unchanged, so the totals must agree.
    for c in &checks {
        assert!(c.compared, "{} was not compared", c.file);
        assert_eq!(c.meter_total, c.game_total, "{}: damage differs", c.file);
    }
}
