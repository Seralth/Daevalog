//! The game's own Damage Analyzer records, set beside the meter's numbers.
//!
//! Ctrl+X in game turns the analyzer on, the next hit starts it, and it runs
//! until Ctrl+X is pressed again. The game then writes `record_<ticks>.dat`:
//! JSON XORed with a 4-byte key, holding damage and counts per skill for one
//! target name. Its times are local time, although they end in "Z".
//!
//! This half is parser core and stays wasm-clean: decoding a record, replaying
//! packets over its window, and the comparison. Finding records on disk and
//! matching them to saved fights is in `files` (desktop only).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use chrono::{NaiveDateTime, TimeZone};
use serde::Serialize;

use crate::capture::stream_processor::StreamProcessor;
use crate::combat::data_storage::{DataStorage, TargetCombatData};
use crate::entity::{skill_group, summon_resolver};
use crate::i18n::lookup::{NpcLookup, SkillLookup};

#[cfg(feature = "desktop")]
pub mod files;

const KEY: [u8; 4] = [0x25, 0xa8, 0x7e, 0x91];

/// The counts the game keeps per skill, in the order `Row::counts` holds them.
pub const COUNTS: [&str; 7] = ["hits", "crit", "perfect", "double", "front", "back", "addhit"];
const GAME_FIELDS: [&str; 7] = [
    "TotalCount", "CriticalCount", "PerfectCount", "HardHitCount",
    "FrontAttackCount", "BackAttackCount", "AdditionalHitCount",
];

/// How far to widen a record's window on each side. The game's clock and the
/// capture's differ a little.
pub const SLACK_MS: i64 = 500;

/// One skill's numbers: damage, then the counts in `COUNTS` order.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Row {
    pub damage: i64,
    pub counts: [i64; 7],
}

impl Row {
    fn add(&mut self, other: &Row) {
        self.damage += other.damage;
        for i in 0..7 {
            self.counts[i] += other.counts[i];
        }
    }
}

/// A decoded record.
#[derive(Debug, Clone)]
pub struct GameRecord {
    /// Local time, as the game wrote it.
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub target: String,
    pub total: i64,
    pub skills: BTreeMap<i32, Row>,
}

impl GameRecord {
    /// The window in ms since the epoch, reading the record's times in `tz`.
    pub fn window_in<Tz: TimeZone>(&self, tz: &Tz) -> Option<(i64, i64)> {
        let ms = |t: &NaiveDateTime| tz.from_local_datetime(t).earliest().map(|d| d.timestamp_millis());
        Some((ms(&self.start)?, ms(&self.end)?))
    }
}

/// XOR with the record key. The same call encodes and decodes.
pub fn xor(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().enumerate().map(|(i, b)| b ^ KEY[i % 4]).collect()
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

/// "2026-10-04T04:45:57.967Z" as the local time it is.
fn local_time(v: &serde_json::Value) -> Option<NaiveDateTime> {
    let text = v.as_str()?.trim_end_matches('Z');
    NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f").ok()
}

/// Decode a record file's bytes. `None` when it is not a record, or has no window.
pub fn decode(bytes: &[u8]) -> Option<GameRecord> {
    let json: serde_json::Value = serde_json::from_slice(&xor(bytes)).ok()?;
    let base = &json["BaseStatData"];
    let mut skills = BTreeMap::new();
    for s in json["SortedSkillStatList"].as_array().into_iter().flatten() {
        let row = row_of(&s["HitStat"], &s["DamageVal"]);
        skills.entry(number(&s["SkillId"]) as i32).or_insert_with(Row::default).add(&row);
    }
    Some(GameRecord {
        start: local_time(&base["AnalyzerStartTime"])?,
        end: local_time(&base["AnalyzerEndTime"])?,
        target: base["TargetName"].as_str().unwrap_or("").to_string(),
        total: number(&json["AttackStatData"]["TotalDamageVal"]),
        skills,
    })
}

/// The meter's rows for one target between two snapshots, per (owner, row skill).
pub fn rows_between(
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
                // Ticks over time add damage only: the game, like the details
                // panel's skill row, counts casts as hits.
                if s.is_dot {
                    continue;
                }
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
    out.retain(|_, r| *r != Row::default());
    out
}

/// The owner whose damage comes closest to `total`: who the record is about,
/// when nothing else says.
pub fn closest_owner(rows: &HashMap<(i32, i32), Row>, total: i64) -> Option<i32> {
    let mut by_owner: BTreeMap<i32, i64> = BTreeMap::new();
    for (&(owner, _), r) in rows {
        *by_owner.entry(owner).or_default() += r.damage;
    }
    by_owner.into_iter().min_by_key(|(_, d)| (d - total).abs()).map(|(o, _)| o)
}

/// One owner's rows, keyed by row skill.
pub fn rows_of(rows: &HashMap<(i32, i32), Row>, owner: i32) -> BTreeMap<i32, Row> {
    rows.iter().filter(|((o, _), _)| *o == owner).map(|((_, s), r)| (*s, *r)).collect()
}

/// Add `more` into `into`, row by row.
pub fn add_rows(into: &mut BTreeMap<i32, Row>, more: &BTreeMap<i32, Row>) {
    for (id, r) in more {
        into.entry(*id).or_default().add(r);
    }
}

/// Snapshots of every target at the start and at the end of a window, taken
/// while packets replay in time order.
pub struct WindowTap {
    from: i64,
    until: i64,
    before: Option<HashMap<i32, TargetCombatData>>,
    after: Option<HashMap<i32, TargetCombatData>>,
}

impl WindowTap {
    pub fn new(from: i64, until: i64) -> Self {
        Self { from, until, before: None, after: None }
    }

    /// Call before each packet with its time. False once the window is over:
    /// feed no more packets.
    pub fn step(&mut self, storage: &DataStorage, at_ms: i64) -> bool {
        if self.after.is_some() {
            return false;
        }
        if at_ms > self.until {
            self.after = Some(storage.get_combat_snapshot_light());
            return false;
        }
        if self.before.is_none() && at_ms >= self.from {
            self.before = Some(storage.get_combat_snapshot_light());
        }
        true
    }

    /// The snapshots at the window's start and end. A window that starts
    /// after the last packet is empty.
    pub fn finish(self, storage: &DataStorage) -> (HashMap<i32, TargetCombatData>, HashMap<i32, TargetCombatData>) {
        let after = self.after.unwrap_or_else(|| storage.get_combat_snapshot_light());
        let before = self.before.unwrap_or_else(|| after.clone());
        (before, after)
    }
}

/// What a saved fight's slice says the player did to the fight's target
/// between `from` and `until` (ms since the epoch).
pub struct SliceWindow<'a> {
    /// The slice's records: offsets from `fight_start_ms`, and packets.
    pub records: &'a [(i32, Vec<u8>)],
    pub fight_start_ms: i64,
    pub target_id: i32,
    pub from: i64,
    pub until: i64,
    /// The player's actor id in the fight, when the meter knew it.
    pub owner: Option<i32>,
    /// The record's total, to pick the player when nothing else says.
    pub game_total: i64,
}

pub fn replay_slice_window(
    w: &SliceWindow,
    skills: &Arc<SkillLookup>,
    npcs: &Arc<NpcLookup>,
    dot_ids: &HashSet<i32>,
) -> BTreeMap<i32, Row> {
    let storage = Arc::new(DataStorage::new());
    let mut processor = StreamProcessor::new(storage.clone(), skills.clone(), npcs.clone());
    processor.set_dot_skill_ids(dot_ids.clone());
    let mut tap = WindowTap::new(w.from, w.until);
    for (dt, packet) in w.records {
        let at = w.fight_start_ms + *dt as i64;
        processor.set_override_timestamp(Some(at));
        if !tap.step(&storage, at) {
            break;
        }
        processor.consume_stream(packet);
    }
    let (before, after) = tap.finish(&storage);
    processor.set_override_timestamp(None);
    let Some(target) = after.get(&w.target_id) else { return BTreeMap::new() };
    let rows = rows_between(before.get(&w.target_id), target, &storage.get_summon_data(), skills);
    let owner = w
        .owner
        .or_else(|| storage.local_player_id().map(|v| v as i32))
        .filter(|o| rows.keys().any(|(owner, _)| owner == o))
        .or_else(|| closest_owner(&rows, w.game_total));
    owner.map(|o| rows_of(&rows, o)).unwrap_or_default()
}

/// One skill row, the game's numbers beside the meter's.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRow {
    pub skill_id: i32,
    pub game: Row,
    pub meter: Row,
    pub same: bool,
}

/// Every row either side has, the game's biggest first.
pub fn compare(game: &BTreeMap<i32, Row>, meter: &BTreeMap<i32, Row>) -> Vec<SkillRow> {
    let ids: std::collections::BTreeSet<i32> = game.keys().chain(meter.keys()).copied().collect();
    let mut rows: Vec<SkillRow> = ids
        .into_iter()
        .map(|id| {
            let g = game.get(&id).copied().unwrap_or_default();
            let m = meter.get(&id).copied().unwrap_or_default();
            SkillRow { skill_id: id, game: g, meter: m, same: g == m }
        })
        .collect();
    rows.sort_by(|a, b| b.game.damage.max(b.meter.damage).cmp(&a.game.damage.max(a.meter.damage)).then(a.skill_id.cmp(&b.skill_id)));
    rows
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A record as the game writes one, encoded.
    pub(crate) fn record_bytes(start: &str, end: &str, target: &str, skills: &[(i32, i64, [i64; 7])]) -> Vec<u8> {
        let list: Vec<serde_json::Value> = skills
            .iter()
            .map(|(id, dmg, c)| {
                let mut stat = serde_json::Map::new();
                for (i, f) in GAME_FIELDS.iter().enumerate() {
                    stat.insert(f.to_string(), c[i].into());
                }
                serde_json::json!({ "SkillId": id, "DamageVal": dmg.to_string(), "HitStat": stat })
            })
            .collect();
        let total: i64 = skills.iter().map(|s| s.1).sum();
        let json = serde_json::json!({
            "magic": 1145131858, "version": 1, "RecordedAt": start,
            "BaseStatData": {
                "AnalyzerStartTime": start, "AnalyzerEndTime": end,
                "bExistAnalyzerTime": true, "bExistTargetFilter": true, "TargetName": target,
            },
            "AttackStatData": { "TotalDamageVal": total.to_string() },
            "SortedSkillStatList": list,
        });
        xor(json.to_string().as_bytes())
    }

    #[test]
    fn a_record_decodes() {
        let bytes = record_bytes(
            "2026-10-04T04:45:57.967Z", "2026-10-04T04:46:49.871Z", "Training Scarecrow",
            &[(16040000, 21923, [21, 1, 2, 0, 21, 0, 6]), (16030000, 13422, [12, 1, 2, 0, 12, 0, 5])],
        );
        // Encoded, it is not JSON.
        assert!(serde_json::from_slice::<serde_json::Value>(&bytes).is_err());
        let r = decode(&bytes).expect("decodes");
        assert_eq!(r.target, "Training Scarecrow");
        assert_eq!(r.total, 35345);
        assert_eq!(r.start.to_string(), "2026-10-04 04:45:57.967");
        assert_eq!(r.skills[&16040000], Row { damage: 21923, counts: [21, 1, 2, 0, 21, 0, 6] });
        // The "Z" is not UTC: the times read as local time in any zone.
        let pdt = chrono::FixedOffset::west_opt(7 * 3600).unwrap();
        let (from, until) = r.window_in(&pdt).unwrap();
        assert_eq!(until - from, 51_904);
        assert_eq!(from, chrono::DateTime::parse_from_rfc3339("2026-10-04T04:45:57.967-07:00").unwrap().timestamp_millis());
    }

    #[test]
    fn anything_else_does_not_decode() {
        assert!(decode(b"").is_none());
        assert!(decode(&xor(b"{\"BaseStatData\":{}}")).is_none());
        assert!(decode(b"{\"BaseStatData\":{}}").is_none());
    }

    #[test]
    fn rows_compare_on_every_number() {
        let game: BTreeMap<i32, Row> = [
            (1, Row { damage: 100, counts: [2, 0, 0, 0, 2, 0, 1] }),
            (2, Row { damage: 50, counts: [1, 0, 0, 0, 1, 0, 0] }),
        ].into();
        let mut meter = game.clone();
        meter.get_mut(&2).unwrap().counts[6] = 1;
        meter.insert(3, Row { damage: 5, counts: [1; 7] });
        let rows = compare(&game, &meter);
        assert_eq!(rows.iter().map(|r| r.skill_id).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(rows.iter().map(|r| r.same).collect::<Vec<_>>(), vec![true, false, false]);
        assert_eq!(rows[2].game, Row::default());
    }

    #[test]
    fn the_closest_owner_is_the_player() {
        let rows: HashMap<(i32, i32), Row> = [
            ((7, 1), Row { damage: 900, ..Row::default() }),
            ((7, 2), Row { damage: 100, ..Row::default() }),
            ((8, 1), Row { damage: 400, ..Row::default() }),
        ].into();
        assert_eq!(closest_owner(&rows, 1000), Some(7));
        assert_eq!(closest_owner(&rows, 300), Some(8));
        assert_eq!(rows_of(&rows, 7).len(), 2);
    }
}
