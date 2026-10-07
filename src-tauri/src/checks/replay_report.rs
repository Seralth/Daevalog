//! Replays a `packets_*.txt` capture through the same assembler, parser and
//! storage the live meter uses, and prints what was counted on one target.
//!
//! ```text
//! A2_REPLAY_FILE=packets_x.txt     the capture (required)
//! A2_REPLAY_TARGET=36734           target entity id (optional: else every target)
//! A2_REPLAY_FROM=04:45:57.945      window, local time of day from the file
//! A2_REPLAY_TO=04:46:44.445
//! A2_REPLAY_HITS=1                 print every change on the target in the window
//! A2_REPLAY_DUMP=0538              print framed packets with this opcode that
//!                                  name the target, in the window
//! A2_REPLAY_RESET_AT=04:48:00      clear combat data there, as the reset button does
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
//! A2_REPLAY_PLAYERS=1              print every player met, at the end: entity id,
//!                                  server, class and where it came from, name;
//!                                  then every Item Level and Combat Power a party,
//!                                  party finder or legion list stated (`gear`
//!                                  lines, capture ms and list first; see
//!                                  `replay_players`)
//! A2_REPLAY_PACKETS=1              print every framed packet (`pkt`) and every
//!                                  bundle opened inside one (`bun`) as hex,
//!                                  capture ms first, in the window
//! A2_REPLAY_CHARACTER=1            print your own character's latest state as
//!                                  JSON instead (see `replay_character`)
//!
//! The report always ends with the deaths of you and your party in the
//! window and in each fight the meter would save (see `replay_deaths`).
//! cargo test --lib replay_report -- --ignored --nocapture
//! ```

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use crate::capture::framing::{self, FrameKind};
use crate::capture::packet_accumulator::PacketAccumulator;
use crate::capture::ping_tracker::PingTracker;
use crate::capture::stream_assembler::StreamAssembler;
use crate::capture::stream_processor::StreamProcessor;
use crate::capture::varint::read_varint;
use crate::combat::data_storage::{DataStorage, TargetCombatData};
use crate::combat::dps_calculator::DpsCalculator;
use crate::entity::summon_resolver;
use crate::i18n::lookup::{NpcLookup, SkillLookup};

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

pub(super) fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    let hex = hex.trim();
    if hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

/// (raw actor, skill, is_dot) -> (hits, damage)
type Counts = HashMap<(i32, i32, bool), (i64, i64)>;

fn counts(t: &TargetCombatData) -> Counts {
    let mut out = Counts::new();
    for (&actor, a) in &t.actors {
        for (&(skill, dot), s) in &a.skills {
            out.insert((actor, skill, dot), (s.hit_count as i64, s.total_damage as i64));
        }
    }
    out
}

/// What changed from `before` to `after`. A segment reset (new first-damage
/// time) starts from nothing.
fn diff(before: Option<&TargetCombatData>, after: Option<&TargetCombatData>) -> Counts {
    let Some(after) = after else { return Counts::new() };
    let base = match before {
        Some(b) if b.first_damage_time == after.first_damage_time => counts(b),
        _ => Counts::new(),
    };
    let now = counts(after);
    let mut out = Counts::new();
    // Keys can vanish too: one actor's rows merged into another's.
    for k in now.keys().chain(base.keys()) {
        let (h, d) = now.get(k).copied().unwrap_or((0, 0));
        let (bh, bd) = base.get(k).copied().unwrap_or((0, 0));
        if h != bh || d != bd {
            out.insert(*k, (h - bh, d - bd));
        }
    }
    out
}

/// Packets inside a buffer, bundles opened, for the opcode dump, the timeline
/// and the player list.
pub(super) fn frames_of(buf: &[u8], top: bool, out: &mut Vec<Vec<u8>>, depth: usize) {
    let walk = if top { framing::walk(buf) } else { framing::walk_inner(buf) };
    for f in &walk.frames {
        match f.kind {
            FrameKind::Packet => out.push(f.bytes(buf).to_vec()),
            FrameKind::Bundle if depth < 4 => {
                if let Some(inner) = framing::decompress_bundle(f.payload(buf)) {
                    frames_of(&inner, false, out, depth + 1);
                }
            }
            FrameKind::Bundle => {}
        }
    }
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
    pub players: bool,
    pub packets: bool,
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
#[ignore]
fn replay_report() {
    let Some(path) = env("A2_REPLAY_FILE") else {
        eprintln!("set A2_REPLAY_FILE");
        return;
    };
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
        dump_op: env("A2_REPLAY_DUMP").and_then(|h| decode_hex(&h)).and_then(|v| v.try_into().ok()),
        timeline: env("A2_REPLAY_TIMELINE").is_some(),
        taken: env("A2_REPLAY_TAKEN").is_some(),
        players: env("A2_REPLAY_PLAYERS").is_some(),
        packets: env("A2_REPLAY_PACKETS").is_some(),
    };
    let _flags = env("A2_REPLAY_FLAGS").map(|_| {
        tracing::subscriber::set_default(
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

/// Replay a capture's text and hand each line of the report to `out`.
pub(crate) fn run(text: &str, options: Options, out: &mut dyn FnMut(String)) {
    let Options {
        target, from, to, show_hits, mut reset_at, dump_op, timeline, taken, players: list_players, packets: list_packets,
    } = options;
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
    let mut walks: HashMap<String, PacketAccumulator> = HashMap::new();

    let (mut lines, mut lines_clean) = (0usize, 0usize);
    let mut window_started = false;
    let mut window: BTreeMap<i32, Counts> = BTreeMap::new();
    let mut prev: HashMap<i32, TargetCombatData> = HashMap::new();
    let mut last_ts = 0i64;
    let mut first_tod = String::new();
    let mut last_tod = String::new();
    let mut gather = timeline.then(|| super::replay_timeline::Gather::new(&data_dir));
    let mut zone = None;
    let _tap = gather.as_ref().map(|g| g.tap.install());
    let mut window_ms = 0i64;
    let mut taken = taken.then(super::replay_taken::Gather::default);
    let mut deaths = super::replay_deaths::Gather::default();
    let mut met = list_players.then(super::replay_players::Players::default);
    let mut first_ts = None;

    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, '|');
        let (Some(ts), Some(key), Some(hex)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let Some(bytes) = decode_hex(hex) else { continue };
        let Ok(when) = chrono::DateTime::parse_from_rfc3339(ts) else { continue };
        let ts_ms = when.timestamp_millis();
        zone.get_or_insert(*when.offset());
        let tod = ts.get(11..).unwrap_or("").to_string();
        if let Some(to) = &to {
            if tod.get(..to.len()).unwrap_or(&tod) > to.as_str() {
                break;
            }
        }
        if reset_at.as_ref().is_some_and(|r| tod.as_str() >= r.as_str()) {
            out(format!("reset at {tod}"));
            storage.flush();
            reset_at = None;
        }
        let in_window = from.as_ref().is_none_or(|f| tod.as_str() >= f.as_str());
        if in_window && !window_started {
            window_started = true;
            prev = storage.get_combat_snapshot_light();
            first_tod = tod.clone();
            window_ms = ts_ms;
        }

        let (assembler, processor) = streams.entry(key.to_string()).or_insert_with(|| {
            let mut p = StreamProcessor::new(storage.clone(), skills.clone(), npcs.clone());
            p.set_dot_skill_ids(dot_ids.clone());
            (StreamAssembler::new(), p)
        });
        processor.set_override_timestamp(Some(ts_ms));
        assembler.process_chunk(&bytes, processor);
        lines += 1;
        if assembler.buffered_bytes() == 0 {
            lines_clean += 1;
        }
        last_ts = ts_ms;
        first_ts.get_or_insert(ts_ms);

        if (in_window && (dump_op.is_some() || list_packets)) || gather.is_some() || met.is_some() {
            let acc = walks.entry(key.to_string()).or_insert_with(PacketAccumulator::new);
            acc.append(&bytes);
            let consumed = framing::walk(acc.snapshot()).consumed;
            let mut packets = Vec::new();
            frames_of(&acc.snapshot()[..consumed], true, &mut packets, 0);
            acc.discard_bytes(consumed);
            if let Some(met) = met.as_mut() {
                for p in &packets {
                    met.scan_spawns(p, ts_ms);
                    met.scan_gear(p, ts_ms);
                    for bundle in framing::embedded_bundles(p) {
                        met.scan_spawns(&bundle.data, ts_ms);
                        let mut inner = Vec::new();
                        frames_of(&bundle.data, false, &mut inner, 1);
                        for q in &inner {
                            met.scan_gear(q, ts_ms);
                        }
                    }
                }
                met.roster_and_self(&storage, ts_ms);
            }
            for p in packets {
                if list_packets && in_window {
                    out(format!("pkt {ts_ms} {}", to_hex(&p)));
                    for bundle in framing::embedded_bundles(&p) {
                        out(format!("bun {ts_ms} {}", to_hex(&bundle.data)));
                    }
                }
                if let Some(g) = &mut gather {
                    g.timeline.note(ts_ms, &p);
                }
                let Some(op) = dump_op.filter(|_| in_window) else { continue };
                let o = read_varint(&p, 0).length.max(0) as usize;
                if p.get(o..o + 2) != Some(&op[..]) {
                    continue;
                }
                if let Some(t) = target {
                    if read_varint(&p, o + 2).value != t {
                        continue;
                    }
                }
                out(format!("dump {tod} {}", to_hex(&p)));
            }
        }
        if let Some(t) = &mut taken {
            t.after_line(&storage);
        }
        deaths.after_line(&storage);
        if let Some(g) = &mut gather {
            g.after_line(ts_ms, storage.local_player_id().map(|v| v as i32), || storage.get_summon_data());
        }

        if window_started {
            last_tod = tod.clone();
            let now = storage.get_combat_snapshot_light();
            deaths.note_fights(now.values());
            let ids: Vec<i32> = match target {
                Some(t) => vec![t],
                None => now.keys().copied().collect(),
            };
            for id in ids {
                let d = diff(prev.get(&id), now.get(&id));
                for (&(actor, skill, dot), &(h, dmg)) in &d {
                    if let (Some(met), true) = (met.as_mut(), h > 0) {
                        met.hit(&storage, actor, skill, ts_ms);
                    }
                    if show_hits {
                        out(format!(
                            "hit {tod} target {id} actor {actor} skill {skill} {}{} x{h} {dmg}",
                            skills.get_skill_name(skill),
                            if dot { " (DoT)" } else { "" }
                        ));
                    }
                    let e = window.entry(id).or_default().entry((actor, skill, dot)).or_default();
                    e.0 += h;
                    e.1 += dmg;
                }
            }
            prev = now;
        }
    }

    out(format!("payloads {lines}, {lines_clean} left nothing buffered"));
    out(format!("window {first_tod} .. {last_tod}"));
    let summons = storage.get_summon_data();
    let local = storage.local_player_id().map(|v| v as i32);
    out(format!("local player {local:?}"));
    // Identity state, for comparing two builds on the same capture.
    let mut links: Vec<_> = summons.iter().map(|(a, b)| (*a, *b)).collect();
    links.sort();
    let mut players: Vec<_> = storage.get_known_player_ids().into_iter().collect();
    players.sort();
    let mut named: Vec<_> = storage.get_nicknames().into_keys().collect();
    named.sort();
    let digest = |v: &str| v.bytes().fold(0u64, |h, b| h.wrapping_mul(1_099_511_628_211) ^ b as u64);
    out(format!(
        "state: {} summon links ({:016x}), {} players ({:016x}), {} named ({:016x}), {} mobs",
        links.len(),
        digest(&format!("{links:?}")),
        players.len(),
        digest(&format!("{players:?}")),
        named.len(),
        digest(&format!("{named:?}")),
        storage.get_mob_data().len()
    ));

    for (id, c) in &window {
        let total: i64 = c.values().map(|v| v.1).sum();
        if total == 0 {
            continue;
        }
        out(format!("\n== target {id}: {total} in the window"));
        // Raw actor -> owner.
        let mut actors: BTreeMap<i32, (i32, i64, i64, i64)> = BTreeMap::new();
        let mut by_skill: BTreeMap<(i32, i32, bool), (i64, i64)> = BTreeMap::new();
        for (&(actor, skill, dot), &(h, d)) in c {
            let owner = summon_resolver::resolve(actor, &summons);
            let a = actors.entry(actor).or_insert((owner, 0, 0, 0));
            if dot { a.3 += d } else { a.2 += d }
            a.1 += h;
            let s = by_skill.entry((owner, skill, dot)).or_default();
            s.0 += h;
            s.1 += d;
        }
        let mut owners: BTreeMap<i32, i64> = BTreeMap::new();
        for (actor, (owner, hits, direct, dot)) in &actors {
            out(format!("actor {actor} owner {owner}: direct {direct} dot {dot} ({hits} records)"));
            *owners.entry(*owner).or_default() += direct + dot;
        }
        for (owner, d) in &owners {
            out(format!("owner {owner}: {d}"));
        }
        let mut rows: Vec<_> = by_skill.into_iter().collect();
        rows.sort_by_key(|(_, (_, d))| -d);
        for ((owner, skill, dot), (h, d)) in rows {
            out(format!(
                "skill owner {owner} {skill} {}{}: {h} hits {d}",
                skills.get_skill_name(skill),
                if dot { " (DoT)" } else { "" }
            ));
        }
    }

    if let Some(met) = met {
        out(String::new());
        for line in met.lines(&storage, first_ts.unwrap_or(0), last_ts) {
            out(line);
        }
    }

    // The meter's own view of the segment at the end of the window, owners
    // merged the way the details panel and saved fights do it.
    if let Some(t) = target {
        crate::clock::set_override(Some(last_ts));
        let calc = DpsCalculator::new(storage.clone(), skills.clone(), npcs.clone(), Arc::new(PingTracker::new()));
        let details = calc.get_target_details(t, None);
        out(format!("\n== meter segment for target {t}: {} over {} ms", details.total_target_damage, details.battle_time));
        let mut rows = details.skills.clone();
        rows.sort_by_key(|s| -(s.dmg as i64));
        for s in rows {
            out(format!(
                "meter actor {} {} {}{}: {} hits {}",
                s.actor_id,
                s.code,
                s.name,
                if s.is_dot { " (DoT)" } else { "" },
                s.time,
                s.dmg
            ));
        }
        crate::clock::set_override(None);
    }
    if let Some(t) = &taken {
        t.report((window_ms, last_ts), local, &skills, &npcs, out);
    }
    deaths.report((window_ms, last_ts), &storage, &skills, &npcs, out);
    if let Some(g) = gather {
        let mobs = storage.get_mob_data();
        let zone = zone.unwrap_or_else(|| chrono::FixedOffset::east_opt(0).unwrap());
        // A fight's ticks start where the window does, when one is given.
        let window = (from.is_some().then_some(window_ms), last_ts);
        g.report(zone, last_ts, storage.get_summon_data(), window, target, &mobs, &skills, &npcs, out);
    }
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
    let text = format!("2026-10-06T12:00:00.000-07:00|Client:50000:13328|{}", to_hex(&outer));
    let at = chrono::DateTime::parse_from_rfc3339("2026-10-06T12:00:00-07:00").unwrap().timestamp_millis();

    let mut lines = Vec::new();
    run(&text, Options { packets: true, ..Default::default() }, &mut |l| lines.push(l));
    let got: Vec<&String> = lines.iter().filter(|l| l.starts_with("pkt ") || l.starts_with("bun ")).collect();
    assert_eq!(got, [&format!("pkt {at} {}", to_hex(&outer)), &format!("bun {at} {}", to_hex(&inner))]);

    let mut lines = Vec::new();
    run(&text, Options::default(), &mut |l| lines.push(l));
    assert!(!lines.iter().any(|l| l.starts_with("pkt ") || l.starts_with("bun ")));
}
