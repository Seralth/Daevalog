//! Who is who: entity ids bound to character names, and which one is you.

use std::collections::BTreeMap;
use std::ops::Range;
use std::ops::RangeInclusive;

use super::PacketDecoder;
use crate::capture::names::{clean_name_field, in_cjk_blocks, name_char, sanitized_at, tutorial_placeholder, NAME_LEN_RANGE};
use crate::capture::opcodes::{OWN_RECORDS, PLAYER_SPAWN, PLAYER_SPAWN_OLD, SELF_IDENTITY};
use crate::capture::varint::{le_u32_at, varint_at, varint_before};
use crate::entity::job_class::JobClass;

/// Entity ids in self and player records. Raids and invasions go past
/// 99,999; ids under 100 are players too.
const RECORD_IDS: RangeInclusive<i32> = 1..=9_999_999;
/// Ids the character list and the nickname patterns take.
const LISTED_IDS: RangeInclusive<i32> = 100..=9_999_999;
/// Ids the loot records take.
const LOOT_IDS: RangeInclusive<i32> = 100..=99_999;
/// Ids an actor anchor takes: no upper limit.
const ANCHOR_IDS: RangeInclusive<i32> = 100..=i32::MAX;
/// Home servers a self record may name; the game's table runs from 1001 to
/// 2560.
const HOME_SERVERS: RangeInclusive<u16> = 1000..=2999;
/// Whether `id` is in the game's ServerName table: 1001-1060, 1101-1160 and
/// so on to 2501-2560. Stricter than `HOME_SERVERS`: the census searches list
/// and player records at every byte, where a loose range lets noise through.
pub(crate) fn listed_server(id: u16) -> bool {
    matches!(id / 100, 10..=15 | 20..=25) && (1..=60).contains(&(id % 100))
}

/// Field lengths the loose readers take.
const LOOSE_FIELD: RangeInclusive<usize> = 1..=36;
/// Name lengths the nickname patterns take.
const PATTERN_FIELD: RangeInclusive<usize> = 2..=36;
/// The head of a 14-byte frame with opcode `00 36`; its first byte was `06`
/// before the June 2026 update. The nickname patterns stop at it.
const STOP_MARKS: [[u8; 3]; 2] = [[0x06, 0x00, 0x36], [0x0E, 0x00, 0x36]];

/// An id, a name, and where the name sits in the packet.
type Sighting = (i32, String, Range<usize>);

impl PacketDecoder {
    /// The account's character list, searched for the character named in
    /// Settings. Only until the game names you: after a self record the list
    /// gives ids of its own, not the ones in the world.
    pub(super) fn find_self_in_char_list(&self, data: &[u8]) {
        let store = &self.data_storage;
        if store.self_known_from_game() {
            return;
        }
        let Some(configured) = store.local_character_name() else { return };
        let wanted = configured.trim();
        if wanted.is_empty() {
            return;
        }
        let entry = (0..data.len()).filter_map(|at| char_list_entry(data, at)).find(|(_, name)| name == wanted);
        if let Some((id, name)) = entry {
            store.name_entity(id, &name);
            tracing::info!("character list: '{}' is entity {}", name, id);
        }
    }

    /// Self and player records anywhere in `data`. The game states these
    /// names itself, so they bind authoritatively.
    pub(super) fn find_masked_self(&self, data: &[u8]) {
        let store = &self.data_storage;
        for record in masked_records(data) {
            let id = record.id;
            let Some(name) = record.name else {
                if store.set_local_identity_from_game(id.into(), None) {
                    tracing::info!("self record: an unnamed new character, entity {}", id);
                }
                continue;
            };
            store.remember_small_id(id);
            store.name_entity(id, &name);
            let Some((server, class)) = record.profile else { continue };
            if store.set_local_identity_from_game(id.into(), Some(name.clone())) {
                tracing::info!("self record: you are '{}', entity {}", name, id);
            }
            store.remember_home_server(&name, server);
            store.store_self_card(&name, Some(class), level_after(data, record.field.end));
        }
    }

    /// A frame of a kind that only ever concerns you (`OWN_RECORDS`); storage
    /// decides when enough of them make their id you.
    pub(super) fn parse_own_record(&self, packet: &[u8]) {
        if let Some(id) = own_record_id(packet)
            && self.data_storage.note_own_record(id)
        {
            tracing::info!("records about you alone: you are entity {}", id);
        }
    }

    /// A player's record or spawn whose opcode ends at `offset_after_opcode`.
    pub(super) fn read_spawn_name(&self, data: &[u8], offset_after_opcode: usize) {
        let store = &self.data_storage;
        match player_spawn_at(data, offset_after_opcode) {
            Some(PlayerSpawn::Named(id, name, _)) => {
                store.remember_small_id(id);
                store.note_player_spawn(id);
                store.name_entity(id, &name);
            }
            Some(PlayerSpawn::Unnamed(id)) => store.note_player_record(id),
            None => {}
        }
    }

    /// Names found after an actor anchor (`actor_name_fields`): the first one
    /// that binds ends it.
    pub(super) fn read_name_binding(&self, packet: &[u8]) -> bool {
        actor_name_fields(packet).into_iter().any(|(actor, start, len)| self.store_utf8_name(packet, actor, start, len))
    }

    /// Offers the loose name in `packet[name_start..][..name_length]` to
    /// storage for an actor with no name that is no summon. True when it was
    /// offered, whether or not storage took it.
    pub(super) fn store_utf8_name(&self, packet: &[u8], actor_id: i32, name_start: usize, name_length: usize) -> bool {
        let store = &self.data_storage;
        if store.knows_name(actor_id) || store.is_pet(actor_id) || !LOOSE_FIELD.contains(&name_length) {
            return false;
        }
        let Some((name, _)) = sanitized_at(packet, name_start, name_length) else { return false };
        store.append_nickname(actor_id, &name);
        true
    }

    /// Names from loot records (`loot_actor_fields`). A lone actor not yet
    /// seen fighting gets its name only once it fights, unless the name is
    /// Han or Hangul.
    pub(super) fn read_loot_claim(&self, packet: &[u8]) -> bool {
        // Per actor the widest field; on a tie the first one stays.
        let mut widest: BTreeMap<i32, (String, usize)> = BTreeMap::new();
        for (actor, name, field) in loot_actor_fields(packet) {
            let held = widest.entry(actor).or_insert_with(|| (name.clone(), field.len()));
            if field.len() > held.1 {
                *held = (name, field.len());
            }
        }
        let store = &self.data_storage;
        if widest.len() == 1
            && let Some((&actor, (name, _))) = widest.first_key_value()
            && !store.seen_in_fight(actor)
            && !name.chars().any(in_cjk_blocks)
        {
            if !store.knows_name(actor) {
                store.hold_name_for_later(actor, name);
            }
            return false;
        }
        let mut offered = false;
        for (&actor, (name, _)) in &widest {
            if !store.knows_name(actor) {
                store.append_nickname(actor, name);
                offered = true;
            }
        }
        offered
    }

    /// Names from the nickname patterns (`nickname_fields`), all offered.
    pub(super) fn read_name_record(&self, packet: &[u8]) -> bool {
        let found = nickname_fields(packet);
        for (id, name, _) in &found {
            self.data_storage.append_nickname(*id, name);
        }
        !found.is_empty()
    }
}

/// The varint at `at` when it reads and its value lies in `ids`, with the
/// offset just past it.
fn varint_in(data: &[u8], at: usize, ids: RangeInclusive<i32>) -> Option<(i32, usize)> {
    let read = varint_at(data, at);
    (read.length > 0 && ids.contains(&read.value)).then(|| (read.value, at + read.length as usize))
}

/// `len` bytes from `start` as text, when they are all there and UTF-8.
fn text_at(data: &[u8], start: usize, len: usize) -> Option<&str> {
    std::str::from_utf8(data.get(start..start.checked_add(len)?)?).ok()
}

/// A character list entry at `at`: `<id u32 LE> <len> <name>`.
fn char_list_entry(data: &[u8], at: usize) -> Option<(i32, String)> {
    let id = i32::try_from(le_u32_at(data.get(at..at + 4)?, 0)).ok().filter(|id| LISTED_IDS.contains(id))?;
    let field = name_field(data, at + 4)?;
    Some((id, clean_name_field(&data[field])?))
}

/// The bytes after the length byte at `len_at`, when that length fits a name
/// field and the bytes are all there.
fn name_field(data: &[u8], len_at: usize) -> Option<Range<usize>> {
    let len = usize::from(*data.get(len_at)?);
    let start = len_at + 1;
    (NAME_LEN_RANGE.contains(&len) && start + len <= data.len()).then_some(start..start + len)
}

/// The front of a self or player record from just past its opcode:
/// `<id> <mask1, 4 bytes> <mask2>`. Gives the id, and where the name's length
/// byte sits when bit 0 of mask2 says a name follows and that byte is there.
fn masked_head(data: &[u8], at: usize) -> Option<(i32, Option<usize>)> {
    let (id, id_end) = varint_in(data, at, RECORD_IDS)?;
    let mask2 = id_end + 4;
    let named = mask2 + 1 < data.len() && data[mask2] & 1 == 1;
    Some((id, named.then_some(mask2 + 1)))
}

/// Every self and player record in `data` that holds a name, or your
/// placeholder. A record found is stepped over whole.
pub(super) fn masked_records(data: &[u8]) -> Vec<MaskedRecord> {
    let mut records = Vec::new();
    let mut at = 0;
    // 9 bytes is the smallest named record: opcode 2, id 1, mask1 4, mask2 1
    // and the length byte 1.
    while data.len().saturating_sub(at) >= 9 {
        match masked_record_at(data, at) {
            Some(record) => {
                at = record.field.end;
                records.push(record);
            }
            None => at += 1,
        }
    }
    records
}

fn masked_record_at(data: &[u8], at: usize) -> Option<MaskedRecord> {
    let op = data.get(at..at + 2)?;
    let own = op == SELF_IDENTITY;
    if !own && op != PLAYER_SPAWN && op != PLAYER_SPAWN_OLD {
        return None;
    }
    let (id, Some(len_at)) = masked_head(data, at + 2)? else { return None };
    let field = name_field(data, len_at)?;
    if own && tutorial_placeholder(&data[field.clone()]) {
        return Some(MaskedRecord { id, field, name: None, profile: None });
    }
    let name = clean_name_field(&data[field.clone()])?;
    // A self record read out of the wrong bytes has no sensible server and
    // class after its name, so those decide whether it is one.
    let profile = if own { Some(home_and_class(data, field.end)?) } else { None };
    Some(MaskedRecord { id, field, name: Some(name), profile })
}

/// The server (u16 LE) and roster class (u32 LE) right after a self record's
/// name.
fn home_and_class(data: &[u8], at: usize) -> Option<(u16, JobClass)> {
    let server = u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?);
    if !HOME_SERVERS.contains(&server) {
        return None;
    }
    Some((server, JobClass::job_from_party_list(le_u32_at(data.get(at + 2..at + 6)?, 0))?))
}

/// Your level in a self record: a u32 LE after the server, the class and one
/// more byte. Anything but 1 to 99 is not read as one.
fn level_after(data: &[u8], name_end: usize) -> Option<u32> {
    Some(le_u32_at(data.get(name_end + 7..name_end + 11)?, 0)).filter(|level| (1..=99).contains(level))
}

/// A self record (`33 36`) or another player's record (`44 36`, `45 36`), as
/// `PacketDecoder::find_masked_self` reads it.
pub(super) struct MaskedRecord {
    pub id: i32,
    /// The name field's bytes.
    pub field: Range<usize>,
    /// `None` for a tutorial character's placeholder in your self record.
    pub name: Option<String>,
    /// Your server and class, in a self record.
    pub profile: Option<(u16, JobClass)>,
}
/// A player spawn's (`44 36`, `45 36`) id and name, as
/// `PacketDecoder::read_spawn_name` reads them.
pub(crate) enum PlayerSpawn {
    /// The name bit is clear: a player all the same.
    Unnamed(i32),
    Named(i32, String, Range<usize>),
}

/// A player's record or spawn from just past its opcode. A set name bit
/// promises a name: a field that is none gives `None`.
pub(crate) fn player_spawn_at(data: &[u8], offset_after_opcode: usize) -> Option<PlayerSpawn> {
    let (id, len_at) = masked_head(data, offset_after_opcode)?;
    let Some(len_at) = len_at else { return Some(PlayerSpawn::Unnamed(id)) };
    let field = name_field(data, len_at)?;
    let name = clean_name_field(&data[field.clone()])?;
    Some(PlayerSpawn::Named(id, name, field))
}

/// The id in a whole frame `<length> <op> <id> <tail>` of a kind in
/// `OWN_RECORDS`, when a fixed tail has exactly its size.
fn own_record_id(frame: &[u8]) -> Option<i32> {
    let (_, op_at) = varint_in(frame, 0, i32::MIN..=i32::MAX)?;
    let op = frame.get(op_at..op_at + 2)?;
    let &(_, tail) = OWN_RECORDS.iter().find(|(kind, _)| op == kind)?;
    let (id, id_end) = varint_in(frame, op_at + 2, i32::MIN..=i32::MAX)?;
    tail.is_none_or(|size| frame.len() == id_end + size).then_some(id)
}

/// `(actor, name start, name length)` for every `07 <len> <name>` up to 64
/// bytes past an actor anchor `36 <actor ≥ 100>`. Every byte is looked at,
/// those inside names too. The `36` of a spawn opcode (`40`, `41`, `44`,
/// `45` before it) leaves the anchor as it was; any other `36` replaces it,
/// with none when no id of 100 or more follows.
pub(super) fn actor_name_fields(packet: &[u8]) -> Vec<(i32, usize, usize)> {
    let mut fields = Vec::new();
    let mut anchor: Option<(i32, usize)> = None;
    for (j, &byte) in packet.iter().enumerate() {
        let spawn_op = j > 0 && matches!(packet[j - 1], 0x40 | 0x41 | 0x44 | 0x45);
        if byte == 0x36 && j + 1 < packet.len() && !spawn_op {
            anchor = varint_in(packet, j + 1, ANCHOR_IDS);
            continue;
        }
        if byte != 0x07 {
            continue;
        }
        let Some((actor, _)) = anchor.filter(|&(_, from)| (from..=from + 64).contains(&j)) else { continue };
        let Some(len) = packet.get(j + 1).map(|&b| usize::from(b)) else { continue };
        if LOOSE_FIELD.contains(&len) && sanitized_at(packet, j + 2, len).is_some() {
            fields.push((actor, j + 2, len));
        }
    }
    fields
}

/// `<actor> <F0..FF> <03|A3> <len> <name>`: the actor, the loose name, and
/// the whole field after the length byte. A field found is stepped over,
/// whether or not it held a name.
pub(super) fn loot_actor_fields(packet: &[u8]) -> Vec<Sighting> {
    let mut found = Vec::new();
    let mut k = 0;
    while k + 2 < packet.len() {
        let marker = packet[k] >= 0xF0 && matches!(packet[k + 1], 0x03 | 0xA3);
        let len = usize::from(packet[k + 2]);
        let field = k + 3..k + 3 + len;
        let fits = marker && LOOSE_FIELD.contains(&len) && field.end <= packet.len();
        let actor = if fits { loot_actor_ending_at(packet, k) } else { None };
        let Some(actor) = actor else {
            k += 1;
            continue;
        };
        if let Some((name, _)) = sanitized_at(packet, field.start, len) {
            found.push((actor, name, field.clone()));
        }
        k = field.end;
    }
    found
}

/// A loot actor's varint, read whole: of the readings that end right before
/// `end` and start at most 8 bytes back, the one from the earliest start with
/// a value in `LOOT_IDS`.
fn loot_actor_ending_at(packet: &[u8], end: usize) -> Option<i32> {
    (end.saturating_sub(8)..end).find_map(|start| varint_in(packet, start, LOOT_IDS).filter(|&(_, past)| past == end).map(|(id, _)| id))
}

/// Names from the three nickname patterns, in the order the scan meets them.
/// At each offset the server tag is tried, then the block, then the stop
/// mark, each from where the one before left the scan.
pub(super) fn nickname_fields(packet: &[u8]) -> Vec<Sighting> {
    let mut found = Vec::new();
    let mut s = 0;
    while s + 2 < packet.len() {
        if let Some((sighting, next)) = server_tagged(packet, s) {
            found.push(sighting);
            s = next;
        }
        found.extend(block_name(packet, s));
        if let Some((sighting, stop)) = before_stop_mark(packet, s) {
            found.push(sighting);
            s = stop;
        }
        s += 1;
    }
    found
}

/// A name the patterns take: a length byte of 2 to 36 at `len_at`, then that
/// many bytes of text that start with a letter or digit and hold a loose
/// name. Gives the name and its own bytes.
fn pattern_name(packet: &[u8], len_at: usize) -> Option<(String, Range<usize>)> {
    let len = usize::from(*packet.get(len_at)?);
    if !PATTERN_FIELD.contains(&len) {
        return None;
    }
    if !text_at(packet, len_at + 1, len)?.starts_with(name_char) {
        return None;
    }
    sanitized_at(packet, len_at + 1, len)
}

/// `pattern_name` at `len_at` when that byte is `len`.
fn pattern_name_of(packet: &[u8], len_at: usize, len: usize) -> Option<(String, Range<usize>)> {
    if usize::from(*packet.get(len_at)?) != len {
        return None;
    }
    pattern_name(packet, len_at)
}

/// `<id> E0|E2 07 <len> <name>`, then maybe a `00` and a short string, both
/// stepped over. `E0 07` and `E2 07` read as servers 2016 and 2018. Gives
/// where the scan goes on.
fn server_tagged(packet: &[u8], s: usize) -> Option<(Sighting, usize)> {
    if !matches!(packet.get(s..s + 2)?, [0xE0 | 0xE2, 0x07]) {
        return None;
    }
    let (name, range) = pattern_name(packet, s + 2)?;
    let id = varint_before(packet, s, 0, LISTED_IDS)?;
    let mut next = s + 3 + usize::from(packet[s + 2]);
    if packet.get(next) == Some(&0) {
        next += 1;
    }
    if let Some(len) = packet.get(next).map(|&b| usize::from(b))
        && LOOSE_FIELD.contains(&len)
        && text_at(packet, next + 1, len).is_some()
    {
        next += 1 + len;
    }
    Some(((id, name, range), next))
}

fn at_stop_mark(packet: &[u8], at: usize) -> bool {
    STOP_MARKS.iter().any(|mark| packet.get(at..).is_some_and(|rest| rest.starts_with(mark)))
}

/// `0F 1D 37 <actor>`, then the first `00 00 <len> <name>` within 500 bytes,
/// unless a stop mark comes first.
fn block_name(packet: &[u8], s: usize) -> Option<Sighting> {
    if !packet.get(s..)?.starts_with(&[0x0F, 0x1D, 0x37]) {
        return None;
    }
    let (actor, from) = varint_in(packet, s + 3, LISTED_IDS)?;
    let limit = packet.len().min(from + 500);
    for p in from..limit.saturating_sub(3) {
        if at_stop_mark(packet, p) {
            return None;
        }
        if packet[p..].starts_with(&[0, 0])
            && let Some((name, range)) = pattern_name(packet, p + 2)
        {
            return Some((actor, name, range));
        }
    }
    None
}

/// `04|00 4C <id>`, then the name that ends at the first stop mark within 128
/// bytes of the id. A name just before that one (touching it, or one byte
/// apart) wins over it. Gives the stop mark's offset.
fn before_stop_mark(packet: &[u8], s: usize) -> Option<(Sighting, usize)> {
    if !matches!(packet.get(s..s + 2)?, [0x04 | 0x00, 0x4C]) {
        return None;
    }
    let (id, id_end) = varint_in(packet, s + 2, LISTED_IDS)?;
    let bound = packet.len().saturating_sub(2).min(s + 2 + 128);
    let stop = (id_end..bound).find(|&q| at_stop_mark(packet, q))?;
    let (len_at, last) = PATTERN_FIELD.clone().find_map(|len| {
        let at = stop.checked_sub(len + 1).filter(|&at| at > s + 2)?;
        pattern_name_of(packet, at, len).map(|got| (at, got))
    })?;
    // Unlike the found name's length byte, which must come after `s + 2`, the
    // earlier one only must not start before the id's end. Both as specified.
    let earlier = PATTERN_FIELD.flat_map(|len| [(len, 0), (len, 1)]).find_map(|(len, gap)| {
        let at = len_at.checked_sub(gap + len + 1).filter(|&at| at >= id_end)?;
        pattern_name_of(packet, at, len)
    });
    let (name, range) = earlier.unwrap_or(last);
    Some(((id, name, range), stop))
}

#[cfg(test)]
mod tests {
    use super::super::tests::processor;
    use super::*;
    use std::sync::Arc;

    use crate::built::{self, F};
    use crate::combat::data_storage::DataStorage;
    use crate::entity::damage_packet::ParsedDamagePacket;

    const QXQZ: [u8; 4] = *b"Qxqz";
    const ZQXV: [u8; 4] = *b"Zqxv";

    /// `actor` lands a hit on entity 900, so it is seen fighting.
    fn fights(store: &DataStorage, actor: i32) {
        let mut hit = ParsedDamagePacket::new();
        hit.set_actor_id(actor);
        hit.set_target_id(900);
        hit.set_skill_code(11_010_000);
        hit.set_damage(100);
        store.append_damage(hit);
    }

    fn joined(parts: &[&[u8]]) -> Vec<u8> {
        parts.concat()
    }

    fn sighting(id: i32, name: &str, range: Range<usize>) -> Sighting {
        (id, name.to_owned(), range)
    }

    // I1: the character list.

    /// Entity 3154 named "Qxqz", as `<u32 LE> <len> <name>`, after one byte
    /// that makes offset 0 read as id 807,594 with length 0.
    fn listed() -> Vec<u8> {
        joined(&[&[0xAA, 0x52, 0x0C, 0x00, 0x00, 0x04], &QXQZ])
    }

    fn list_binds(configured: Option<&str>, data: &[u8]) -> Arc<DataStorage> {
        let (store, p) = processor();
        store.set_local_character_name(configured.map(str::to_owned));
        p.find_self_in_char_list(data);
        store
    }

    #[test]
    fn the_list_names_only_the_character_set_in_settings() {
        assert_eq!(list_binds(Some("Qxqz"), &listed()).stored_name(3154).as_deref(), Some("Qxqz"));
        let other_first = joined(&[&[0x80, 0x0C, 0x00, 0x00, 0x04], &ZQXV, &listed()]);
        let store = list_binds(Some("Qxqz"), &other_first);
        assert_eq!(store.stored_name(3154).as_deref(), Some("Qxqz"));
        assert_eq!(store.stored_name(3200), None);
    }

    #[test]
    fn the_set_name_is_trimmed_but_its_case_counts() {
        assert_eq!(list_binds(Some(" Qxqz "), &listed()).stored_name(3154).as_deref(), Some("Qxqz"));
        for configured in [Some("qxqz"), Some("   "), None] {
            assert_eq!(list_binds(configured, &listed()).stored_name(3154), None, "{configured:?}");
        }
    }

    #[test]
    fn the_list_is_left_alone_once_the_game_named_you() {
        let (store, p) = processor();
        store.set_local_character_name(Some("Qxqz".into()));
        store.set_local_identity_from_game(5_000, Some("Qxqz".into()));
        p.find_self_in_char_list(&listed());
        assert_eq!(store.stored_name(3154), None);
    }

    #[test]
    fn a_list_entry_needs_a_real_id_and_its_whole_name() {
        let low = joined(&[&[0x63, 0x00, 0x00, 0x00, 0x04], &QXQZ]);
        assert!(list_binds(Some("Qxqz"), &low).all_names().is_empty());
        let cut = listed();
        assert!(list_binds(Some("Qxqz"), &cut[..cut.len() - 1]).all_names().is_empty());
    }

    // I2: self and player records.

    /// Your id, name, server, class and level as storage has them.
    fn you(store: &DataStorage) -> (Option<i64>, Option<String>, u16, Option<JobClass>, Option<u32>) {
        let profile = store.self_card();
        (store.local_player_id(), profile.name, profile.server_id, profile.class, profile.level)
    }

    fn scanned(data: &[u8]) -> Arc<DataStorage> {
        let (store, p) = processor();
        p.find_masked_self(data);
        store
    }

    #[test]
    fn your_self_record_gives_who_you_are() {
        let store = scanned(&built::self_record(3001, "Qxqz", 1202, 30, 30));
        assert_eq!(you(&store), (Some(3001), Some("Qxqz".into()), 1202, Some(JobClass::Cleric), Some(30)));
        assert_eq!(store.stored_name(3001).as_deref(), Some("Qxqz"));
    }

    #[test]
    fn a_level_past_1_to_99_is_not_read() {
        for level in [0, 100] {
            let store = scanned(&built::self_record(3001, "Qxqz", 1202, 30, level));
            assert_eq!(you(&store).4, None, "{level}");
        }
        // The level known before stays.
        let (store, p) = processor();
        p.find_masked_self(&built::self_record(3001, "Qxqz", 1202, 30, 30));
        p.find_masked_self(&built::self_record(3001, "Qxqz", 1202, 30, 100));
        assert_eq!(you(&store).4, Some(30));
    }

    #[test]
    fn a_self_record_needs_a_home_server_and_a_class() {
        for (server, class) in [(999, 30), (3000, 30), (1202, 4)] {
            let record = built::self_record(3001, "Qxqz", server, class, 28);
            assert!(masked_records(&record).is_empty(), "{server} {class}");
            let store = scanned(&record);
            assert_eq!((store.local_player_id(), store.stored_name(3001)), (None, None));
        }
        for server in [1000, 2999] {
            assert_eq!(scanned(&built::self_record(3001, "Qxqz", server, 30, 28)).local_player_id(), Some(3001));
        }
    }

    #[test]
    fn a_short_name_with_no_server_after_it_is_no_self_record() {
        // Id 16, mask2 `27`, "éx", then server 65535.
        let region = [0x33, 0x36, 0x10, 0x1D, 0x37, 0x8B, 0x27, 0x27, 0x03, 0xC3, 0xA9, 0x78, 0xFF, 0xFF, 0xFF];
        assert!(masked_records(&region).is_empty());
        let store = scanned(&region);
        assert_eq!((store.local_player_id(), store.stored_name(16)), (None, None));
    }

    #[test]
    fn bit_0_of_mask2_says_a_name_follows() {
        let mut record = built::self_record(3001, "Qxqz", 1202, 30, 30);
        assert_eq!(record[9], 0x37);
        record[9] = 0x3F;
        assert_eq!(scanned(&record).local_player_id(), Some(3001));
        record[9] = 0x36;
        assert!(masked_records(&record).is_empty());
    }

    #[test]
    fn a_player_record_binds_its_name_and_leaves_you_alone() {
        for op in [PLAYER_SPAWN, PLAYER_SPAWN_OLD] {
            let mut record = built::player_record(9303, Some("Qxqz"), &[]);
            record[1..3].copy_from_slice(&op);
            let found = masked_records(&record);
            assert_eq!(found.len(), 1);
            let r = &found[0];
            assert_eq!((r.id, r.field.clone(), r.name.as_deref(), r.profile.is_none()), (9303, 11..15, Some("Qxqz"), true));
            let store = scanned(&record);
            assert_eq!(store.stored_name(9303).as_deref(), Some("Qxqz"));
            assert_eq!(store.local_player_id(), None);
            // Bound authoritatively: a loose reader cannot rename it.
            store.append_nickname(9303, "Zqxv");
            assert_eq!(store.stored_name(9303).as_deref(), Some("Qxqz"));
        }
    }

    #[test]
    fn a_player_record_name_is_read_strictly() {
        assert_eq!(scanned(&built::player_record(9303, Some("Q"), &[])).stored_name(9303).as_deref(), Some("Q"));
        for name in ["Qxqzqxqzqxqzq", "Qx qz"] {
            assert!(masked_records(&built::player_record(9303, Some(name), &[])).is_empty(), "{name}");
        }
    }

    #[test]
    fn an_unnamed_new_character_is_still_you() {
        let placeholder = "$Qx9Zz0Aa0q1";
        let record = built::packet(SELF_IDENTITY, &[F::V(3877), F::U32(0x0100_0000), F::U8(0x37), F::Str(placeholder)]);
        let found = masked_records(&record);
        assert_eq!(found.len(), 1);
        assert!(found[0].name.is_none() && found[0].profile.is_none());
        let store = scanned(&record);
        assert_eq!((store.local_player_id(), store.local_character_name()), (Some(3877), None));
        assert!(store.self_known_from_game());
        assert_eq!(store.stored_name(3877), None);
        // In another player's record it is no name at all.
        let store = scanned(&built::player_record(3877, Some(placeholder), &[]));
        assert!(store.all_names().is_empty());
        assert!(!store.self_known_from_game());
    }

    #[test]
    fn a_record_is_read_from_the_bytes_it_is_given() {
        // Entity 51, mask2 `8f`, the name "A".
        let region = [0x44, 0x36, 0x33, 0x80, 0x70, 0x60, 0x50, 0x8F, 0x01, 0x41];
        let found = masked_records(&region);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].id, found[0].field.clone(), found[0].name.as_deref()), (51, 9..10, Some("A")));
    }

    #[test]
    fn record_ids_run_from_1_to_9_999_999() {
        for id in [0, 10_000_000] {
            assert!(masked_records(&built::player_record(id, Some("Qxqz"), &[])).is_empty(), "{id}");
        }
        for id in [1, 99, 150_000, 9_999_999] {
            let store = scanned(&built::player_record(id, Some("Qxqz"), &[]));
            assert_eq!(store.stored_name(id as i32).as_deref(), Some("Qxqz"), "{id}");
        }
        // Under 100, the id is also announced as a real entity.
        assert!(scanned(&built::player_record(16, Some("Qxqz"), &[])).looks_like_entity(16));
    }

    #[test]
    fn records_found_are_stepped_over_whole() {
        let mut data = built::player_record(9303, Some("Qxqz"), &[]);
        data.extend(built::player_record(2000, Some("Zqxv"), &[0; 4]));
        let ids: Vec<i32> = masked_records(&data).iter().map(|r| r.id).collect();
        assert_eq!(ids, [9303, 2000]);
    }

    // I3: player spawns.

    #[test]
    fn a_spawn_without_a_name_still_marks_a_player() {
        let record = built::player_record(9303, None, &[0]);
        assert!(matches!(player_spawn_at(&record, 3), Some(PlayerSpawn::Unnamed(9303))));
    }

    #[test]
    fn a_named_spawn_gives_its_name_and_field() {
        let record = built::player_record(9303, Some("Qxqz"), &[]);
        let Some(PlayerSpawn::Named(id, name, field)) = player_spawn_at(&record, 3) else { panic!("no name") };
        assert_eq!((id, name.as_str(), field), (9303, "Qxqz", 11..15));
        let (store, p) = processor();
        p.read_spawn_name(&record, 3);
        assert_eq!(store.stored_name(9303).as_deref(), Some("Qxqz"));
        store.append_nickname(9303, "Zqxv");
        assert_eq!(store.stored_name(9303).as_deref(), Some("Qxqz"));
    }

    #[test]
    fn a_spawn_cut_before_its_name_is_unnamed() {
        let record = built::player_record(9303, Some("Qxqz"), &[]);
        // Cut after mask1, then after a mask2 whose name bit is set.
        for end in [9, 10] {
            assert!(matches!(player_spawn_at(&record[..end], 3), Some(PlayerSpawn::Unnamed(9303))), "{end}");
        }
    }

    #[test]
    fn a_set_name_bit_needs_a_real_name() {
        let record = built::player_record(9303, Some("Qxqz"), &[]);
        for len in [0, 49] {
            let mut bad = record.clone();
            bad[10] = len;
            assert!(player_spawn_at(&bad, 3).is_none(), "{len}");
        }
        assert!(player_spawn_at(&record[..14], 3).is_none());
        assert!(player_spawn_at(&built::player_record(9303, Some("$Qx9Zz0Aa0q1"), &[]), 3).is_none());
        let (store, p) = processor();
        p.read_spawn_name(&record[..14], 3);
        fights(&store, 9303);
        assert_eq!(store.pet_owners().get(&9303), Some(&crate::combat::data_storage::UNATTRIBUTED_ID));
    }

    #[test]
    fn spawn_ids_past_99_999_are_read() {
        let record = built::player_record(150_000, Some("Qxqz"), &[]);
        assert_eq!(&record[3..6], &[0xF0, 0x93, 0x09]);
        assert!(matches!(player_spawn_at(&record, 3), Some(PlayerSpawn::Named(150_000, _, _))));
        assert!(player_spawn_at(&built::player_record(10_000_000, Some("Qxqz"), &[]), 3).is_none());
    }

    // I4: records about you alone.

    /// `4A 36` for entity 6200, its tail 8 zero bytes.
    fn next_id_frame() -> Vec<u8> {
        joined(&[&[0x10, 0x4A, 0x36, 0xB8, 0x30], &[0; 8]])
    }

    #[test]
    fn a_record_about_you_has_its_kind_s_exact_size() {
        let long_tail = next_id_frame();
        let cases: [(&[u8], Option<i32>); 6] = [
            (&[0x0C, 0x03, 0x8D, 0xE1, 0x21, 0, 0, 0, 0], Some(4321)),
            (&[0x08, 0x42, 0x37, 0xE1, 0x21], Some(4321)),
            (&[0x0D, 0x03, 0x8D, 0xE1, 0x21, 0, 0, 0, 0, 0], None),
            (&long_tail, Some(6200)),
            (&[0x07, 0x42, 0x37, 0xE1], None),
            (&[0x08, 0x4B, 0x36, 0xE1, 0x21], None),
        ];
        for (frame, want) in cases {
            assert_eq!(own_record_id(frame), want, "{frame:02x?}");
        }
    }

    #[test]
    fn records_about_you_go_to_storage() {
        let (store, p) = processor();
        store.set_local_identity_from_game(5_100, Some("Qxqz".into()));
        for _ in 0..3 {
            p.parse_own_record(&next_id_frame());
        }
        assert_eq!(store.local_player_id(), Some(6200));
    }

    // I5: names after an actor anchor.

    #[test]
    fn an_anchor_names_the_field_after_it() {
        let packet = joined(&[&[0x36, 0xDC, 0x0B, 0x07, 0x04], &QXQZ]);
        assert_eq!(actor_name_fields(&packet), [(1500, 5, 4)]);
        let (store, p) = processor();
        fights(&store, 1500);
        assert!(p.read_name_binding(&packet));
        assert_eq!(store.stored_name(1500).as_deref(), Some("Qxqz"));
    }

    #[test]
    fn the_field_starts_at_most_64_bytes_past_the_anchor() {
        let at = |gap: usize| actor_name_fields(&joined(&[&[0x36, 0xDC, 0x0B], &vec![0; gap], &[0x07, 0x04], &QXQZ]));
        assert_eq!(at(64), [(1500, 69, 4)]);
        assert!(at(65).is_empty());
    }

    #[test]
    fn an_anchor_is_100_or_more_and_not_a_spawn_opcode() {
        assert!(actor_name_fields(&joined(&[&[0x36, 0x63, 0x07, 0x04], &QXQZ])).is_empty());
        assert!(actor_name_fields(&joined(&[&[0x45, 0x36, 0xDC, 0x0B, 0x07, 0x04], &QXQZ])).is_empty());
        let kept = joined(&[&[0x36, 0xD0, 0x0F, 0x45, 0x36, 0xDC, 0x0B, 0x07, 0x04], &QXQZ]);
        assert_eq!(actor_name_fields(&kept), [(2000, 9, 4)]);
        // No upper limit.
        let big = joined(&[&[0x36, 0x80, 0xAD, 0xE2, 0x04, 0x07, 0x04], &QXQZ]);
        assert_eq!(actor_name_fields(&big), [(10_000_000, 7, 4)]);
    }

    #[test]
    fn a_low_id_after_36_drops_the_anchor() {
        let packet = joined(&[&[0x36, 0xDC, 0x0B, 0x07, 0x04], &QXQZ, &[0x36, 0x05, 0x07, 0x04], &ZQXV]);
        assert_eq!(actor_name_fields(&packet), [(1500, 5, 4)]);
    }

    /// Two anchors, each followed by a name: 1500 "Qxqz", 2000 "Zqxv".
    fn two_anchors() -> Vec<u8> {
        joined(&[&[0x36, 0xDC, 0x0B, 0x07, 0x04], &QXQZ, &[0x36, 0xD0, 0x0F, 0x07, 0x04], &ZQXV])
    }

    #[test]
    fn anchor_binding_stops_at_the_first_name_offered() {
        assert_eq!(actor_name_fields(&two_anchors()), [(1500, 5, 4), (2000, 14, 4)]);
        let (store, p) = processor();
        fights(&store, 1500);
        fights(&store, 2000);
        assert!(p.read_name_binding(&two_anchors()));
        assert_eq!((store.stored_name(1500).as_deref(), store.stored_name(2000)), (Some("Qxqz"), None));
        assert!(p.read_name_binding(&two_anchors()));
        assert_eq!(store.stored_name(2000).as_deref(), Some("Zqxv"));
        assert!(!p.read_name_binding(&two_anchors()));
    }

    #[test]
    fn an_anchor_field_may_start_with_white_space() {
        let packet = joined(&[&[0x36, 0xDC, 0x0B, 0x07, 0x05, 0x20], &QXQZ]);
        assert_eq!(actor_name_fields(&packet), [(1500, 5, 5)]);
        assert_eq!(sanitized_at(&packet, 5, 5), Some(("Qxqz".to_owned(), 6..10)));
    }

    #[test]
    fn register_refuses_summons_and_unreadable_fields() {
        let (store, p) = processor();
        store.link_pet_owner(1500, 2500);
        assert!(store.is_pet(2500));
        let packet = joined(&[&[0x07, 0x04], &QXQZ, &[0xC3, 0x28]]);
        assert!(!p.store_utf8_name(&packet, 2500, 2, 4));
        for (start, len) in [(2, 0), (2, 37), (4, 5), (6, 2)] {
            assert!(!p.store_utf8_name(&packet, 3000, start, len), "{start} {len}");
        }
        assert!(p.store_utf8_name(&packet, 3000, 2, 4));
        // Offered even though storage declined it: 3000 has never been seen.
        assert_eq!(store.stored_name(3000), None);
        store.name_entity(3000, "Zqxv");
        assert!(!p.store_utf8_name(&packet, 3000, 2, 4));
    }

    // I6: loot records.

    #[test]
    fn a_loot_name_waits_until_its_actor_fights() {
        let packet = joined(&[&[0xDC, 0x0B, 0xF0, 0x03, 0x04], &QXQZ]);
        assert_eq!(loot_actor_fields(&packet), [sighting(1500, "Qxqz", 5..9)]);
        let (store, p) = processor();
        assert!(!p.read_loot_claim(&packet));
        assert_eq!(store.stored_name(1500), None);
        fights(&store, 1500);
        assert_eq!(store.stored_name(1500).as_deref(), Some("Qxqz"));
        // Seen fighting already: bound at once.
        let (store, p) = processor();
        fights(&store, 1500);
        assert!(p.read_loot_claim(&packet));
        assert_eq!(store.stored_name(1500).as_deref(), Some("Qxqz"));
    }

    #[test]
    fn a_han_or_hangul_loot_name_does_not_wait() {
        let han = joined(&[&[0xDC, 0x0B, 0xF0, 0x03, 0x06], "丂丂".as_bytes()]);
        let (store, p) = processor();
        assert!(p.read_loot_claim(&han));
        fights(&store, 1500);
        assert_eq!(store.stored_name(1500), None, "offered, not left pending");
        let kana = joined(&[&[0xDC, 0x0B, 0xF0, 0x03, 0x06], "ぁぁ".as_bytes()]);
        let (store, p) = processor();
        assert!(!p.read_loot_claim(&kana));
        fights(&store, 1500);
        assert_eq!(store.stored_name(1500).as_deref(), Some("ぁぁ"));
    }

    #[test]
    fn two_loot_actors_both_bind() {
        let packet = joined(&[&[0xDC, 0x0B, 0xF0, 0x03, 0x04], &QXQZ, &[0xD0, 0x0F, 0xF7, 0xA3, 0x04], &ZQXV]);
        assert_eq!(loot_actor_fields(&packet), [sighting(1500, "Qxqz", 5..9), sighting(2000, "Zqxv", 14..18)]);
        let (store, p) = processor();
        fights(&store, 1500);
        fights(&store, 2000);
        assert!(p.read_loot_claim(&packet));
        assert_eq!((store.stored_name(1500).as_deref(), store.stored_name(2000).as_deref()), (Some("Qxqz"), Some("Zqxv")));
        let (store, p) = processor();
        fights(&store, 2000);
        store.name_entity(1500, "Vqzx");
        assert!(p.read_loot_claim(&packet));
        assert_eq!((store.stored_name(1500).as_deref(), store.stored_name(2000).as_deref()), (Some("Vqzx"), Some("Zqxv")));
        // Both named: nothing offered.
        assert!(!p.read_loot_claim(&packet));
    }

    #[test]
    fn loot_actors_bind_in_ascending_id() {
        // One name for two actors: the later offer takes it, so 2000 ends up
        // with it although 1500 comes last in the packet.
        let packet = joined(&[&[0xD0, 0x0F, 0xF0, 0x03, 0x04], &QXQZ, &[0xDC, 0x0B, 0xF0, 0x03, 0x04], &QXQZ]);
        let (store, p) = processor();
        fights(&store, 1500);
        fights(&store, 2000);
        assert!(p.read_loot_claim(&packet));
        assert_eq!((store.stored_name(1500), store.stored_name(2000).as_deref()), (None, Some("Qxqz")));
    }

    #[test]
    fn each_loot_actor_keeps_its_widest_field() {
        let wider_later = joined(&[&[0xDC, 0x0B, 0xF0, 0x03, 0x02, 0x51, 0x78, 0xDC, 0x0B, 0xF0, 0x03, 0x04], &QXQZ]);
        let tie = joined(&[&[0xDC, 0x0B, 0xF0, 0x03, 0x04, 0x51, 0x78, 0x00, 0x00, 0xDC, 0x0B, 0xF0, 0x03, 0x04], &QXQZ]);
        for (packet, want) in [(wider_later, "Qxqz"), (tie, "Qx")] {
            let (store, p) = processor();
            fights(&store, 1500);
            assert!(p.read_loot_claim(&packet));
            assert_eq!(store.stored_name(1500).as_deref(), Some(want));
        }
    }

    #[test]
    fn a_loot_actor_is_read_whole_and_in_range() {
        let actor = |head: &[u8]| loot_actor_fields(&joined(&[head, &[0xF0, 0x03, 0x04], &QXQZ])).first().map(|f| f.0);
        assert_eq!(actor(&[0x9A, 0x6D]), Some(13_978));
        // `80 80 07` reads as 114,688, too big; `80 07` is 896.
        assert_eq!(actor(&[0, 0, 0, 0, 0, 0x80, 0x80, 0x07]), Some(896));
        assert_eq!(actor(&[0x64]), Some(100));
        assert_eq!(actor(&[0x63]), None);
        // `a0 8d 06` is 100,000, one too many; `8d 06` is 781.
        assert_eq!(actor(&[0xA0, 0x8D, 0x06]), Some(781));
        // `e3 00` is 99, `00` is 0.
        assert_eq!(actor(&[0xE3, 0x00]), None);
    }

    #[test]
    fn a_loot_marker_is_03_or_a3_before_a_readable_name() {
        assert!(loot_actor_fields(&joined(&[&[0xDC, 0x0B, 0xF0, 0x13, 0x04], &QXQZ])).is_empty());
        assert!(loot_actor_fields(&[0xDC, 0x0B, 0xF0, 0x03, 0x02, 0xC3, 0x28]).is_empty());
        assert!(loot_actor_fields(&joined(&[&[0xDC, 0x0B, 0xF0, 0x03, 0x05], &QXQZ])).is_empty());
        let (_, p) = processor();
        assert!(!p.read_loot_claim(&[0xDC, 0x0B, 0xF0, 0x03, 0x02, 0xC3, 0x28]));
    }

    // I7: the nickname patterns.

    fn tagged(head: &[u8], tail: &[u8]) -> Vec<Sighting> {
        nickname_fields(&joined(&[head, tail]))
    }

    #[test]
    fn a_server_tagged_name_takes_its_whole_id() {
        let name = joined(&[&[0x04], &QXQZ]);
        assert_eq!(tagged(&[0x01, 0x9A, 0x6D, 0xE2, 0x07], &name), [sighting(13_978, "Qxqz", 6..10)]);
        assert_eq!(tagged(&[0x01, 0x98, 0x75, 0xE0, 0x07], &name)[0].0, 15_000);
        assert_eq!(tagged(&[0x01, 0x6D, 0xE2, 0x07], &name)[0].0, 109);
        assert_eq!(tagged(&[0x01, 0xBD, 0x44, 0xE2, 0x07], &name)[0].0, 8_765);
    }

    #[test]
    fn a_server_tagged_name_needs_an_id_and_a_clean_start() {
        assert!(tagged(&[0x63, 0xE2, 0x07, 0x04], &QXQZ).is_empty());
        assert!(tagged(&[0x01, 0x9A, 0x6D, 0xE2, 0x07, 0x01], b"Q").is_empty());
        assert!(tagged(&[0x01, 0x9A, 0x6D, 0xE2, 0x07, 0x05, 0x20], &QXQZ).is_empty());
        assert!(tagged(&[0x01, 0x9A, 0x6D, 0xE2, 0x07, 0x25], &[b'Q'; 37]).is_empty());
        assert_eq!(tagged(&[0x01, 0x9A, 0x6D, 0xE2, 0x07, 0x04], b"Qx-z"), [sighting(13_978, "Qx", 6..8)]);
    }

    #[test]
    fn the_string_after_a_tagged_name_is_stepped_over() {
        let packet = joined(&[&[0x01, 0x9A, 0x6D, 0xE2, 0x07, 0x04], &QXQZ, &[0x02, 0x5A, 0x71, 0xE2, 0x07, 0x04], &ZQXV]);
        assert_eq!(nickname_fields(&packet), [sighting(13_978, "Qxqz", 6..10)]);
        // Read on its own, the second tag is a name.
        assert_eq!(nickname_fields(&packet[10..]), [sighting(113, "Zqxv", 6..10)]);
    }

    /// A stop-mark name: `<lead> 4C <id> 04 "Qxqz" 0E 00 36`.
    fn marked(lead: u8, id: &[u8]) -> Vec<u8> {
        joined(&[&[lead, 0x4C], id, &[0x04], &QXQZ, &STOP_MARKS[1]])
    }

    #[test]
    fn what_follows_a_tagged_name_decides_where_the_scan_goes_on() {
        let tag = joined(&[&[0x01, 0x9A, 0x6D, 0xE2, 0x07, 0x04], &QXQZ]);
        let first = sighting(13_978, "Qxqz", 6..10);
        // At the packet's end.
        assert_eq!(nickname_fields(&tag), [first.clone()]);
        // `04 4c dc 0b` as a string is not UTF-8, so the scan stays at the
        // name's end, where the stop-mark pattern starts.
        let after = joined(&[&tag, &marked(0x04, &[0xDC, 0x0B])]);
        assert_eq!(nickname_fields(&after), [first.clone(), sighting(1500, "Qxqz", 15..19)]);
        // A `00` and a length past 36: the scan goes on after the `00`, so a
        // pattern led by that `00` is never seen.
        let skipped = joined(&[&tag, &marked(0x00, &[0x64])]);
        assert_eq!(nickname_fields(&skipped), [first.clone()]);
        assert_eq!(nickname_fields(&marked(0x00, &[0x64])), [sighting(100, "Qxqz", 4..8)]);
        // A `00` and a string: what the string holds is stepped over.
        let string = marked(0x04, &[0x64]);
        let hidden = joined(&[&tag, &[0x00, string.len() as u8], &string]);
        assert_eq!(nickname_fields(&hidden), [first]);
        assert_eq!(nickname_fields(&string), [sighting(100, "Qxqz", 4..8)]);
    }

    #[test]
    fn a_block_name_follows_two_zero_bytes() {
        let head = [0x0F, 0x1D, 0x37, 0xDC, 0x0B];
        let name = joined(&[&[0x00, 0x00, 0x04], &QXQZ]);
        assert_eq!(nickname_fields(&joined(&[&head, &[1, 2, 3], &name])), [sighting(1500, "Qxqz", 11..15)]);
        assert!(nickname_fields(&joined(&[&head, &STOP_MARKS[1], &name])).is_empty());
        assert!(nickname_fields(&joined(&[&head, &STOP_MARKS[0], &name])).is_empty());
        // A field that is no name is passed for the next one.
        let dashes = joined(&[&head, &[0x00, 0x00, 0x04], b"----", &name]);
        assert_eq!(nickname_fields(&dashes), [sighting(1500, "Qxqz", 15..19)]);
        assert!(nickname_fields(&joined(&[&[0x0F, 0x1D, 0x37, 0x63], &name])).is_empty());
    }

    #[test]
    fn a_block_name_starts_within_500_bytes() {
        let at = |gap: usize| nickname_fields(&joined(&[&[0x0F, 0x1D, 0x37, 0xDC, 0x0B], &vec![0x11; gap], &[0x00, 0x00, 0x04], &QXQZ]));
        assert_eq!(at(496).len(), 1);
        assert!(at(497).is_empty());
    }

    #[test]
    fn a_stop_mark_name_ends_at_the_mark() {
        let packet = joined(&[&[0x04, 0x4C, 0xDC, 0x0B, 0x11, 0x22, 0x04], &QXQZ, &STOP_MARKS[1]]);
        assert_eq!(nickname_fields(&packet), [sighting(1500, "Qxqz", 7..11)]);
        let old_mark = joined(&[&[0x04, 0x4C, 0xDC, 0x0B, 0x11, 0x22, 0x04], &QXQZ, &STOP_MARKS[0]]);
        assert_eq!(nickname_fields(&old_mark), [sighting(1500, "Qxqz", 7..11)]);
        assert!(nickname_fields(&joined(&[&[0x04, 0x4C, 0x63, 0x11, 0x04], &QXQZ, &STOP_MARKS[1]])).is_empty());
    }

    #[test]
    fn the_name_before_a_stop_mark_name_wins() {
        let touching = joined(&[&[0x04, 0x4C, 0xDC, 0x0B, 0x04], &QXQZ, &[0x05], b"Zqxvq", &STOP_MARKS[1]]);
        assert_eq!(nickname_fields(&touching), [sighting(1500, "Qxqz", 5..9)]);
        let apart = joined(&[&[0x04, 0x4C, 0xDC, 0x0B, 0x04], &QXQZ, &[0x00, 0x05], b"Zqxvq", &STOP_MARKS[1]]);
        assert_eq!(nickname_fields(&apart), [sighting(1500, "Qxqz", 5..9)]);
        // Two bytes apart is too far.
        let far = joined(&[&[0x04, 0x4C, 0xDC, 0x0B, 0x04], &QXQZ, &[0x00, 0x00, 0x05], b"Zqxvq", &STOP_MARKS[1]]);
        assert_eq!(nickname_fields(&far), [sighting(1500, "Zqxvq", 12..17)]);
    }

    #[test]
    fn the_stop_mark_lies_within_128_bytes_of_the_id() {
        let at = |gap: usize| nickname_fields(&joined(&[&[0x04, 0x4C, 0xDC, 0x0B], &vec![0x11; gap], &[0x04], &QXQZ, &STOP_MARKS[1]]));
        assert_eq!(at(120).len(), 1);
        assert!(at(121).is_empty());
    }

    #[test]
    fn only_the_first_stop_mark_counts() {
        let packet = joined(&[&[0x04, 0x4C, 0xDC, 0x0B, 0x11], &STOP_MARKS[1], &[0x04], &QXQZ, &STOP_MARKS[1]]);
        assert!(nickname_fields(&packet).is_empty());
    }

    #[test]
    fn every_pattern_name_is_offered() {
        let packet = joined(&[&[0x01, 0x9A, 0x6D, 0xE2, 0x07, 0x04], &QXQZ, &[0x11, 0x0F, 0x1D, 0x37, 0xDC, 0x0B, 0x00, 0x00, 0x04], &ZQXV]);
        let (store, p) = processor();
        fights(&store, 13_978);
        fights(&store, 1500);
        assert!(p.read_name_record(&packet));
        assert_eq!((store.stored_name(13_978).as_deref(), store.stored_name(1500).as_deref()), (Some("Qxqz"), Some("Zqxv")));
        assert!(!p.read_name_record(&[0x01, 0x02, 0x03, 0x04]));
    }
}
