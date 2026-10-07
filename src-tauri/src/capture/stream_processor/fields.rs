//! Where a packet holds character names: the name field of every record the
//! parser reads a name from, found by the parser's own readers. The slice and
//! the bug-report copy blind names there, at any length: a name of one byte
//! cannot be searched for, since that byte turns up everywhere.

use std::ops::Range;

use super::identity::{actor_name_fields, loot_actor_fields, masked_records, nickname_fields, player_spawn_at, PlayerSpawn};
use super::roster::rosters;
use super::spawn::{embedded_spawns, ownership_at_front, ownership_records, spawn_name_at};
use crate::capture::names::sanitized_at;
use crate::capture::opcodes::{PLAYER_SPAWN, PLAYER_SPAWN_OLD, SPAWN, SPAWN_OLD};
use crate::capture::varint::read_varint;

/// A record that carries a character name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameRecord {
    /// `33 36`, `44 36`, `45 36`: your self record and other players' records
    /// and spawns.
    Player,
    /// `40 36`, `41 36`: the owner or caster a spawn names.
    SpawnOwner,
    /// `02 97`: a party member.
    PartyMember,
    /// `04 8D`: a summon's owner, or a mob's loot owner.
    Owner,
    /// `07 <len> <name>` after an actor anchor (`36 <actor>`).
    ActorName,
    /// `<actor> F0..FF 03|A3 <len> <name>`.
    LootActor,
    /// The nickname patterns `E0|E2 07`, `0F 1D 37` and `04|00 4C`.
    Nickname,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameField {
    pub range: Range<usize>,
    pub record: NameRecord,
}

/// The name fields of one frame, as a replay reads that frame: the readers
/// `parse_perfect_packet` runs on it, and the record scans over it
/// (`scan_records`). Sorted by where they start; two readers that find the same
/// field give it once.
pub fn name_fields(frame: &[u8]) -> Vec<NameField> {
    let mut out = Vec::new();
    at_front(frame, &mut out);
    for (_, start, len) in actor_name_fields(frame) {
        if let Some((_, range)) = sanitized_at(frame, start, len) {
            out.push(NameField { range, record: NameRecord::ActorName });
        }
    }
    for (_, _, field) in loot_actor_fields(frame) {
        if let Some((_, range)) = sanitized_at(frame, field.start, field.len()) {
            out.push(NameField { range, record: NameRecord::LootActor });
        }
    }
    for (_, _, range) in nickname_fields(frame) {
        out.push(NameField { range, record: NameRecord::Nickname });
    }
    scanned(frame, &mut out);
    tidy(out)
}

/// The name fields of bytes between frames: only the record scans read those.
pub fn name_fields_unframed(bytes: &[u8]) -> Vec<NameField> {
    let mut out = Vec::new();
    scanned(bytes, &mut out);
    tidy(out)
}

fn tidy(mut out: Vec<NameField>) -> Vec<NameField> {
    out.sort_by_key(|f| (f.range.start, f.range.end));
    out.dedup_by(|a, b| a.range == b.range);
    out
}

/// The records `parse_summon_packet` and `parse_summon_ownership_packet` read
/// at the front of a packet.
fn at_front(packet: &[u8], out: &mut Vec<NameField>) {
    let length = read_varint(packet, 0);
    if length.length < 0 {
        return;
    }
    let offset = length.length as usize;
    if offset + 1 >= packet.len() {
        return;
    }
    let opcode = [packet[offset], packet[offset + 1]];
    if opcode == PLAYER_SPAWN || opcode == PLAYER_SPAWN_OLD {
        if let Some(PlayerSpawn::Named(_, _, range)) = player_spawn_at(packet, offset + 2) {
            out.push(NameField { range, record: NameRecord::Player });
        }
    } else if opcode == SPAWN || opcode == SPAWN_OLD {
        if let Some(range) = spawn_owner(packet, offset + 2) {
            out.push(NameField { range, record: NameRecord::SpawnOwner });
        }
    }
    if let Some((_, _, Some((start, len)))) = ownership_at_front(packet)
        && let Some((_, range)) = sanitized_at(packet, start, len)
    {
        out.push(NameField { range, record: NameRecord::Owner });
    }
}

/// The name a `40/41 36` spawn carries, as `parse_summon_spawn_at` reads it.
fn spawn_owner(packet: &[u8], offset_after_opcode: usize) -> Option<Range<usize>> {
    let id = read_varint(packet, offset_after_opcode);
    if id.length < 0 {
        return None;
    }
    let offset = offset_after_opcode + id.length as usize;
    if offset + 2 >= packet.len() {
        return None;
    }
    spawn_name_at(packet, offset).map(|(_, range)| range)
}

/// The records the scans read anywhere in a frame (see `scan_records`). The
/// character-select list (`scan_char_list_self`) is left out: it names only
/// the character the meter was told to look for.
fn scanned(data: &[u8], out: &mut Vec<NameField>) {
    for record in ownership_records(data) {
        out.push(NameField { range: record.field, record: NameRecord::Owner });
    }
    for (i, opcode, _) in embedded_spawns(data) {
        if opcode == PLAYER_SPAWN || opcode == PLAYER_SPAWN_OLD {
            if let Some(PlayerSpawn::Named(_, _, range)) = player_spawn_at(data, i + 2) {
                out.push(NameField { range, record: NameRecord::Player });
            }
        } else if let Some(range) = spawn_owner(data, i + 2) {
            // Read whether or not the parser already knows the id as a mob.
            out.push(NameField { range, record: NameRecord::SpawnOwner });
        }
    }
    for record in masked_records(data) {
        if record.name.is_some() {
            out.push(NameField { range: record.field, record: NameRecord::Player });
        }
    }
    for roster in rosters(data) {
        for range in roster.name_fields {
            out.push(NameField { range, record: NameRecord::PartyMember });
        }
    }
}
