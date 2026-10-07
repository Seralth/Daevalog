//! Spawns and summons: who owns a summon, and what a spawned mob is.

use super::StreamProcessor;
use crate::capture::opcodes::{PLAYER_SPAWN, PLAYER_SPAWN_OLD, SPAWN, SPAWN_OLD, SUMMON_OWNERSHIP};
use crate::capture::names::{exact_name, NAME_FIELD_BYTES};
use crate::capture::varint::{find_pattern, parse_u32_le, read_varint, varint_ending_at};

use std::ops::Range;

impl StreamProcessor {
    // ===== SUMMON OWNERSHIP (04 8D) =====

    pub(super) fn parse_summon_ownership_packet(&self, packet: &[u8]) -> bool {
        let Some((summon_id, owner_id, name)) = ownership_at_front(packet) else {
            return false;
        };

        // Only link confirmed summons
        if self.data_storage.is_confirmed_summon(summon_id) {
            self.data_storage.append_summon(owner_id, summon_id);
        }

        // Name field after owner ID
        if let Some((start, len)) = name {
            self.register_utf8_nickname(packet, owner_id, start, len);
        }

        true
    }

    // ===== EMBEDDED 04 8D SCAN =====

    pub(super) fn scan_for_embedded_04_8d(&self, data: &[u8]) -> bool {
        let records = ownership_records(data);
        for r in &records {
            let (summon_id, owner_id, name) = (r.summon_id, r.owner_id, &r.name);
            if self.data_storage.is_confirmed_summon(summon_id) {
                self.data_storage.append_summon(owner_id, summon_id);
            }
            self.data_storage.append_nickname(owner_id, name);
            self.data_storage.note_player_server(name, r.server_id);
            // For a mob that was fought (it follows the mob's `35 38` despawn)
            // this is the loot owner, which so far has always been you.
            if !self.data_storage.is_confirmed_summon(summon_id)
                && self.data_storage.is_damage_target(summon_id)
                && self.data_storage.note_loot_owner(summon_id, owner_id, name)
            {
                tracing::info!("loot record: local player '{}' -> entity {}", name, owner_id);
            }
        }
        !records.is_empty()
    }

    // ===== EMBEDDED 40 36 SCAN =====

    pub(super) fn scan_for_embedded_40_36(&mut self, data: &[u8]) {
        for (i, opcode, id) in embedded_spawns(data) {
            if opcode == PLAYER_SPAWN_OLD || opcode == PLAYER_SPAWN {
                // 44/45 36 = player spawn — extract name
                self.parse_player_spawn_name(data, i + 2);
            } else {
                // 40/41 36 = summon/mob spawn
                let mut real_id = id;
                if real_id > 1_000_000 {
                    real_id = (real_id & 0x3FFF) | 0x4000;
                }
                if !self.data_storage.is_mob(real_id) {
                    self.parse_summon_spawn_at(data, i + 2);
                }
            }
        }
    }

    // ===== SUMMON PACKET (40 36) =====

    pub(super) fn parse_summon_packet(&mut self, packet: &[u8]) -> bool {
        let length_info = read_varint(packet, 0);
        if length_info.length < 0 {
            return false;
        }
        let offset = length_info.length as usize;
        if offset + 1 >= packet.len() {
            return false;
        }
        let opcode = [packet[offset], packet[offset + 1]];
        // Player spawn: 0x3644 pre-2026-06, 0x3645 after the June 2026 +1 shift.
        if opcode == PLAYER_SPAWN_OLD || opcode == PLAYER_SPAWN {
            self.parse_player_spawn_name(packet, offset + 2);
            return false;
        }
        // Mob/summon spawn: 0x3640 pre-2026-06, 0x3641 after the shift.
        if opcode != SPAWN_OLD && opcode != SPAWN {
            return false;
        }
        let linked = self.parse_summon_spawn_at(packet, offset + 2);
        self.note_effect_parent(packet, offset + 2);
        linked
    }

    /// A `0x1C` skill-effect entity's record ends with the entity it belongs
    /// to: `<u32 parent> 00 <u8> 00 00 00 00`, after its abnormal list and one
    /// byte (06 or 01). Read only from a record that is a whole packet, where
    /// its end is known. For a monster's effect the parent is the monster:
    /// Saraswati's 24810 and Bakarma's 28846 (2026-10-06) named their boss,
    /// and the game's records counted their hits on the player as damage
    /// taken.
    fn note_effect_parent(&self, packet: &[u8], offset_after_opcode: usize) {
        let id = read_varint(packet, offset_after_opcode);
        if id.length <= 0 {
            return;
        }
        let kind_at = offset_after_opcode + id.length as usize;
        if packet.get(kind_at) != Some(&0x1C) || packet.len() < kind_at + 11 {
            return;
        }
        let tail = &packet[packet.len() - 10..];
        if tail[4] != 0 || tail[6..] != [0, 0, 0, 0] {
            return;
        }
        let mut effect = id.value;
        if effect > 1_000_000 {
            effect = (effect & 0x3FFF) | 0x4000;
        }
        let parent = i32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]);
        self.data_storage.note_effect_parent(effect, parent);
    }

    /// Parse a `41 36` spawn record (NPCs, summons/pets and transient
    /// skill-effect entities — never a real player, who gets a `45 36`/`33 36`
    /// record instead).
    ///
    /// ```text
    /// 41 36 <entity_id varint> <mask u32 LE> <subtree…> ×3 … [mask & 0x0010] <parent_key u32 LE>
    /// ```
    ///
    /// The low byte of `mask` doubles as the entity kind: `0x0C`/`0x0D` = NPC,
    /// `0x5F` = summon/pet, `0x1C` = a short-lived skill-effect entity parented
    /// to the skill's *target*. The first subtree opens with its own `mask2` byte
    /// whose bit 0 signals an inline name string (for a summon that name is the
    /// owner's), followed by the `u32` model / NPC-type id.
    ///
    /// `mask & 0x0010` declares a `parent_key`, and for a summon that is its
    /// owner's entity id — the one link that works even when nobody has been
    /// named yet. It sits past three variable-length subtrees, so instead of
    /// walking those we anchor on the owner block that directly follows it (see
    /// `find_spawn_parent_key`).
    pub(super) fn parse_summon_spawn_at(&mut self, packet: &[u8], offset_after_opcode: usize) -> bool {
        let mut offset = offset_after_opcode;
        let target_info = read_varint(packet, offset);
        if target_info.length < 0 {
            return false;
        }
        offset += target_info.length as usize;

        let mut real_actor_id = target_info.value;
        if real_actor_id > 1_000_000 {
            real_actor_id = (real_actor_id & 0x3FFF) | 0x4000;
        }

        // This entity spawned via a mob/summon spawn (40/41 36), never a player
        // spawn. Record it so a summon / spell-effect that deals class-band damage
        // isn't mistaken for a player and can be attributed to its owner.
        self.data_storage.note_summon_spawn(real_actor_id);
        self.data_storage.note_low_id_entity(real_actor_id);

        if offset + 2 >= packet.len() {
            self.extract_and_register_mob_type(packet, offset, real_actor_id);
            return false;
        }
        // Read the mask as a u32 regardless of which width the server sent. The
        // two fields we test live in the low half either way — `kind` is the low
        // byte and `parent_key` is gated by bit 4 — so a wide read is correct for
        // both formats and only picks up bytes we never look at.
        let mask = u32::from_le_bytes([
            packet[offset],
            packet[offset + 1],
            *packet.get(offset + 2).unwrap_or(&0),
            *packet.get(offset + 3).unwrap_or(&0),
        ]);
        let kind = packet[offset];

        // The mask width changed from u16 to u32, which moves the subtree byte
        // that gates the inline name. Nothing else in this record is sensitive to
        // it: `find_spawn_parent_key` scans forward rather than indexing, and the
        // mob-type scan anchors on `offset`. So rather than version-sniffing the
        // stream, try both positions and keep whichever actually yields a name.
        //
        // Getting this wrong is not cosmetic. For a summon the inline name is the
        // *owner's* character name, and it is the fallback that attributes a pet's
        // damage to its player when no parent_key is present. A silently
        // mispositioned gate shows up as summons drifting back into their own rows.
        let (spawn_name, cursor) = match spawn_name_at(packet, offset) {
            Some((name, field)) => (Some(name), field.end),
            None => (None, offset + MASK_U32_SUBTREE + 1),
        };

        // Mob type / boss flag / HP still come from the existing scan, which
        // anchors on the model field this cursor now sits on.
        let code = self.extract_and_register_mob_type(packet, offset, real_actor_id);

        // A monster's summon names a player too, the one it targets: a Blazing
        // Totem linked to the player it burned, and its Burn ticks on them
        // counted as their healing (2026-10-05). The NPC table says whose it
        // can be. Only this record's code: an id keeps the code of the last
        // entity under it, and a spirit spawns with none.
        if code.is_some_and(|c| self.npc_lookup.is_no_players_summon(c)) {
            return false;
        }

        // A `0x1C` effect entity is a monster's skill effect, parented to the
        // skill's TARGET, so its parent_key is never an owner. Its name, when
        // present, is a player's too, nearly always the one it hits: never an
        // owner either (docs/summon-attribution.md).
        let is_summon = kind == 0x5F;

        if is_summon
            && mask & 0x0010 != 0
            && let Some(owner_id) = self.find_spawn_parent_key(packet, cursor, real_actor_id)
        {
            self.data_storage.note_low_id_entity(owner_id);
            self.data_storage
                .register_confirmed_summon_by_id(real_actor_id, owner_id);
            tracing::debug!(
                "Summon {} linked to owner {} via parent_key",
                real_actor_id,
                owner_id
            );
            return true;
        }

        // Fall back to the name a summon's spawn carries: its owner's. A
        // `0x1C` effect names the player it targets, so it uses the name only
        // when its code is a player summon's: a monster new since the last
        // table update links to no one.
        if (kind != 0x1C || code.is_some_and(|c| self.npc_lookup.is_players_summon(c)))
            && let Some(name) = &spawn_name
            && let Some(owner_id) = self.data_storage.find_id_by_nickname(name)
            && owner_id != real_actor_id
        {
            self.data_storage
                .register_confirmed_summon_by_id(real_actor_id, owner_id);
            tracing::trace!(
                "Summon {} linked to owner {} via spawn name '{}'",
                real_actor_id,
                owner_id,
                name
            );
            return true;
        }

        // Last resort for spirits: the caster recorded on the entity's own buff
        // block, which is its owner. It reads back as the spirit itself for
        // some skills, hence `!= self`. Other players' spirits (`0x1F`, `0x1D`,
        // `0x5D`) spawn with no parent_key and no name, so this is their link
        // at spawn. Checked against the spirit/owner link records in five
        // captures (2026-10-04): 1,292 of 1,295 spirit spawns named the right
        // owner, none a wrong one, the rest none; mobs and effect entities
        // read back as themselves.
        if matches!(kind, 0x5F | 0x1F | 0x1D | 0x5D) {
            let owner_id = self.extract_summon_owner_from_spawn(packet, offset);
            if owner_id > 0 && owner_id != real_actor_id {
                self.data_storage
                    .register_confirmed_summon_by_id(real_actor_id, owner_id);
                return true;
            }
        }

        // A Sorcerer's lingering ground spell (Cold Storm, Bittercold Wind) also
        // spawns as `0x1F` (some as `0x5F`), but its buff block names the spell
        // itself. Its caster follows the spawn position as `07 02 06` or
        // `07 02 01` and a `u32`. Over every 0x1F/0x5F spawn in the check kit's
        // captures that deals class damage, this matched the existing link 2,759
        // times, added 4 (each a same-class player) and was wrong 0 times.
        if matches!(kind, 0x1F | 0x5F)
            && let Some(caster) = self.find_effect_caster(packet, offset, real_actor_id)
        {
            self.data_storage.note_low_id_entity(caster);
            self.data_storage
                .register_confirmed_summon_by_id(real_actor_id, caster);
            return true;
        }

        false
    }

    /// The caster of a ground spell: the `u32` after the `07 02 06` or
    /// `07 02 01` that follows the spawn position. Never the spell itself or a
    /// known mob.
    fn find_effect_caster(&self, packet: &[u8], start_offset: usize, self_id: i32) -> Option<i32> {
        let end = packet.len().min(start_offset + 240);
        let at = packet
            .get(start_offset..end)?
            .windows(3)
            .position(|w| w[0] == 0x07 && w[1] == 0x02 && (w[2] == 0x06 || w[2] == 0x01))?;
        let i = start_offset + at + 3;
        let caster = i32::from_le_bytes(packet.get(i..i + 4)?.try_into().ok()?);
        ((100..=9_999_999).contains(&caster) && caster != self_id && !self.data_storage.is_mob(caster))
            .then_some(caster)
    }

    /// Find the `parent_key` a `41 36` spawn declares via `mask & 0x0010`.
    ///
    /// The field sits behind three variable-length subtrees that are impractical
    /// to walk, so we anchor on the owner block that immediately follows it:
    ///
    /// ```text
    /// <parent_key u32 LE> <legion_id u32> <u16 = 0> <u16 server_id> <len u8> <utf8 legion name>
    /// ```
    ///
    /// preceded by the record's `mask & 0x0004` byte (constant `0x06`). Six
    /// independent constraints have to line up at once, which is why this pinned
    /// the owner on 81 of 81 summons in the reference capture with no false
    /// positives — including a Spiritmaster's 54 pets whose owner had never been
    /// named at the time they spawned.
    fn find_spawn_parent_key(&self, packet: &[u8], search_from: usize, self_id: i32) -> Option<i32> {
        let mut i = search_from.max(1);
        while i + 13 <= packet.len() {
            if packet[i - 1] == 0x06
                && let Some(parent) = parse_spawn_owner_block(packet, i, self_id)
            {
                return Some(parent);
            }
            i += 1;
        }
        None
    }

    fn extract_summon_owner_from_spawn(&self, packet: &[u8], start_offset: usize) -> i32 {
        let anchor: [u8; 8] = [0x80, 0x75, 0xD5, 0x2A, 0xBB, 0x03, 0x00, 0x00];
        let max_search = std::cmp::min(packet.len().saturating_sub(anchor.len()), start_offset + 120);
        for i in start_offset..=max_search {
            if packet[i..].starts_with(&anchor) {
                let owner_info = read_varint(packet, i + anchor.len());
                // Low ids are real (see `is_plausible_entity_id`).
                if owner_info.length > 0 && (1..=9_999_999).contains(&owner_info.value) {
                    return owner_info.value;
                }
            }
        }
        -1
    }

    /// The NPC code this record names, if it names one.
    fn extract_and_register_mob_type(&self, packet: &[u8], start_offset: usize, real_actor_id: i32) -> Option<i32> {
        let mut scan_offset = start_offset;
        let max_scan = std::cmp::min(packet.len().saturating_sub(2), start_offset + 60);

        while scan_offset < max_scan {
            if packet[scan_offset] == 0x00
                && (packet[scan_offset + 1] == 0x40 || packet[scan_offset + 1] == 0x00)
                && packet[scan_offset + 2] == 0x02
            {
                if scan_offset >= start_offset + 3 {
                    let b1 = packet[scan_offset - 3] as i32;
                    let b2 = packet[scan_offset - 2] as i32;
                    let b3 = packet[scan_offset - 1] as i32;
                    let mob_type_id = b1 | (b2 << 8) | (b3 << 16);
                    self.data_storage.append_mob(real_actor_id, mob_type_id);

                    // Register boss entities from NPC DB
                    if self.npc_lookup.is_boss(mob_type_id) {
                        self.data_storage.register_boss(real_actor_id);
                    }
                    if self.npc_lookup.is_training_dummy(mob_type_id) {
                        self.data_storage.register_training_dummy(real_actor_id);
                    }

                    // Try to extract HP
                    let mut hp_scan = scan_offset + 3;
                    let hp_end = std::cmp::min(packet.len().saturating_sub(2), hp_scan + 64);
                    while hp_scan < hp_end {
                        if packet[hp_scan] == 0x01 {
                            let current_hp = read_varint(packet, hp_scan + 1);
                            if current_hp.length > 0 && current_hp.value > 0 {
                                let max_hp = read_varint(packet, hp_scan + 1 + current_hp.length as usize);
                                if max_hp.length > 0 && max_hp.value >= current_hp.value {
                                    self.data_storage.append_mob_hp(real_actor_id, max_hp.value);
                                    break;
                                }
                            }
                        }
                        hp_scan += 1;
                    }
                    return Some(mob_type_id);
                }
                break;
            }
            scan_offset += 1;
        }
        None
    }
}

/// The `04 8D` record at the front of a packet, as
/// `StreamProcessor::parse_summon_ownership_packet` reads it: the summon, its
/// owner, and the owner's name field (start, length) if one follows.
pub(super) fn ownership_at_front(packet: &[u8]) -> Option<(i32, i32, Option<(usize, usize)>)> {
    let length_info = read_varint(packet, 0);
    if length_info.length < 0 {
        return None;
    }
    let offset = length_info.length as usize;
    if offset + 1 >= packet.len() {
        return None;
    }
    if packet[offset..offset + 2] != SUMMON_OWNERSHIP {
        return None;
    }

    let mut pos = offset + 2;
    let summon_info = read_varint(packet, pos);
    if summon_info.length <= 0 || summon_info.value < 100 {
        return None;
    }
    let summon_id = summon_info.value;
    pos += summon_info.length as usize;

    // Skip 4-byte fixed field
    if pos + 4 > packet.len() {
        return None;
    }
    pos += 4;

    let owner_info = read_varint(packet, pos);
    if owner_info.length <= 0 || owner_info.value < 100 {
        return None;
    }
    let owner_id = owner_info.value;
    pos += owner_info.length as usize;

    if owner_id == summon_id {
        return None;
    }

    // Name field after owner ID
    let mut name = None;
    let meta_info = read_varint(packet, pos);
    if meta_info.length > 0 {
        pos += meta_info.length as usize;
        if pos < packet.len() {
            let name_len = packet[pos] as usize;
            if (1..=36).contains(&name_len) && pos + 1 + name_len <= packet.len() {
                name = Some((pos + 1, name_len));
            }
        }
    }
    Some((summon_id, owner_id, name))
}

/// A `04 8D` ownership or loot record found anywhere in `data`.
pub(super) struct Ownership {
    pub summon_id: i32,
    pub owner_id: i32,
    pub server_id: u16,
    pub name: String,
    pub field: Range<usize>,
}

/// Every `04 8D` record in `data` that names its owner, as
/// `StreamProcessor::scan_for_embedded_04_8d` reads them.
pub(super) fn ownership_records(data: &[u8]) -> Vec<Ownership> {
    let mut out = Vec::new();
    let mut search_offset = 0;
    let pattern: [u8; 2] = SUMMON_OWNERSHIP;

    while search_offset + 1 < data.len() {
        let idx = find_pattern(data, search_offset, &pattern);
        if idx.is_none() {
            break;
        }
        let idx = idx.unwrap();

        search_offset = idx + 2;
        if search_offset >= data.len() {
            break;
        }

        let summon_info = read_varint(data, search_offset);
        if summon_info.length <= 0 || !(100..=9_999_999).contains(&summon_info.value) {
            continue;
        }
        let summon_id = summon_info.value;

        let fixed_field_start = search_offset + summon_info.length as usize;
        if fixed_field_start + 4 > data.len() {
            continue;
        }

        // The owner follows: `<owner varint> <server id u16 LE> <len><name>`.
        // The server id was once matched as the literal bytes `E0 07` /
        // `E2 07` (servers 2016 and 2018), which skipped every other server:
        // a Sorcerer on Ventus (1305, bytes `19 05`) never got a name. Any id
        // in the servers' 1000–2999 range is accepted now, and a candidate
        // only counts when the owner id sits wholly after the fixed field and
        // the whole name field is a name.
        let after_fixed = fixed_field_start + 4;
        // A zero there is no owner: the record a summon gets as it
        // despawns, all zeros. The scan below then ran on into whatever
        // followed, and found an "owner" in the next damage records: a
        // Cleric's Divine Aura went to entity 10210, named "M", and showed
        // as its own row (2026-10-04, Divine Auldor).
        if data.get(after_fixed).is_none_or(|&b| b == 0) {
            continue;
        }
        let scan_end = std::cmp::min(data.len().saturating_sub(2), after_fixed + 128);
        let mut found = None;
        for server_idx in after_fixed + 1..scan_end {
            let server_id = u16::from_le_bytes([data[server_idx], data[server_idx + 1]]);
            if !(1000..=2999).contains(&server_id) {
                continue;
            }
            // The owner `ed 74` (14957) ends in a byte that alone reads as
            // an id too (`74`, 116); see `varint_ending_at`.
            let owner_id = varint_ending_at(data, server_idx, after_fixed, 100..=99_999);
            let Some(owner_id) = owner_id.filter(|&id| id != summon_id) else {
                continue;
            };
            let name_len_idx = server_idx + 2;
            let name_len = data[name_len_idx] as usize;
            let name_end = name_len_idx + 1 + name_len;
            if !NAME_FIELD_BYTES.contains(&name_len) || name_end > data.len() {
                continue;
            }
            if let Some(name) = exact_name(&data[name_len_idx + 1..name_end]) {
                found = Some(Ownership { summon_id, owner_id, server_id, name, field: name_len_idx + 1..name_end });
                break;
            }
        }
        let Some(record) = found else {
            continue;
        };
        search_offset = record.field.end;
        out.push(record);
    }
    out
}

/// Every spawn opcode (40/41/44/45 36) in `data` that
/// `StreamProcessor::scan_for_embedded_40_36` reads: (where, opcode, entity id).
pub(super) fn embedded_spawns(data: &[u8]) -> Vec<(usize, [u8; 2], i32)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 5 < data.len() {
        // Spawn family shifted +1 in June 2026: mob/summon 0x40->0x41,
        // player 0x44->0x45. Accept both old and new leading bytes.
        let opcode = [data[i], data[i + 1]];
        if [SPAWN_OLD, SPAWN, PLAYER_SPAWN_OLD, PLAYER_SPAWN].contains(&opcode) {
            if i > 0 && data[i - 1] == 0x00 {
                i += 2;
                continue;
            }
            let target_info = read_varint(data, i + 2);
            if target_info.length > 0 && (100..=9_999_999).contains(&target_info.value) {
                out.push((i, opcode, target_info.value));
            }
            i += 2 + target_info.length.max(0) as usize;
        } else {
            i += 1;
        }
    }
    out
}

/// Where a spawn's mask sits relative to `offset` (just past its entity id),
/// in the u16 and u32 formats: the subtree byte that gates the inline name.
const MASK_U16_SUBTREE: usize = 2;
const MASK_U32_SUBTREE: usize = 4;

/// The inline name of a `40/41 36` spawn (its owner's or caster's), and the
/// name field's bytes. `offset` is just past the entity id.
///
/// The mask width changed from u16 to u32, which moves the subtree byte
/// that gates the inline name, so both positions are tried, the current
/// format first so a live stream never depends on the fallback.
pub(super) fn spawn_name_at(packet: &[u8], offset: usize) -> Option<(String, Range<usize>)> {
    let read_name_at = |sub_offset: usize| -> Option<(String, Range<usize>)> {
        let gate = *packet.get(offset + sub_offset)?;
        if gate & 0x01 == 0 {
            return None;
        }
        let cursor = offset + sub_offset + 1;
        let name_len = *packet.get(cursor)? as usize;
        if !NAME_FIELD_BYTES.contains(&name_len) || cursor + 1 + name_len > packet.len() {
            return None;
        }
        // The whole field must be a name. This check is what makes trying
        // two positions safe: a wrong guess almost never decodes cleanly.
        let name = exact_name(&packet[cursor + 1..cursor + 1 + name_len])?;
        Some((name, cursor + 1..cursor + 1 + name_len))
    };
    read_name_at(MASK_U32_SUBTREE).or_else(|| read_name_at(MASK_U16_SUBTREE))
}

/// Validate the owner block that follows a spawn's `parent_key` and return the
/// parent id. See `StreamProcessor::find_spawn_parent_key`.
///
/// ```text
/// <parent_key u32 LE> <legion_id u32> <u16 = 0> <u16 server_id> <len u8> <utf8 legion name>
/// ```
fn parse_spawn_owner_block(packet: &[u8], at: usize, self_id: i32) -> Option<i32> {
    if at + 13 > packet.len() {
        return None;
    }
    let parent = parse_u32_le(packet, at);
    if parent == 0 || parent > 9_999_999 || parent as i32 == self_id {
        return None;
    }
    // Two-byte pad that is always zero, then a plausible world id.
    if u16::from_le_bytes([packet[at + 8], packet[at + 9]]) != 0 {
        return None;
    }
    let server_id = u16::from_le_bytes([packet[at + 10], packet[at + 11]]);
    if server_id == 0 || server_id > 9_999 {
        return None;
    }
    let name_len = packet[at + 12] as usize;
    if name_len > 40 || at + 13 + name_len > packet.len() {
        return None;
    }
    // A legion-less owner has an empty name here; anything else must decode.
    std::str::from_utf8(&packet[at + 13..at + 13 + name_len]).ok()?;
    Some(parent as i32)
}
