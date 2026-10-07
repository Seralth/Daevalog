//! The party roster (02 97): who is in your party, their levels, gear and combat power.

use super::StreamProcessor;
use crate::capture::opcodes::PARTY_ROSTER;
use crate::capture::varint::{parse_u32_le, read_varint};

use std::ops::Range;

impl StreamProcessor {
    // ===== PARTY ROSTER (02 97) =====

    /// Read the party roster the server broadcasts on any party change.
    ///
    /// ```text
    /// 02 97
    /// party_key    u32
    /// party_name   str            (u8 len + utf8)
    /// party_size   u8             party max size
    /// dungeon_id   u32
    /// unnamed      u8, u8
    /// leader_dbid  u64
    /// unnamed      u8, u8, u8
    /// member_count varint
    /// member × member_count:
    ///   presence_mask u8
    ///   slot          u8          1-based party slot
    ///   dbid          u64         account-level id; high u16 is the world id
    ///   nickname      str
    ///   unnamed       u32
    ///   level         u32
    ///   gear_score    u32         equip item level
    ///   server        u16, u16
    ///   unnamed       u8
    ///   combat_power  u64
    ///   unnamed       u16, u8
    /// ```
    ///
    /// This is the only packet that states who is in your party outright, so it
    /// is the authoritative roster — and it is where combat power comes from.
    /// It joins to in-world entities by NAME: `dbid` is an account id, unrelated
    /// to the session-scoped entity ids everything else uses.
    ///
    /// Vacant slots are records too: a zero mask, the slot number, a zero dbid,
    /// an empty name and zero stats, in a shorter form. Slots after a vacant
    /// one can still be filled (2026-10-06: slots 1 and 5 filled, 2 to 4
    /// vacant), so the walk skips them.
    pub(super) fn scan_party_roster(&self, data: &[u8]) {
        for roster in rosters(data) {
            tracing::debug!(
                "Party roster: {} members (complete={}, dungeon={})",
                roster.members.len(),
                roster.complete,
                roster.dungeon_id
            );
            self.data_storage.set_current_dungeon(roster.dungeon_id);
            self.data_storage.set_party_roster(roster.members, roster.complete);
        }
    }
}

/// A party roster as `StreamProcessor::scan_party_roster` reads it.
pub(super) struct Roster {
    pub members: Vec<(String, crate::combat::data_storage::PartyMember)>,
    pub complete: bool,
    pub dungeon_id: i32,
    /// Each member's name field, in member order.
    pub name_fields: Vec<Range<usize>>,
}

/// Every party roster in `data`.
pub(super) fn rosters(data: &[u8]) -> Vec<Roster> {
    let mut out = Vec::new();
    if data.len() < 32 {
        return out;
    }
    let mut i = 0;
    while i + 24 < data.len() {
        if data[i..i + 2] != PARTY_ROSTER {
            i += 1;
            continue;
        }
        match parse_party_roster_at(data, i + 2) {
            Some(roster) => {
                out.push(roster);
                i += 2;
            }
            None => i += 1,
        }
    }
    out
}

/// Decode the body of a `02 97` party roster packet. `at` is the first byte
/// after the opcode. Returns the members that parsed cleanly, or `None` if the
/// header does not look like a roster (the opcode is scanned for in raw byte
/// streams, so the header checks double as the false-positive filter).
/// See `StreamProcessor::scan_party_roster` for the layout.
fn parse_party_roster_at(data: &[u8], at: usize) -> Option<Roster> {
    use crate::combat::data_storage::PartyMember;

    let mut o = at.checked_add(4)?; // party_key u32
    let name_len = *data.get(o)? as usize;
    o += 1;
    if !(1..=40).contains(&name_len) {
        return None;
    }
    std::str::from_utf8(data.get(o..o + name_len)?).ok()?;
    o += name_len;

    let party_size = *data.get(o)? as usize;
    o += 1;
    if !(1..=12).contains(&party_size) {
        return None;
    }
    // The instance the party is queued for / inside. Identifies both the dungeon
    // and its difficulty tier: Ferocious Horn Den is 600091/600092/600093 for
    // Exploration / Conquest [Normal] / Conquest [Hard].
    let dungeon_id = parse_u32_le(data.get(o..o + 4)?, 0) as i32;
    o += 4 + 2 + 8 + 3; // dungeon_id, 2 pad, leader_dbid, 3 pad
    let count_info = read_varint(data, o);
    if count_info.length <= 0 || !(1..=12).contains(&count_info.value) {
        return None;
    }
    o += count_info.length as usize;

    let mut members = Vec::new();
    let mut name_fields = Vec::new();
    let mut complete = false;
    for index in 0..count_info.value {
        if o + 20 > data.len() {
            break;
        }
        let slot = data[o + 1];
        if is_vacant_slot(data, o) {
            if index + 1 == count_info.value {
                complete = true;
                break;
            }
            // Its length is not fixed either, so find the next record as
            // after a member.
            match find_next_member(data, o + VACANT_HEADER, slot.wrapping_add(1)) {
                Some(next) => o = next,
                None => break,
            }
            continue;
        }
        o += 2; // presence_mask, slot
        let dbid = u64::from_le_bytes(data.get(o..o + 8)?.try_into().ok()?);
        o += 8;
        let server_id = (dbid >> 48) as u16;
        let nick_len = *data.get(o)? as usize;
        o += 1;
        if nick_len == 0 || nick_len > 40 || o + nick_len > data.len() {
            break;
        }
        let nickname = match std::str::from_utf8(&data[o..o + nick_len]) {
            Ok(s) => s.to_string(),
            Err(_) => break,
        };
        let name_field = o..o + nick_len;
        o += nick_len;
        if o + 12 > data.len() {
            break;
        }
        let job = crate::entity::job_class::JobClass::from_roster_class(parse_u32_le(data, o));
        o += 4;
        let level = parse_u32_le(data, o) as i32;
        o += 4;
        if !(1..=200).contains(&level) {
            break;
        }
        let gear_score = parse_u32_le(data, o) as i32;
        o += 4;
        if !(0..=1_000_000).contains(&gear_score) {
            break;
        }

        // The stretch between the gear score and combat power is not fixed
        // width — the same roster can carry an extra byte for one member and not
        // another, and a party-state change widens every record's tail. Anchor
        // on the member's world id instead: it is repeated here as a u16 and we
        // already know its value from the top half of `dbid`. Combat power then
        // sits a fixed distance past it.
        let Some(anchor) = find_u16(data, o, o + 10, server_id) else {
            break;
        };
        o = anchor + 2 + 2 + 1; // world id, a second u16, one tag byte
        let combat_power = u64::from_le_bytes(data.get(o..o + 8)?.try_into().ok()?);
        o += 8;
        if combat_power > 100_000_000 {
            break;
        }

        name_fields.push(name_field);
        members.push((
            nickname,
            PartyMember {
                slot,
                level,
                gear_score,
                combat_power: combat_power as i64,
                server_id,
                dbid,
                job,
            },
        ));

        if index + 1 == count_info.value {
            complete = true;
            break;
        }
        // The record tail is likewise variable, so re-acquire the next member by
        // its header: the following slot number, a world id in the top of its
        // dbid, and a decodable name right behind it.
        match find_next_member(data, o, slot.wrapping_add(1)) {
            Some(next) => o = next,
            None => break,
        }
    }
    if members.is_empty() {
        return None;
    }
    Some(Roster { members, complete, dungeon_id, name_fields })
}

/// Find a little-endian `u16` equal to `wanted` in `data[from..to]`.
fn find_u16(data: &[u8], from: usize, to: usize, wanted: u16) -> Option<usize> {
    let end = to.min(data.len().saturating_sub(2));
    (from..=end).find(|&i| u16::from_le_bytes([data[i], data[i + 1]]) == wanted)
}

/// Bytes of a vacant slot's record up to its name: mask, slot, dbid, name length.
const VACANT_HEADER: usize = 11;

/// A vacant slot's record at `at`: a zero mask, then (after the slot number)
/// a zero dbid, an empty name and a zero class, level and gear score. The 12
/// zeros after the name are what tell it from a stray `00 <slot>` in the
/// zeros of the record before it, whose next nonzero byte comes sooner.
fn is_vacant_slot(data: &[u8], at: usize) -> bool {
    data.get(at) == Some(&0)
        && data.get(at + 2..at + VACANT_HEADER + 12).is_some_and(|zeros| zeros.iter().all(|&b| b == 0))
}

/// Re-acquire the start of the next record by its header shape: a vacant
/// slot, or a member's `<mask u8> <slot u8> <dbid u64> <name_len u8> <utf8
/// name>`, where the slot is known and the top `u16` of the dbid is a
/// plausible world id.
fn find_next_member(data: &[u8], from: usize, expected_slot: u8) -> Option<usize> {
    let end = (from + 32).min(data.len().saturating_sub(12));
    for i in from..=end {
        if data[i + 1] != expected_slot {
            continue;
        }
        if is_vacant_slot(data, i) {
            return Some(i);
        }
        // A member's mask is never 0 (0x0c, 0x0e, 0x1c or 0x1e in every
        // capture so far). A record tail of `00 05 00 00 00 00 00 00 00 01 02`
        // before slot 5 read as a member whose name was the next two bytes.
        if data[i] == 0 {
            continue;
        }
        let server_id = u16::from_le_bytes([data[i + 8], data[i + 9]]);
        if server_id == 0 || server_id > 9_999 {
            continue;
        }
        let name_len = data[i + 10] as usize;
        if name_len == 0 || name_len > 40 || i + 11 + name_len > data.len() {
            continue;
        }
        if std::str::from_utf8(&data[i + 11..i + 11 + name_len]).is_err() {
            continue;
        }
        return Some(i);
    }
    None
}
