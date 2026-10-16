//! Fights rebuilt from an Evidence Slice. The slice's records go through the
//! meter's own parser and combat totals once more, so whoever holds a slice
//! gets the same saved fight the desktop meter wrote, without trusting the
//! numbers that came with it. Parser core: no files, no clock of its own.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::capture::{fight_tape, ping_tracker::PingTracker, decoder::PacketDecoder};
use crate::combat::{data_storage::DataStorage, data_storage::TargetTotals, dps_calculator::DpsCalculator};
use crate::entity::{detail_bundle::SkillLine, fight_record::FightRecord};
use crate::{i18n::lookup::NpcLookup, i18n::lookup::SkillLookup, version::UPLOAD_COMPAT_VERSION};

/// The boss fight a slice was cut for, as the meter would have saved it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RebuiltFight {
    pub parser_version: String,
    pub record: FightRecord,
    /// The target's total in the storage after the replay.
    pub total_damage: i64,
    /// Token to roster id, in token order.
    pub blind_map: BTreeMap<String, u64>,
    /// Every record the slice holds, also those a teleport left out.
    pub records: usize,
}

/// Why a slice gave no answer. The log service tells the two apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebuildError {
    NotASlice,
    NothingDerived,
}

impl fmt::Display for RebuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RebuildError::NotASlice => "these bytes are not an evidence slice this build can read",
            RebuildError::NothingDerived => "no saved fight could be rebuilt from this slice",
        })
    }
}

/// How far past the slice's last record "now" stands when the fights are
/// taken: far enough that every fight in the slice has ended.
const SETTLE_MS: i64 = 60_000;

/// The lookups and DoT ids every run of the parser over one slice shares.
struct Inputs {
    skills: Arc<SkillLookup>,
    npcs: Arc<NpcLookup>,
    dots: HashSet<i32>,
}

impl Inputs {
    /// No names, no NPC kinds, no DoT skills.
    fn bare() -> Inputs {
        Inputs { skills: Arc::new(SkillLookup::new()), npcs: Arc::new(NpcLookup::new()), dots: HashSet::new() }
    }

    fn load(npcs_json: &str, skills_json: &str, dot_ids_json: &str) -> Inputs {
        let inputs = Inputs::bare();
        inputs.npcs.load_from_json(npcs_json);
        inputs.skills.load_from_json(skills_json);
        Inputs { dots: dot_ids(dot_ids_json), ..inputs }
    }
}

/// The DoT skill ids when the text is a JSON list of i32s, else none (the
/// parser's own start).
fn dot_ids(json: &str) -> HashSet<i32> {
    match serde_json::from_str::<Vec<i32>>(json) {
        Ok(ids) => ids.into_iter().collect(),
        Err(_) => HashSet::new(),
    }
}

/// One pass of the parser over a run of slice records.
struct Replay {
    storage: Arc<DataStorage>,
    parser: PacketDecoder,
    /// Index of the first record after the pull (offset above 0) that made a
    /// zone change clear combat.
    reset_after_pull: Option<usize>,
}

impl Replay {
    /// Feeds `records` in order to a fresh storage. Each record holds whole
    /// frames, so it goes in as one buffer, at its own time.
    fn run(records: &[(i32, Vec<u8>)], inputs: &Inputs) -> Replay {
        let storage = Arc::new(DataStorage::new());
        let mut parser = PacketDecoder::new(Arc::clone(&storage), Arc::clone(&inputs.skills), Arc::clone(&inputs.npcs));
        parser.use_tick_skills(inputs.dots.clone());
        let mut reset_after_pull = None;
        for (index, (offset, bytes)) in records.iter().enumerate() {
            let at = i64::from(*offset);
            let resets_so_far = storage.zone_changed_at();
            parser.pin_clock(Some(at));
            parser.decode_buffer(bytes);
            if at > 0 && reset_after_pull.is_none() && storage.zone_changed_at() != resets_so_far {
                reset_after_pull = Some(index);
            }
        }
        parser.pin_clock(None);
        Replay { storage, parser, reset_after_pull }
    }

    /// Every boss or dummy fight worth a save, with "now" pinned to `now`
    /// while they are taken; this thread's clock is unpinned after.
    fn saved_fights(&mut self, inputs: &Inputs, now: i64) -> Vec<FightRecord> {
        self.parser.pin_clock(Some(now));
        let pings = Arc::new(PingTracker::new());
        let mut calculator =
            DpsCalculator::new(Arc::clone(&self.storage), Arc::clone(&inputs.skills), Arc::clone(&inputs.npcs), pings);
        let fights = calculator.close_boss_fights_now();
        self.parser.pin_clock(None);
        fights
    }
}

/// "Now" while the fights are taken: a while after the slice's last record.
fn settle_time(records: &[(i32, Vec<u8>)]) -> i64 {
    let last = records.last().map_or(0, |(offset, _)| i64::from(*offset));
    last + SETTLE_MS
}

/// Sort key of a candidate fight, best first. The slice's clock starts at the
/// first hit of the fight it was cut for, so the start nearest 0 wins; the
/// slice may also hold a whole earlier boss, or someone else's dummy.
fn fit(start_ms: i64, target_id: i32, totals: &HashMap<i32, i64>) -> (u64, Reverse<i64>, i32) {
    let total = totals.get(&target_id).copied().unwrap_or(0);
    (start_ms.unsigned_abs(), Reverse(total), target_id)
}

/// The saved fight of the boss fight `slice` was cut around. The tables are
/// the NPC table, the skill name table and the DoT skill id list, as JSON.
pub fn fight_from_slice(slice: &[u8], npcs_json: &str, skills_json: &str, dot_ids_json: &str) -> Result<RebuiltFight, RebuildError> {
    let Some((records, blind_map)) = fight_tape::decode(slice) else {
        return Err(RebuildError::NotASlice);
    };
    let inputs = Inputs::load(npcs_json, skills_json, dot_ids_json);

    // A teleport in the tail (after a wipe or a kill) clears combat, and the
    // fight with it: replay again, without it and what follows.
    let mut pass = match Replay::run(&records, &inputs) {
        Replay { reset_after_pull: Some(cut), .. } => Replay::run(&records[..cut], &inputs),
        whole => whole,
    };
    let totals: HashMap<i32, i64> =
        pass.storage.copy_targets_brief().into_iter().map(|(id, t)| (id, t.total_damage)).collect();

    let fights = pass.saved_fights(&inputs, settle_time(&records));
    let Some(mut record) = fights.into_iter().min_by_key(|r| fit(r.start_time_ms, r.target_id, &totals)) else {
        return Err(RebuildError::NothingDerived);
    };
    put_in_order(&mut record);
    let total_damage = totals.get(&record.target_id).copied().unwrap_or(0);
    Ok(RebuiltFight {
        record,
        total_damage,
        records: records.len(),
        blind_map: BTreeMap::from_iter(blind_map),
        parser_version: UPLOAD_COMPAT_VERSION.into(),
    })
}

/// Puts the lists the calculator fills from hash maps in one order, so a
/// slice gives the same bytes on every run and every platform. The lists of
/// structs (`actors`, `skills`, `heal_skills`) sort stably: equal keys keep
/// the calculator's order. Plain numbers need no stable sort.
fn put_in_order(record: &mut FightRecord) {
    record.job_ids.sort_unstable();
    record.jobs.sort();
    record.actors.sort_by_key(|a| a.actor_id);
    let details = &mut record.details;
    for entry in details.skills.iter_mut().chain(details.heal_skills.iter_mut()) {
        entry.hit_timestamps.sort_unstable();
    }
    let key = |e: &SkillLine| (e.actor_id, e.code, e.is_dot);
    details.skills.sort_by_key(key);
    details.heal_skills.sort_by_key(key);
}

/// What a slice holds, with no data tables: every target that took damage.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SliceSummary {
    pub parser_version: String,
    pub dungeon_id: i32,
    pub total_damage: i64,
    /// The longest target's span, not their sum.
    pub duration_ms: i64,
    /// Most damaged first.
    pub targets: Vec<SummaryTarget>,
    pub blind_map: HashMap<String, u64>,
    pub records: usize,
}

/// One target of a summary, its actors most damage first.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SummaryTarget {
    pub target_id: i32,
    /// 0 when the slice holds no spawn for it.
    pub mob_code: i32,
    pub total_damage: i64,
    /// First hit to last.
    pub duration_ms: i64,
    pub actors: Vec<SummaryActor>,
}

/// One actor's damage on a summary target.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SummaryActor {
    pub actor_id: i32,
    /// The blinded name, empty when the slice names no one.
    pub token: String,
    /// The class number, 0 when unknown.
    pub job_id: i32,
    pub damage: i64,
    pub dps: f64,
}

/// One target's row, with every actor that did it damage, most first.
fn summary_row(target_id: i32, target: &TargetTotals, mob_code: i32, names: &HashMap<i32, String>) -> SummaryTarget {
    let span = i64::max(target.last_damage_time - target.first_damage_time, 0);
    // A lone hit spans nothing: it counts as one ms.
    let seconds = f64::max(span as f64 / 1000.0, 0.001);
    let mut actors = Vec::with_capacity(target.actors.len());
    for (&actor_id, row) in &target.actors {
        if row.total_damage <= 0 {
            continue;
        }
        actors.push(SummaryActor {
            dps: row.total_damage as f64 / seconds,
            damage: row.total_damage,
            job_id: row.job.map_or(0, |j| j.job_code_head()),
            token: names.get(&actor_id).cloned().unwrap_or_default(),
            actor_id,
        });
    }
    actors.sort_by_key(|a| (Reverse(a.damage), a.actor_id));
    SummaryTarget { actors, duration_ms: span, total_damage: target.total_damage, mob_code, target_id }
}

/// The rows of the live targets that took damage, most damaged first.
fn summary_rows(
    live: HashMap<i32, TargetTotals>,
    mob_codes: &HashMap<i32, i32>,
    names: &HashMap<i32, String>,
) -> Vec<SummaryTarget> {
    let mut rows = Vec::new();
    for (id, target) in live {
        if target.total_damage > 0 {
            rows.push(summary_row(id, &target, mob_codes.get(&id).copied().unwrap_or(0), names));
        }
    }
    rows.sort_by_key(|r| (Reverse(r.total_damage), r.target_id));
    rows
}

/// Every target in `slice` that took damage, read with no data tables and
/// every record, for a look at what a slice holds.
pub fn derive(slice: &[u8]) -> Result<SliceSummary, RebuildError> {
    let Some((records, blind_map)) = fight_tape::decode(slice) else {
        return Err(RebuildError::NotASlice);
    };
    let pass = Replay::run(&records, &Inputs::bare());
    let storage = &pass.storage;
    let targets = summary_rows(storage.copy_targets_brief(), &storage.enemy_code_map(), &storage.all_names());
    if targets.is_empty() {
        return Err(RebuildError::NothingDerived);
    }
    let mut total_damage = 0;
    let mut longest = 0;
    for row in &targets {
        total_damage += row.total_damage;
        longest = longest.max(row.duration_ms);
    }
    Ok(SliceSummary {
        targets,
        total_damage,
        duration_ms: longest,
        dungeon_id: storage.current_dungeon_id(),
        records: records.len(),
        blind_map,
        parser_version: UPLOAD_COMPAT_VERSION.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::built::{self, F};
    use crate::capture::fight_tape::{SliceRecord, FightTape, NameMap, SliceCounts};
    use crate::capture::opcodes::ZONE_CHANGE;
    use crate::clock;
    use crate::combat::data_storage::ActorTotals;
    use crate::entity::job_class::JobClass;
    use std::collections::VecDeque;

    /// The scarecrow spawn's code; the made-up NPC table below calls it a boss.
    const WARDEN_CODE: i32 = 2_399_011;
    const WARDEN: u32 = 26_700;
    const CASTER: u32 = 1_500;
    const HEALER: u32 = 1_620;

    fn warden_table() -> String {
        format!(r#"{{"{WARDEN_CODE}":{{"name":"Gate Warden","isBoss":true}}}}"#)
    }

    /// A direct hit of `damage` by `actor` on `target`; `uid` keeps hits apart.
    fn hit(actor: u32, target: u32, damage: u32, uid: u8) -> Vec<u8> {
        built::plain_hit(actor, target, 16_010_000, damage, uid).packet()
    }

    /// `23 36 00`: you changed zone.
    fn teleport() -> Vec<u8> {
        built::packet(ZONE_CHANGE, &[F::U8(0)])
    }

    fn file_of(records: Vec<(i32, Vec<u8>)>, tokens: &[&str]) -> Vec<u8> {
        let blind_map = tokens.iter().map(|t| (t.to_string(), 0)).collect();
        fight_tape::encode(&FightTape { records, blind_map, stats: SliceCounts::default() })
    }

    /// The warden spawned in the lead-in and two of the caster's own stats
    /// records (a fight is saved only once the meter knows you), then two
    /// players' hits from 100 to 6,000.
    fn warden_records() -> Vec<(i32, Vec<u8>)> {
        let own = built::stats(CASTER, &[(44, 0)]);
        vec![
            (-5_000, built::scarecrow_spawn(WARDEN)),
            (-4_000, [own.clone(), own].concat()),
            (100, hit(CASTER, WARDEN, 1_200, 1)),
            (2_000, hit(HEALER, WARDEN, 300, 2)),
            (6_000, hit(CASTER, WARDEN, 900, 3)),
        ]
    }

    fn warden_slice() -> Vec<u8> {
        file_of(warden_records(), &["0d", "08", "2a"])
    }

    fn rederived(slice: &[u8]) -> Result<RebuiltFight, RebuildError> {
        fight_from_slice(slice, &warden_table(), "{}", "[]")
    }

    fn json_of(record: &FightRecord) -> serde_json::Value {
        serde_json::to_value(record).unwrap()
    }

    #[test]
    fn bytes_that_are_no_slice_are_refused() {
        let whole = warden_slice();
        // Where the blind map starts: an empty file ends with its u16 count
        // and the u32 record count.
        let map_at = file_of(Vec::new(), &[]).len() - 6;
        // Cut inside the blind map's first token, then inside the last record.
        for bytes in [&b""[..], &b"nope"[..], &whole[..map_at + 4], &whole[..whole.len() - 3]] {
            assert_eq!(derive(bytes).unwrap_err(), RebuildError::NotASlice);
            assert_eq!(rederived(bytes).unwrap_err(), RebuildError::NotASlice);
        }
    }

    #[test]
    fn a_slice_holding_only_a_zone_change_derives_nothing() {
        let packet = SliceRecord { captured_at_ms: 0, stream: "Client:1".to_string(), bytes: vec![0x07, 0x23, 0x36, 0x00] };
        let slice = fight_tape::build(&[packet], 0, 1_000, &NameMap::new()).expect("slice");
        let bytes = fight_tape::encode(&slice);
        assert_eq!(derive(&bytes).unwrap_err(), RebuildError::NothingDerived);
        assert_eq!(rederived(&bytes).unwrap_err(), RebuildError::NothingDerived);
    }

    #[test]
    fn a_slice_with_no_records_settles_a_minute_on_and_derives_nothing() {
        assert_eq!(settle_time(&[]), 60_000);
        assert_eq!(settle_time(&[(-400, vec![]), (15_321, vec![])]), 75_321);
        let empty = file_of(Vec::new(), &[]);
        assert_eq!(rederived(&empty).unwrap_err(), RebuildError::NothingDerived);
        assert_eq!(derive(&empty).unwrap_err(), RebuildError::NothingDerived);
    }

    #[test]
    fn the_dot_list_counts_only_when_it_reads_as_i32s() {
        assert_eq!(dot_ids("[20,21]"), HashSet::from([20, 21]));
        for text in ["", "{}", "[1.5]", "[3000000000]", "20"] {
            assert!(dot_ids(text).is_empty(), "{text:?}");
        }
        // A list that does not read is no error: the derivation goes on.
        let plain = json_of(&rederived(&warden_slice()).unwrap().record);
        for text in ["", "{}", "[1.5]"] {
            let got = fight_from_slice(&warden_slice(), &warden_table(), "{}", text).expect(text);
            assert_eq!(json_of(&got.record), plain, "{text:?}");
        }
    }

    #[test]
    fn the_warden_fight_comes_back_from_its_slice() {
        let fight = rederived(&warden_slice()).expect("a fight");
        assert_eq!(fight.parser_version, UPLOAD_COMPAT_VERSION);
        assert_eq!(fight.records, 5);
        assert_eq!((fight.record.target_id, fight.record.mob_code), (WARDEN as i32, WARDEN_CODE));
        assert_eq!((fight.record.start_time_ms, fight.record.duration_ms), (100, 5_900));
        assert_eq!(fight.total_damage, 2_400);
        assert_eq!(fight.record.details.total_target_damage, 2_400);
        let tokens: Vec<&str> = fight.blind_map.keys().map(String::as_str).collect();
        assert_eq!(tokens, ["08", "0d", "2a"]);

        let text = serde_json::to_string(&fight).unwrap();
        let keys = ["\"parserVersion\":", "\"record\":", "\"totalDamage\":", "\"blindMap\":{\"08\":0,", "\"records\":5}"];
        let at: Vec<usize> = keys.iter().map(|k| text.find(k).expect(k)).collect();
        assert!(at.windows(2).all(|w| w[0] < w[1]), "{text}");
        let back: RebuiltFight = serde_json::from_str(&text).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), text);
    }

    #[test]
    fn without_the_npc_table_no_boss_is_found() {
        let slice = warden_slice();
        assert_eq!(fight_from_slice(&slice, "{}", "{}", "[]").unwrap_err(), RebuildError::NothingDerived);
        assert_eq!(fight_from_slice(&slice, "", "", "").unwrap_err(), RebuildError::NothingDerived);
    }

    #[test]
    fn a_teleport_after_the_pull_ends_the_replay_before_it() {
        let mut records = warden_records();
        records.push((9_000, teleport()));
        let pass = Replay::run(&records, &Inputs::bare());
        assert_eq!(pass.reset_after_pull, Some(5));
        assert_eq!(pass.storage.zone_changed_at(), 9_000);

        let cut = rederived(&file_of(records.clone(), &["0d"])).expect("the fight before the teleport");
        let before = rederived(&file_of(warden_records(), &["0d"])).unwrap();
        assert_eq!(json_of(&cut.record), json_of(&before.record));
        assert_eq!((cut.total_damage, cut.records, before.records), (before.total_damage, 6, 5));
    }

    #[test]
    fn a_teleport_in_the_lead_in_does_not_end_the_replay() {
        // A fight elsewhere, then a teleport, all before the pull.
        let mut records = vec![(-9_000, hit(CASTER, 41_000, 50, 9)), (-6_000, teleport())];
        records.extend(warden_records());
        records.push((12_000, hit(HEALER, WARDEN, 100, 4)));
        let pass = Replay::run(&records, &Inputs::bare());
        assert_eq!(pass.storage.zone_changed_at(), -6_000);
        assert_eq!(pass.reset_after_pull, None);

        let fight = rederived(&file_of(records, &[])).expect("every record used");
        assert_eq!((fight.record.start_time_ms, fight.record.duration_ms), (100, 11_900));
        assert_eq!((fight.total_damage, fight.records), (2_500, 8));
    }

    /// The target whose fight `fit` puts first, of `(start, target)` pairs.
    fn winner(pairs: &[(i64, i32)], totals: &[(i32, i64)]) -> i32 {
        let totals: HashMap<i32, i64> = totals.iter().copied().collect();
        pairs.iter().min_by_key(|(start, id)| fit(*start, *id, &totals)).unwrap().1
    }

    #[test]
    fn the_start_nearest_zero_wins_then_the_total_then_the_id() {
        assert_eq!(winner(&[(-40, 10), (40, 5)], &[(10, 100), (5, 500)]), 5);
        assert_eq!(winner(&[(0, 7), (-1, 3)], &[(7, 1), (3, 1_000_000)]), 7);
        assert_eq!(winner(&[(30, 9), (30, 4)], &[(9, 50), (4, 50)]), 4);
        // Target 8 is not live: it counts 0.
        assert_eq!(winner(&[(12, 8), (12, 2)], &[(2, 1)]), 2);
    }

    #[test]
    fn the_chosen_record_is_put_in_one_order() {
        let mut record = rederived(&warden_slice()).unwrap().record;
        let entry = record.details.skills[0].clone();
        let keyed = |actor_id: i32, code: i32, is_dot: bool, hits: Vec<i64>| {
            let mut e = entry.clone();
            (e.actor_id, e.code, e.is_dot, e.hit_timestamps) = (actor_id, code, is_dot, hits);
            e
        };
        // The two (4, 10, false) entries are told apart by their one hit.
        record.details.skills = vec![
            keyed(5, 200, true, vec![30, 10, 20]),
            keyed(4, 10, false, vec![1]),
            keyed(5, 200, false, vec![]),
            keyed(4, 10, false, vec![2]),
            keyed(3, 900, false, vec![2, 1]),
        ];
        record.details.heal_skills = vec![keyed(8, 1, false, vec![9, 4]), keyed(2, 1, false, vec![])];
        let actor = record.actors[0].clone();
        record.actors = [30, 10, 20]
            .map(|actor_id| {
                let mut a = actor.clone();
                a.actor_id = actor_id;
                a
            })
            .to_vec();
        record.jobs = vec!["b".into(), "a".into()];
        record.job_ids = vec![12, 3, 7];

        put_in_order(&mut record);
        let keys = |list: &[SkillLine]| {
            list.iter().map(|e| (e.actor_id, e.code, e.is_dot, e.hit_timestamps.clone())).collect::<Vec<_>>()
        };
        assert_eq!(
            keys(&record.details.skills),
            [
                (3, 900, false, vec![1, 2]),
                (4, 10, false, vec![1]),
                (4, 10, false, vec![2]),
                (5, 200, false, vec![]),
                (5, 200, true, vec![10, 20, 30])
            ]
        );
        assert_eq!(keys(&record.details.heal_skills), [(2, 1, false, vec![]), (8, 1, false, vec![4, 9])]);
        let ids: Vec<i32> = record.actors.iter().map(|a| a.actor_id).collect();
        assert_eq!(ids, [10, 20, 30]);
        assert_eq!(record.jobs, ["a", "b"]);
        assert_eq!(record.job_ids, [3, 7, 12]);
    }

    #[test]
    fn a_derivation_leaves_the_thread_clock_unpinned() {
        let floor = clock::system_ms();
        let entries: [fn() -> bool; 3] = [
            || rederived(&warden_slice()).is_ok(),
            || rederived(&file_of(Vec::new(), &[])).err() == Some(RebuildError::NothingDerived),
            || derive(&warden_slice()).is_ok(),
        ];
        for (n, entry) in entries.into_iter().enumerate() {
            clock::set_override(Some(5));
            assert!(entry(), "entry {n}");
            assert!(clock::now_ms() >= floor, "entry {n}");
        }

        // A derivation on another thread leaves this thread's pin alone.
        clock::set_override(Some(42));
        let elsewhere = std::thread::spawn(|| rederived(&warden_slice()).is_ok()).join().unwrap();
        assert!(elsewhere);
        assert_eq!(clock::now_ms(), 42);
        clock::set_override(None);
    }

    fn target(id: i32, total: i64, first: i64, last: i64, actors: &[(i32, i64, Option<JobClass>)]) -> TargetTotals {
        let actors = actors
            .iter()
            .map(|&(who, damage, job)| {
                let row = ActorTotals {
                    total_damage: damage,
                    party_heal: 0,
                    regen: 0,
                    first_damage_time: first,
                    last_damage_time: last,
                    job,
                    skills: HashMap::new(),
                    by_second: VecDeque::new(),
                    seq: 0,
                };
                (who, row)
            })
            .collect();
        TargetTotals {
            target_id: id,
            total_damage: total,
            first_damage_time: first,
            last_damage_time: last,
            last_packet_id: -1,
            actors,
            ours: true,
            dungeon_id: 0,
            open_world: false,
            pull: 0,
            enrage_at: None,
        }
    }

    #[test]
    fn summary_rows_list_damaged_targets_and_actors_in_order() {
        let live = HashMap::from([
            (60, target(60, 3_000, 5_000, 5_000, &[(9, 3_000, None)])),
            (50, target(50, 3_000, 1_000, 3_000, &[(9, 1_000, None), (7, 2_000, Some(JobClass::Sorcerer)), (11, 0, None)])),
            (70, target(70, 0, 2_000, 2_500, &[(7, 0, None)])),
        ]);
        let mob_codes = HashMap::from([(50, WARDEN_CODE)]);
        let names = HashMap::from([(7, "1f".to_string())]);
        let rows = summary_rows(live, &mob_codes, &names);

        let ids: Vec<(i32, i32, i64, i64)> = rows.iter().map(|r| (r.target_id, r.mob_code, r.total_damage, r.duration_ms)).collect();
        assert_eq!(ids, [(50, WARDEN_CODE, 3_000, 2_000), (60, 0, 3_000, 0)]);
        let first = &rows[0].actors;
        assert_eq!(first.len(), 2, "no row for the actor with 0");
        assert_eq!(
            first[0],
            SummaryActor { actor_id: 7, token: "1f".into(), job_id: JobClass::Sorcerer.job_code_head(), damage: 2_000, dps: 1_000.0 }
        );
        assert_eq!((first[1].actor_id, first[1].token.as_str(), first[1].job_id, first[1].dps), (9, "", 0, 500.0));
        assert_eq!(rows[1].actors[0].dps, 3_000_000.0);
    }

    #[test]
    fn equal_damage_actors_go_by_id() {
        let live = HashMap::from([(50, target(50, 200, 0, 1_000, &[(31, 100, None), (12, 100, None)]))]);
        let rows = summary_rows(live, &HashMap::new(), &HashMap::new());
        let ids: Vec<i32> = rows[0].actors.iter().map(|a| a.actor_id).collect();
        assert_eq!(ids, [12, 31]);
    }

    #[test]
    fn derive_does_not_stop_at_a_teleport() {
        let mut records = warden_records();
        records.push((9_000, teleport()));
        let slice = file_of(records, &[]);
        // The teleport clears the fight; only `fight_from_slice` leaves it out.
        assert_eq!(derive(&slice).unwrap_err(), RebuildError::NothingDerived);
        assert!(rederived(&slice).is_ok());
    }

    #[test]
    fn derive_lists_the_targets_of_a_slice() {
        let mut records = warden_records();
        records.push((7_000, hit(HEALER, 41_000, 5_000, 5)));
        let summary = derive(&file_of(records, &["0d"])).expect("targets");
        assert_eq!((summary.parser_version.as_str(), summary.records), (UPLOAD_COMPAT_VERSION, 6));
        assert_eq!(summary.blind_map, HashMap::from([("0d".to_string(), 0)]));
        let rows: Vec<(i32, i32, i64, i64)> =
            summary.targets.iter().map(|t| (t.target_id, t.mob_code, t.total_damage, t.duration_ms)).collect();
        assert_eq!(rows, [(41_000, 0, 5_000, 0), (WARDEN as i32, WARDEN_CODE, 2_400, 5_900)]);
        assert_eq!((summary.total_damage, summary.duration_ms), (7_400, 5_900));
        let warden: Vec<(i32, i64)> = summary.targets[1].actors.iter().map(|a| (a.actor_id, a.damage)).collect();
        assert_eq!(warden, [(CASTER as i32, 2_100), (HEALER as i32, 300)]);
    }

    #[test]
    fn errors_print_their_variant_and_a_sentence() {
        assert_eq!(format!("{:?}", RebuildError::NotASlice), "NotASlice");
        assert_eq!(format!("{:?}", RebuildError::NothingDerived), "NothingDerived");
        for e in [RebuildError::NotASlice, RebuildError::NothingDerived] {
            let text = e.to_string();
            assert!(!text.is_empty() && text == text.to_lowercase(), "{text}");
        }
        assert_ne!(RebuildError::NotASlice.to_string(), RebuildError::NothingDerived.to_string());
    }
}
