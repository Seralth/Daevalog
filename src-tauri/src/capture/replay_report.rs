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
//! cargo test --lib replay_report -- --ignored --nocapture
//! ```

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use super::framing::{self, FrameKind};
use super::packet_accumulator::PacketAccumulator;
use super::stream_assembler::StreamAssembler;
use super::stream_processor::{read_varint, StreamProcessor};
use crate::combat::data_storage::{DataStorage, TargetCombatData};
use crate::combat::dps_calculator::DpsCalculator;
use crate::combat::ping_tracker::PingTracker;
use crate::entity::summon_resolver;
use crate::i18n::lookup::{NpcLookup, SkillLookup};

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
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

/// Packets inside a buffer, bundles opened, for the opcode dump.
fn frames_of(buf: &[u8], top: bool, out: &mut Vec<Vec<u8>>, depth: usize) {
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

#[test]
#[ignore]
fn replay_report() {
    let Some(path) = env("A2_REPLAY_FILE") else {
        eprintln!("set A2_REPLAY_FILE");
        return;
    };
    let target: Option<i32> = env("A2_REPLAY_TARGET").and_then(|v| v.parse().ok());
    let from = env("A2_REPLAY_FROM");
    let to = env("A2_REPLAY_TO");
    let show_hits = env("A2_REPLAY_HITS").is_some();
    let mut reset_at = env("A2_REPLAY_RESET_AT");
    let dump_op: Option<[u8; 2]> = env("A2_REPLAY_DUMP").and_then(|h| decode_hex(&h)).and_then(|v| v.try_into().ok());

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

    let text = std::fs::read_to_string(&path).expect("capture readable");
    let (mut lines, mut lines_clean) = (0usize, 0usize);
    let mut window_started = false;
    let mut window: BTreeMap<i32, Counts> = BTreeMap::new();
    let mut prev: HashMap<i32, TargetCombatData> = HashMap::new();
    let mut last_ts = 0i64;
    let mut first_tod = String::new();
    let mut last_tod = String::new();

    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, '|');
        let (Some(ts), Some(key), Some(hex)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let Some(bytes) = decode_hex(hex) else { continue };
        let Ok(when) = chrono::DateTime::parse_from_rfc3339(ts) else { continue };
        let ts_ms = when.timestamp_millis();
        let tod = ts.get(11..).unwrap_or("").to_string();
        if let Some(to) = &to {
            if tod.get(..to.len()).unwrap_or(&tod) > to.as_str() {
                break;
            }
        }
        if reset_at.as_ref().is_some_and(|r| tod.as_str() >= r.as_str()) {
            println!("reset at {tod}");
            storage.flush();
            reset_at = None;
        }
        let in_window = from.as_ref().is_none_or(|f| tod.as_str() >= f.as_str());
        if in_window && !window_started {
            window_started = true;
            prev = storage.get_combat_snapshot_light();
            first_tod = tod.clone();
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

        if let (true, Some(op)) = (in_window, dump_op) {
            let acc = walks.entry(key.to_string()).or_insert_with(PacketAccumulator::new);
            acc.append(&bytes);
            let consumed = framing::walk(acc.snapshot()).consumed;
            let mut packets = Vec::new();
            frames_of(&acc.snapshot()[..consumed], true, &mut packets, 0);
            acc.discard_bytes(consumed);
            for p in packets {
                let o = read_varint(&p, 0).length.max(0) as usize;
                if p.get(o..o + 2) != Some(&op[..]) {
                    continue;
                }
                if let Some(t) = target {
                    if read_varint(&p, o + 2).value != t {
                        continue;
                    }
                }
                let hex: String = p.iter().map(|b| format!("{b:02x}")).collect();
                println!("dump {tod} {hex}");
            }
        }

        if window_started {
            last_tod = tod.clone();
            let now = storage.get_combat_snapshot_light();
            let ids: Vec<i32> = match target {
                Some(t) => vec![t],
                None => now.keys().copied().collect(),
            };
            for id in ids {
                let d = diff(prev.get(&id), now.get(&id));
                for (&(actor, skill, dot), &(h, dmg)) in &d {
                    if show_hits {
                        println!(
                            "hit {tod} target {id} actor {actor} skill {skill} {}{} x{h} {dmg}",
                            skills.get_skill_name(skill),
                            if dot { " (DoT)" } else { "" }
                        );
                    }
                    let e = window.entry(id).or_default().entry((actor, skill, dot)).or_default();
                    e.0 += h;
                    e.1 += dmg;
                }
            }
            prev = now;
        }
    }

    println!("payloads {lines}, {lines_clean} left nothing buffered");
    println!("window {first_tod} .. {last_tod}");
    let summons = storage.get_summon_data();
    let local = storage.local_player_id().map(|v| v as i32);
    println!("local player {local:?}");
    // Identity state, for comparing two builds on the same capture.
    let mut links: Vec<_> = summons.iter().map(|(a, b)| (*a, *b)).collect();
    links.sort();
    let mut players: Vec<_> = storage.get_known_player_ids().into_iter().collect();
    players.sort();
    let mut named: Vec<_> = storage.get_nicknames().into_keys().collect();
    named.sort();
    let digest = |v: &str| v.bytes().fold(0u64, |h, b| h.wrapping_mul(1_099_511_628_211) ^ b as u64);
    println!(
        "state: {} summon links ({:016x}), {} players ({:016x}), {} named ({:016x}), {} mobs",
        links.len(),
        digest(&format!("{links:?}")),
        players.len(),
        digest(&format!("{players:?}")),
        named.len(),
        digest(&format!("{named:?}")),
        storage.get_mob_data().len()
    );

    for (id, c) in &window {
        let total: i64 = c.values().map(|v| v.1).sum();
        if total == 0 {
            continue;
        }
        println!("\n== target {id}: {total} in the window");
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
            println!("actor {actor} owner {owner}: direct {direct} dot {dot} ({hits} records)");
            *owners.entry(*owner).or_default() += direct + dot;
        }
        for (owner, d) in &owners {
            println!("owner {owner}: {d}");
        }
        let mut rows: Vec<_> = by_skill.into_iter().collect();
        rows.sort_by_key(|(_, (_, d))| -d);
        for ((owner, skill, dot), (h, d)) in rows {
            println!(
                "skill owner {owner} {skill} {}{}: {h} hits {d}",
                skills.get_skill_name(skill),
                if dot { " (DoT)" } else { "" }
            );
        }
    }

    // The meter's own view of the segment at the end of the window, owners
    // merged the way the details panel and saved fights do it.
    if let Some(t) = target {
        crate::clock::set_override(Some(last_ts));
        let calc = DpsCalculator::new(storage.clone(), skills.clone(), npcs.clone(), Arc::new(PingTracker::new()));
        let details = calc.get_target_details(t, None);
        println!("\n== meter segment for target {t}: {} over {} ms", details.total_target_damage, details.battle_time);
        let mut rows = details.skills.clone();
        rows.sort_by_key(|s| -(s.dmg as i64));
        for s in rows {
            println!(
                "meter actor {} {} {}{}: {} hits {}",
                s.actor_id,
                s.code,
                s.name,
                if s.is_dot { " (DoT)" } else { "" },
                s.time,
                s.dmg
            );
        }
        crate::clock::set_override(None);
    }
}
