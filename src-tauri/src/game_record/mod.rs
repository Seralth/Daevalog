//! The game's own Damage Analyzer records, set beside the meter's numbers.
//!
//! Ctrl+X in game turns the analyzer on, the next hit starts it, and it runs
//! until Ctrl+X is pressed again. The game then writes `record_<ticks>.dat`:
//! JSON XORed with a 4-byte key, holding damage and counts per skill for one
//! target name. Its times are local time, although they end in "Z".
//!
//! This half is parser core and stays wasm-clean: decoding a record, replaying
//! packets over its window, and the comparison. Finding records on disk and
//! matching them to saved fights is in `files` (backend only).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use chrono::{NaiveDateTime, TimeZone};
use serde::Serialize;

use crate::capture::stream_processor::StreamProcessor;
use crate::combat::data_storage::{add_tick, DataStorage, SkillCombatData, TakenBy, TargetCombatData, UNATTRIBUTED_ID};
use crate::entity::taken::TakenStats;
use crate::entity::{skill_group, summon_resolver};
use crate::i18n::lookup::{NpcLookup, SkillLookup};

#[cfg(feature = "backend")]
pub mod files;

const KEY: [u8; 4] = [0x25, 0xa8, 0x7e, 0x91];

/// How many counts the game keeps per skill.
pub const N_COUNTS: usize = 12;
/// The counts the game keeps per skill, in the order `Row::counts` holds them.
/// The last five are hit results: see `meter_counts` for what each is
/// compared with.
pub const COUNTS: [&str; N_COUNTS] = [
    "hits", "crit", "perfect", "double", "front", "back", "addhit",
    "block", "miss", "immune", "ironwall", "restore",
];
const GAME_FIELDS: [&str; N_COUNTS] = [
    "TotalCount", "CriticalCount", "PerfectCount", "HardHitCount",
    "FrontAttackCount", "BackAttackCount", "AdditionalHitCount",
    "BlockCount", "MissCount", "ImmuneCount", "IronWallCount", "RestorationCount",
];

/// How many counts the game keeps of the damage the player took.
pub const N_TAKEN: usize = 12;
/// The counts of `TakeStatData`, in the order `TakeStat::counts` holds them.
pub const TAKEN_COUNTS: [&str; N_TAKEN] = [
    "total", "accuracy", "crit", "perfect", "double", "front", "back",
    "block", "miss", "immune", "ironwall", "restore",
];
const TAKEN_FIELDS: [&str; N_TAKEN] = [
    "TotalCount", "AccuracyCount", "CriticalCount", "PerfectCount", "HardHitCount",
    "FrontAttackCount", "BackAttackCount", "BlockCount", "MissCount", "ImmuneCount",
    "IronWallCount", "RestorationCount",
];

/// How far to widen a record's window on each side. The game's clock and the
/// capture's differ a little.
pub const SLACK_MS: i64 = 500;

/// One skill's numbers: damage, then the counts in `COUNTS` order.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Row {
    pub damage: i64,
    pub counts: [i64; N_COUNTS],
}

impl Row {
    fn add(&mut self, other: &Row) {
        self.damage += other.damage;
        for i in 0..N_COUNTS {
            self.counts[i] += other.counts[i];
        }
    }
}

/// The damage the player took: damage, then the counts in `TAKEN_COUNTS` order.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TakeStat {
    pub damage: i64,
    pub counts: [i64; N_TAKEN],
}

impl TakeStat {
    fn of(take: &serde_json::Value) -> Self {
        let mut out = TakeStat { damage: number(&take["TotalDamageVal"]), ..TakeStat::default() };
        for (i, f) in TAKEN_FIELDS.iter().enumerate() {
            out.counts[i] = number(&take["HitStat"][*f]);
        }
        out
    }

    /// The meter's damage taken, counted as the game counts it. Checked on
    /// three boss records of 2026-10-06: TotalCount is the hits with a value,
    /// the reflects and the immunes; AccuracyCount the hits with a value;
    /// Block flag 0x02 (Parry), with flag 0x01 (Shield Block) by the game's
    /// names only. Misses, Endurance and Regeneration had no case.
    pub fn of_meter(s: &TakenStats) -> Self {
        let counts = [
            s.total(), s.hits, s.crit, s.perfect, s.double, s.front, s.back,
            s.shield_block.saturating_add(s.parry), s.miss, s.immune, s.iron_wall, s.regeneration,
        ]
        .map(i64::from);
        TakeStat { damage: s.damage, counts }
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
    /// What the player took over the window, from every attacker.
    pub taken: TakeStat,
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
        taken: TakeStat::of(&json["TakeStatData"]),
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
                for (i, v) in meter_counts(s).iter().enumerate() {
                    r.counts[i] += sign * v;
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

/// A skill's counts in `COUNTS` order, as the game would count them.
fn meter_counts(s: &SkillCombatData) -> [i64; N_COUNTS] {
    [
        s.hit_count, s.crit_count, s.perfect_count, s.double_count,
        s.frontal_count, s.back_count, s.multi_hit_count,
        // Block counts flag 0x02, the meter's Parry (a 2026-10-06 record).
        // Flag 0x01, Shield Block, only by the game's names: no record yet.
        s.shield_block_count + s.parry_count,
        // Hit type 1. The game's Immune is a `05 38` effect 0x30 record the
        // meter does not read yet; a Resist (hit type 6) is the skill's effect
        // resisted, not the hit, and the game does not count it.
        s.miss_count,
        0,
        // Flags 0x10 and 0x20, not yet checked.
        s.iron_wall_count,
        s.regeneration_count,
    ]
    .map(i64::from)
}

/// The owner whose damage comes closest to `total`: who the record is about,
/// when nothing else says.
pub fn closest_owner(rows: &HashMap<(i32, i32), Row>, total: i64) -> Option<i32> {
    let mut by_owner: BTreeMap<i32, i64> = BTreeMap::new();
    for (&(owner, _), r) in rows.iter().filter(|((o, _), _)| *o != UNATTRIBUTED_ID) {
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
    /// Where to count the damage the player took: the record's own window,
    /// not widened, or this fight's part of it. The game matches it so.
    pub taken: (i64, i64),
}

/// A slice replayed over a record's window.
#[derive(Debug, Default)]
pub struct SliceReplay {
    /// The player's rows on the fight's target.
    pub rows: BTreeMap<i32, Row>,
    /// What the player took over `SliceWindow::taken`, from every attacker.
    pub taken: TakenStats,
}

pub fn replay_slice_window(
    w: &SliceWindow,
    skills: &Arc<SkillLookup>,
    npcs: &Arc<NpcLookup>,
    dot_ids: &HashSet<i32>,
) -> SliceReplay {
    let storage = Arc::new(DataStorage::new());
    let mut processor = StreamProcessor::new(storage.clone(), skills.clone(), npcs.clone());
    processor.set_dot_skill_ids(dot_ids.clone());
    let mut tap = WindowTap::new(w.from, w.until);
    // Summon links and the local player as they were when the damage
    // window closed: the packets after it are read for damage taken only.
    let mut closed = None;
    let mut seen = 0;
    let mut taken = TakenBy::new();
    for (dt, packet) in w.records {
        let at = w.fight_start_ms + *dt as i64;
        processor.set_override_timestamp(Some(at));
        if closed.is_none() && !tap.step(&storage, at) {
            closed = Some((storage.get_summon_data(), storage.local_player_id()));
        }
        if closed.is_some() && at > w.taken.1 {
            break;
        }
        processor.consume_stream(packet);
        // After each packet, so none is lost to pruning.
        for t in storage.taken_since(seen) {
            seen = t.seq;
            if (w.taken.0..=w.taken.1).contains(&t.hit.at) {
                add_tick(&mut taken, &t);
            }
        }
    }
    let (before, after) = tap.finish(&storage);
    processor.set_override_timestamp(None);
    let (summons, local) = closed.unwrap_or_else(|| (storage.get_summon_data(), storage.local_player_id()));
    let rows = after
        .get(&w.target_id)
        .map(|target| rows_between(before.get(&w.target_id), target, &summons, skills))
        .unwrap_or_default();
    let known = w.owner.or_else(|| local.map(|v| v as i32));
    let owner = known
        .filter(|o| rows.keys().any(|(owner, _)| owner == o))
        .or_else(|| closest_owner(&rows, w.game_total));
    let mut out = SliceReplay { rows: owner.map(|o| rows_of(&rows, o)).unwrap_or_default(), ..SliceReplay::default() };
    // The player who dealt nothing to the target still took what they took.
    if let Some(skills) = owner.or(known).and_then(|o| taken.get(&o)) {
        for d in skills.values() {
            out.taken.absorb(&d.stats);
        }
    }
    out
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

    /// A row with its first counts given, the rest zero.
    pub(crate) fn row(damage: i64, first: &[i64]) -> Row {
        let mut counts = [0; N_COUNTS];
        counts[..first.len()].copy_from_slice(first);
        Row { damage, counts }
    }

    /// A record as the game writes one, encoded. Counts left out are zero.
    pub(crate) fn record_bytes(start: &str, end: &str, target: &str, skills: &[(i32, i64, &[i64])]) -> Vec<u8> {
        let list: Vec<serde_json::Value> = skills
            .iter()
            .map(|(id, dmg, c)| {
                let mut stat = serde_json::Map::new();
                for (i, f) in GAME_FIELDS.iter().enumerate() {
                    stat.insert(f.to_string(), c.get(i).copied().unwrap_or(0).into());
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
            &[
                (16040000, 21923, &[21, 1, 2, 0, 21, 0, 6]),
                // The Kernon of the West record (2026-10-06): one Block.
                (16110000, 29588, &[5, 0, 0, 0, 0, 4, 0, 1, 0, 0, 0, 0]),
            ],
        );
        // Encoded, it is not JSON.
        assert!(serde_json::from_slice::<serde_json::Value>(&bytes).is_err());
        let r = decode(&bytes).expect("decodes");
        assert_eq!(r.target, "Training Scarecrow");
        assert_eq!(r.total, 51511);
        assert_eq!(r.start.to_string(), "2026-10-04 04:45:57.967");
        assert_eq!(r.skills[&16040000], row(21923, &[21, 1, 2, 0, 21, 0, 6]));
        assert_eq!(r.skills[&16110000].counts[7], 1);
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
            (1, row(100, &[2, 0, 0, 0, 2, 0, 1])),
            (2, row(50, &[1, 0, 0, 0, 1, 0, 0])),
        ].into();
        let mut meter = game.clone();
        meter.get_mut(&2).unwrap().counts[7] = 1;
        meter.insert(3, Row { damage: 5, counts: [1; N_COUNTS] });
        let rows = compare(&game, &meter);
        assert_eq!(rows.iter().map(|r| r.skill_id).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(rows.iter().map(|r| r.same).collect::<Vec<_>>(), vec![true, false, false]);
        assert_eq!(rows[2].game, Row::default());
    }

    /// The hit results the game keeps, from the meter's flags and hit types.
    #[test]
    fn hit_results_are_counted_as_the_game_counts_them() {
        use crate::combat::data_storage::NoDamageHit;
        use crate::entity::damage_packet::ParsedDamagePacket;
        use crate::entity::special_damage::SpecialDamage;
        let storage = DataStorage::new();
        let hit = |specials: Vec<SpecialDamage>| {
            let mut p = ParsedDamagePacket::new();
            p.set_timestamp(1_000);
            p.set_target_id(500);
            p.set_actor_id(700);
            p.set_skill_code(16040000);
            p.set_type(2);
            p.set_specials(specials);
            p.set_damage(100);
            storage.append_damage(p);
        };
        hit(vec![SpecialDamage::Parry]);
        hit(vec![SpecialDamage::ShieldBlock]);
        hit(vec![SpecialDamage::IronWall, SpecialDamage::Regeneration]);
        hit(vec![]);
        storage.append_no_damage_hit(500, 700, 16040000, NoDamageHit::Miss);
        storage.append_no_damage_hit(500, 700, 16040000, NoDamageHit::Resist);
        let after = &storage.get_combat_snapshot_light()[&500];
        let rows = rows_between(None, after, &HashMap::new(), &SkillLookup::new());
        let r = rows[&(700, 16040000)];
        assert_eq!(r.counts[0], 4);
        // block, miss, immune, ironwall, restore
        assert_eq!(r.counts[7..], [2, 1, 0, 1, 1]);
    }

    /// Packets of 2026-10-06 on Melee Training Scarecrow 26622: its spawn
    /// (15:45:37.214), two own records of the player 4525, the player's last
    /// direct hit (15:48:19.614), then Jointstrike: Corrode ticks of 148 each
    /// second. The game's Damage Analyzer restarted on a tick; its record of
    /// 15:48:24.192-15:48:26.402 holds Corrode only: 444, three ticks after
    /// the last direct hit.
    #[test]
    fn a_dummy_s_ticks_after_the_last_direct_hit_match_the_game() {
        let spawn = "94014136fecf01042000239f240040026063f1c7fb7dd5c70016c04600d08942003101d3980694a70764000000640000\
            000000000000000000000000000000000000000000640000000100000000000000000000000000000000000000010601110181\
            969800ffffffffffffffff8075d52abb030000fecf0101006063f1c7fb7dd5c70016c04601000a000000a495f91800";
        let own = "174a36ad23012c00000000000000000000000000";
        let hit = "240438fecf011600ad23104bf40003028400014b526d5f01000000c052fe070100";
        let corrode = "180538fecf010aad23b246d5f142609401fa6df600";
        let ms = |t: &str| {
            let at = chrono::DateTime::parse_from_rfc3339(&format!("2026-10-06T{t}-07:00")).unwrap();
            at.timestamp_millis()
        };
        let start = ms("15:45:37.214");
        let lines = [
            ("15:45:37.214", spawn),
            ("15:47:14.211", own),
            ("15:47:15.211", own),
            ("15:48:19.614", hit),
            ("15:48:24.119", corrode),
            ("15:48:25.110", corrode),
            ("15:48:26.114", corrode),
            ("15:48:27.114", corrode),
        ];
        let records: Vec<(i32, Vec<u8>)> = lines
            .iter()
            .map(|(t, hex)| ((ms(t) - start) as i32, (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect()))
            .collect();
        let bytes = record_bytes("2026-10-06T15:48:24.192Z", "2026-10-06T15:48:26.402Z", "Melee Training Scarecrow", &[(16150000, 444, &[])]);
        let record = decode(&bytes).unwrap();
        let (from, until) = record.window_in(&chrono::FixedOffset::west_opt(7 * 3600).unwrap()).unwrap();
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data");
        let dot_ids: Vec<i32> = serde_json::from_str(&std::fs::read_to_string(data.join("dot_skill_ids.json")).unwrap()).unwrap();
        let w = SliceWindow {
            records: &records,
            fight_start_ms: start,
            target_id: 26622,
            from: from - SLACK_MS,
            until: until + SLACK_MS,
            owner: Some(4525),
            game_total: record.total,
            taken: (from, until),
        };
        let (skills, npcs) = (Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
        crate::i18n::lookup::load_language(&skills, &npcs, &data, "en");
        let replay = replay_slice_window(&w, &skills, &npcs, &dot_ids.into_iter().collect());
        let rows = compare(&record.skills, &replay.rows);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].same, "game {:?}, meter {:?}", rows[0].game, rows[0].meter);
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
