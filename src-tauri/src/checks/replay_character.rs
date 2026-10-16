//! The character part of the replay report (`A2_REPLAY_CHARACTER=1`): your own
//! character's latest state as one JSON object, read from the packet logs with
//! `capture::character_report`.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::capture::character_report::{Character, Names};
use crate::capture::byte_queue::ByteQueue;
use crate::capture::packet_log::bytes_from_hex;

/// Feeds every packet of one log's text to `character`, in order. Each
/// stream's bytes are framed on their own; the stamp goes on as written.
fn replay_text(text: &str, names: &Names, character: &mut Character) {
    let mut pending: HashMap<&str, ByteQueue> = HashMap::new();
    for line in text.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let Some((stamp, rest)) = line.split_once('|') else { continue };
        let Some((stream, hex)) = rest.split_once('|') else { continue };
        let Some(bytes) = bytes_from_hex(hex) else { continue };
        let packets = super::replay_report::whole_packets(pending.entry(stream).or_default(), &bytes);
        for packet in &packets {
            character.feed(names, stamp, packet);
        }
    }
}

/// Replay the logs (paths joined by ':', oldest first) and return the state
/// at the end as pretty JSON.
pub(super) fn report(paths: &str) -> String {
    // Without the game's tables every name stays an id.
    let names = std::env::var_os("A2_GAME_DATA")
        .filter(|v| !v.is_empty())
        .map_or_else(Names::default, |root| Names::load(&PathBuf::from(root)));
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
