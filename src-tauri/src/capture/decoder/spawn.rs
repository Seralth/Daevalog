//! Spawns and summons: who owns a summon, and what a spawned mob is.

use std::ops::Range;

use super::PacketDecoder;
use crate::capture::abnormal::NEVER;
use crate::capture::names::{clean_name_field,NAME_LEN_RANGE};
use crate::capture::opcodes::{PLAYER_SPAWN, PLAYER_SPAWN_OLD, SPAWN, SPAWN_OLD, SUMMON_OWNERSHIP};
use crate::capture::varint::{index_of_bytes, le_u32_at, varint_at, next_varint, varint_before, VarintRead};

/// A `04 8D` ownership or loot record found anywhere in `data`.
pub(super) struct Ownership {
    pub summon_id: i32,
    pub owner_id: i32,
    pub server_id: u16,
    pub name: String,
    pub field: Range<usize>,
}

/// Kind bytes (the byte after a spawn's entity id).
const PET: u8 = 0x5F;
const SPIRIT: u8 = 0x1F;
const MOB_EFFECT: u8 = 0x1C;

/// The `end` of an abnormal that never ends, as its bytes.
const NEVER_ENDS: [u8; 8] = NEVER.to_le_bytes();

/// Owners, casters and other ids read out of a record.
const RECORD_IDS: std::ops::RangeInclusive<i32> = 1..=9_999_999;
/// Ids a scan accepts where it cannot tell a record from stray bytes.
const SCANNED_IDS: std::ops::RangeInclusive<i32> = 100..=9_999_999;

/// Ids past a million are folded into 16384..32767, as the meter shows them.
fn folded(id: i32) -> i32 {
    if id > 1_000_000 {
        (id & 0x3FFF) | 0x4000
    } else {
        id
    }
}

/// What a spawn's fields tell before anyone is linked.
struct SpawnFacts {
    id: i32,
    kind: u8,
    /// Where the kind byte sits.
    kind_at: usize,
    name: Option<(String, Range<usize>)>,
    code: Option<i32>,
}

/// Where a spawn's owner was found.
enum OwnerSource {
    /// The owner block after a pet's name.
    Block,
    /// The inline name, as a known player's.
    Name,
    /// The caster of the spawn's own never-ending abnormal.
    Endless,
    /// A ground spell's caster field.
    Caster,
}

/// The folded entity id at `packet[at]`, and where the kind byte after it sits.
fn spawn_head(packet: &[u8], at: usize) -> Option<(i32, usize)> {
    let raw = varint_at(packet, at);
    let length = usize::try_from(raw.length).ok()?;
    Some((folded(raw.value), at + length))
}

impl PacketDecoder {
    /// A `04 8D` record at the front of `packet`: link the summon when the id
    /// is one, and name its owner from the record.
    pub(super) fn read_pet_owner(&self, packet: &[u8]) -> bool {
        let Some((summon, owner, name)) = ownership_at_front(packet) else {
            return false;
        };
        if self.data_storage.pet_is_confirmed(summon) {
            self.data_storage.link_pet_owner(owner, summon);
        }
        if let Some((start, len)) = name {
            self.store_utf8_name(packet, owner, start, len);
        }
        true
    }

    /// A spawn at the front of `packet`. True only when it linked a summon.
    pub(super) fn read_pet_spawn(&mut self, packet: &[u8]) -> bool {
        let Ok(body) = usize::try_from(varint_at(packet, 0).length) else {
            return false;
        };
        let Some(&[a, b]) = packet.get(body..body + 2) else {
            return false;
        };
        let after = body + 2;
        if [a, b] == SPAWN || [a, b] == SPAWN_OLD {
            let linked = self.read_pet_spawn_at(packet, after);
            self.link_effect_to_parent(packet, after);
            linked
        } else {
            if [a, b] == PLAYER_SPAWN || [a, b] == PLAYER_SPAWN_OLD {
                self.read_spawn_name(packet, after);
            }
            false
        }
    }

    /// Every `04 8D` record in `data`. True when there was one.
    pub(super) fn seek_inner_048d(&self, data: &[u8]) -> bool {
        let found = ownership_records(data);
        let storage = &self.data_storage;
        for record in &found {
            let (summon, owner) = (record.summon_id, record.owner_id);
            let linked = storage.pet_is_confirmed(summon);
            if linked {
                storage.link_pet_owner(owner, summon);
            }
            storage.append_nickname(owner, &record.name);
            storage.remember_home_server(&record.name, record.server_id);
            // Not a summon: a fought mob's kill, which names who looted it.
            if !linked && storage.was_hit(summon) {
                storage.claim_loot(summon, owner, &record.name);
            }
        }
        !found.is_empty()
    }

    /// Spawns anywhere in `data`. A mob already known is not read again.
    pub(super) fn seek_inner_4036(&mut self, data: &[u8]) {
        for (at, opcode, id) in embedded_spawns(data) {
            if opcode == PLAYER_SPAWN || opcode == PLAYER_SPAWN_OLD {
                self.read_spawn_name(data, at + 2);
            } else if !self.data_storage.is_mob(folded(id)) {
                self.read_pet_spawn_at(data, at + 2);
            }
        }
    }

    /// Reads the spawn whose entity id starts at `packet[offset_after_opcode]`:
    /// marks the id as a new entity, stores a mob's code and HP, and links a
    /// summon to its owner. True when it linked.
    pub(super) fn read_pet_spawn_at(&mut self, packet: &[u8], offset_after_opcode: usize) -> bool {
        let Some(spawn) = self.read_spawn_facts(packet, offset_after_opcode) else {
            return false;
        };
        // A monster's own summon can carry a player's name: the one it hits.
        if spawn.code.is_some_and(|code| self.npc_lookup.is_no_players_summon(code)) {
            return false;
        }
        let Some((owner, source)) = self.find_spawn_owner(packet, &spawn) else {
            return false;
        };
        // An owner read out of a field is a real entity; only a pet's own
        // fields may then give it the pet's inline name.
        if matches!(source, OwnerSource::Block | OwnerSource::Caster) {
            self.data_storage.remember_small_id(owner);
        }
        self.data_storage.confirm_pet(spawn.id, owner);
        let may_name = match source {
            OwnerSource::Block => true,
            OwnerSource::Caster => spawn.kind == PET,
            OwnerSource::Name | OwnerSource::Endless => false,
        };
        if may_name {
            self.name_owner_from_spawn(owner, &spawn);
        }
        true
    }

    /// The id marks, then what a mob's fields hold. `None` when the record
    /// stops before its kind and the two bytes after it.
    fn read_spawn_facts(&self, packet: &[u8], at: usize) -> Option<SpawnFacts> {
        let (id, kind_at) = spawn_head(packet, at)?;
        // A reused id is a new entity: its old marks go before anything else.
        self.data_storage.remember_pet_spawn(id);
        self.data_storage.remember_small_id(id);
        if packet.len() < kind_at + 3 {
            return None;
        }
        let code = self.store_mob_fields(packet, id, kind_at);
        Some(SpawnFacts { id, kind: packet[kind_at], kind_at, name: spawn_name_at(packet, kind_at), code })
    }

    /// The NPC code and max HP, stored for the id. Returns the code.
    fn store_mob_fields(&self, packet: &[u8], id: i32, kind_at: usize) -> Option<i32> {
        let marker = npc_code_marker(packet, kind_at)?;
        let code = i32::from_le_bytes([packet[marker - 3], packet[marker - 2], packet[marker - 1], 0]);
        let storage = &self.data_storage;
        storage.append_mob(id, code);
        if self.npc_lookup.is_boss(code) {
            storage.register_boss(id);
        }
        if self.npc_lookup.npc_is_dummy(code) {
            storage.flag_dummy(id);
        }
        if let Some(max) = max_hp_after(packet, marker + 3) {
            storage.remember_enemy_hp(id, max);
        }
        Some(code)
    }

    /// The spawn's owner, tried source by source, and where it was found.
    fn find_spawn_owner(&self, packet: &[u8], spawn: &SpawnFacts) -> Option<(i32, OwnerSource)> {
        let storage = &self.data_storage;
        let k = spawn.kind_at;
        // Only pets (kind 0x5F) carry an owner block.
        if spawn.kind == PET {
            let from = spawn.name.as_ref().map_or(k + 5, |(_, field)| field.end);
            if let Some(owner) = legion_block_owner(packet, from, spawn.id) {
                return Some((owner, OwnerSource::Block));
            }
        }
        // The inline name: a summon's owner. A monster effect's name is the
        // player it hits, so only a player's own effect counts.
        let by_name_ok = spawn.kind != MOB_EFFECT || spawn.code.is_some_and(|code| self.npc_lookup.is_players_summon(code));
        if by_name_ok
            && let Some((name, _)) = &spawn.name
            && let Some(named) = storage.id_for_name(name)
            && named != spawn.id
        {
            return Some((named, OwnerSource::Name));
        }
        // 0x1D and 0x5D: other players' spirits, which mostly carry no code.
        if matches!(spawn.kind, PET | SPIRIT | 0x1D | 0x5D)
            && let Some(caster) = own_endless_caster(packet, k)
            && caster != spawn.id
        {
            return Some((caster, OwnerSource::Endless));
        }
        if matches!(spawn.kind, PET | SPIRIT)
            && let Some(caster) = ground_spell_caster(packet, k)
            && caster != spawn.id
            && !storage.is_mob(caster)
        {
            return Some((caster, OwnerSource::Caster));
        }
        None
    }

    /// A pet's spawn carries its owner's name: give it to an owner that has
    /// none, through the gated bind.
    fn name_owner_from_spawn(&self, owner: i32, spawn: &SpawnFacts) {
        let Some((name, _)) = &spawn.name else {
            return;
        };
        let storage = &self.data_storage;
        if !storage.knows_name(owner) && !storage.is_mob(owner) {
            storage.append_nickname(owner, name);
        }
    }

    /// A monster's skill effect (kind 0x1C) ends with the monster it belongs
    /// to, whose damage its hits are. Read only at a frame's front: the parent
    /// sits in the frame's last 10 bytes, and an embedded spawn has no known end.
    fn link_effect_to_parent(&self, frame: &[u8], at: usize) {
        let Some((id, kind_at)) = spawn_head(frame, at) else {
            return;
        };
        if frame.get(kind_at) != Some(&MOB_EFFECT) || frame.len() < kind_at + 11 {
            return;
        }
        let tail = &frame[frame.len() - 10..];
        if tail[4] == 0 && tail[6..] == [0, 0, 0, 0] {
            let parent = le_u32_at(tail, 0) as i32;
            self.data_storage.note_effect_parent(id, parent);
        }
    }
}

/// Where `00 <00|40> 02` closes the NPC code, searched over the 60 bytes from
/// the kind on. A marker too close to the kind leaves no room for a code.
fn npc_code_marker(packet: &[u8], kind_at: usize) -> Option<usize> {
    let stop = packet.len().saturating_sub(2).min(kind_at + 60);
    let at = (kind_at..stop).find(|&s| packet[s] == 0 && matches!(packet[s + 1], 0x00 | 0x40) && packet[s + 2] == 0x02)?;
    (at >= kind_at + 3).then_some(at)
}

/// `01 <hp> <max hp>` after the 18 bytes of the entity's place (three
/// floats, its heading, two more bytes), where nearly every spawn has it.
/// An `01` there is the HP: its pair, or no max when the pair does not read
/// (no HP, or past 31 bits, whose low 32 bits would be a smaller, wrong
/// max). Else the first pair within 64 bytes of `from` past the place; with
/// none, the live HP records give the max.
///
/// A pair inside the place never counts: its floats can read as one (a
/// world boss's x as a small HP far below its own), and a wrong max too
/// large never comes down.
fn max_hp_after(packet: &[u8], from: usize) -> Option<i32> {
    const PLACE: usize = 18;
    let at = from + PLACE;
    let stop = packet.len().saturating_sub(2).min(from + 64);
    let pair_at = |h: usize| {
        if packet.get(h) != Some(&0x01) {
            return None;
        }
        let now = varint_at(packet, h + 1);
        if now.length < 0 || now.value <= 0 || past_i32(packet, h + 1, now) {
            return None;
        }
        let max_at = h + 1 + now.length as usize;
        let max = varint_at(packet, max_at);
        (max.length > 0 && max.value >= now.value && !past_i32(packet, max_at, max)).then_some(max.value)
    };
    if packet.get(at) == Some(&0x01) {
        return pair_at(at);
    }
    let found = (at..stop).find_map(pair_at);
    tracing::debug!(?found, "spawn: no HP after the place");
    found
}

/// Whether the varint read at `packet[at..]` held more than 31 bits.
fn past_i32(packet: &[u8], at: usize, read: VarintRead) -> bool {
    read.length == 5 && packet.get(at + 4).is_some_and(|&b| b > 0x07)
}

/// A pet's owner block: `06 <owner u32> <legion u32> 00 00 <server u16> <len>
/// <legion name>`, searched from `from` to the end of `packet`.
fn legion_block_owner(packet: &[u8], from: usize, id: i32) -> Option<i32> {
    (from.max(1)..)
        .take_while(|&i| i + 13 <= packet.len())
        .filter(|&i| packet[i - 1] == 0x06)
        .find_map(|i| {
            let owner = i32::try_from(le_u32_at(packet, i)).ok()?;
            let len = packet[i + 12] as usize;
            let fits = RECORD_IDS.contains(&owner)
                && owner != id
                && u16_le(packet, i + 8) == 0
                && (1..=9_999).contains(&u16_le(packet, i + 10))
                && len <= 40
                && i + 13 + len <= packet.len();
            (fits && std::str::from_utf8(&packet[i + 13..i + 13 + len]).is_ok()).then_some(owner)
        })
}

/// The caster of the first never-ending abnormal within 120 bytes of the
/// kind. A spirit's own buff names its owner; a mob's names the mob.
fn own_endless_caster(packet: &[u8], kind_at: usize) -> Option<i32> {
    let last = packet.len().checked_sub(8)?.min(kind_at + 120);
    (kind_at..=last).find_map(|i| {
        if packet[i..i + 8] != NEVER_ENDS {
            return None;
        }
        let caster = varint_at(packet, i + 8);
        (caster.length > 0 && RECORD_IDS.contains(&caster.value)).then_some(caster.value)
    })
}

/// A ground spell's caster: the u32 after the first `07 02 06` or `07 02 01`
/// within 240 bytes of the kind.
fn ground_spell_caster(packet: &[u8], kind_at: usize) -> Option<i32> {
    let window = packet.get(kind_at..packet.len().min(kind_at + 240))?;
    let marker = window.windows(3).position(|w| matches!(w, [0x07, 0x02, 0x06 | 0x01]))?;
    let at = kind_at + marker + 3;
    if at + 4 > packet.len() {
        return None;
    }
    let caster = i32::try_from(le_u32_at(packet, at)).ok()?;
    SCANNED_IDS.contains(&caster).then_some(caster)
}

/// The u16 LE at `b[at]`.
fn u16_le(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

/// `<len> <name>` with the length byte at `data[len_at]`: an exact name, and
/// where its bytes sit.
fn exact_name_after(data: &[u8], len_at: usize) -> Option<(String, Range<usize>)> {
    let len = *data.get(len_at)? as usize;
    if !NAME_LEN_RANGE.contains(&len) {
        return None;
    }
    let field = len_at + 1..len_at + 1 + len;
    Some((clean_name_field(data.get(field.clone())?)?, field))
}

/// A `04 8D` record at the front of `packet`: (first id, second id, the name
/// field as (start, length)).
pub(super) fn ownership_at_front(packet: &[u8]) -> Option<(i32, i32, Option<(usize, usize)>)> {
    let mut at = usize::try_from(varint_at(packet, 0).length).ok()?;
    if packet.get(at..at + 2)? != SUMMON_OWNERSHIP {
        return None;
    }
    at += 2;
    let first = next_varint(packet, &mut at).filter(|&v| v >= 100)?;
    // The skill id, a u32.
    at += 4;
    if at > packet.len() {
        return None;
    }
    let second = next_varint(packet, &mut at).filter(|&v| v >= 100 && v != first)?;
    // The server id, read as a varint: a server whose low byte is under 0x80
    // takes one byte here, and its high byte then reads as the length.
    let _ = next_varint(packet, &mut at);
    let field = packet.get(at).and_then(|&len| {
        let len = len as usize;
        ((1..=36).contains(&len) && at + 1 + len <= packet.len()).then_some((at + 1, len))
    });
    Some((first, second, field))
}

/// Every `04 8D` record in `data` that names its second id's server and
/// name, in the order found.
pub(super) fn ownership_records(data: &[u8]) -> Vec<Ownership> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = index_of_bytes(data, from, &SUMMON_OWNERSHIP) {
        match ownership_record_at(data, at) {
            Some(record) => {
                from = record.field.end;
                found.push(record);
            }
            None => from = at + 2,
        }
    }
    found
}

/// The record whose opcode sits at `data[at]`. The second id is found back
/// from the server id, which lies within 128 bytes of the skill id.
fn ownership_record_at(data: &[u8], at: usize) -> Option<Ownership> {
    let mut lead = at + 2;
    let summon_id = next_varint(data, &mut lead).filter(|v| SCANNED_IDS.contains(v))?;
    // The skill id, a u32.
    lead += 4;
    // A despawn's record is zeros after the skill id.
    if *data.get(lead)? == 0 {
        return None;
    }
    let stop = data.len().saturating_sub(2).min(lead + 128);
    (lead + 1..stop).find_map(|j| {
        let server_id = u16_le(data, j);
        if !(1000..=2999).contains(&server_id) {
            return None;
        }
        let owner_id = varint_before(data, j, lead, 100..=99_999).filter(|&b| b != summon_id)?;
        let (name, field) = exact_name_after(data, j + 2)?;
        Some(Ownership { summon_id, owner_id, server_id, name, field })
    })
}

/// Spawn and player spawn opcodes anywhere in `data`: (where the opcode
/// starts, the opcode, the entity id as read). One right after a `00` is
/// taken for stray bytes.
pub(crate) fn embedded_spawns(data: &[u8]) -> Vec<(usize, [u8; 2], i32)> {
    let mut found = Vec::new();
    let mut i = 0;
    while i + 5 < data.len() {
        let opcode = [data[i], data[i + 1]];
        if ![SPAWN, SPAWN_OLD, PLAYER_SPAWN, PLAYER_SPAWN_OLD].contains(&opcode) {
            i += 1;
            continue;
        }
        let at = i;
        i += 2;
        if at > 0 && data[at - 1] == 0 {
            continue;
        }
        if let Some(id) = next_varint(data, &mut i).filter(|v| SCANNED_IDS.contains(v)) {
            found.push((at, opcode, id));
        }
    }
    found
}

/// The name a spawn carries, `offset` being its kind byte: a gate byte with
/// bit 0 set, 4 or else 2 bytes on, then `<len> <name>`. Gives the name and
/// where its bytes sit.
pub(super) fn spawn_name_at(packet: &[u8], offset: usize) -> Option<(String, Range<usize>)> {
    [offset + 4, offset + 2].into_iter().find_map(|gate| {
        if packet.get(gate)? & 0x01 == 0 {
            return None;
        }
        exact_name_after(packet, gate + 1)
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::built::{self, Abnormal, Spawn, F};
    use crate::combat::data_storage::{DataStorage, UNATTRIBUTED_ID};
    use crate::entity::damage_packet::ParsedDamagePacket;
    use crate::entity::taken::{TakenHit, TakenKind};
    use crate::i18n::lookup::{NpcLookup, SkillLookup};

    const TOTEM_TABLE: &str = r#"{"2920063":{"name":"Totem","isBoss":false}}"#;

    /// A storage and a processor over it with this NPC table, time at 1,000.
    fn meter(npcs: &str) -> (Arc<DataStorage>, PacketDecoder) {
        let storage: Arc<DataStorage> = DataStorage::new().into();
        let table = NpcLookup::new();
        table.load_from_json(npcs);
        let mut p = PacketDecoder::new(Arc::clone(&storage), SkillLookup::new().into(), table.into());
        p.pin_clock(Some(1_000));
        (storage, p)
    }

    fn owner_of(storage: &DataStorage, id: i32) -> Option<i32> {
        storage.pet_owners().get(&id).copied().filter(|&o| o != UNATTRIBUTED_ID)
    }

    /// `id` hits entity 900, so the gated name bind takes it for real.
    fn fought(storage: &DataStorage, id: i32) {
        let mut hit = ParsedDamagePacket::new();
        hit.set_actor_id(id);
        hit.set_target_id(900);
        hit.set_skill_code(11_010_000);
        hit.set_damage(100);
        storage.append_damage(hit);
    }

    /// `41 36 <id> <rest>`, unframed.
    fn spawn_bytes(id: u32, rest: &[u8]) -> Vec<u8> {
        let mut out = SPAWN.to_vec();
        out.extend(built::varint(id));
        out.extend_from_slice(rest);
        out
    }

    /// `41 36` and a built spawn's fields, unframed.
    fn opened(spawn: &Spawn) -> Vec<u8> {
        let mut out = SPAWN.to_vec();
        out.extend(spawn.fields());
        out
    }

    /// An owner block on server 1001 with an empty legion name.
    fn plain_legion(owner: u32) -> Vec<u8> {
        built::owner_block(owner, 0, 1_001, 0, &[])
    }

    /// A pet's spawn with no name: `5F 00 00 00 00`, then `tail`.
    fn pet(id: u32, tail: &[u8]) -> Vec<u8> {
        let mut rest = vec![0x5F, 0, 0, 0, 0];
        rest.extend_from_slice(tail);
        spawn_bytes(id, &rest)
    }

    fn read(p: &mut PacketDecoder, spawn: &[u8]) -> bool {
        p.read_pet_spawn_at(spawn, 2)
    }

    #[test]
    fn a_framed_spawn_returns_whether_it_linked() {
        let (storage, mut p) = meter("");
        assert!(p.read_pet_spawn(&built::frame(&pet(5_002, &plain_legion(1_500)))));
        assert_eq!(owner_of(&storage, 5_002), Some(1_500));
        let mob = Spawn { id: 56_001, kind: 0x0C, code: 2_920_548, hp: 100, ..Spawn::default() };
        assert!(!p.read_pet_spawn(&mob.packet()));
        assert_eq!(storage.enemy_code(56_001), Some(2_920_548));
        assert!(!p.read_pet_spawn(&[0x07, 0x23, 0x36, 0x01]));
    }

    #[test]
    fn a_framed_player_spawn_names_the_player_and_links_nothing() {
        let (storage, mut p) = meter("");
        assert!(!p.read_pet_spawn(&built::player_record(7_001, Some("Abcd"), &[0, 0, 0, 0])));
        assert_eq!(storage.stored_name(7_001).as_deref(), Some("Abcd"));
    }

    #[test]
    fn a_frame_length_that_does_not_read_is_skipped() {
        let (storage, mut p) = meter("");
        assert!(!p.read_pet_spawn(&[0x80]));
        assert!(storage.pet_spawn_ids().is_empty());
    }

    #[test]
    fn a_frame_cut_inside_its_opcode_is_skipped() {
        let (storage, mut p) = meter("");
        assert!(!p.read_pet_spawn(&[0x05, 0x41]));
        assert!(storage.pet_spawn_ids().is_empty());
    }

    #[test]
    fn ids_past_a_million_are_folded() {
        for (raw, kept) in [(1_048_577, 16_385), (1_000_001, 16_961), (1_000_000, 1_000_000)] {
            let (storage, mut p) = meter("");
            assert!(!read(&mut p, &spawn_bytes(raw, &[0x0C])));
            assert_eq!(storage.pet_spawn_ids(), std::collections::HashSet::from([kept]), "{raw}");
        }
        let (storage, mut p) = meter("");
        assert!(read(&mut p, &pet(1_048_577, &plain_legion(1_500))));
        assert_eq!(owner_of(&storage, 16_385), Some(1_500));
    }

    #[test]
    fn a_spawn_whose_id_does_not_read_marks_nothing() {
        let (storage, mut p) = meter("");
        assert!(!p.read_pet_spawn_at(&[0x41, 0x36, 0x80], 2));
        assert!(storage.pet_spawn_ids().is_empty());
    }

    #[test]
    fn every_readable_spawn_marks_its_id_first() {
        let (storage, mut p) = meter("");
        assert!(!read(&mut p, &[0x41, 0x36, 0xE8, 0x07]));
        assert!(storage.pet_spawn_ids().contains(&1_000));
        assert!(!storage.looks_like_entity(50));
        assert!(!read(&mut p, &spawn_bytes(50, &[0x0C, 0x00])));
        assert!(storage.looks_like_entity(50));
        // The spawn is a new entity under a reused id: the old link goes.
        storage.confirm_pet(5_002, 1_500);
        assert!(!read(&mut p, &spawn_bytes(5_002, &[0x0C, 0x00, 0x00])));
        assert!(!storage.is_pet(5_002));
    }

    #[test]
    fn a_spawn_name_sits_behind_a_gate_four_or_two_bytes_on() {
        let name = |bytes: &[u8]| spawn_name_at(bytes, 0);
        assert_eq!(name(b"\x5F\x00\x01\x04Abcd"), Some(("Abcd".to_string(), 4..8)));
        assert_eq!(name(b"\x5F\x00\x00\x00\x01\x04Abcd"), Some(("Abcd".to_string(), 6..10)));
        // Both gates read; the one four bytes on wins.
        assert_eq!(name(&[0x5F, 0x00, 0x01, 0x01, 0x41, 0x01, 0x42]), Some(("B".to_string(), 6..7)));
        let mut too_long = vec![0x5F, 0x00, 0x01, 0x31];
        too_long.extend([0x41; 49]);
        let none: [&[u8]; 5] = [b"\x5F\x00\x00\x04Abcd", &[0x5F, 0x00, 0x01, 0x00, 0x00], &too_long, b"\x5F\x00\x01\x05Ab", b"\x5F\x00\x01\x04Ab d"];
        for bytes in none {
            assert_eq!(name(bytes), None, "{bytes:02x?}");
        }
    }

    #[test]
    fn a_mob_spawn_gives_its_code_and_max_hp() {
        let (storage, mut p) = meter("");
        read(&mut p, &opened(&Spawn { id: 56_001, kind: 0x1C, code: 2_909_999, hp: 100, ..Spawn::default() }));
        assert_eq!((storage.enemy_code(56_001), storage.enemy_max_hp(56_001)), (Some(2_909_999), Some(100)));
        // A made-up code, 2,909,999 (`2F 67 2C`), and an empty place.
        let mut rest = vec![0x0C, 0, 0, 0x2F, 0x67, 0x2C, 0, 0, 0x02];
        rest.extend([0; 18]);
        rest.extend([0x01, 0x0A, 0x0A]);
        read(&mut p, &spawn_bytes(56_002, &rest));
        assert_eq!((storage.enemy_code(56_002), storage.enemy_max_hp(56_002)), (Some(2_909_999), Some(10)));
    }

    #[test]
    fn a_code_marker_right_after_the_kind_leaves_no_room_for_a_code() {
        let (storage, mut p) = meter("");
        read(&mut p, &spawn_bytes(56_003, &[0x0C, 0, 0, 0x02, 0x2F, 0x67, 0x2C, 0x00, 0x40, 0x02, 0x01, 0x0A, 0x0A]));
        assert_eq!(storage.enemy_code(56_003), None);
        assert!(!storage.is_mob(56_003));
    }

    #[test]
    fn the_code_marker_lies_within_sixty_bytes_of_the_kind() {
        for (zeros, code) in [(55, Some(2_909_999)), (56, None)] {
            let mut rest = vec![0x0C];
            rest.extend(vec![0; zeros]);
            rest.extend([0x2F, 0x67, 0x2C, 0x00, 0x40, 0x02, 0x01, 0x0A, 0x0A]);
            let (storage, mut p) = meter("");
            read(&mut p, &spawn_bytes(56_004, &rest));
            assert_eq!(storage.enemy_code(56_004), code, "{zeros} zeros");
        }
    }

    /// A spawn of `code`, its marker and an 18-byte place, then `after`.
    fn spawned(id: u32, code: [u8; 3], place: [u8; 18], after: &[u8]) -> Arc<DataStorage> {
        let mut rest = vec![0x0C, 0, 0];
        rest.extend(code);
        rest.extend([0x00, 0x40, 0x02]);
        rest.extend(place);
        rest.extend(after);
        let (storage, mut p) = meter("");
        read(&mut p, &spawn_bytes(id, &rest));
        storage
    }

    /// Code 2,909,999, little-endian.
    const CODE: [u8; 3] = [0x2F, 0x67, 0x2C];

    /// The max HP read from `spawned` with `CODE`.
    fn max_hp_of(id: u32, place: [u8; 18], after: &[u8]) -> Option<i32> {
        spawned(id, CODE, place, after).enemy_max_hp(id as i32)
    }

    #[test]
    fn with_no_hp_right_after_the_place_the_first_live_reading_past_it_counts() {
        // A zero HP, then an HP over its max, then a live one.
        assert_eq!(max_hp_of(56_005, [0; 18], &[0x00, 0x01, 0x00, 0x00, 0x01, 0x64, 0x32, 0x01, 0x0A, 0x0A]), Some(10));
    }

    #[test]
    fn a_pair_inside_the_place_never_counts() {
        let mut place = [0; 18];
        place[..3].copy_from_slice(&[0x01, 0x0A, 0x0A]);
        assert_eq!(max_hp_of(56_006, place, &[]), None);
    }

    #[test]
    fn hp_right_after_the_place_that_does_not_read_gives_no_max() {
        // No HP there, though a pair further on reads.
        assert_eq!(max_hp_of(56_009, [0; 18], &[0x01, 0x00, 0x00, 0x01, 0x0A, 0x0A]), None);
    }

    /// A world boss's spawn whose place's x starts with `01`, which reads as
    /// a small HP pair; its own HP follows the place.
    #[test]
    fn max_hp_follows_the_place_before_any_pair_inside_it() {
        let place = [0x01, 0xA9, 0x19, 0xC7, 0x4C, 0x0E, 0x98, 0x46, 0x00, 0x9C, 0x1D, 0x46, 0xE9, 0x9A, 0xE2, 0x42, 0x81, 0xC9];
        let hp = [0x01, 0xD9, 0xE0, 0xAB, 0x94, 0x02, 0xE0, 0x9F, 0x96, 0xBC, 0x03, 0x64];
        let storage = spawned(23_430, [0x99, 0xAC, 0x27], place, &hp);
        assert_eq!(storage.enemy_code(23_430), Some(2_600_089));
        assert_eq!(storage.enemy_max_hp(23_430), Some(931_500_000));
    }

    /// HP past 31 bits gives no max: its low 32 bits would be a smaller,
    /// wrong one. 2,000,000,000 still fits.
    #[test]
    fn a_max_hp_past_31_bits_gives_none() {
        let wide = |mut v: u64| {
            let mut out = Vec::new();
            while v >= 0x80 {
                out.push(v as u8 | 0x80);
                v >>= 7;
            }
            out.push(v as u8);
            out
        };
        let max_after = |id: u32, hp: u64| {
            let storage = spawned(id, CODE, [0; 18], &[&[0x01][..], &wide(hp), &wide(hp)].concat());
            (storage.is_mob(id as i32), storage.enemy_max_hp(id as i32))
        };
        assert_eq!(max_after(56_007, 2_000_000_000), (true, Some(2_000_000_000)));
        assert_eq!(max_after(56_008, 5_000_000_000), (true, None));
    }

    #[test]
    fn a_boss_code_makes_the_id_a_boss() {
        let (storage, mut p) = meter(r#"{"2310218":{"name":"Boss","isBoss":true}}"#);
        read(&mut p, &opened(&Spawn { id: 56_006, kind: 0x04, code: 2_310_218, hp: 900, ..Spawn::default() }));
        assert!(storage.is_boss(56_006));
    }

    #[test]
    fn a_monsters_summon_links_to_nobody() {
        let (storage, mut p) = meter(TOTEM_TABLE);
        storage.name_entity(2_500, "Abcd");
        let effect = Spawn { id: 51_001, kind: 0x1C, name: Some("Abcd"), code: 2_920_063, hp: 3, ..Spawn::default() };
        assert!(!read(&mut p, &opened(&effect)));
        assert_eq!((owner_of(&storage, 51_001), storage.enemy_code(51_001)), (None, Some(2_920_063)));
        let pet_spawn = Spawn { id: 51_002, kind: 0x5F, code: 2_920_063, hp: 3, end: plain_legion(1_500), ..Spawn::default() };
        assert!(!read(&mut p, &opened(&pet_spawn)));
        assert_eq!(owner_of(&storage, 51_002), None);
    }

    #[test]
    fn only_the_code_in_the_spawn_itself_refuses_a_link() {
        let (storage, mut p) = meter(TOTEM_TABLE);
        storage.append_mob(5_002, 2_920_063);
        assert!(read(&mut p, &pet(5_002, &plain_legion(1_500))));
        assert_eq!(owner_of(&storage, 5_002), Some(1_500));
    }

    #[test]
    fn a_pets_owner_block_must_hold_together() {
        let mut after_zeros = vec![0; 20];
        after_zeros.extend(plain_legion(1_500));
        // `5F 00 00` and the block: its 06 sits before the cursor.
        let mut too_early = vec![0x5F, 0, 0];
        too_early.extend(plain_legion(1_500));
        let cases: [(&str, Vec<u8>, Option<i32>); 14] = [
            ("plain", pet(5_002, &plain_legion(1_500)), Some(1_500)),
            ("20 zero bytes first", pet(5_002, &after_zeros), Some(1_500)),
            ("pad 1", pet(5_002, &built::owner_block(1_500, 1, 1_001, 0, &[])), None),
            ("server 0", pet(5_002, &built::owner_block(1_500, 0, 0, 0, &[])), None),
            ("server 9,999", pet(5_002, &built::owner_block(1_500, 0, 9_999, 0, &[])), Some(1_500)),
            ("server 10,000", pet(5_002, &built::owner_block(1_500, 0, 10_000, 0, &[])), None),
            ("owner 0", pet(5_002, &plain_legion(0)), None),
            ("owner 9,999,999", pet(5_002, &plain_legion(9_999_999)), Some(9_999_999)),
            ("owner 10,000,000", pet(5_002, &plain_legion(10_000_000)), None),
            ("owner = the pet", pet(5_002, &plain_legion(5_002)), None),
            ("40-byte name", pet(5_002, &built::owner_block(1_500, 0, 1_001, 40, &[b'a'; 40])), Some(1_500)),
            ("name not utf-8", pet(5_002, &built::owner_block(1_500, 0, 1_001, 3, &[0xFF; 3])), None),
            ("no 06", pet(5_002, &plain_legion(1_500)[1..]), None),
            ("before the cursor", spawn_bytes(5_002, &too_early), None),
        ];
        for (what, spawn, owner) in cases {
            let (storage, mut p) = meter("");
            assert_eq!(read(&mut p, &spawn), owner.is_some(), "{what}");
            assert_eq!(owner_of(&storage, 5_002), owner, "{what}");
        }
    }

    #[test]
    fn an_owner_block_cut_short_gives_no_owner() {
        let (storage, mut p) = meter("");
        // The 06 and only 12 bytes after it.
        assert!(!read(&mut p, &pet(5_002, &plain_legion(1_500)[..13])));
        assert_eq!(owner_of(&storage, 5_002), None);
    }

    #[test]
    fn a_legion_name_too_long_or_past_the_end_gives_no_owner() {
        for tail in [built::owner_block(1_500, 0, 1_001, 41, &[b'a'; 41]), built::owner_block(1_500, 0, 1_001, 5, b"abc")] {
            let (storage, mut p) = meter("");
            assert!(!read(&mut p, &pet(5_002, &tail)));
            assert_eq!(owner_of(&storage, 5_002), None);
        }
    }

    /// `41 36 <120> <kind> 00 [00 00] 01 <name> [<code u24> 00 40 02]` and
    /// 24 zero bytes.
    fn named(kind: u8, wide: bool, name: &str, code: Option<u32>) -> Vec<u8> {
        let mut rest = vec![kind, 0];
        if wide {
            rest.extend([0, 0]);
        }
        rest.extend(built::fields(&[F::U8(0x01), F::Str(name)]));
        if let Some(code) = code {
            rest.extend(&code.to_le_bytes()[..3]);
            rest.extend([0x00, 0x40, 0x02]);
        }
        rest.extend([0; 24]);
        spawn_bytes(120, &rest)
    }

    #[test]
    fn a_spawns_inline_name_links_to_the_player_holding_it() {
        let cases = [
            ("pet, gate 2 on", named(0x5F, false, "Abcd", None), true),
            ("pet, gate 4 on", named(0x5F, true, "Abcd", None), true),
            ("nobody's name", named(0x5F, false, "Efgh", None), false),
            ("effect, no code", named(0x1C, false, "Abcd", None), false),
            ("player spirit as effect", named(0x1C, false, "Abcd", Some(2_920_149)), true),
            ("kind 0x0C", named(0x0C, false, "Abcd", None), true),
            ("kind 0x2F", named(0x2F, false, "Abcd", None), true),
            ("13 letters", named(0x5F, false, "Abcdefghijklm", None), false),
        ];
        for (what, spawn, links) in cases {
            let (storage, mut p) = meter("");
            storage.name_entity(4_200, "Abcd");
            assert_eq!(read(&mut p, &spawn), links, "{what}");
            assert_eq!(owner_of(&storage, 120), links.then_some(4_200), "{what}");
        }
    }

    #[test]
    fn a_spawn_naming_itself_links_nothing() {
        let (storage, mut p) = meter("");
        storage.name_entity(120, "Abcd");
        assert!(!read(&mut p, &named(0x5F, false, "Abcd", None)));
        assert_eq!(owner_of(&storage, 120), None);
    }

    #[test]
    fn an_effect_whose_code_is_no_players_is_not_the_player_it_names() {
        let (storage, mut p) = meter(TOTEM_TABLE);
        storage.name_entity(4_200, "Abcd");
        assert!(!read(&mut p, &named(0x1C, false, "Abcd", Some(2_920_999))));
        assert_eq!(owner_of(&storage, 120), None);
    }

    /// A Wind Spirit's spawn (47400) with these abnormals.
    fn wind_spirit(kind: u8, abnormals: Vec<Abnormal>, filler: usize) -> Vec<u8> {
        opened(&Spawn { id: 47_400, kind, code: 2_920_149, hp: 500, filler, abnormals, ..Spawn::default() })
    }

    fn endless(casters: &[u32]) -> Vec<Abnormal> {
        casters.iter().map(|&c| Abnormal::endless(c)).collect()
    }

    #[test]
    fn a_spirits_own_endless_buff_names_its_owner() {
        for (kind, owner) in [(0x1F, Some(6_400)), (0x1D, Some(6_400)), (0x5D, Some(6_400)), (0x5F, Some(6_400)), (0x2F, None), (0x0C, None)] {
            let (storage, mut p) = meter("");
            assert_eq!(read(&mut p, &wind_spirit(kind, endless(&[6_400]), 0)), owner.is_some(), "kind {kind:02x}");
            assert_eq!(owner_of(&storage, 47_400), owner, "kind {kind:02x}");
        }
    }

    #[test]
    fn an_endless_buff_cast_by_nobody_is_passed_over() {
        let (storage, mut p) = meter("");
        assert!(read(&mut p, &wind_spirit(0x1F, endless(&[0, 6_400]), 0)));
        assert_eq!(owner_of(&storage, 47_400), Some(6_400));
    }

    #[test]
    fn only_the_first_endless_buff_with_a_caster_counts() {
        let (storage, mut p) = meter("");
        assert!(!read(&mut p, &wind_spirit(0x1F, endless(&[47_400, 6_400]), 0)));
        assert_eq!(owner_of(&storage, 47_400), None);
        assert!(read(&mut p, &wind_spirit(0x1F, endless(&[50]), 0)));
        assert_eq!(owner_of(&storage, 47_400), Some(50));
    }

    #[test]
    fn the_endless_buff_lies_within_120_bytes_of_the_kind() {
        for (filler, links) in [(60, true), (100, false)] {
            let (_, mut p) = meter("");
            assert_eq!(read(&mut p, &wind_spirit(0x1F, endless(&[6_400]), filler)), links, "filler {filler}");
        }
    }

    /// A ground spell's spawn whose own buff names itself, then `end`.
    fn ground_spell(id: u32, kind: u8, filler: usize, end: Vec<u8>) -> Vec<u8> {
        opened(&Spawn { id, kind, code: 2_920_011, hp: 4_777, filler, abnormals: endless(&[id]), end, ..Spawn::default() })
    }

    #[test]
    fn a_ground_spell_links_to_the_caster_after_its_position() {
        let mut twice = built::spawn_caster(0x06, 21_007);
        twice.extend(built::spawn_caster(0x06, 2_600));
        let cases = [
            (21_001, 0x1F, 40, built::spawn_caster(0x06, 2_600), Some(2_600)),
            (21_002, 0x1F, 40, built::spawn_caster(0x06, 21_002), None),
            (21_004, 0x1F, 40, built::spawn_caster(0x01, 191), Some(191)),
            (21_005, 0x1F, 40, built::spawn_caster(0x06, 99), None),
            (21_006, 0x1F, 300, built::spawn_caster(0x06, 2_600), None),
            (21_007, 0x1F, 40, twice, None),
            (21_008, 0x1D, 40, built::spawn_caster(0x06, 2_600), None),
        ];
        for (id, kind, filler, end, owner) in cases {
            let (storage, mut p) = meter("");
            assert_eq!(read(&mut p, &ground_spell(id, kind, filler, end)), owner.is_some(), "{id}");
            assert_eq!(owner_of(&storage, id as i32), owner, "{id}");
        }
    }

    #[test]
    fn a_mob_is_no_ground_spells_caster() {
        let (storage, mut p) = meter("");
        storage.append_mob(30_000, 1);
        assert!(!read(&mut p, &ground_spell(21_003, 0x1F, 40, built::spawn_caster(0x06, 30_000))));
        assert_eq!(owner_of(&storage, 21_003), None);
    }

    /// Pet 5002 named "Abcd", then 1500's owner block.
    fn named_pet() -> Vec<u8> {
        let mut rest = b"\x5F\x00\x01\x04Abcd".to_vec();
        rest.extend(plain_legion(1_500));
        spawn_bytes(5_002, &rest)
    }

    #[test]
    fn a_pets_name_goes_to_its_unnamed_owner() {
        let (storage, mut p) = meter("");
        fought(&storage, 1_500);
        assert!(read(&mut p, &named_pet()));
        assert_eq!((owner_of(&storage, 5_002), storage.stored_name(1_500).as_deref()), (Some(1_500), Some("Abcd")));
    }

    #[test]
    fn a_mob_owner_is_not_named_from_a_pet() {
        let (storage, mut p) = meter("");
        fought(&storage, 1_500);
        storage.append_mob(1_500, 2_300_475);
        assert!(read(&mut p, &named_pet()));
        assert_eq!((owner_of(&storage, 5_002), storage.stored_name(1_500)), (Some(1_500), None));
    }

    #[test]
    fn a_name_the_game_gave_the_owner_is_kept() {
        let (storage, mut p) = meter("");
        storage.name_entity(1_500, "Wxyz");
        assert!(read(&mut p, &named_pet()));
        assert_eq!(storage.stored_name(1_500).as_deref(), Some("Wxyz"));
    }

    #[test]
    fn only_a_pet_found_by_its_casters_field_names_the_caster() {
        for (kind, name) in [(0x5F, Some("Efgh")), (0x1F, None)] {
            let (storage, mut p) = meter("");
            fought(&storage, 1_600);
            let spawn = Spawn { id: 21_009, kind, name: Some("Efgh"), code: 2_920_011, hp: 100, end: built::spawn_caster(0x06, 1_600), ..Spawn::default() };
            assert!(read(&mut p, &opened(&spawn)), "kind {kind:02x}");
            assert_eq!(owner_of(&storage, 21_009), Some(1_600), "kind {kind:02x}");
            assert_eq!(storage.stored_name(1_600).as_deref(), name, "kind {kind:02x}");
        }
    }

    #[test]
    fn an_owner_found_by_the_endless_buff_is_not_named() {
        let (storage, mut p) = meter("");
        fought(&storage, 6_400);
        let spawn = Spawn { id: 47_401, kind: 0x5F, name: Some("Abcd"), code: 2_920_149, hp: 500, abnormals: endless(&[6_400]), ..Spawn::default() };
        assert!(read(&mut p, &opened(&spawn)));
        assert_eq!((owner_of(&storage, 47_401), storage.stored_name(6_400)), (Some(6_400), None));
    }

    /// The source of a hit by effect 51001 on player 2500, after `stream`.
    fn source_after(kind: u8, inside_another_record: bool) -> i32 {
        let (storage, mut p) = meter("");
        storage.append_mob(30_500, 2_300_475);
        storage.name_entity(2_500, "Abcd");
        let effect = Spawn { id: 51_001, kind, code: 2_920_048, hp: 100, end: built::effect_parent(30_500), ..Spawn::default() };
        let stream = if inside_another_record {
            let mut body = vec![0x2A, 0x37, 0x10];
            body.extend(opened(&effect));
            built::frame(&body)
        } else {
            effect.packet()
        };
        p.decode_buffer(&stream);
        storage.append_taken(TakenHit::plain(1_000, 2_500, 51_001, 1_800_510, 50, TakenKind::Hit));
        storage.taken_since(0).last().map(|t| t.source).unwrap_or_default()
    }

    #[test]
    fn a_monster_effects_hits_count_as_its_parents() {
        assert_eq!(source_after(0x1C, false), 30_500);
    }

    #[test]
    fn an_effect_parent_is_read_only_at_a_frames_front() {
        assert_eq!(source_after(0x1C, true), 51_001);
    }

    #[test]
    fn only_a_monster_effect_ends_with_its_parent() {
        assert_eq!(source_after(0x1F, false), 51_001);
    }

    #[test]
    fn spawn_opcodes_are_found_anywhere_but_after_a_zero() {
        let one = |at| vec![(at, SPAWN, 1_000)];
        assert_eq!(embedded_spawns(&[0x10, 0x41, 0x36, 0xE8, 0x07, 0, 0, 0]), one(1));
        assert_eq!(embedded_spawns(&[0x41, 0x36, 0xE8, 0x07, 0, 0, 0]), one(0));
        assert_eq!(embedded_spawns(&[0x01, 0x41, 0x36, 0xE8, 0x07, 0x00, 0x00]), one(1));
        assert_eq!(embedded_spawns(&[0x41, 0x36, 0x80, 0x80, 0x80, 0x80, 0x80, 0x41, 0x36, 0xE8, 0x07, 0, 0, 0]), one(7));
        let none: [&[u8]; 4] = [
            &[0x00, 0x41, 0x36, 0xE8, 0x07, 0, 0, 0],
            // Id 99.
            &[0x41, 0x36, 0x63, 0, 0, 0, 0],
            // Too near the end.
            &[0x01, 0x41, 0x36, 0xE8, 0x07, 0x00],
            // The first id reads as 65; the second opcode is stepped over.
            &[0x41, 0x36, 0x41, 0x36, 0xE8, 0x07, 0, 0, 0],
        ];
        for bytes in none {
            assert!(embedded_spawns(bytes).is_empty(), "{bytes:02x?}");
        }
        let mut all = vec![0x01];
        for (opcode, id) in [(SPAWN_OLD, [0xE8, 0x07]), (PLAYER_SPAWN_OLD, [0xE9, 0x07]), (PLAYER_SPAWN, [0xEA, 0x07])] {
            all.extend(opcode);
            all.extend(id);
        }
        all.extend([0; 5]);
        assert_eq!(embedded_spawns(&all), vec![(1, SPAWN_OLD, 1_000), (5, PLAYER_SPAWN_OLD, 1_001), (9, PLAYER_SPAWN, 1_002)]);
    }

    #[test]
    fn an_embedded_pet_is_linked_unless_its_id_is_a_known_mob() {
        let mut region = vec![0x10];
        region.extend(pet(5_002, &plain_legion(1_500)));
        let (storage, mut p) = meter("");
        p.seek_inner_4036(&region);
        assert_eq!(owner_of(&storage, 5_002), Some(1_500));

        let (storage, mut p) = meter("");
        storage.append_mob(5_002, 1);
        p.seek_inner_4036(&region);
        assert_eq!(owner_of(&storage, 5_002), None);

        region[0] = 0x00;
        let (storage, mut p) = meter("");
        p.seek_inner_4036(&region);
        assert_eq!(owner_of(&storage, 5_002), None);
    }

    #[test]
    fn an_embedded_player_spawn_names_the_player() {
        let mut region = vec![0x10];
        region.extend(&built::player_record(7_001, Some("Abcd"), &[0, 0, 0, 0])[1..]);
        let (storage, mut p) = meter("");
        p.seek_inner_4036(&region);
        assert_eq!(storage.stored_name(7_001).as_deref(), Some("Abcd"));
    }

    /// `04 8D <a> <skill u32> <b> <rest>`, framed.
    fn kill(a: u32, b: u32, rest: &[F]) -> Vec<u8> {
        let mut list = vec![F::V(a), F::U32(14_340_230), F::V(b)];
        list.extend_from_slice(rest);
        built::packet(SUMMON_OWNERSHIP, &list)
    }

    #[test]
    fn a_front_kill_record_gives_both_ids_and_the_name_field() {
        assert_eq!(ownership_at_front(&kill(50_001, 1_600, &[F::U16(2_016), F::Str("Abcd")])), Some((50_001, 1_600, Some((15, 4)))));
        assert_eq!(ownership_at_front(&kill(50_001, 1_600, &[])), Some((50_001, 1_600, None)));
        assert_eq!(ownership_at_front(&kill(20_000_000, 1_600, &[])), Some((20_000_000, 1_600, None)));
        let bad = [
            kill(99, 1_600, &[]),
            kill(50_001, 99, &[]),
            kill(50_001, 50_001, &[]),
            built::packet(SUMMON_OWNERSHIP, &[F::V(50_001), F::Raw(&[1, 2, 3])]),
            built::packet([0x05, 0x8D], &[F::V(50_001), F::U32(14_340_230), F::V(1_600)]),
        ];
        for bytes in bad {
            assert_eq!(ownership_at_front(&bytes), None, "{bytes:02x?}");
        }
    }

    #[test]
    fn a_front_name_field_holds_up_to_36_bytes() {
        let (n36, n37) = ("a".repeat(36), "a".repeat(37));
        assert_eq!(ownership_at_front(&kill(50_001, 1_600, &[F::U16(2_016), F::Str(&n36)])), Some((50_001, 1_600, Some((15, 36)))));
        assert_eq!(ownership_at_front(&kill(50_001, 1_600, &[F::U16(2_016), F::Str(&n37)])), Some((50_001, 1_600, None)));
    }

    /// Kept as the meter has always read it: the server id goes as a varint,
    /// so server 1305 (`19 05`) takes one byte and `05` reads as the length.
    #[test]
    fn a_server_whose_low_byte_is_under_0x80_shifts_the_front_name_field() {
        assert_eq!(ownership_at_front(&kill(50_001, 1_600, &[F::U16(1_305), F::Str("Abcd")])), Some((50_001, 1_600, Some((14, 5)))));
    }

    #[test]
    fn a_front_kill_record_names_the_owner_it_holds() {
        let (storage, p) = meter("");
        fought(&storage, 1_600);
        assert!(p.read_pet_owner(&kill(50_001, 1_600, &[F::U16(2_016), F::Str("Abcd")])));
        assert_eq!(storage.stored_name(1_600).as_deref(), Some("Abcd"));
        assert!(p.read_pet_owner(&kill(50_002, 1_700, &[])));
        assert!(!p.read_pet_owner(&[0x07, 0x23, 0x36, 0x01]));
    }

    #[test]
    fn a_front_kill_record_moves_a_confirmed_summon_to_its_owner() {
        let (storage, p) = meter("");
        storage.confirm_pet(50_001, 9_000);
        assert!(p.read_pet_owner(&kill(50_001, 1_600, &[])));
        assert_eq!(owner_of(&storage, 50_001), Some(1_600));
    }

    /// `04 8D <50001> <skill u32> <rest>`, unframed.
    fn kill_inside(rest: &[F]) -> Vec<u8> {
        let mut list = vec![F::Raw(&SUMMON_OWNERSHIP), F::V(50_001), F::U32(0x00E9_2872)];
        list.extend_from_slice(rest);
        built::fields(&list)
    }

    /// A kill record naming `owner` on `server`, then a server name.
    fn kill_by(owner: u32, server: u16, name: &str) -> Vec<u8> {
        kill_inside(&[F::V(owner), F::U16(server), F::Str(name), F::Str("Server"), F::U32(1)])
    }

    fn records(data: &[u8]) -> Vec<(i32, i32, u16, String, Range<usize>)> {
        ownership_records(data).into_iter().map(|r| (r.summon_id, r.owner_id, r.server_id, r.name, r.field)).collect()
    }

    #[test]
    fn an_embedded_kill_record_names_the_owner_and_server() {
        let record = kill_by(1_600, 1_305, "Abcdef");
        assert_eq!(records(&record), vec![(50_001, 1_600, 1_305, "Abcdef".to_string(), 14..20)]);
        let mut garbled = record;
        garbled[13] = 0x07;
        assert!(records(&garbled).is_empty());
    }

    #[test]
    fn an_embedded_kill_record_needs_a_server_from_1000_to_2999() {
        for (server, found) in [(999, false), (1_000, true), (2_999, true), (3_000, false)] {
            assert_eq!(records(&kill_by(1_600, server, "Abcdef")).len(), usize::from(found), "server {server}");
        }
    }

    #[test]
    fn an_embedded_owner_is_read_back_from_the_server_id() {
        // 100,000 is past the range, so the reading falls back to its last
        // two bytes.
        for (owner, read) in [(99_999, Some(99_999)), (100_000, Some(781)), (50_001, None)] {
            let found = ownership_records(&kill_by(owner, 1_305, "Abcdef"));
            assert_eq!(found.first().map(|r| r.owner_id), read, "owner {owner}");
        }
    }

    #[test]
    fn a_despawns_zeros_after_the_skill_name_nobody() {
        let record = kill_inside(&[F::U8(0), F::V(1_600), F::U16(1_305), F::Str("Abcdef")]);
        assert!(records(&record).is_empty());
    }

    #[test]
    fn a_byte_between_the_skill_and_the_owner_is_stepped_over() {
        let record = kill_inside(&[F::U8(7), F::V(1_600), F::U16(1_305), F::Str("Abcdef")]);
        assert_eq!(ownership_records(&record).first().map(|r| r.owner_id), Some(1_600));
    }

    #[test]
    fn an_embedded_owner_name_must_be_a_whole_name() {
        assert!(records(&kill_by(1_600, 1_305, "Ab def")).is_empty());
    }

    #[test]
    fn the_server_id_lies_within_128_bytes_of_the_skill() {
        let record = kill_inside(&[F::Raw(&[0x01; 130]), F::V(1_600), F::U16(1_305), F::Str("Abcdef")]);
        assert!(records(&record).is_empty());
    }

    #[test]
    fn kill_records_back_to_back_are_both_found_in_order() {
        let mut data = kill_inside(&[F::V(1_600), F::U16(1_305), F::Str("Abcdef")]);
        let second = data.len() + 14;
        data.extend(kill_inside(&[F::V(1_700), F::U16(1_306), F::Str("Ghijk")]));
        let expected = vec![(50_001, 1_600, 1_305, "Abcdef".to_string(), 14..20), (50_001, 1_700, 1_306, "Ghijk".to_string(), second..second + 5)];
        assert_eq!(records(&data), expected);
    }

    #[test]
    fn an_embedded_kill_record_notes_the_owners_name_and_server() {
        let (storage, p) = meter("");
        fought(&storage, 1_600);
        let record = kill_by(1_600, 1_305, "Abcdef");
        assert!(p.seek_inner_048d(&record));
        assert_eq!(storage.player_servers().get("Abcdef"), Some(&1_305));
        assert_eq!(storage.stored_name(1_600).as_deref(), Some("Abcdef"));
        let mut garbled = record;
        garbled[13] = 0x07;
        assert!(!p.seek_inner_048d(&garbled));
    }

    #[test]
    fn an_embedded_kill_record_moves_a_confirmed_summon_to_its_owner() {
        let (storage, p) = meter("");
        storage.confirm_pet(50_001, 9_000);
        assert!(p.seek_inner_048d(&kill_by(1_600, 1_305, "Abcdef")));
        assert_eq!(owner_of(&storage, 50_001), Some(1_600));
    }
}
