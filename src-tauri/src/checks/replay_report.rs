//! Replays a `packets_*.txt` capture through the same assembler, parser and
//! storage the live meter uses, and prints what was counted on one target.
//!
//! ```text
//! A2_REPLAY_FILE=packets_x.txt     the capture (required)
//! A2_REPLAY_TARGET=40001           target entity id (optional: else every target)
//! A2_REPLAY_FROM=HH:MM:SS.mmm      window, local time of day from the file
//! A2_REPLAY_TO=HH:MM:SS.mmm
//! A2_REPLAY_HITS=1                 print every change on the target in the window
//! A2_REPLAY_DUMP=0538              print framed packets with this opcode that
//!                                  name the target, in the window
//! A2_REPLAY_RESET_AT=HH:MM:SS      clear combat data there, as the reset button does
//! A2_REPLAY_FLAGS=1                print every hit's raw type, flag and direction
//!                                  bytes, its additional hits and their damage,
//!                                  and the scalar before its value (`hit_flags`
//!                                  lines, capture ms first)
//! A2_REPLAY_TIMELINE=1             print each fight of the local player in the
//!                                  window with its hits, DoT ticks, buffs and
//!                                  stats (see `replay_timeline`); takes the
//!                                  `hit_flags` and `dot_ticks` lines
//! A2_REPLAY_TAKEN=1                print the damage players took in the window,
//!                                  per player and skill (see `replay_taken`)
//! A2_REPLAY_PACKETS=1              print every framed packet (`pkt`) and every
//!                                  bundle opened inside one (`bun`) as hex,
//!                                  capture ms first, in the window
//! A2_REPLAY_CHARACTER=1            print your own character's latest state as
//!                                  JSON instead (see `replay_character`; needs
//!                                  `backend`): A2_REPLAY_FILE may name several
//!                                  logs joined by ':', oldest first, and
//!                                  A2_GAME_DATA=dir the game data folder for
//!                                  names (without it the ids stay)
//! A2_REPLAY_TICK_MS=100           read the meter that often instead of every 500 ms
//! A2_REPLAY_MODE=encounter         run the meter in this mode (a mode id, as
//!                                  `TargetScope::from_id` takes), read
//!                                  every 500 ms as the live meter does, and
//!                                  print its rows (`meter` lines: id and
//!                                  damage, then id and damage taken) each
//!                                  time they change in the window
//!
//! The report always ends with the deaths of you and your party in the
//! window and in each fight the meter would save (see `replay_deaths`).
//! cargo test --lib replay_report -- --ignored --nocapture
//! ```

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::capture::framing;
use crate::capture::byte_queue::ByteQueue;
use crate::capture::packet_log::{bytes_from_hex, LogLine, PacketLogReplay, RepeatFilter};
use crate::capture::ping_tracker::PingTracker;
use crate::capture::varint::varint_at;
use crate::combat::data_storage::{DataStorage, TargetTotals};
use crate::combat::dps_calculator::DpsCalculator;
use crate::entity::owner_chain;
use crate::i18n::lookup::{NpcLookup, SkillLookup};

use super::replay_timeline;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}


/// What to report, from the environment variables above.
#[derive(Default)]
pub(crate) struct Options {
    pub target: Option<i32>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub show_hits: bool,
    pub reset_at: Option<String>,
    pub dump_op: Option<[u8; 2]>,
    pub timeline: bool,
    pub taken: bool,
    pub packets: bool,
    pub mode: Option<String>,
}

/// How often the live meter reads its rows.
const METER_TICK_MS: i64 = 500;

/// The meter's rows as one line: id and damage, most first, then id and
/// damage taken as the meter orders them.
fn meter_rows(dps: &crate::entity::dps_data::DpsData) -> String {
    let mut rows: Vec<(i32, i64)> = dps.map.iter().map(|(&id, d)| (id, d.amount as i64)).collect();
    rows.sort_by_key(|&(id, amount)| (-amount, id));
    let rows: Vec<String> = rows.iter().map(|(id, amount)| format!("{id} {amount}")).collect();
    let taken: Vec<String> = dps.taken.iter().map(|r| format!("{} {}", r.actor_id, r.stats.damage)).collect();
    format!("{}: {} | taken: {}", dps.target_mode, rows.join(", "), taken.join(", "))
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 64-bit FNV-style fold of a list's `Debug` text, to tell two lists apart
/// in one short word.
fn digest(list: &impl std::fmt::Debug) -> String {
    let text = format!("{list:?}");
    let h = text.bytes().fold(0u64, |h, b| h.wrapping_mul(1_099_511_628_211) ^ u64::from(b));
    format!("{h:016x}")
}

/// The ids the storage knows at the end, each list sorted, as one line.
fn state_line(storage: &DataStorage) -> String {
    let mut links: Vec<(i32, i32)> = storage.pet_owners().into_iter().collect();
    links.sort();
    let mut players: Vec<i32> = storage.known_player_set().into_iter().collect();
    players.sort();
    let mut named: Vec<i32> = storage.all_names().into_keys().collect();
    named.sort();
    format!(
        "state: {} summon links ({}), {} players ({}), {} named ({}), {} mobs",
        links.len(),
        digest(&links),
        players.len(),
        digest(&players),
        named.len(),
        digest(&named),
        storage.enemy_code_map().len()
    )
}

fn skill_label(skills: &SkillLookup, skill: i32, dot: bool) -> String {
    let name = skills.skill_title(skill);
    if dot { format!("{skill} {name} (DoT)") } else { format!("{skill} {name}") }
}

/// The packets one line adds to its stream, for the dump, the packet list
/// and the timeline. Kept apart from the replay's own framing.
#[derive(Default)]
struct Packets {
    streams: HashMap<String, ByteQueue>,
    repeats: RepeatFilter,
}

impl Packets {
    /// `None` when the line is a repeat, or nothing wants its packets.
    fn of_line(&mut self, line: &LogLine, wanted: bool) -> Option<Vec<Vec<u8>>> {
        if self.repeats.is_repeat(line) || !wanted {
            return None;
        }
        let held = self.streams.entry(line.stream.to_string()).or_default();
        Some(framing::take_packets(held, &line.bytes))
    }
}

/// The window as it opened and as it stands.
#[derive(Default)]
struct Window {
    open: bool,
    first_tod: String,
    last_tod: String,
    first_ms: i64,
    /// Every target as it stood after the window's last line.
    before: HashMap<i32, TargetTotals>,
    /// Per target: (raw actor, skill, is_dot) -> (hits, damage).
    sums: BTreeMap<i32, Sums>,
}

type Sums = BTreeMap<(i32, i32, bool), (i64, i64)>;

/// One change on a target: (target, (raw actor, skill, is_dot), (hits, damage)).
type Change = (i32, (i32, i32, bool), (i64, i64));

impl Window {
    fn open_at(&mut self, tod: &str, ms: i64, storage: &DataStorage) {
        self.open = true;
        self.first_tod = tod.to_string();
        self.first_ms = ms;
        self.before = storage.copy_targets_brief();
    }

    /// What changed on each target (only `target`, when given) since the
    /// window's last line, by target, then actor and skill. Adds it to the
    /// sums and keeps `now` for the next line.
    fn add_window_changes(&mut self, now: HashMap<i32, TargetTotals>, target: Option<i32>) -> Vec<Change> {
        let mut targets: Vec<i32> = match target {
            Some(t) => vec![t],
            None => now.keys().copied().collect(),
        };
        targets.sort();
        let mut out = Vec::new();
        for target in targets {
            let changes = now.get(&target).map(|t| t.row_changes(self.before.get(&target))).unwrap_or_default();
            for (key, (hits, damage)) in changes {
                let sum = self.sums.entry(target).or_default().entry(key).or_default();
                sum.0 += hits;
                sum.1 += damage;
                out.push((target, key, (hits, damage)));
            }
        }
        self.before = now;
        out
    }
}

/// A line's time of day and capture ms, and whether the window takes it.
#[derive(Clone, Copy)]
struct At<'a> {
    tod: &'a str,
    ms: i64,
    in_window: bool,
}

/// The meter in `Options::mode`, read every `tick` ms of capture time.
struct Meter {
    calc: DpsCalculator,
    tick: i64,
    next_read: Option<i64>,
    /// The rows last read.
    shown: Option<String>,
}

impl Meter {
    fn new(mode: &str, storage: &Arc<DataStorage>, skills: &Arc<SkillLookup>, npcs: &Arc<NpcLookup>) -> Meter {
        let mut calc = DpsCalculator::new(storage.clone(), skills.clone(), npcs.clone(), Arc::new(PingTracker::new()));
        calc.set_target_selection_mode(mode);
        let tick = env("A2_REPLAY_TICK_MS").and_then(|v| v.parse::<i64>().ok()).unwrap_or(METER_TICK_MS);
        Meter { calc, tick, next_read: None, shown: None }
    }

    /// Reads the rows when a read is due, and prints them in the window
    /// when they changed.
    fn read(&mut self, at: At, out: &mut dyn FnMut(String)) {
        if self.next_read.is_some_and(|due| at.ms < due) {
            return;
        }
        self.next_read = Some(at.ms + self.tick);
        let rows = meter_rows(&self.calc.get_dps());
        if at.in_window && self.shown.as_ref() != Some(&rows) {
            out(format!("meter {} {rows}", at.tod));
        }
        self.shown = Some(rows);
    }
}

/// Hands one line's packets to the timeline, and prints
/// the packet list and the opcode dump in the window.
fn scan_line_packets(
    list: &[Vec<u8>],
    at: At,
    options: &Options,
    mut timeline: Option<&mut replay_timeline::Gather>,
    out: &mut dyn FnMut(String),
) {
    let ms = at.ms;
    for p in list {
        if options.packets && at.in_window {
            out(format!("pkt {ms} {}", to_hex(p)));
            for inner in framing::embedded_bundles(p) {
                out(format!("bun {ms} {}", to_hex(&inner.data)));
            }
        }
        if let Some(g) = timeline.as_mut() {
            g.timeline.note(ms, p);
        }
        if let (Some(op), true) = (options.dump_op, at.in_window) {
            let head = varint_at(p, 0).length.max(0) as usize;
            let names_target = options.target.is_none_or(|t| varint_at(p, head + 2).value == t);
            if p.get(head..head + 2) == Some(&op[..]) && names_target {
                out(format!("dump {} {}", at.tod, to_hex(p)));
            }
        }
    }
}

/// One target's damage in the window, by raw actor, by owner, then by the
/// owner's skill, most first. Nothing when it took no damage.
fn target_summary(target: i32, sums: &Sums, links: &HashMap<i32, i32>, skills: &SkillLookup, out: &mut dyn FnMut(String)) {
    let total: i64 = sums.values().map(|s| s.1).sum();
    if total == 0 {
        return;
    }
    out(format!("\n== target {target}: {total} in the window"));
    // raw actor -> (owner, direct, dot, records)
    let mut actors: BTreeMap<i32, (i32, i64, i64, i64)> = BTreeMap::new();
    let mut owners: BTreeMap<i32, i64> = BTreeMap::new();
    let mut by_skill = Sums::new();
    for (&(actor, skill, dot), &(hits, damage)) in sums {
        let owner = owner_chain::resolve(actor, links);
        let a = actors.entry(actor).or_insert((owner, 0, 0, 0));
        if dot { a.2 += damage } else { a.1 += damage }
        a.3 += hits;
        *owners.entry(owner).or_default() += damage;
        let s = by_skill.entry((owner, skill, dot)).or_default();
        s.0 += hits;
        s.1 += damage;
    }
    let actor_lines = actors.iter().map(|(actor, (owner, direct, dot, records))| {
        format!("actor {actor} owner {owner}: direct {direct} dot {dot} ({records} records)")
    });
    let owner_lines = owners.iter().map(|(owner, damage)| format!("owner {owner}: {damage}"));
    actor_lines.chain(owner_lines).for_each(&mut *out);
    // Most damage first. The map gave them in key order, and a stable
    // sort keeps that order among equals.
    let mut skill_lines: Vec<(i64, String)> = by_skill
        .into_iter()
        .map(|((owner, skill, dot), (hits, damage))| {
            (damage, format!("skill owner {owner} {}: {hits} hits {damage}", skill_label(skills, skill, dot)))
        })
        .collect();
    skill_lines.sort_by_key(|line| std::cmp::Reverse(line.0));
    skill_lines.into_iter().for_each(|(_, line)| out(line));
}

/// The meter's own segment on `target`, as it reads at the capture's end.
fn meter_segment(
    target: i32,
    last_ms: i64,
    storage: &Arc<DataStorage>,
    skills: &Arc<SkillLookup>,
    npcs: &Arc<NpcLookup>,
    out: &mut dyn FnMut(String),
) {
    crate::clock::set_override(Some(last_ms));
    let calc = DpsCalculator::new(storage.clone(), skills.clone(), npcs.clone(), Arc::new(PingTracker::new()));
    let details = calc.target_breakdown(target, None);
    out(format!(
        "\n== meter segment for target {target}: {} over {} ms",
        details.total_target_damage, details.battle_time
    ));
    let mut rows: Vec<_> = details.skills.iter().collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.dmg));
    for r in rows {
        let dot = if r.is_dot { " (DoT)" } else { "" };
        out(format!("meter actor {} {} {}{dot}: {} hits {}", r.actor_id, r.code, r.name, r.time, r.dmg));
    }
    crate::clock::set_override(None);
}

/// Replays a packet log's text and hands each report line to `out`, as the
/// options at the top of this file ask.
pub(crate) fn run(text: &str, options: Options, out: &mut dyn FnMut(String)) {
    let (skills, npcs, dot_ids) = super::lookups();
    let storage = Arc::new(DataStorage::new());
    let mut replay = PacketLogReplay::new(Arc::clone(&storage), Arc::clone(&skills), Arc::clone(&npcs), dot_ids);
    let mut meter = options.mode.as_deref().map(|mode| Meter::new(mode, &storage, &skills, &npcs));
    let mut timeline = options.timeline.then(|| replay_timeline::Gather::new(&super::data_dir()));
    let _tap = timeline.as_ref().map(|g| g.tap.install());
    let mut taken = options.taken.then(super::replay_taken::Gather::default);
    let mut deaths = super::replay_deaths::Gather::default();

    let mut packets = Packets::default();
    let mut window = Window::default();
    let mut reset_at = options.reset_at.clone();
    let mut zone = None;
    let (mut lines, mut clean) = (0usize, 0usize);
    let mut last_ms = 0i64;

    for line in text.lines().filter_map(LogLine::parse) {
        if zone.is_none() {
            zone = line.offset();
        }
        let tod = line.time_of_day();
        let ms = line.at_ms;
        if let Some(to) = options.to.as_deref() {
            let head = tod.get(..to.len()).unwrap_or(tod);
            if head > to {
                break;
            }
        }
        if reset_at.as_deref().is_some_and(|at| tod >= at) {
            out(format!("reset at {tod}"));
            storage.flush();
            reset_at = None;
        }
        let in_window = options.from.as_deref().is_none_or(|from| tod >= from);
        if in_window && !window.open {
            window.open_at(tod, ms, &storage);
            if let Some(m) = meter.as_mut() {
                m.shown = None;
            }
        }
        let at = At { tod, ms, in_window };

        lines += 1;
        if replay.feed(&line) {
            clean += 1;
        }
        last_ms = ms;

        let wanted = (in_window && (options.dump_op.is_some() || options.packets)) || options.timeline;
        if let Some(list) = packets.of_line(&line, wanted) {
            scan_line_packets(&list, at, &options, timeline.as_mut(), out);
        }
        if let Some(t) = taken.as_mut() {
            t.after_line(&storage);
        }
        deaths.after_line(&storage);
        if let Some(m) = meter.as_mut() {
            m.read(at, out);
        }
        if let Some(g) = timeline.as_mut() {
            let local = storage.local_player_id().map(|id| id as i32);
            g.after_line(ms, local, |id| storage.enemy_code(id), || storage.pet_owners());
        }

        if window.open {
            window.last_tod = tod.to_string();
            let now = storage.copy_targets_brief();
            deaths.note_fights(now.values());
            for (target, (actor, skill, dot), (hits, damage)) in window.add_window_changes(now, options.target) {
                if options.show_hits {
                    let label = skill_label(&skills, skill, dot);
                    out(format!("hit {tod} target {target} actor {actor} skill {label} x{hits} {damage}"));
                }
            }
        }
    }

    let local = storage.local_player_id().map(|id| id as i32);
    out(format!("payloads {lines}, {clean} left nothing buffered"));
    out(format!("window {} .. {}", window.first_tod, window.last_tod));
    out(format!("local player {local:?}"));
    out(state_line(&storage));
    let links = storage.pet_owners();
    for (target, sums) in &window.sums {
        target_summary(*target, sums, &links, &skills, out);
    }
    if let Some(target) = options.target {
        meter_segment(target, last_ms, &storage, &skills, &npcs, out);
    }
    let span = (window.first_ms, last_ms);
    if let Some(t) = &taken {
        t.report(span, local, &skills, &npcs, out);
    }
    deaths.report(span, &storage, &skills, &npcs, out);
    if let Some(g) = timeline {
        let zone = zone.unwrap_or_else(|| chrono::FixedOffset::east_opt(0).expect("UTC"));
        let from = options.from.as_ref().map(|_| window.first_ms);
        let mobs = storage.enemy_code_map();
        g.report(zone, last_ms, storage.pet_owners(), (from, last_ms), options.target, &mobs, &skills, &npcs, out);
    }
}

#[test]
#[ignore]
fn replay_report() {
    let Some(path) = env("A2_REPLAY_FILE") else {
        eprintln!("set A2_REPLAY_FILE");
        return;
    };
    #[cfg(feature = "backend")]
    if env("A2_REPLAY_CHARACTER").is_some() {
        println!("{}", super::replay_character::report(&path));
        return;
    }
    let options = Options {
        target: env("A2_REPLAY_TARGET").and_then(|v| v.parse().ok()),
        from: env("A2_REPLAY_FROM"),
        to: env("A2_REPLAY_TO"),
        show_hits: env("A2_REPLAY_HITS").is_some(),
        reset_at: env("A2_REPLAY_RESET_AT"),
        dump_op: env("A2_REPLAY_DUMP").and_then(|h| bytes_from_hex(&h)).and_then(|v| v.try_into().ok()),
        timeline: env("A2_REPLAY_TIMELINE").is_some(),
        taken: env("A2_REPLAY_TAKEN").is_some(),
        packets: env("A2_REPLAY_PACKETS").is_some(),
        mode: env("A2_REPLAY_MODE"),
    };
    let _flags = env("A2_REPLAY_FLAGS").map(|_| {
        crate::trace_tap::set_default(
            tracing_subscriber::fmt()
                .with_env_filter("hit_flags=trace")
                .without_time()
                .with_target(false)
                .with_level(false)
                .with_writer(std::io::stdout)
                .finish(),
        )
    });
    let text = std::fs::read_to_string(&path).expect("capture readable");
    run(&text, options, &mut |line| println!("{line}"));
}

/// A packet carrying a compressed bundle prints as itself, then the bundle's
/// contents opened.
#[test]
fn packets_prints_each_packet_and_the_bundles_inside() {
    let frame = |body: &[u8]| {
        let mut out = vec![framing::length_value(body.len()) as u8];
        out.extend(body);
        out
    };
    let inner = frame(&[0x45, 0x36, 0x05, 0x00]);
    let mut bundle = vec![0xff, 0xff];
    bundle.extend((inner.len() as u32).to_le_bytes());
    bundle.extend(lz4_flex::compress(&inner));
    let mut body = vec![0x12, 0x34];
    body.extend(frame(&bundle));
    let outer = frame(&body);
    let text = format!("2026-01-02T12:00:00.000+00:00|Client:50000:13328|{}", to_hex(&outer));
    let at = chrono::DateTime::parse_from_rfc3339("2026-01-02T12:00:00+00:00").unwrap().timestamp_millis();

    let mut lines = Vec::new();
    run(&text, Options { packets: true, ..Default::default() }, &mut |l| lines.push(l));
    let got: Vec<&String> = lines.iter().filter(|l| l.starts_with("pkt ") || l.starts_with("bun ")).collect();
    assert_eq!(got, [&format!("pkt {at} {}", to_hex(&outer)), &format!("bun {at} {}", to_hex(&inner))]);

    let mut lines = Vec::new();
    run(&text, Options::default(), &mut |l| lines.push(l));
    assert!(!lines.iter().any(|l| l.starts_with("pkt ") || l.starts_with("bun ")));
}

/// A Cold Shock with three additional hits. One class skill and no name: an
/// effect, on the unattributed row.
fn cold_shock() -> String {
    let hit = crate::built::Hit {
        target: 19_601,
        switch: 0x36,
        actor: 1_425,
        skill: 16_010_030,
        uid: 0x50,
        hit_type: 2,
        plotter: Some((0x84, 0, 2)),
        effect: 1_601_003_011,
        count: 1,
        values: vec![12_000, 1_200, 3, 100, 100, 100, 1, 0],
        ..Default::default()
    };
    crate::built::to_hex(&hit.packet())
}

/// A payload the server sent again, logged twice, counts once, and the
/// packet list shows it once.
#[test]
fn a_resent_payload_counts_once() {
    let hit = cold_shock();
    let text = format!(
        "2026-01-02T12:00:35.098+00:00|Client:50000:13328|{hit}\n2026-01-02T12:00:35.540+00:00|Client:50000:13328|{hit}"
    );
    let row = crate::combat::data_storage::UNATTRIBUTED_ID;
    let mut lines = Vec::new();
    run(&text, Options { mode: Some("allTargets".into()), packets: true, ..Default::default() }, &mut |l| lines.push(l));
    let meter: Vec<&String> = lines.iter().filter(|l| l.starts_with("meter ")).collect();
    assert_eq!(meter, [&format!("meter 12:00:35.098+00:00 allTargets: {row} 1200 | taken: ")]);
    assert_eq!(lines.iter().filter(|l| l.starts_with("pkt ")).count(), 1);
}

/// With a mode, the meter's rows as the live meter reads them, when the
/// window opens and each time they change.
#[test]
fn mode_prints_the_meter_rows_when_they_change() {
    // The Cold Shock twice, over 5 s apart: closer, the replay takes the
    // copy for a resend.
    let hit = cold_shock();
    let text = format!(
        "2026-01-02T12:00:35.098+00:00|Client:50000:13328|{hit}\n2026-01-02T12:00:41.000+00:00|Client:50000:13328|{hit}"
    );
    let row = crate::combat::data_storage::UNATTRIBUTED_ID;

    let mut lines = Vec::new();
    run(&text, Options { mode: Some("allTargets".into()), ..Default::default() }, &mut |l| lines.push(l));
    let got: Vec<&String> = lines.iter().filter(|l| l.starts_with("meter ")).collect();
    assert_eq!(got, [
        &format!("meter 12:00:35.098+00:00 allTargets: {row} 1200 | taken: "),
        &format!("meter 12:00:41.000+00:00 allTargets: {row} 2400 | taken: "),
    ]);

    let mut lines = Vec::new();
    run(&text, Options::default(), &mut |l| lines.push(l));
    assert!(!lines.iter().any(|l| l.starts_with("meter ")));
}

#[test]
fn the_state_digest_folds_the_list_text() {
    assert_eq!(digest(&Vec::<i32>::new()), "00005b0000009afc");
    assert_eq!(digest(&vec![1, 2]), "35b4898c6fe5e9b7");
    assert_eq!(digest(&vec![(5, 7), (9, 7)]), "0a12d0f2397d2bdc");
    let empty = DataStorage::new();
    let none = "00005b0000009afc";
    assert_eq!(state_line(&empty), format!("state: 0 summon links ({none}), 0 players ({none}), 0 named ({none}), 0 mobs"));
}

/// The Cold Shock at three times, 6 s apart so none is taken for a resend.
fn three_shocks() -> String {
    let hit = cold_shock();
    ["35.098", "41.000", "47.000"]
        .map(|s| format!("2026-01-02T12:00:{s}+00:00|Client:50000:13328|{hit}"))
        .join("\n")
}

fn report(options: Options) -> Vec<String> {
    let mut lines = Vec::new();
    run(&three_shocks(), options, &mut |l| lines.push(l));
    crate::clock::set_override(None);
    lines
}

fn starting<'a>(lines: &'a [String], head: &str) -> Vec<&'a str> {
    lines.iter().filter(|l| l.starts_with(head)).map(String::as_str).collect()
}

#[test]
fn to_stops_after_the_last_line_its_prefix_admits() {
    let lines = report(Options { to: Some("12:00:41.000".into()), ..Default::default() });
    assert_eq!(starting(&lines, "payloads "), ["payloads 2, 2 left nothing buffered"]);
    assert_eq!(starting(&lines, "window "), ["window 12:00:35.098+00:00 .. 12:00:41.000+00:00"]);
    assert!(lines.iter().any(|l| l.starts_with("\n== target 19601: 2400 in the window")));
    let lines = report(Options { to: Some("12:00:40.9".into()), ..Default::default() });
    assert_eq!(starting(&lines, "payloads "), ["payloads 1, 1 left nothing buffered"]);
}

#[test]
fn from_opens_the_window_and_hits_show_what_each_line_added() {
    let lines = report(Options { from: Some("12:00:40".into()), show_hits: true, ..Default::default() });
    assert_eq!(starting(&lines, "window "), ["window 12:00:41.000+00:00 .. 12:00:47.000+00:00"]);
    let hits = starting(&lines, "hit ");
    assert_eq!(hits, [
        "hit 12:00:41.000+00:00 target 19601 actor 1425 skill 16010000 Cold Shock x1 1200",
        "hit 12:00:47.000+00:00 target 19601 actor 1425 skill 16010000 Cold Shock x1 1200",
    ]);
    assert!(lines.iter().any(|l| l == "\n== target 19601: 2400 in the window"));
    // Never opened: an empty window and no sums.
    let lines = report(Options { from: Some("18".into()), ..Default::default() });
    assert_eq!(starting(&lines, "window "), ["window  .. "]);
    assert!(!lines.iter().any(|l| l.starts_with("\n== target")));
}

#[test]
fn reset_at_clears_the_storage_once() {
    let lines = report(Options { reset_at: Some("12:00:40".into()), ..Default::default() });
    assert_eq!(starting(&lines, "reset at "), ["reset at 12:00:41.000+00:00"]);
}

#[test]
fn dump_prints_the_opcode_s_packets_on_the_target_in_the_window() {
    let shock = cold_shock();
    let dump = |target: Option<i32>| {
        let options = Options { dump_op: Some([0x04, 0x38]), target, from: Some("12:00:40".into()), ..Default::default() };
        report(options).into_iter().filter(|l| l.starts_with("dump ")).collect::<Vec<_>>()
    };
    let expected = [format!("dump 12:00:41.000+00:00 {shock}"), format!("dump 12:00:47.000+00:00 {shock}")];
    assert_eq!(dump(None), expected);
    assert_eq!(dump(Some(19_601)), expected);
    assert!(dump(Some(19_602)).is_empty());
}

#[test]
fn a_target_adds_the_meter_s_own_segment() {
    let lines = report(Options { target: Some(19_601), ..Default::default() });
    let head = lines.iter().position(|l| l.starts_with("\n== meter segment for target 19601: ")).expect("segment");
    assert!(lines[head].ends_with(" ms"), "{}", lines[head]);
    assert!(lines[head + 1].starts_with("meter actor "), "{}", lines[head + 1]);
}
