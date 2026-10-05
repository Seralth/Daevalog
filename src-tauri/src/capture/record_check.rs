//! Sets a game Damage Analyzer record beside a replay of the same fight, skill
//! by skill: the game's numbers are the truth the meter is checked against.
//!
//! ```text
//! A2_RECORD=record_<ticks>.dat     the game's record (required); Aion 2 saves
//!                                  them under .../PersistentDownloadDir/DamageAnalyzer/
//! A2_REPLAY_FILE=packets_x.txt     a capture that covers the fight (required)
//! A2_SLACK_MS=1000                 keep replaying this long after the record ends
//! cargo test --lib record_check -- --ignored --nocapture
//! ```
//!
//! The record is JSON XORed with a 4-byte key. Its times are local time,
//! although they end in "Z", so they line up with the capture's local times.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use super::stream_assembler::StreamAssembler;
use super::stream_processor::StreamProcessor;
use crate::combat::data_storage::{DataStorage, TargetCombatData};
use crate::entity::{skill_group, summon_resolver};
use crate::i18n::lookup::{NpcLookup, SkillLookup};

const KEY: [u8; 4] = [0x25, 0xa8, 0x7e, 0x91];

/// The counts the game keeps per skill, in the order they are printed.
const COUNTS: [&str; 7] = ["hits", "crit", "perfect", "double", "front", "back", "addhit"];
const GAME_FIELDS: [&str; 7] = [
    "TotalCount", "CriticalCount", "PerfectCount", "HardHitCount",
    "FrontAttackCount", "BackAttackCount", "AdditionalHitCount",
];

#[derive(Default, Clone, Copy, PartialEq)]
struct Row {
    damage: i64,
    counts: [i64; 7],
}

struct Record {
    start: String,
    end: String,
    target: String,
    total: i64,
    skills: BTreeMap<i32, Row>,
}

fn number(v: &serde_json::Value) -> i64 {
    v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(0)
}

fn row_of(stat: &serde_json::Value, damage: &serde_json::Value) -> Row {
    let mut row = Row { damage: number(damage), ..Row::default() };
    for (i, f) in GAME_FIELDS.iter().enumerate() {
        row.counts[i] = number(&stat[*f]);
    }
    row
}

fn read_record(path: &str) -> Record {
    let bytes = std::fs::read(path).expect("record readable");
    let plain: Vec<u8> = bytes.iter().enumerate().map(|(i, b)| b ^ KEY[i % 4]).collect();
    let json: serde_json::Value = serde_json::from_slice(&plain).expect("record is JSON");
    // "2026-10-04T04:45:57.967Z" -> "04:45:57.967"
    let tod = |v: &serde_json::Value| v.as_str().unwrap_or("").get(11..23).unwrap_or("").to_string();
    let base = &json["BaseStatData"];
    let mut skills = BTreeMap::new();
    for s in json["SortedSkillStatList"].as_array().into_iter().flatten() {
        skills.insert(number(&s["SkillId"]) as i32, row_of(&s["HitStat"], &s["DamageVal"]));
    }
    Record {
        start: tod(&base["AnalyzerStartTime"]),
        end: tod(&base["AnalyzerEndTime"]),
        target: base["TargetName"].as_str().unwrap_or("").to_string(),
        total: number(&json["AttackStatData"]["TotalDamageVal"]),
        skills,
    }
}

fn add_time(tod: &str, ms: i64) -> String {
    let t = chrono::NaiveTime::parse_from_str(tod, "%H:%M:%S%.3f").expect("record time");
    (t + chrono::Duration::milliseconds(ms)).format("%H:%M:%S%.3f").to_string()
}

/// The meter's rows for one target between two snapshots, per (owner, row skill).
fn rows_between(
    before: Option<&TargetCombatData>,
    after: &TargetCombatData,
    summons: &HashMap<i32, i32>,
    skills: &SkillLookup,
) -> HashMap<(i32, i32), Row> {
    let mut out: HashMap<(i32, i32), Row> = HashMap::new();
    let mut add = |t: &TargetCombatData, sign: i64| {
        for (&actor, a) in &t.actors {
            let owner = summon_resolver::resolve(actor, summons);
            for s in a.skills.values() {
                let r = out.entry((owner, skill_group::row_skill(s.skill_code, skills))).or_default();
                r.damage += sign * s.total_damage as i64;
                let c = [s.hit_count, s.crit_count, s.perfect_count, s.double_count,
                         s.frontal_count, s.back_count, s.multi_hit_count];
                for (i, v) in c.iter().enumerate() {
                    r.counts[i] += sign * *v as i64;
                }
            }
        }
    };
    add(after, 1);
    // A new segment (another first-damage time) starts from nothing.
    if let Some(b) = before.filter(|b| b.first_damage_time == after.first_damage_time) {
        add(b, -1);
    }
    out
}

#[test]
#[ignore]
fn record_check() {
    let env = |n: &str| std::env::var(n).ok().filter(|v| !v.is_empty());
    let (Some(record_path), Some(capture)) = (env("A2_RECORD"), env("A2_REPLAY_FILE")) else {
        eprintln!("set A2_RECORD and A2_REPLAY_FILE");
        return;
    };
    let slack: i64 = env("A2_SLACK_MS").and_then(|v| v.parse().ok()).unwrap_or(1000);
    let record = read_record(&record_path);
    let until = add_time(&record.end, slack);
    println!("record: {} {} .. {}, {} damage, {} skills", record.target, record.start, record.end,
             record.total, record.skills.len());

    let data_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data");
    let skills = Arc::new(SkillLookup::new());
    let npcs = Arc::new(NpcLookup::new());
    crate::i18n::lookup::load_language(&skills, &npcs, &data_dir, "en");
    let dot_ids: HashSet<i32> = std::fs::read_to_string(data_dir.join("dot_skill_ids.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<i32>>(&t).ok())
        .unwrap_or_default()
        .into_iter()
        .collect();

    let storage = Arc::new(DataStorage::new());
    let mut streams: HashMap<String, (StreamAssembler, StreamProcessor)> = HashMap::new();
    let mut before: Option<HashMap<i32, TargetCombatData>> = None;
    for line in std::fs::read_to_string(&capture).expect("capture readable").lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, '|');
        let (Some(ts), Some(key), Some(hex)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let tod = ts.get(11..23).unwrap_or("");
        if tod > until.as_str() {
            break;
        }
        if before.is_none() && tod >= record.start.as_str() {
            before = Some(storage.get_combat_snapshot());
        }
        let Ok(when) = chrono::DateTime::parse_from_rfc3339(ts) else { continue };
        let Some(bytes) = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()).collect::<Option<Vec<u8>>>() else { continue };
        let (assembler, processor) = streams.entry(key.to_string()).or_insert_with(|| {
            let mut p = StreamProcessor::new(storage.clone(), skills.clone(), npcs.clone());
            p.set_dot_skill_ids(dot_ids.clone());
            (StreamAssembler::new(), p)
        });
        processor.set_override_timestamp(Some(when.timestamp_millis()));
        assembler.process_chunk(&bytes, processor);
    }
    let before = before.expect("the capture reaches the record's start");
    let after = storage.get_combat_snapshot();
    let summons = storage.get_summon_data();

    // The fight: the target named as in the record whose damage from one
    // owner in the window comes closest to the record's total.
    let mut best: Option<(i64, i32, i32, HashMap<(i32, i32), Row>)> = None;
    for (&target, t) in &after {
        let rows = rows_between(before.get(&target), t, &summons, &skills);
        let mut by_owner: HashMap<i32, i64> = HashMap::new();
        for (&(owner, _), r) in &rows {
            *by_owner.entry(owner).or_default() += r.damage;
        }
        let named = npcs.get_npc_name(storage.get_mob_data().get(&target).copied().unwrap_or(0)) == record.target;
        for (owner, d) in by_owner {
            let gap = (d - record.total).abs() + if named { 0 } else { record.total };
            if best.as_ref().is_none_or(|b| gap < b.0) {
                best = Some((gap, target, owner, rows.clone()));
            }
        }
    }
    let Some((_, target, owner, rows)) = best else {
        println!("no damage in the window");
        return;
    };
    let meter: BTreeMap<i32, Row> = rows.into_iter().filter(|((o, _), _)| *o == owner).map(|((_, s), r)| (s, r)).collect();
    let total: i64 = meter.values().map(|r| r.damage).sum();
    println!("meter: target {target}, actor {owner} (local player {:?}), {total} damage, {} skills",
             storage.local_player_id(), meter.len());

    println!("\n{:>9} {:<34} {:>16}  {}", "skill", "name", "damage game/meter",
             COUNTS.map(|c| format!("{c:>11}")).join(""));
    let ids: std::collections::BTreeSet<i32> = record.skills.keys().chain(meter.keys()).copied().collect();
    let (mut same, mut damage_same) = (0, 0);
    for id in ids {
        let g = record.skills.get(&id).copied().unwrap_or_default();
        let m = meter.get(&id).copied().unwrap_or_default();
        if g == m {
            same += 1;
        }
        if g.damage == m.damage {
            damage_same += 1;
        }
        let cell = |a: i64, b: i64| if a == b { format!("{a:>11}") } else { format!("{:>11}", format!("{a}/{b}*")) };
        let counts: String = (0..7).map(|i| cell(g.counts[i], m.counts[i])).collect();
        println!("{id:>9} {:<34.34} {:>16}  {counts}", skills.get_skill_name(id),
                 if g.damage == m.damage { g.damage.to_string() } else { format!("{}/{}*", g.damage, m.damage) });
    }
    println!("\nrows: {} game, {} meter; damage equal on {damage_same}; every count equal on {same}. * = game/meter differ",
             record.skills.len(), meter.len());
    println!("total: game {}, meter {total}", record.total);
}
