//! Who is who: entity ids bound to character names, and which one is you.

use std::ops::Range;

use super::StreamProcessor;
use crate::capture::opcodes::{OWN_RECORDS, PLAYER_SPAWN, PLAYER_SPAWN_OLD, SELF_IDENTITY, SPAWN, SPAWN_OLD};
use crate::capture::names::{
    exact_name, is_placeholder_name, sanitize_nickname, sanitized_at, unicode_script, UnicodeScript, NAME_FIELD_BYTES,
};
use crate::capture::varint::{can_read_varint, read_varint, varint_ending_at, VarIntResult};
use crate::entity::job_class::JobClass;

impl StreamProcessor {
    /// Bind the local player from the account character-select list.
    ///
    /// At login (and after a relog) the game broadcasts the account's character
    /// list as a run of entries `03 <entity_id u32 LE> <name_len u8> <utf8 name>`,
    /// one per character on the account — including alts that are not in the world.
    /// When you are already loaded into a zone there is no in-world `45 36` spawn
    /// for the character you are playing, so this list is the only place the local
    /// player's name↔id pairing appears (verified against two live captures where
    /// the self id, e.g. 4715/6289 across a relog, only surfaced here).
    ///
    /// We bind ONLY the entry whose name matches the user-configured character
    /// name — that drives the existing local-player auto-bind. Alts are
    /// deliberately skipped so we never create a phantom entity that never
    /// fights. The entity id is u32 little-endian here, unlike the varint used by
    /// the `36`-family spawn opcodes. The entry has no reliable leading tag (a
    /// `03` seen in one class's list was coincidental), so we scan for the
    /// `<id u32-LE> <name_len> <name>` shape directly and let the exact
    /// character-name match reject false positives.
    pub(super) fn scan_char_list_self(&self, data: &[u8]) {
        // Once the game has sent its self record this list has nothing to add,
        // and its ids are the list's own, not in-world entities: at character
        // select after playing Spirtmasta (entity 10044) it listed her as 7796,
        // and binding that moved "you" onto an entity that never fights.
        if self.data_storage.local_identity_from_game() {
            return;
        }
        let local_name = match self.data_storage.local_character_name() {
            Some(n) => n.trim().to_string(),
            None => return,
        };
        if local_name.is_empty() {
            return;
        }
        let mut i = 0;
        while i + 5 < data.len() {
            let entity_id =
                u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
            if !(100..=9_999_999).contains(&entity_id) {
                i += 1;
                continue;
            }
            let name_len = data[i + 4] as usize;
            if !NAME_FIELD_BYTES.contains(&name_len) {
                i += 1;
                continue;
            }
            let name_start = i + 5;
            let name_end = name_start + name_len;
            if name_end > data.len() {
                i += 1;
                continue;
            }
            if let Some(name) = exact_name(&data[name_start..name_end]) {
                if name == local_name {
                    self.data_storage.append_nickname_authoritative(entity_id as i32, &name);
                    tracing::info!(
                        "char-list: bound local player '{}' -> entity {}",
                        name,
                        entity_id
                    );
                    return;
                }
            }
            i += 1;
        }
    }

    /// Bind entity ids to character names from the game's masked identity records.
    ///
    /// The self record (`33 36`) and the other-player record (`45 36`) share one
    /// layout:
    ///
    /// ```text
    /// <opcode 2B> <entity_id varint> <mask1 u32 LE> <mask2 u8> [mask2 & 0x01] <len u8><utf8 name>
    /// ```
    ///
    /// The name is present exactly when bit 0 of `mask2` is set; the remaining
    /// bits of `mask2` vary with whatever else the record carries. Earlier
    /// versions of this parser keyed off the literal bytes that happened to sit
    /// in that position — `0B 37` for the self record, a bare `07` for player
    /// spawns — but those are just two observed `mask2` values. A patch that set
    /// any other bit silently stopped resolving names: in the reference capture
    /// the self record carries `mask2 = 0x37` and player records `0x07`, so the
    /// `0B 37` matcher found nothing and every party member stayed as `#id`.
    /// Reading the mask bit is version-stable across that kind of change.
    ///
    /// `33 36` is the record for the character *you* are playing. It appears for
    /// exactly one entity, so it identifies the local player outright — no need
    /// for the user to have typed their character name into settings, and it
    /// works when you are already loaded into a zone (where there is no login
    /// char-list and you never see your own spawn).
    pub(super) fn scan_masked_identity(&self, data: &[u8]) {
        for record in masked_records(data) {
            let id = record.id;
            let Some(sanitized) = record.name else {
                if self.data_storage.set_local_identity_from_game(id as i64, None) {
                    tracing::info!("self record: unnamed tutorial character -> entity {}", id);
                }
                continue;
            };
            let after = record.field.end;
            self.data_storage.note_low_id_entity(id);
            self.data_storage.append_nickname_authoritative(id, &sanitized);
            if let Some((server, job)) = record.profile {
                // The game's word on who you are replaces whatever name was
                // configured. That name comes from the window title or the last
                // session, and both go stale: the title does not change when a
                // new character is created, and switching character or server
                // leaves the previous name behind.
                if self
                    .data_storage
                    .set_local_identity_from_game(id as i64, Some(sanitized.clone()))
                {
                    tracing::info!("self record: local player '{}' -> entity {}", sanitized, id);
                }
                self.data_storage.note_player_server(&sanitized, server);
                // A byte, then level (u32). Confirmed by a level-up, 28
                // then 29 (Naicha, 2026-10-04), and against the roster's
                // levels for three other players.
                let level = data
                    .get(after + 7..after + 11)
                    .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .filter(|l| (1..=99).contains(l));
                self.data_storage.note_self_profile(&sanitized, Some(job), level);
            } else {
                tracing::debug!("player record: '{}' -> entity {}", sanitized, id);
            }
        }
    }

    /// `<len> <opcode> <entity_id varint> ...` for an opcode in `OWN_RECORDS`.
    ///
    /// Counted over 19 captures (2026-10-04 to 10-06): in the 80 zone loads
    /// whose self record named you, 4,848 of these named you, in every one of
    /// those zones, and none named anyone else: not party members, players
    /// nearby or mobs. The other 46 were `4a 36` naming your next id in the
    /// second before a zone load. A meter restarted mid-zone knew you from
    /// them after 13 s at the median, and the loot records after 45 s.
    pub(super) fn parse_own_record(&self, packet: &[u8]) {
        let len = read_varint(packet, 0);
        if len.length <= 0 {
            return;
        }
        let o = len.length as usize;
        let Some(op) = packet.get(o..o + 2) else { return };
        let Some(&(_, tail)) = OWN_RECORDS.iter().find(|(code, _)| code == op) else { return };
        let id = read_varint(packet, o + 2);
        if id.length <= 0 {
            return;
        }
        // Where the length is fixed, a record of any other length is not one.
        let end = o + 2 + id.length as usize;
        if tail.is_some_and(|t| end + t != packet.len()) {
            return;
        }
        if self.data_storage.note_own_record(id.value) {
            tracing::info!("own records: local player -> entity {}", id.value);
        }
    }

    /// Extract the player name from a `44/45 36` player spawn sub-packet.
    ///
    /// Uses the same mask-gated layout as `scan_masked_identity`
    /// (`<id varint> <mask1 u32> <mask2 u8> [mask2 & 0x01] <len><utf8>`) rather
    /// than hunting for the literal `0x07` that older builds happened to put in
    /// the `mask2` slot.
    pub(super) fn parse_player_spawn_name(&self, data: &[u8], offset_after_opcode: usize) {
        let (actor_id, sanitized) = match player_spawn_at(data, offset_after_opcode) {
            None => return,
            // A player all the same, which tells their damage from an effect's.
            Some(PlayerSpawn::Unnamed(actor_id)) => {
                self.data_storage.note_player_record(actor_id);
                return;
            }
            Some(PlayerSpawn::Named(actor_id, name, _)) => (actor_id, name),
        };
        // 45/44 36 player spawn is an authoritative id↔name source.
        self.data_storage.note_low_id_entity(actor_id);
        self.data_storage.note_player_spawn(actor_id);
        self.data_storage
            .append_nickname_authoritative(actor_id, &sanitized);
    }

    // ===== ACTOR NAME BINDING =====

    pub(super) fn parse_actor_name_binding_rules(&self, packet: &[u8]) -> bool {
        actor_name_fields(packet)
            .into_iter()
            .any(|(actor_id, name_start, name_length)| self.register_utf8_nickname(packet, actor_id, name_start, name_length))
    }

    pub(super) fn register_utf8_nickname(&self, packet: &[u8], actor_id: i32, name_start: usize, name_length: usize) -> bool {
        if self.data_storage.has_nickname(actor_id) {
            return false;
        }
        if self.data_storage.is_summon(actor_id) {
            return false;
        }
        if name_length == 0 || name_length > 36 {
            return false;
        }
        let name_end = name_start + name_length;
        if name_end > packet.len() {
            return false;
        }
        let name_bytes = &packet[name_start..name_end];
        let name = match std::str::from_utf8(name_bytes) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let sanitized = match sanitize_nickname(name) {
            Some(s) => s,
            None => return false,
        };
        self.data_storage.append_nickname(actor_id, &sanitized);
        true
    }

    // ===== LOOT ATTRIBUTION ACTOR NAME =====

    pub(super) fn parse_loot_attribution_actor_name(&self, packet: &[u8]) -> bool {
        let mut candidates: std::collections::HashMap<i32, (String, Vec<u8>)> = std::collections::HashMap::new();
        for (actor_id, sanitized, field) in loot_actor_fields(packet) {
            let name_bytes = &packet[field];
            let existing = candidates.get(&actor_id);
            if existing.is_none() || name_bytes.len() > existing.unwrap().1.len() {
                candidates.insert(actor_id, (sanitized, name_bytes.to_vec()));
            }
        }

        if candidates.is_empty() {
            return false;
        }

        let mut found_any = false;
        let allow_prepopulate = candidates.len() > 1;

        for (actor_id, (name, _)) in &candidates {
            let existing = self.data_storage.get_nickname(*actor_id);
            let has_cjk = name.chars().any(|ch| {
                matches!(unicode_script(ch), UnicodeScript::Han | UnicodeScript::Hangul)
            });

            if !allow_prepopulate && !self.data_storage.actor_appears_in_combat(*actor_id) && !has_cjk {
                if existing.is_none() {
                    self.data_storage.cache_pending_nickname(*actor_id, name);
                }
                continue;
            }

            if existing.is_some() {
                continue;
            }

            self.data_storage.append_nickname(*actor_id, name);
            found_any = true;
        }

        found_any
    }

    // ===== NICKNAME PARSING =====

    pub(super) fn parsing_nickname(&self, packet: &[u8]) -> bool {
        let fields = nickname_fields(packet);
        for (id, name, _) in &fields {
            self.data_storage.append_nickname(*id, name);
        }
        !fields.is_empty()
    }
}

/// Where `parse_actor_name_binding_rules` reads a name: `07 <len> <name>` up to
/// 64 bytes after an anchor `36 <actor varint>`. (actor, name start, length)
pub(super) fn actor_name_fields(packet: &[u8]) -> Vec<(i32, usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut last_anchor: Option<(i32, usize, usize)> = None; // (actor_id, start, end)

    while i < packet.len() {
        if packet[i] == 0x36 {
            // Skip spawn opcodes (40/41 36 mob, 44/45 36 player) — the
            // 0x36 family shifted +1 in June 2026.
            if i > 0 && [SPAWN_OLD, SPAWN, PLAYER_SPAWN_OLD, PLAYER_SPAWN].contains(&[packet[i - 1], packet[i]]) {
                i += 1;
                continue;
            }
            if i + 1 >= packet.len() {
                i += 1;
                continue;
            }
            let actor_info = read_varint(packet, i + 1);
            last_anchor = if actor_info.length > 0 && actor_info.value >= 100 {
                Some((actor_info.value, i, i + 1 + actor_info.length as usize))
            } else {
                None
            };
            i += 1;
            continue;
        }

        if packet[i] == 0x07
            && let Some((name_start, name_length)) = read_utf8_name(packet, i)
            && let Some((actor_id, _, end_idx)) = last_anchor
        {
            let distance = i as isize - end_idx as isize;
            if (0..=64).contains(&distance) {
                out.push((actor_id, name_start, name_length));
            }
        }
        i += 1;
    }
    out
}

fn read_utf8_name(packet: &[u8], anchor_index: usize) -> Option<(usize, usize)> {
    let length_index = anchor_index + 1;
    if length_index >= packet.len() {
        return None;
    }
    let name_length = packet[length_index] as usize;
    if !(1..=36).contains(&name_length) {
        return None;
    }
    let name_start = length_index + 1;
    let name_end = name_start + name_length;
    if name_end > packet.len() {
        return None;
    }
    let name_bytes = &packet[name_start..name_end];
    let name = std::str::from_utf8(name_bytes).ok()?;
    let sanitized = sanitize_nickname(name)?;
    if sanitized.is_empty() {
        return None;
    }
    Some((name_start, name_length))
}

/// Where `parse_loot_attribution_actor_name` reads a name:
/// `<actor varint> <F0..FF> <03|A3> <len> <name>`. (actor, name, whole field)
pub(super) fn loot_actor_fields(packet: &[u8]) -> Vec<(i32, String, Range<usize>)> {
    let mut out = Vec::new();
    let mut idx = 0;

    while idx + 2 < packet.len() {
        let marker = packet[idx] as u32;
        let marker_next = packet[idx + 1] as u32;
        let is_marker = (0xF0..=0xFF).contains(&marker) && (marker_next == 0x03 || marker_next == 0xA3);

        if is_marker {
            // Scan backward for actor ID
            let mut actor_info: Option<VarIntResult> = None;
            let min_offset = idx.saturating_sub(8);
            for actor_offset in min_offset..idx {
                if !can_read_varint(packet, actor_offset) {
                    continue;
                }
                let candidate = read_varint(packet, actor_offset);
                if candidate.length <= 0 || actor_offset + candidate.length as usize != idx {
                    continue;
                }
                if !(100..=99999).contains(&candidate.value) {
                    continue;
                }
                actor_info = Some(candidate);
                break;
            }

            let actor_info = match actor_info {
                Some(a) => a,
                None => { idx += 1; continue; }
            };

            let length_idx = idx + 2;
            if length_idx >= packet.len() {
                idx += 1;
                continue;
            }
            let name_length = packet[length_idx] as usize;
            if !(1..=36).contains(&name_length) {
                idx += 1;
                continue;
            }
            let name_start = length_idx + 1;
            let name_end = name_start + name_length;
            if name_end > packet.len() {
                idx += 1;
                continue;
            }
            let name = match std::str::from_utf8(&packet[name_start..name_end]) {
                Ok(s) => s,
                Err(_) => { idx = name_end; continue; }
            };
            let sanitized = match sanitize_nickname(name) {
                Some(s) => s,
                None => { idx = name_end; continue; }
            };
            out.push((actor_info.value, sanitized, name_start..name_end));
            idx = name_end;
            continue;
        }
        idx += 1;
    }
    out
}

/// Where `parsing_nickname` reads a name, by its three patterns. (id, name,
/// the name's bytes)
pub(super) fn nickname_fields(packet: &[u8]) -> Vec<(i32, String, Range<usize>)> {
    let mut out = Vec::new();
    let mut search_offset = 0;

    while search_offset + 2 < packet.len() {
        // PATTERN A: E2/E0 07 anchor
        if (packet[search_offset] == 0xE2 || packet[search_offset] == 0xE0)
            && packet[search_offset + 1] == 0x07
        {
            let len_idx = search_offset + 2;
            if len_idx < packet.len() {
                let name_len = packet[len_idx] as usize;
                if (2..=36).contains(&name_len) && len_idx + 1 + name_len <= packet.len() {
                    let np = &packet[len_idx + 1..len_idx + 1 + name_len];
                    if let Ok(possible_name) = std::str::from_utf8(np) {
                        if !possible_name.is_empty() && possible_name.chars().next().unwrap().is_alphanumeric() {
                            if let Some((sanitized, range)) = sanitized_at(packet, len_idx + 1, name_len) {
                                if sanitized.len() >= 2
                                    && let Some(id) = varint_ending_at(packet, search_offset, 0, 100..=9_999_999)
                                {
                                    out.push((id, sanitized, range));
                                    search_offset = len_idx + 1 + name_len;
                                    // Skip guild name
                                    search_offset = skip_guild_name(packet, search_offset);
                                }
                            }
                        }
                    }
                }
            }
        }

        // PATTERN B: 0F 1D 37 block anchor
        if search_offset + 2 < packet.len()
            && packet[search_offset] == 0x0F
            && packet[search_offset + 1] == 0x1D
            && packet[search_offset + 2] == 0x37
        {
            let id_offset = search_offset + 3;
            if can_read_varint(packet, id_offset) {
                let block_actor = read_varint(packet, id_offset);
                if (100..=9_999_999).contains(&block_actor.value) {
                    let mut block_scan = id_offset + block_actor.length as usize;
                    let block_end = std::cmp::min(packet.len(), block_scan + 500);

                    while block_scan + 3 < block_end {
                        // Stop at terminator. The leading byte changed
                        // 0x06 -> 0x0E in the June 2026 update; accept both.
                        if (packet[block_scan] == 0x06 || packet[block_scan] == 0x0E) && packet[block_scan + 1] == 0x00 && packet[block_scan + 2] == 0x36 {
                            break;
                        }
                        // Name must be preceded by 00 00
                        if packet[block_scan] == 0x00 && packet[block_scan + 1] == 0x00 {
                            let len_idx = block_scan + 2;
                            if len_idx < packet.len() {
                                let name_len = packet[len_idx] as usize;
                                if (2..=36).contains(&name_len) && len_idx + 1 + name_len <= packet.len() {
                                    let np = &packet[len_idx + 1..len_idx + 1 + name_len];
                                    if let Ok(possible_name) = std::str::from_utf8(np) {
                                        if !possible_name.is_empty() && possible_name.chars().next().unwrap().is_alphanumeric() {
                                            if let Some((sanitized, range)) = sanitized_at(packet, len_idx + 1, name_len) {
                                                if sanitized.len() >= 2 {
                                                    out.push((block_actor.value, sanitized, range));
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        block_scan += 1;
                    }
                }
            }
        }

        // PATTERN D: Terminator anchor (04/00 4C)
        if search_offset + 1 < packet.len() {
            let b0 = packet[search_offset] as u32;
            let b1 = packet[search_offset + 1] as u32;

            if (b0 == 0x04 || b0 == 0x00) && b1 == 0x4C {
                let id_idx = search_offset + 2;
                if can_read_varint(packet, id_idx) {
                    let player_info = read_varint(packet, id_idx);
                    if player_info.length > 0 && (100..=9_999_999).contains(&player_info.value) {
                        let stop_at = std::cmp::min(packet.len().saturating_sub(2), id_idx + 128);
                        let mut scan_idx = id_idx + player_info.length as usize;

                        while scan_idx < stop_at {
                            // Terminator: leading byte changed 0x06 -> 0x0E
                            // in the June 2026 update; accept both.
                            if (packet[scan_idx] == 0x06 || packet[scan_idx] == 0x0E)
                                && packet[scan_idx + 1] == 0x00
                                && packet[scan_idx + 2] == 0x36
                            {
                                // Look backwards for name
                                for test_len in 2..=36usize {
                                    if scan_idx < test_len + 1 + id_idx {
                                        continue;
                                    }
                                    let len_byte_idx = scan_idx - test_len - 1;
                                    if len_byte_idx <= id_idx {
                                        continue;
                                    }
                                    let possible_len = packet[len_byte_idx] as usize;
                                    if possible_len == test_len {
                                        let np = &packet[len_byte_idx + 1..len_byte_idx + 1 + test_len];
                                        if let Ok(possible_name) = std::str::from_utf8(np) {
                                            if !possible_name.is_empty() && possible_name.chars().next().unwrap().is_alphanumeric() {
                                                if let Some(found) = sanitized_at(packet, len_byte_idx + 1, test_len) {
                                                    if found.0.len() >= 2 {
                                                        // Try to find earlier name (player name vs guild)
                                                        let before_name = find_name_before(
                                                            packet, len_byte_idx,
                                                            id_idx + player_info.length as usize,
                                                        );
                                                        let (final_name, range) = before_name.unwrap_or(found);
                                                        out.push((player_info.value, final_name, range));
                                                        search_offset = scan_idx;
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                break;
                            }
                            scan_idx += 1;
                        }
                    }
                }
            }
        }

        search_offset += 1;
    }
    out
}

fn find_name_before(packet: &[u8], before_idx: usize, min_idx: usize) -> Option<(String, Range<usize>)> {
    for test_len in 2..=36usize {
        for gap in 0..=1usize {
            if before_idx < gap + test_len + 1 {
                continue;
            }
            let name_len_idx = before_idx - gap - test_len - 1;
            if name_len_idx < min_idx {
                continue;
            }
            let possible_len = packet[name_len_idx] as usize;
            if possible_len != test_len {
                continue;
            }
            let np = &packet[name_len_idx + 1..name_len_idx + 1 + test_len];
            if let Ok(possible_name) = std::str::from_utf8(np) {
                if possible_name.is_empty() || !possible_name.chars().next().unwrap().is_alphanumeric() {
                    continue;
                }
                if let Some(found) = sanitized_at(packet, name_len_idx + 1, test_len) {
                    if found.0.len() >= 2 {
                        return Some(found);
                    }
                }
            }
        }
    }
    None
}

fn skip_guild_name(packet: &[u8], start_index: usize) -> usize {
    if start_index >= packet.len() {
        return start_index;
    }
    let mut offset = start_index;
    if packet[offset] == 0x00 {
        offset += 1;
        if offset >= packet.len() {
            return offset;
        }
    }
    let length = packet[offset] as usize;
    if !(1..=36).contains(&length) {
        return offset;
    }
    let name_start = offset + 1;
    let name_end = name_start + length;
    if name_end > packet.len() {
        return offset;
    }
    if std::str::from_utf8(&packet[name_start..name_end]).is_err() {
        return offset;
    }
    name_end
}


/// A self record (`33 36`) or another player's record (`44 36`, `45 36`), as
/// `StreamProcessor::scan_masked_identity` reads it.
pub(super) struct MaskedRecord {
    pub id: i32,
    /// The name field's bytes.
    pub field: Range<usize>,
    /// `None` for a tutorial character's placeholder in your self record.
    pub name: Option<String>,
    /// Your server and class, in a self record.
    pub profile: Option<(u16, JobClass)>,
}

/// Every masked identity record in `data`:
/// `<opcode 2B> <entity_id varint> <mask1 u32 LE> <mask2 u8> [mask2 & 0x01] <len u8><utf8 name>`.
pub(super) fn masked_records(data: &[u8]) -> Vec<MaskedRecord> {
    let mut out = Vec::new();
    if data.len() < 9 {
        return out;
    }
    let mut i = 0;
    while i + 8 < data.len() {
        let opcode = [data[i], data[i + 1]];
        // 0x33 = self, 0x45/0x44 = another player (pre/post the June 2026 shift).
        let is_self = opcode == SELF_IDENTITY;
        if !is_self && opcode != PLAYER_SPAWN && opcode != PLAYER_SPAWN_OLD {
            i += 1;
            continue;
        }
        let id = read_varint(data, i + 2);
        if id.length <= 0 || !(1..=9_999_999).contains(&id.value) {
            i += 1;
            continue;
        }
        // mask1 is 4 bytes; mask2 is the byte after it and gates the name.
        let mask2_idx = i + 2 + id.length as usize + 4;
        if mask2_idx + 1 >= data.len() || data[mask2_idx] & 0x01 == 0 {
            i += 1;
            continue;
        }
        let name_len = data[mask2_idx + 1] as usize;
        if !NAME_FIELD_BYTES.contains(&name_len) || mask2_idx + 2 + name_len > data.len() {
            i += 1;
            continue;
        }
        let field = mask2_idx + 2..mask2_idx + 2 + name_len;
        let Ok(raw) = std::str::from_utf8(&data[field.clone()]) else {
            i += 1;
            continue;
        };
        // A new character plays the tutorial before it has a name; until
        // then the game calls it `$` plus random letters and digits (seen:
        // `$Kc03nyeQHr4`, entity 3877, on 2026-10-01). It is still you, so
        // bind the entity, but with no name: a placeholder would be noise,
        // and keeping the previous character's name would be wrong.
        if is_self && is_placeholder_name(raw) {
            i = field.end;
            out.push(MaskedRecord { id: id.value, field, name: None, profile: None });
            continue;
        }
        // The whole field must be one clean name; otherwise we landed
        // mid-record rather than on a real name string.
        let Some(sanitized) = exact_name(&data[field.clone()]) else {
            i += 1;
            continue;
        };
        // A self record without your server and class after the name is
        // not one. The last `33 36` of a `1d 37` record ending `33 36 33 36`
        // read with the next record's bytes as entity 16 and a two-letter
        // name (2026-10-05 17:42:14), and the meter took that for you.
        // Entity ids under 100 are real players, so the id cannot tell.
        let profile = if is_self {
            let Some(profile) = self_profile(data, field.end) else {
                i += 1;
                continue;
            };
            Some(profile)
        } else {
            None
        };
        i = field.end;
        out.push(MaskedRecord { id: id.value, field, name: Some(sanitized), profile });
    }
    out
}

/// A player spawn's (`44 36`, `45 36`) id and name, as
/// `StreamProcessor::parse_player_spawn_name` reads them.
pub(super) enum PlayerSpawn {
    /// The name bit is clear: a player all the same.
    Unnamed(i32),
    Named(i32, String, Range<usize>),
}

/// The player spawn whose id starts at `offset_after_opcode`, if one is there.
/// Uses the same mask-gated layout as `masked_records`
/// (`<id varint> <mask1 u32> <mask2 u8> [mask2 & 0x01] <len><utf8>`).
pub(super) fn player_spawn_at(data: &[u8], offset_after_opcode: usize) -> Option<PlayerSpawn> {
    let actor_info = read_varint(data, offset_after_opcode);
    // Raid/invasion player ids run well past 99,999, so accept the full entity-id
    // range (matching the embedded-scan gate) or those spawns are silently dropped.
    if actor_info.length <= 0 || !(1..=9_999_999).contains(&actor_info.value) {
        return None;
    }
    let actor_id = actor_info.value;
    let mask2_idx = offset_after_opcode + actor_info.length as usize + 4;
    if mask2_idx + 1 >= data.len() || data[mask2_idx] & 0x01 == 0 {
        return Some(PlayerSpawn::Unnamed(actor_id));
    }
    let name_len = data[mask2_idx + 1] as usize;
    if !NAME_FIELD_BYTES.contains(&name_len) || mask2_idx + 2 + name_len > data.len() {
        return None;
    }
    let field = mask2_idx + 2..mask2_idx + 2 + name_len;
    let name = exact_name(&data[field.clone()])?;
    Some(PlayerSpawn::Named(actor_id, name, field))
}

/// Your server (u16) and class (u32, the roster's encoding), which follow the
/// name in a self record. Both must read as one.
fn self_profile(data: &[u8], after: usize) -> Option<(u16, JobClass)> {
    let rest = data.get(after..after + 6)?;
    let server = u16::from_le_bytes([rest[0], rest[1]]);
    let class = u32::from_le_bytes([rest[2], rest[3], rest[4], rest[5]]);
    let job = JobClass::from_roster_class(class)?;
    (1000..3000).contains(&server).then_some((server, job))
}
