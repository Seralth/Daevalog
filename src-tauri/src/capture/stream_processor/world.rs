//! Zone, map, death and HP records, and the party-scope record.

use super::StreamProcessor;
use crate::capture::opcodes::{DEATH, DEATH_OLD, HP_MP, MAP_LOAD, PARTY_SCOPE, ZONE_CHANGE};
use crate::capture::varint::{parse_u32_le, read_varint};

/// The `42 36` flag of an entity leaving the world, a spirit unsummoned among
/// others (197 of the 370 in a 2026-10-06 capture were linked spirits).
const DESPAWN_FLAG: i32 = 7;

impl StreamProcessor {
    // ===== PARTY SCOPE (06 38) =====

    /// `<len> 06 38 <entity_id varint> ...`: a record the server sends about
    /// you and your party only. What it carries is not decoded; who it is
    /// about is what identifies your loot (see `DataStorage::note_party_scope`).
    pub(super) fn parse_party_scope_packet(&self, packet: &[u8]) {
        let length_info = read_varint(packet, 0);
        if length_info.length <= 0 {
            return;
        }
        let offset = length_info.length as usize;
        if offset + 3 >= packet.len() || packet[offset..offset + 2] != PARTY_SCOPE {
            return;
        }
        let id = read_varint(packet, offset + 2);
        if id.length > 0 {
            self.data_storage.note_party_scope(id.value);
        }
    }

    // ===== ZONE CHANGE (23 36) =====

    /// Self/world teleport packet: `<len varint> 23 36 <entity_id varint = 0> <x><y><z> ...`.
    /// This fires once on entering a zone/instance (the local player is teleported in)
    /// and not during combat, so it drives a combat reset — the actual reset is gated
    /// by a lull + debounce in `note_zone_change`, so an in-combat teleport (a boss
    /// knockback/pull) never wipes an active fight. The June 2026 +1 opcode shift moved
    /// the spawn/death family but this position opcode (0x23) is unaffected.
    pub(super) fn parse_zone_change_packet(&self, packet: &[u8]) {
        let length_info = read_varint(packet, 0);
        if length_info.length < 0 {
            return;
        }
        let offset = length_info.length as usize;
        if offset + 2 >= packet.len() {
            return;
        }
        if packet[offset..offset + 2] != ZONE_CHANGE {
            return;
        }
        // entity id 0 == the local player being teleported (a zone load), as opposed
        // to another entity's routine position update.
        if packet[offset + 2] != 0x00 {
            return;
        }
        self.data_storage.note_zone_change();
    }

    // ===== MAP LOAD (21 36) =====

    /// `<len> 21 36 <u32 count> <u32 map id> ...`: sent on every zone load,
    /// naming the map from the game's Map table. A teleport inside an instance
    /// names the instance again; leaving names an open-world map. Seen on all
    /// 36 loads in the 2026-10-04 captures, each 52 bytes long.
    pub(super) fn parse_map_load_packet(&self, packet: &[u8]) {
        let length_info = read_varint(packet, 0);
        if length_info.length <= 0 {
            return;
        }
        let offset = length_info.length as usize;
        if offset + 10 > packet.len() || packet[offset..offset + 2] != MAP_LOAD {
            return;
        }
        let map_id = parse_u32_le(packet, offset + 6) as i32;
        self.data_storage.note_map_load(map_id);
    }

    // ===== DEATH PACKET (41 36) =====

    pub(super) fn parse_death_packet(&self, packet: &[u8]) {
        let length_info = read_varint(packet, 0);
        if length_info.length < 0 {
            return;
        }
        let offset = length_info.length as usize;
        if offset + 1 >= packet.len() {
            return;
        }
        // Death opcode. Pre-2026-06 it was 0x3641 ([0x41,0x36]); the June 2026
        // update shifted the 0x36 spawn/death family by +1, so it is now 0x3642
        // ([0x42,0x36]). Accept both — the flag==3 check below rejects anything
        // that isn't actually a combat death.
        if packet[offset..offset + 2] != DEATH && packet[offset..offset + 2] != DEATH_OLD {
            return;
        }
        let mut pos = offset + 2;

        let entity_info = read_varint(packet, pos);
        if entity_info.length <= 0 {
            return;
        }
        let entity_id = entity_info.value;
        pos += entity_info.length as usize;

        // Skip VarInt (always 0)
        let skip_info = read_varint(packet, pos);
        if skip_info.length <= 0 {
            return;
        }
        pos += skip_info.length as usize;

        // Death flag: 1 = zone-init (entity loaded dead), 3 = combat death,
        // 7 = gone from the world.
        let flag_info = read_varint(packet, pos);
        if flag_info.length <= 0 {
            return;
        }

        if flag_info.value == 3 {
            tracing::trace!("Death event: entity {} killed in combat", entity_id);
            self.data_storage.mark_entity_dead(entity_id);
        }
        if flag_info.value == DESPAWN_FLAG {
            self.data_storage.note_despawn(entity_id);
        }
    }

    // ===== HP/MP UPDATE =====

    pub(super) fn parse_hp_mp_update_packet(&self, packet: &[u8]) -> bool {
        let length_info = read_varint(packet, 0);
        if length_info.length < 0 {
            return false;
        }
        let offset = length_info.length as usize;
        if offset + 1 >= packet.len() {
            return false;
        }
        if packet[offset..offset + 2] != HP_MP {
            return false;
        }

        let mut pos = offset + 2;
        let actor_info = read_varint(packet, pos);
        if actor_info.length <= 0 || actor_info.value < 100 || actor_info.value > 9_999_999 {
            return false;
        }
        let actor_id = actor_info.value;
        pos += actor_info.length as usize;

        let hp_info = read_varint(packet, pos);
        if hp_info.length <= 0 {
            return false;
        }
        pos += hp_info.length as usize;

        let hp_max_info = read_varint(packet, pos);
        if hp_max_info.length <= 0 || hp_max_info.value <= 0 || hp_max_info.value > 50_000_000 {
            return false;
        }

        // Always store HP — the entity may not be in mob_data yet if spawn
        // packet arrived before the capture started
        self.data_storage.append_mob_hp(actor_id, hp_max_info.value);
        self.data_storage.note_party_hp(actor_id, hp_info.value as i64);

        true
    }

    // ===== HP RECORDS (00 8D) =====

    /// `<len> 00 8D <id> <groups> <n> (<type> <u32>)* <n> (<type> <u64>)*`:
    /// an entity's stats as they change. Bit 0x01 of `groups` says the u32
    /// list is there, 0x02 the u64 list; type 0 in the u64 list is current
    /// HP, type 7 max HP. All 282,154 framed records of the 2026-10-04 to
    /// 10-06 captures read to their exact length this way. Monsters, party
    /// members and players near you get one on every HP change; you get
    /// yours too, with your MP and other values in the u32 list.
    ///
    /// Read in each stretch of the data (`framing::regions`), so a record
    /// the framing resynchronised over is read too; the length before the
    /// opcode must match.
    pub(super) fn scan_for_hp_records(&self, data: &[u8]) {
        let mut i = 0;
        while i + 1 < data.len() {
            if data[i] != 0x00 || data[i + 1] != 0x8D {
                i += 1;
                continue;
            }
            // The length varint before the opcode, one or two bytes.
            let record = (1..=2usize).filter(|&w| i >= w).find_map(|w| {
                let len = read_varint(data, i - w);
                if len.length != w as i32 {
                    return None;
                }
                read_hp_record(&data[i..]).filter(|r| r.end as i64 + 4 == len.value as i64)
            });
            match record {
                Some(r) => {
                    if let Some(hp) = r.current_hp {
                        self.data_storage.note_hp(r.id, hp);
                    }
                    i += r.end;
                }
                None => i += 1,
            }
        }
    }

    // ===== LIVE ENTITY HP (8D <id> 02 01 00 <u32 LE current HP>) =====

    /// Live current-HP feed. Combat packets embed, per affected entity, a record
    /// `8D <entityId varint> <disc 3 bytes> <u32 LE current HP> 00 00 00 00`, where
    /// `disc` is `02 01 00` for NPCs/mobs and `01 01 01` for the local player. We
    /// capture the NPC readings so the boss bar can show REAL current HP — it
    /// declines as the boss is hit and jumps back up on a heal/phase reset (a
    /// Training Scarecrow floors at 1 then resets to full). Max HP is not in this
    /// record; it comes from the spawn packet or the observed peak (see
    /// `set_mob_current_hp`). The `02` discriminator keeps the player's own
    /// `01 01 01` record out of the mob HP store.
    pub(super) fn scan_for_entity_hp(&self, data: &[u8]) {
        let mut i = 0;
        while i + 1 < data.len() {
            if data[i] != 0x8D {
                i += 1;
                continue;
            }
            let id_info = read_varint(data, i + 1);
            if id_info.length <= 0 || !(100..=9_999_999).contains(&id_info.value) {
                i += 1;
                continue;
            }
            let disc = i + 1 + id_info.length as usize;
            // Full record: `02 01 00 <u32 LE current HP> 00 00 00 00`. The 4 trailing
            // bytes (a second u32, always zero in this feed) are REQUIRED — without
            // them a look-alike sub-record `8D <id> 02 01 00 <other u32> <terminator>`
            // gets misread as a huge HP value and inflates the max (denominator),
            // which makes the boss-bar percentage read far too low.
            if disc + 11 > data.len() {
                i += 1;
                continue;
            }
            if data[disc] == 0x02
                && data[disc + 1] == 0x01
                && data[disc + 2] == 0x00
                && data[disc + 7] == 0x00
                && data[disc + 8] == 0x00
                && data[disc + 9] == 0x00
                && data[disc + 10] == 0x00
            {
                let h = disc + 3;
                let cur = u32::from_le_bytes([data[h], data[h + 1], data[h + 2], data[h + 3]]);
                // Sanity bound: real HP is well under this; rejects misparses.
                if cur <= 100_000_000 {
                    self.data_storage.set_mob_current_hp(id_info.value, cur as i32);
                }
                i = disc + 11;
                continue;
            }
            i += 1;
        }
    }
}

/// An `00 8D` record read from its opcode: the entity, its current HP when
/// the record has it, and where the record ends.
pub(super) struct HpRecord {
    pub id: i32,
    pub current_hp: Option<i64>,
    pub end: usize,
}

pub(super) fn read_hp_record(data: &[u8]) -> Option<HpRecord> {
    if data.get(..2)? != [0x00, 0x8D] {
        return None;
    }
    let id = read_varint(data, 2);
    if id.length <= 0 || !(100..=9_999_999).contains(&id.value) {
        return None;
    }
    let mut at = 2 + id.length as usize;
    let groups = *data.get(at)?;
    if groups == 0 || groups > 3 {
        return None;
    }
    at += 1;
    let mut current_hp = None;
    for (bit, width) in [(0x01, 4usize), (0x02, 8usize)] {
        if groups & bit == 0 {
            continue;
        }
        let n = *data.get(at)? as usize;
        at += 1;
        for _ in 0..n {
            let kind = *data.get(at)?;
            let value = data.get(at + 1..at + 1 + width)?;
            if width == 8 && kind == 0 {
                let v = u64::from_le_bytes(value.try_into().ok()?);
                current_hp = Some(i64::try_from(v).ok().filter(|&v| v <= i32::MAX as i64)?);
            }
            at += 1 + width;
        }
    }
    Some(HpRecord { id: id.value, current_hp, end: at })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::combat::data_storage::{DataStorage, PartyMember};
    use crate::i18n::lookup::{NpcLookup, SkillLookup};

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// Feed `(capture ms, packet)` in order; returns the deaths counted.
    fn replay(storage: &Arc<DataStorage>, packets: &[(i64, &str)]) -> Vec<(i64, i32, i64)> {
        let mut p = StreamProcessor::new(storage.clone(), Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
        for &(at, packet) in packets {
            p.set_override_timestamp(Some(at));
            p.consume_stream(&hex(packet));
        }
        p.set_override_timestamp(None);
        storage.deaths_since(0).iter().map(|d| (d.at, d.player, d.before)).collect()
    }

    /// Records from the 2026-10-04 to 10-06 captures, each read to its length.
    #[test]
    fn hp_records_are_read_to_their_length() {
        let read = |packet: &str| {
            let bytes = hex(packet);
            let r = read_hp_record(&bytes[1..]).unwrap();
            assert_eq!(r.end + 1, bytes.len(), "{packet}");
            (r.id, r.current_hp)
        };
        // Current HP alone; current and max HP together (a player nearby).
        assert_eq!(read("13008dad230201003705000000000000"), (4525, Some(1335)));
        assert_eq!(read("1c008dda02020200f30f00000000000007f30f000000000000"), (346, Some(4083)));
        // Your own: a value in the u32 list, then current HP.
        assert_eq!(read("19008dfc7a030103600301000100cc1c000000000000"), (15740, Some(7372)));
        // Your MP only; your max HP among other stats.
        assert_eq!(read("0f008da15801010106090000"), (11297, None));
        assert_eq!(read("2d008da1580305089a0700000a70c001000bf04902000ca08601000d3519050001078a17000000000000"), (11297, None));
    }

    /// You, killed at the Toblini field boss (2026-10-06 16:39 capture):
    /// 4458, 2875, 1335, then 0, and 7815 three seconds later at the map
    /// load that revived you. One death; a second 0 is the same one.
    #[test]
    fn your_hp_falling_to_zero_is_one_death() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(4525));
        let deaths = replay(&s, &[
            (1_791_330_032_168, "13008dad230201006a11000000000000"),
            (1_791_330_032_270, "13008dad230201003b0b000000000000"),
            (1_791_330_032_366, "13008dad230201003705000000000000"),
            (1_791_330_032_467, "13008dad230201000000000000000000"),
            (1_791_330_032_500, "13008dad230201000000000000000000"),
            (1_791_330_035_420, "13008dad23020100871e000000000000"),
        ]);
        assert_eq!(deaths, vec![(1_791_330_032_467, 4525, 1335)]);
    }

    /// A party member who died out of your sight (Kasia capture, 2026-10-06):
    /// the party record went from full HP to 0, its last byte 1 to 0, and back
    /// to full 7.3 s later. A `00 8D` record of 0 just after (the bytes the
    /// game sent at this member's next death) is the same death.
    #[test]
    fn a_party_member_dies_in_the_party_record() {
        let s = Arc::new(DataStorage::new());
        let deaths = replay(&s, &[
            (1_791_313_553_910, "251b92fc18963e963e730c0000730c0000000000000000000064000000f049020001"),
            (1_791_313_578_110, "241b92fc1800963e730c0000730c0000000000000000000064000000f049020000"),
            (1_791_313_578_120, "13008dfc180201000000000000000000"),
            (1_791_313_585_410, "251b92fc18963e963e730c0000730c0000000000000000000000000000f049020001"),
        ]);
        assert_eq!(deaths, vec![(1_791_313_578_110, 3196, 7958)]);
    }

    /// A monster killed (2026-10-06 16:39 capture), and a player outside your
    /// party at 0: no death is counted for either.
    #[test]
    fn a_monster_or_a_stranger_at_zero_is_no_death() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(4525));
        s.append_nickname_authoritative(3196, "Stranger");
        s.set_party_roster(vec![("Member".into(), PartyMember::default()), ("Other".into(), PartyMember::default())], true);
        let deaths = replay(&s, &[
            (1_000, "14008d878c04020100ed03000000000000"),
            (2_000, "14008d878c040201000000000000000000"),
            (3_000, "13008dfc18020100371c000000000000"),
            (4_000, "13008dfc180201000000000000000000"),
        ]);
        assert!(deaths.is_empty());
    }
}
