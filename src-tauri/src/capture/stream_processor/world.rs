//! Zone, map, death and HP records, and the party-scope record.

use super::StreamProcessor;
use crate::capture::varint::{parse_u32_le, read_varint};

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
        if offset + 3 >= packet.len() || packet[offset] != 0x06 || packet[offset + 1] != 0x38 {
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
        if packet[offset] != 0x23 || packet[offset + 1] != 0x36 {
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
        if offset + 10 > packet.len() || packet[offset] != 0x21 || packet[offset + 1] != 0x36 {
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
        if packet[offset + 1] != 0x36 || (packet[offset] != 0x41 && packet[offset] != 0x42) {
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

        // Death flag: 1 = zone-init (entity loaded dead), 3 = combat death
        let flag_info = read_varint(packet, pos);
        if flag_info.length <= 0 {
            return;
        }

        if flag_info.value == 3 {
            tracing::trace!("Death event: entity {} killed in combat", entity_id);
            self.data_storage.mark_entity_dead(entity_id);
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
        if packet[offset] != 0x1B || packet[offset + 1] != 0x92 {
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

        true
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
