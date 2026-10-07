//! The character part of the replay report (`A2_REPLAY_CHARACTER=1`): your own
//! character's latest state as one JSON object, read from the packet logs with
//! `capture::character_report`.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::capture::character_report::{Character, Names};
use crate::capture::framing;
use crate::capture::packet_accumulator::PacketAccumulator;

/// Replay the logs (paths joined by ':', oldest first) and return the state
/// at the end as pretty JSON.
pub(super) fn report(paths: &str) -> String {
    let root = std::env::var("A2_GAME_DATA")
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join("Projects/aion2-data")))
        .unwrap_or_default();
    let names = Names::load(&root);
    let files: Vec<String> = paths.split(':').filter(|p| !p.is_empty()).map(str::to_string).collect();
    let mut character = Character::default();
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            eprintln!("character: {path} not readable");
            continue;
        };
        replay_text(&text, &names, &mut character);
    }
    serde_json::to_string_pretty(&character.to_json(&names, &files)).unwrap_or_default()
}

fn replay_text(text: &str, names: &Names, character: &mut Character) {
    let mut streams: HashMap<String, PacketAccumulator> = HashMap::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, '|');
        let (Some(ts), Some(key), Some(hex)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let Some(bytes) = super::replay_report::decode_hex(hex) else { continue };
        let acc = streams.entry(key.to_string()).or_insert_with(PacketAccumulator::new);
        acc.append(&bytes);
        let consumed = framing::walk(acc.snapshot()).consumed;
        let mut packets = Vec::new();
        super::replay_report::frames_of(&acc.snapshot()[..consumed], true, &mut packets, 0);
        acc.discard_bytes(consumed);
        for p in packets {
            character.feed(names, ts, &p);
        }
    }
}
