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

use crate::capture::decoder::PacketDecoder;
use crate::combat::data_storage::{add_tick, DataStorage, SkillTotals, TakenBy, TargetTotals, UNATTRIBUTED_ID};
use crate::entity::taken::TakenStats;
use crate::entity::{skill_group, owner_chain};
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

    /// The meter's damage taken, counted as the game counts it. TotalCount is the hits with a value,
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

/// "2026-01-02T12:00:00.500Z" as the local time it is.
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
    before: Option<&TargetTotals>,
    after: &TargetTotals,
    summons: &HashMap<i32, i32>,
    skills: &SkillLookup,
) -> HashMap<(i32, i32), Row> {
    let mut out: HashMap<(i32, i32), Row> = HashMap::new();
    let mut add = |t: &TargetTotals, sign: i64| {
        for (&actor, a) in &t.actors {
            let owner = owner_chain::resolve(actor, summons);
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
fn meter_counts(s: &SkillTotals) -> [i64; N_COUNTS] {
    [
        s.hit_count, s.crit_count, s.perfect_count, s.double_count,
        s.frontal_count, s.back_count, s.multi_hit_count,
        // Block counts flag 0x02, the meter's Parry.
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
    before: Option<HashMap<i32, TargetTotals>>,
    after: Option<HashMap<i32, TargetTotals>>,
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
            self.after = Some(storage.copy_targets_brief());
            return false;
        }
        if self.before.is_none() && at_ms >= self.from {
            self.before = Some(storage.copy_targets_brief());
        }
        true
    }

    /// The snapshots at the window's start and end. A window that starts
    /// after the last packet is empty.
    pub fn finish(self, storage: &DataStorage) -> (HashMap<i32, TargetTotals>, HashMap<i32, TargetTotals>) {
        let after = self.after.unwrap_or_else(|| storage.copy_targets_brief());
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

/// Plays a fight's slice into a fresh storage and reads off the player's rows
/// on the fight's target over the window, and what the player took over
/// `w.taken`.
pub fn replay_slice_window(
    w: &SliceWindow,
    skills: &Arc<SkillLookup>,
    npcs: &Arc<NpcLookup>,
    dot_ids: &HashSet<i32>,
) -> SliceReplay {
    let mut tap = WindowTap::new(w.from, w.until);
    let storage = Arc::new(DataStorage::new());
    // Each record is a whole packet: no assembler.
    let mut parser = PacketDecoder::new(Arc::clone(&storage), Arc::clone(skills), Arc::clone(npcs));
    parser.use_tick_skills(dot_ids.clone());
    // The links and the player as they stood when the damage window shut.
    let mut at_close: Option<(HashMap<i32, i32>, Option<i64>)> = None;
    let mut taken = TakenBy::new();
    let mut seen_seq = 0u64;
    for (offset, packet) in w.records {
        let at = w.fight_start_ms + i64::from(*offset);
        parser.pin_clock(Some(at));
        if at_close.is_none() && !tap.step(&storage, at) {
            at_close = Some((storage.pet_owners(), storage.local_player_id()));
        }
        if at_close.is_some() && at > w.taken.1 {
            break;
        }
        parser.decode_buffer(packet);
        for tick in storage.taken_since(seen_seq) {
            seen_seq = seen_seq.max(tick.seq);
            if (w.taken.0..=w.taken.1).contains(&tick.hit.at) {
                add_tick(&mut taken, &tick);
            }
        }
    }
    let (before, after) = tap.finish(&storage);
    parser.pin_clock(None);
    let (links, local) = at_close.unwrap_or_else(|| (storage.pet_owners(), storage.local_player_id()));

    let rows = match after.get(&w.target_id) {
        Some(end) => rows_between(before.get(&w.target_id), end, &links, skills),
        None => HashMap::new(),
    };
    let known = w.owner.or(local.map(|id| id as i32));
    let owner = match known {
        Some(id) if rows.keys().any(|(o, _)| *o == id) => Some(id),
        _ => closest_owner(&rows, w.game_total),
    };
    let mut out = SliceReplay::default();
    if let Some(id) = owner {
        out.rows = rows_of(&rows, id);
    }
    if let Some(by_skill) = owner.or(known).and_then(|id| taken.get(&id)) {
        for entry in by_skill.values() {
            out.taken.absorb(&entry.stats);
        }
    }
    out
}

/// One skill row of a record set beside the meter's.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRow {
    pub skill_id: i32,
    pub game: Row,
    pub meter: Row,
    pub same: bool,
}

/// The game's rows and the meter's side by side, one per skill either has
/// (a missing side reads as zero). Biggest damage first, then by skill id.
pub fn compare(game: &BTreeMap<i32, Row>, meter: &BTreeMap<i32, Row>) -> Vec<SkillRow> {
    let mut pairs: BTreeMap<i32, (Row, Row)> = BTreeMap::new();
    for (id, row) in game {
        pairs.entry(*id).or_default().0 = *row;
    }
    for (id, row) in meter {
        pairs.entry(*id).or_default().1 = *row;
    }
    let mut out: Vec<SkillRow> =
        pairs.into_iter().map(|(skill_id, (game, meter))| SkillRow { skill_id, same: game == meter, game, meter }).collect();
    // A stable sort: equal damage stays in id order.
    out.sort_by_key(|r| std::cmp::Reverse(r.game.damage.max(r.meter.damage)));
    out
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
            "2026-01-02T12:00:00.500Z", "2026-01-02T12:00:52.400Z", "Training Scarecrow",
            &[
                (16040000, 20000, &[20, 1, 2, 0, 20, 0, 6]),
                // A row with one Block.
                (16110000, 30000, &[5, 0, 0, 0, 0, 4, 0, 1, 0, 0, 0, 0]),
            ],
        );
        // Encoded, it is not JSON.
        assert!(serde_json::from_slice::<serde_json::Value>(&bytes).is_err());
        let r = decode(&bytes).expect("decodes");
        assert_eq!(r.target, "Training Scarecrow");
        assert_eq!(r.total, 50000);
        let noon = chrono::NaiveDate::from_ymd_opt(2026, 1, 2).and_then(|d| d.and_hms_milli_opt(12, 0, 0, 500));
        assert_eq!(Some(r.start), noon);
        assert_eq!(r.skills[&16040000], row(20000, &[20, 1, 2, 0, 20, 0, 6]));
        assert_eq!(r.skills[&16110000].counts[7], 1);
        // The "Z" is not UTC: the times read as local time in any zone.
        let zone = chrono::FixedOffset::east_opt(9 * 3600).unwrap();
        let (from, until) = r.window_in(&zone).unwrap();
        assert_eq!(until - from, 51_900);
        assert_eq!(from, chrono::DateTime::parse_from_rfc3339("2026-01-02T12:00:00.500+09:00").unwrap().timestamp_millis());
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

    /// Packets on a Melee Training Scarecrow, made-up values: its spawn, two
    /// own records of the player, the player's last direct hit, then
    /// Jointstrike: Corrode ticks of 150 each second. A game record that
    /// starts on a tick 4.5 s after the hit and lasts 2.2 s holds Corrode
    /// only: 450, three ticks.
    #[test]
    fn a_dummy_s_ticks_after_the_last_direct_hit_match_the_game() {
        use crate::built::{self, Hit, F};
        const DUMMY: u32 = 26_601;
        const ME: u32 = 4_600;
        let spawn = built::scarecrow_spawn(DUMMY);
        let own = built::stats(ME, &[(44, 0)]);
        let hit = Hit {
            target: DUMMY,
            switch: 0x16,
            actor: ME,
            skill: 16_010_000,
            uid: 0x04,
            hit_type: 2,
            plotter: Some((0x84, 0, 1)),
            effect: 1_601_000_011,
            count: 1,
            values: vec![10_500, 1_000, 1, 0],
            ..Hit::default()
        }
        .packet();
        let corrode = built::tick(DUMMY, 0x0a, ME, 9_000, 1_615_000_021, 150, &[F::U32(16_150_010)]);
        let ms = |t: &str| {
            let at = chrono::DateTime::parse_from_rfc3339(&format!("2026-01-02T{t}+00:00")).unwrap();
            at.timestamp_millis()
        };
        let start = ms("12:00:00.000");
        let lines = [
            ("12:00:00.000", spawn),
            ("12:01:37.000", own.clone()),
            ("12:01:38.000", own),
            ("12:02:42.400", hit),
            ("12:02:46.900", corrode.clone()),
            ("12:02:47.900", corrode.clone()),
            ("12:02:48.900", corrode.clone()),
            ("12:02:49.900", corrode),
        ];
        let records: Vec<(i32, Vec<u8>)> = lines.iter().map(|(t, bytes)| ((ms(t) - start) as i32, bytes.clone())).collect();
        let bytes = record_bytes("2026-01-02T12:02:47.000Z", "2026-01-02T12:02:49.200Z", "Melee Training Scarecrow", &[(16150000, 450, &[])]);
        let record = decode(&bytes).unwrap();
        let (from, until) = record.window_in(&chrono::Utc).unwrap();
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data");
        let dot_ids: Vec<i32> = serde_json::from_str(&std::fs::read_to_string(data.join("dot_skill_ids.json")).unwrap()).unwrap();
        let w = SliceWindow {
            records: &records,
            fight_start_ms: start,
            target_id: DUMMY as i32,
            from: from - SLACK_MS,
            until: until + SLACK_MS,
            owner: Some(ME as i32),
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
    fn compare_orders_by_the_bigger_side_then_by_id() {
        let only = |d: i64| row(d, &[1]);
        let game: BTreeMap<i32, Row> = [(1, only(100)), (2, only(50))].into();
        let meter: BTreeMap<i32, Row> = [(2, only(70)), (3, only(70))].into();
        let rows = compare(&game, &meter);
        let order: Vec<(i32, bool)> = rows.iter().map(|r| (r.skill_id, r.same)).collect();
        assert_eq!(order, vec![(1, false), (2, false), (3, false)]);
        assert_eq!((rows[0].meter, rows[2].game), (Row::default(), Row::default()));
        assert!(compare(&game, &game).iter().all(|r| r.same));
        assert!(compare(&BTreeMap::new(), &BTreeMap::new()).is_empty());
    }

    #[test]
    fn hit_results_land_in_the_game_s_count_columns() {
        use crate::combat::data_storage::{record, NoDamageHit};
        use crate::entity::mark_kinds::HitMark as M;
        const TARGET: i32 = 500;
        const ACTOR: i32 = 700;
        const SKILL: i32 = 16_040_000;
        let storage = DataStorage::new();
        for marks in [vec![M::Parry], vec![M::ShieldBlock], vec![M::IronWall, M::Regeneration], vec![]] {
            let mut p = record(ACTOR, TARGET, SKILL, 100, 1_000);
            p.set_mark_list(marks);
            p.set_hit_kind(2);
            storage.append_damage(p);
        }
        storage.append_no_damage_hit(TARGET, ACTOR, SKILL, NoDamageHit::Miss);
        storage.append_no_damage_hit(TARGET, ACTOR, SKILL, NoDamageHit::Resist);
        let end = storage.copy_targets_brief();
        let rows = rows_between(None, &end[&TARGET], &HashMap::new(), &SkillLookup::new());
        let counts = rows[&(ACTOR, SKILL)].counts;
        assert_eq!(counts[0], 4);
        assert_eq!(counts[7..], [2, 1, 0, 1, 1]);
    }

    /// The scarecrow's spawn.
    const START: i64 = 1_767_380_400_000;

    /// A dummy (id 26,601) hit by you (4,600) at 1 s, 2 s and 3 s after the
    /// fight's start, 1,000 each.
    fn three_hits() -> Vec<(i32, Vec<u8>)> {
        let you = crate::built::self_record(4_600, "Abcd", 1_202, 30, 30);
        let mut out = vec![(0, crate::built::scarecrow_spawn(26_601)), (0, you)];
        for (n, uid) in [(1, 1u8), (2, 2), (3, 3)] {
            out.push((n * 1_000, crate::built::plain_hit(4_600, 26_601, 16_010_000, 1_000, uid).packet()));
        }
        out
    }

    fn slice_of<'a>(records: &'a [(i32, Vec<u8>)], target: i32, owner: Option<i32>, until: i64) -> SliceWindow<'a> {
        SliceWindow {
            records,
            fight_start_ms: START,
            target_id: target,
            from: START,
            until: START + until,
            owner,
            game_total: 2_000,
            taken: (START, START + until),
        }
    }

    fn play(w: &SliceWindow) -> SliceReplay {
        let (skills, npcs) = (Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
        let out = replay_slice_window(w, &skills, &npcs, &HashSet::new());
        // The replay pinned this thread's clock to the slice's times.
        let now = crate::clock::now_ms();
        assert!(now > START + 3_600_000, "the clock is left pinned at {now}");
        out
    }

    fn damage(rows: &BTreeMap<i32, Row>) -> i64 {
        rows.values().map(|r| r.damage).sum()
    }

    #[test]
    fn a_slice_window_stops_counting_at_its_end() {
        let records = three_hits();
        assert_eq!(damage(&play(&slice_of(&records, 26_601, Some(4_600), 2_500)).rows), 2_000);
        // A window past the last packet takes every hit.
        assert_eq!(damage(&play(&slice_of(&records, 26_601, Some(4_600), 60_000)).rows), 3_000);
    }

    #[test]
    fn a_slice_owner_missing_from_the_rows_falls_back_to_the_closest() {
        let records = three_hits();
        let out = play(&slice_of(&records, 26_601, Some(9_999), 2_500));
        assert_eq!(damage(&out.rows), 2_000);
    }

    #[test]
    fn a_slice_without_its_target_or_an_owner_has_no_rows() {
        let records = three_hits();
        let elsewhere = play(&slice_of(&records, 31_337, Some(4_600), 60_000));
        assert!(elsewhere.rows.is_empty());
        assert_eq!(elsewhere.taken, TakenStats::default());
        let nobody = play(&slice_of(&records, 31_337, None, 60_000));
        assert!(nobody.rows.is_empty());
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
