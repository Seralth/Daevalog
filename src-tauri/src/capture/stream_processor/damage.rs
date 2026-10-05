//! Damage, damage over time and the heals that ride on damage records.

use super::{PendingCompactSkillContext, StreamProcessor};
use crate::capture::opcodes::{DAMAGE, DOT};
use crate::capture::varint::{parse_u32_le, read_varint, try_read_varint};
use crate::combat::data_storage::NoDamageHit;
use crate::entity::damage_packet::ParsedDamagePacket;
use crate::entity::special_damage::{self, SpecialDamage};

/// Theostone raw item ids, read as skill codes after `* 10 + 1`.
const THEOSTONE_ITEM_IDS: std::ops::RangeInclusive<i64> = 3_000_000..=3_099_999;
/// 7-digit skill codes are monster (NPC) skills.
const MONSTER_SKILL_CODES: std::ops::RangeInclusive<i64> = 1_000_000..=9_999_999;
/// The skill code of a compact aggregation record; its real skill comes from
/// `PendingCompactSkillContext`.
const COMPACT_AGGREGATE_SKILL: i32 = 99_745_942;
/// Switch `0x36`: layout 6 with both multi-hit bits (`0x30`) set. The
/// repeated-hit fallbacks apply to this switch only.
const MULTI_HIT_SWITCH: i32 = 54;

impl StreamProcessor {
    // ===== DOT PACKET =====

    pub(super) fn parse_dot_packet(&mut self, packet: &[u8]) {
        let length_info = read_varint(packet, 0);
        if length_info.length < 0 {
            return;
        }
        let offset = length_info.length as usize;

        if packet.len() <= offset + 1 {
            return;
        }
        if packet[offset..offset + 2] != DOT {
            return;
        }
        let mut offset = offset + 2;
        let target_info = read_varint(packet, offset);
        if target_info.length < 0 {
            return;
        }
        offset += target_info.length as usize;

        if offset >= packet.len() {
            return;
        }
        let effect_type = packet[offset] as u32;
        offset += 1;
        // Effect type: 0x02/0x0A = damage; 0x01/0x09 = heal; 0x0B = HoT.
        // 0x00 = status, 0x08 = buff. Exact match, NOT bitmask — 0x0B (HoT) has bit
        // 1 set and would leak through a mask.
        let is_damage = effect_type == 0x02 || effect_type == 0x0A;
        let is_heal = effect_type == 0x01 || effect_type == 0x09 || effect_type == 0x0B;
        if !is_damage && !is_heal {
            return;
        }

        let actor_info = read_varint(packet, offset);
        // Damage-on-self is rejected as noise; a self-HEAL is legitimate healing.
        if actor_info.length < 0 || (is_damage && actor_info.value == target_info.value) {
            tracing::debug!("DOT: bad actor or self-damage");
            return;
        }
        offset += actor_info.length as usize;

        let unknown_info = read_varint(packet, offset);
        if unknown_info.length < 0 {
            return;
        }
        offset += unknown_info.length as usize;

        if offset + 4 > packet.len() {
            return;
        }
        let skill_code = parse_u32_le(packet, offset) as i32 / 100;
        offset += 4;

        if !is_valid_skill_code(skill_code) {
            return;
        }

        let amount_info = read_varint(packet, offset);
        if amount_info.length < 0 || amount_info.value <= 0 || amount_info.value > 99_999_999 {
            return;
        }

        if is_heal {
            // Healing done — recorded per healer. No allowlist (the heal effect_type
            // is the gate). HoT = 0x0B. This runs on framed perfect packets only, so
            // there is no embedded over-read into adjacent records (the artifact that
            // produced bogus multi-million single-tick "heals" in offline scans).
            self.data_storage.append_heal(
                actor_info.value,
                skill_code,
                amount_info.value as i64,
                effect_type == 0x0B,
                self.override_timestamp.unwrap_or_else(crate::clock::now_ms),
            );
            return;
        }

        // Damage DoT: gated by the curated dot-skill allowlist. What it keeps
        // out is not damage on a mob: in five captures (2026-10-04) every such
        // tick was a heal (Recuperation, Light of Regeneration, the last tick
        // of Spirit's Benediction) or a mob's hit on a player.
        if !self.dot_damage_skill_ids.contains(&skill_code) {
            if self.unknown_dot_skills.insert(skill_code) {
                tracing::debug!(
                    "DOT: skill {} is not a known DoT, its ticks are not counted (actor {}, target {})",
                    skill_code,
                    actor_info.value,
                    target_info.value
                );
            }
            return;
        }

        let mut pdp = ParsedDamagePacket::new();
        if let Some(ts) = self.override_timestamp {
            pdp.set_timestamp(ts);
        }
        pdp.set_dot(true);
        pdp.set_target_id(target_info.value);
        pdp.set_actor_id(actor_info.value);
        pdp.set_skill_code(skill_code);
        pdp.set_damage(amount_info.value);

        if pdp.actor_id() != pdp.target_id() {
            self.data_storage.append_damage(pdp);
        }
    }

    // ===== EMBEDDED DAMAGE PACKET =====

    fn try_parse_embedded_damage_packet(&mut self, packet: &[u8]) -> bool {
        if packet.len() < 6 {
            return false;
        }
        let mut parsed_any = false;
        let mut search_offset = 0;

        while search_offset + 1 < packet.len() {
            if packet[search_offset..search_offset + 2] != DAMAGE {
                search_offset += 1;
                continue;
            }

            let raw_key = to_hex_range(packet, search_offset, std::cmp::min(search_offset + 64, packet.len()));
            if self.seen_embedded_hexes.contains(&raw_key) {
                search_offset += 1;
                continue;
            }

            let mut headless = vec![0xFF, 0x01];
            headless.extend_from_slice(&packet[search_offset..]);

            if self.parsing_damage_inner(&headless, false, true) {
                self.seen_embedded_hexes.insert(raw_key);
                parsed_any = true;
                search_offset += 2;
            } else {
                search_offset += 1;
            }
        }
        parsed_any
    }

    // ===== DAMAGE PARSING =====

    pub(super) fn parsing_damage(&mut self, packet: &[u8], allow_embedded_scan: bool, require_trusted: bool) -> bool {
        self.parsing_damage_inner(packet, allow_embedded_scan, require_trusted)
    }

    fn parsing_damage_inner(&mut self, packet: &[u8], allow_embedded_scan: bool, require_trusted: bool) -> bool {
        let length_info = read_varint(packet, 0);
        if length_info.length < 0 {
            return false;
        }
        let mut offset = length_info.length as usize;

        if offset >= packet.len() || offset + 1 >= packet.len() {
            return false;
        }

        // STRICT GATEKEEPER: 04 38
        if packet[offset..offset + 2] != DAMAGE {
            if allow_embedded_scan {
                return self.try_parse_embedded_damage_packet(packet);
            }
            return false;
        }
        offset += 2;

        let mut parsed_any = false;
        let mask = 0x0F;

        while offset < packet.len() {
            // Chained hit marker
            let mut is_chained = false;
            if offset + 1 < packet.len() && packet[offset] == 0x01 && packet[offset + 1] == 0x00 {
                offset += 2;
                is_chained = true;
            }

            if parsed_any && !is_chained {
                break;
            }

            // Target. `>= 100` is a resync gate for the varint walk, not a real
            // protocol bound — the game does hand out sub-100 entity ids, and a
            // player who draws one had every hit they took (and dealt, below)
            // silently dropped. Ids a spawn or identity record has confirmed are
            // let through; unconfirmed small values still bail out, so the gate
            // keeps doing its job.
            let target_value = match try_read_varint(packet, &mut offset) {
                Some(v) if self.data_storage.is_plausible_entity_id(v) => v,
                _ => { break; }
            };

            // Switch value
            let switch_value = match try_read_varint(packet, &mut offset) {
                Some(v) => v,
                None => break,
            };
            let layout = switch_value & mask;
            // Switch bit 0x04: the record has a value. Without it, the hit
            // type says why: a miss or a resist, read below and counted.
            let no_value = matches!(layout, 0 | 2);

            if !(4..=7).contains(&layout) && !no_value {
                break;
            }

            // Unused flag
            if try_read_varint(packet, &mut offset).is_none() { break; }

            // Actor (same gate as the target above).
            let actor_value = match try_read_varint(packet, &mut offset) {
                Some(v) if self.data_storage.is_plausible_entity_id(v) => v,
                _ => { break; }
            };

            // Exact 4-byte skill ID
            if offset + 4 > packet.len() {
                break;
            }
            let mut exact_skill_code = i64::from(packet[offset] as u32)
                | (i64::from(packet[offset + 1] as u32) << 8)
                | (i64::from(packet[offset + 2] as u32) << 16)
                | (i64::from(packet[offset + 3] as u32) << 24);
            offset += 4;

            // Theostone raw item IDs
            if THEOSTONE_ITEM_IDS.contains(&exact_skill_code) {
                exact_skill_code = exact_skill_code * 10 + 1;
            }

            if !(1..=299_999_999).contains(&exact_skill_code) {
                break;
            }

            // Skip 7-digit NPC skills
            if MONSTER_SKILL_CODES.contains(&exact_skill_code) {
                break;
            }

            // Skip 1-byte UID field
            if offset < packet.len() {
                offset += 1;
            }

            let hit_type = match try_read_varint(packet, &mut offset) {
                Some(v) => v,
                None => break,
            };
            let damage_type = hit_type as u8;

            // Hit type 1 (Miss) and 6 (Resist) carry no damage: count them on
            // the skill and stop here, as the parser always did on these.
            if no_value {
                if !require_trusted && actor_value != target_value {
                    if let Some(kind) = NoDamageHit::from_hit_type(hit_type) {
                        let skill = self.normalize_skill_id(exact_skill_code as i32);
                        let counted = self.data_storage.append_no_damage_hit(target_value, actor_value, skill, kind);
                        tracing::trace!(
                            target: "hit_flags",
                            "{} actor={actor_value} target={target_value} skill={skill} damage=- type={hit_type} layout={layout} counted={counted}",
                            crate::clock::now_ms(),
                        );
                    }
                }
                break;
            }

            let fixed_tail_len: usize = match layout {
                5 => 12,
                6 => 10,
                7 => 14,
                _ => 8,
            };

            // Switch bit 0x02: the game's damage plotter follows the hit type,
            // `<flags byte> <restoration HP varint> <angle byte>`. The HP is
            // two bytes from 128 up; `fixed_tail_len` counts it as one.
            let mut specials = Vec::new();
            // The raw flag bytes, for `A2_REPLAY_FLAGS` in the replay report.
            let mut raw_mods: Option<u8> = None;
            let mut raw_dir: Option<u8> = None;
            let mut raw_hp: Option<i32> = None;
            let mut plotter_extra = 0;
            if layout & 0x02 != 0 {
                let Some(plotter) = read_plotter(packet, offset) else { break };
                // Flags byte, bit for bit the game's plotter fields. Verified
                // per skill against the game's Damage Analyzer: Perfect 0x04,
                // Double 0x08 (the game's HardHit, 강타). From the 2026-10-04
                // captures (hit sizes, issue #5): Shield Block 0x01, Parry 0x02,
                // Iron Wall 0x10, Regeneration 0x20, Perfect Block 0x40. 0x80
                // is not a plotter field: it mirrors switch bit 0x10 and is
                // fixed per skill.
                raw_mods = Some(plotter.flags);
                specials = special_damage::from_hit_flags(plotter.flags);
                // Angle byte. Verified against the combat log and the game's
                // Damage Analyzer (BackAttackCount, FrontAttackCount per skill):
                // 0x00 = no positional tag, 0x01 = Back, 0x02 = Front.
                raw_dir = plotter.angle;
                raw_hp = Some(plotter.hp);
                match plotter.angle {
                    Some(0x01) => specials.push(SpecialDamage::Back),
                    Some(0x02) => specials.push(SpecialDamage::Frontal),
                    _ => {}
                }
                plotter_extra = plotter.hp_len - 1;
            }
            // Damage type 3 = Critical, verified per skill against CriticalCount.
            if damage_type == 3 {
                specials.push(SpecialDamage::Critical);
            }

            offset += fixed_tail_len + plotter_extra;
            if offset >= packet.len() {
                break;
            }

            // Struct data extraction
            let mut first_value = match try_read_varint(packet, &mut offset) {
                Some(v) => v,
                None => break,
            };
            let mut after_first_offset = offset;
            let mut second_value = match try_read_varint(packet, &mut offset) {
                Some(v) => v,
                None => break,
            };

            // Post-2026-06 layout shift: these records now carry a leading zero
            // pad plus a POWER SCALAR ahead of the real value, so the damage lands
            // one varint later than the parser historically expected. A
            // `first_value` of 0 is that pad — realign by one varint (first <- the
            // scalar, second <- the real value). Verified live: a Power Burst crit
            // read the scalar instead of its true 60876, which sits in this next
            // varint.
            //
            // The scalar is not a constant marker (as this once assumed): it is
            // the actor's damage multiplier in hundredths of a percent — mobs read
            // 10000 (= 100.00%), geared players 16000-22000 — and it shifts with
            // buffs.
            if first_value == 0 {
                let after_second_offset = offset;
                if let Some(third) = try_read_varint(packet, &mut offset) {
                    first_value = second_value;
                    after_first_offset = after_second_offset;
                    second_value = third;
                }
            }

            let first_is_damage = should_treat_first_value_as_damage(first_value, second_value, layout, damage_type as i32);

            let mut final_damage = if first_is_damage {
                offset = after_first_offset;
                first_value
            } else {
                second_value
            };

            // The tail after the value, checked against the game's own Damage
            // Analyzer record of the same fight (2026-10-04): layout 4 carries
            // one varint, then switch bit 0x20 marks a hit that triggered
            // additional hits, as a count and that many damage values which the
            // value above already includes. Records that do not end cleanly
            // this way keep the older reading below.
            let strict_tail = if [4, 6].contains(&layout) && exact_skill_code != i64::from(COMPACT_AGGREGATE_SKILL) {
                parse_hit_tail(packet, offset, layout, switch_value, final_damage)
            } else {
                None
            };

            // Multi-hit extra field
            if strict_tail.is_none() && (switch_value & 0x30) == 0x30 && offset < packet.len() {
                try_read_varint(packet, &mut offset);
            }

            let mut hit_count = 0;
            let pre_hit_offset = offset;

            if strict_tail.is_none() && offset < packet.len() {
                let is_marker_next = offset + 1 < packet.len()
                    && packet[offset + 1] == 0x00
                    && (1..=7).contains(&(packet[offset] as i32));

                if !is_marker_next {
                    if let Some(peek_val) = try_read_varint(packet, &mut offset) {
                        if (0..=25).contains(&peek_val) {
                            hit_count = peek_val;
                        } else {
                            let is_marker_after = offset + 1 < packet.len()
                                && packet[offset + 1] == 0x00
                                && (1..=7).contains(&(packet[offset] as i32));
                            if !is_marker_after {
                                if let Some(actual) = try_read_varint(packet, &mut offset) {
                                    if (0..=25).contains(&actual) {
                                        hit_count = actual;
                                    } else {
                                        offset = pre_hit_offset;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if final_damage < 0 || final_damage > 99_999_999 {
                break;
            }

            // Extract multi-hits
            let mut multi_hit_count = 0;
            // i64: up to 25 additional hits of up to 99,999,999 each.
            let mut multi_hit_damage: i64 = 0;
            let mut first_multi_hit_value: Option<i32> = None;
            let mut all_multi_hits_match = true;

            if strict_tail.is_none() && hit_count > 0 && offset < packet.len() {
                let safe_max = std::cmp::min(hit_count, 25);
                let multi_hit_cap = std::cmp::max(final_damage, 500_000);
                let mut hits_read = 0;

                while hits_read < safe_max && offset < packet.len() {
                    let is_marker_next = offset + 1 < packet.len()
                        && packet[offset + 1] == 0x00
                        && (1..=7).contains(&(packet[offset] as i32));
                    let is_next_packet = offset + 1 < packet.len()
                        && packet[offset..offset + 2] == DAMAGE;

                    if is_marker_next || is_next_packet {
                        break;
                    }

                    let hit_value = match try_read_varint(packet, &mut offset) {
                        Some(v) => v,
                        None => break,
                    };

                    if hit_value > multi_hit_cap || hit_value < 50 {
                        multi_hit_damage = 0;
                        first_multi_hit_value = None;
                        all_multi_hits_match = true;
                        break;
                    }

                    match first_multi_hit_value {
                        None => first_multi_hit_value = Some(hit_value),
                        Some(fv) if fv != hit_value => all_multi_hits_match = false,
                        _ => {}
                    }

                    multi_hit_damage += i64::from(hit_value);
                    hits_read += 1;
                }
                multi_hit_count = hits_read;
            }

            if strict_tail.is_none() && switch_value == MULTI_HIT_SWITCH && hit_count > multi_hit_count && multi_hit_count == 1 {
                if let Some(fv) = first_multi_hit_value {
                    if all_multi_hits_match {
                        multi_hit_count = hit_count;
                        multi_hit_damage = i64::from(fv) * i64::from(hit_count);
                    }
                }
            }

            if strict_tail.is_none() && should_use_repeated_hit_damage(switch_value, second_value, multi_hit_count, first_multi_hit_value, all_multi_hits_match) {
                final_damage = first_multi_hit_value.unwrap();
            }

            if let Some((end, field, count, damage)) = strict_tail {
                offset = end;
                hit_count = field;
                multi_hit_count = count;
                multi_hit_damage = i64::from(damage);
            }

            if multi_hit_count > 0 && multi_hit_damage > 0 && i64::from(final_damage) > multi_hit_damage {
                // Smaller than final_damage here, so it fits back into i32.
                final_damage -= multi_hit_damage as i32;
            }

            // Compact skill context handling
            let pending = self.pending_compact_skill_context.clone();
            let aggregated_compact = pending.as_ref().is_some_and(|ctx| {
                exact_skill_code as i32 == COMPACT_AGGREGATE_SKILL
                    && actor_value == ctx.actor_id
                    && hit_count > 1
                    && multi_hit_damage > 0
                    && i64::from(second_value) > multi_hit_damage
            });

            let raw_for_spec = if aggregated_compact {
                pending.as_ref().unwrap().skill_raw
            } else {
                exact_skill_code as i32
            };
            let spec_flags = decode_spec_flags(raw_for_spec);
            let resolved_skill_code = if aggregated_compact {
                pending.as_ref().unwrap().skill_raw
            } else {
                self.normalize_skill_id(exact_skill_code as i32)
            };

            if aggregated_compact {
                final_damage = (i64::from(second_value) - multi_hit_damage) as i32;
                self.pending_compact_skill_context = None;
            }

            // Heal/life-steal suffix: [0x03, 0x00] marker + HealAmount VarInt
            let mut heal_amount = 0;
            if offset + 1 < packet.len()
                && packet[offset] == 0x03
                && packet[offset + 1] == 0x00
            {
                offset += 2;
                if let Some(heal_val) = try_read_varint(packet, &mut offset) {
                    if heal_val > 0 && heal_val < 10_000_000 {
                        heal_amount = heal_val;
                    }
                }
            }

            if require_trusted && !self.is_trusted_recovered_damage_shape(actor_value, target_value, hit_type as u8, final_damage, resolved_skill_code) {
                break;
            }

            if actor_value != target_value {
                let mut pdp = ParsedDamagePacket::new();
                if let Some(ts) = self.override_timestamp {
                    pdp.set_timestamp(ts);
                }
                pdp.set_target_id(target_value);
                pdp.set_actor_id(actor_value);
                pdp.set_skill_code(resolved_skill_code);
                pdp.set_spec_flags(spec_flags);
                pdp.set_type(hit_type);
                pdp.set_specials(specials);
                pdp.set_multi_hit_count(multi_hit_count);
                pdp.set_multi_hit_damage(multi_hit_damage);
                pdp.set_heal_amount(heal_amount);
                pdp.set_damage(final_damage);
                tracing::trace!(
                    target: "hit_flags",
                    "{} actor={actor_value} target={target_value} skill={resolved_skill_code} damage={final_damage} type={hit_type} layout={layout} mods={} dir={} multi={multi_hit_count} hp={}",
                    pdp.timestamp(),
                    raw_mods.map_or("-".to_string(), |m| format!("{m:#04x}")),
                    raw_dir.map_or("-".to_string(), |d| format!("{d:#04x}")),
                    raw_hp.map_or("-".to_string(), |h| h.to_string()),
                );

                self.data_storage.append_damage(pdp);
            } else if final_damage > 1 && self.data_storage.is_known_player(actor_value) {
                // Self-cast `04 38` record from a known player: an instant SELF-HEAL
                // (Radiant Recovery / Absolution / Healing Light etc.). The general
                // parser drops actor==target as self-damage, but for these records the
                // `E6 6F` field is read as first_value and the real heal lands in
                // second_value, so `final_damage` is the correct heal amount. Recording
                // it as healing makes the HEAL view capture instant self-heals, not just
                // HoTs. (The cast-marker variant breaks out earlier on its layout.)
                self.data_storage
                    .append_heal(
                        actor_value,
                        resolved_skill_code,
                        final_damage as i64,
                        false,
                        self.override_timestamp.unwrap_or_else(crate::clock::now_ms),
                    );
            }

            parsed_any = true;
        }

        parsed_any
    }

    pub(super) fn extract_pending_compact_skill_context(&self, packet: &[u8]) -> Option<PendingCompactSkillContext> {
        let length_info = read_varint(packet, 0);
        if length_info.length <= 0 || length_info.length as usize >= packet.len() {
            return None;
        }
        let body = &packet[length_info.length as usize..];

        // Find marker: 08 3B/3D 38 00 00
        let mut marker_index: Option<usize> = None;
        for idx in 0..body.len().saturating_sub(4) {
            if body[idx] == 0x08
                && (body[idx + 1] == 0x3B || body[idx + 1] == 0x3D)
                && body[idx + 2] == 0x38
                && body[idx + 3] == 0x00
                && body[idx + 4] == 0x00
            {
                marker_index = Some(idx);
                break;
            }
        }
        let marker_index = marker_index?;

        // Find compact opcode 38
        let mut compact_opcode: Option<usize> = None;
        for idx in (marker_index + 5)..body.len() {
            if body[idx] == 0x38 {
                compact_opcode = Some(idx);
                break;
            }
        }
        let compact_opcode = compact_opcode?;
        if compact_opcode + 2 >= body.len() {
            return None;
        }

        let actor_info = read_varint(body, compact_opcode + 1);
        if actor_info.length <= 0 || actor_info.value < 100 {
            return None;
        }

        let uid_offset = compact_opcode + 1 + actor_info.length as usize;
        if uid_offset >= body.len() {
            return None;
        }
        let skill_offset = uid_offset + 1;
        if skill_offset + 3 > body.len() {
            return None;
        }

        let mut candidates = Vec::new();
        if skill_offset + 4 <= body.len() {
            let full_skill = parse_u32_le(body, skill_offset) as i32;
            candidates.push(full_skill);
        }
        let compact_skill = (body[skill_offset] as i32)
            | ((body[skill_offset + 1] as i32) << 8)
            | ((body[skill_offset + 2] as i32) << 16);
        candidates.push(compact_skill);

        for candidate in candidates {
            if self.is_known_skill_code(candidate) {
                return Some(PendingCompactSkillContext {
                    actor_id: actor_info.value,
                    skill_raw: self.normalize_skill_id(candidate),
                });
            }
        }
        None
    }

    // ===== HELPERS =====

    fn normalize_skill_id(&self, raw: i32) -> i32 {
        crate::entity::skill_group::row_skill(raw, &self.skill_lookup)
    }

    fn is_known_skill_code(&self, skill_code: i32) -> bool {
        if !is_valid_skill_code(skill_code) {
            return false;
        }
        let normalized = self.normalize_skill_id(skill_code);
        if !is_valid_skill_code(normalized) {
            return false;
        }
        if (30_000_000..=30_999_999).contains(&normalized) {
            return true;
        }
        !self.skill_lookup.get_skill_name(normalized).is_empty()
            || !self.skill_lookup.get_skill_name(skill_code).is_empty()
    }

    fn is_trusted_recovered_damage_shape(&self, actor_id: i32, target_id: i32, damage_type: u8, damage: i32, skill_code: i32) -> bool {
        if actor_id == target_id || !(1..=3).contains(&(damage_type as i32)) || damage <= 0 {
            return false;
        }
        self.is_known_skill_code(skill_code)
    }
}

// ===== FREE FUNCTIONS =====

fn is_valid_skill_code(skill_code: i32) -> bool {
    (1..=299_999_999).contains(&skill_code)
}

fn decode_spec_flags(raw: i32) -> [bool; 5] {
    let mut result = [false; 5];
    if (30_000_000..=30_999_999).contains(&raw) {
        return result;
    }
    let mut suffix = (raw % 10000) / 10;
    if suffix <= 0 {
        return result;
    }
    while suffix > 0 {
        let slot = suffix % 10;
        if slot < 1 || slot > 5 {
            return [false; 5];
        }
        result[(slot - 1) as usize] = true;
        suffix /= 10;
    }
    result
}

fn should_treat_first_value_as_damage(first_value: i32, second_value: i32, layout: i32, damage_type: i32) -> bool {
    if !(1_000..=99_999_999).contains(&first_value) { return false; }
    if !(0..=25).contains(&second_value) { return false; }
    if first_value > 5_000_000 { return false; }
    layout == 6 && damage_type == 3
}

/// The tail of a damage record after its value, when it has the shape the
/// game's own record confirms: `(end offset, layout-4 field, additional hits,
/// their damage)`. `None` when the bytes do not end cleanly at the next record.
///
/// A spirit's layout-4 record has no layout-4 field: the additional hits follow
/// the value directly (2026-10-04, the game's AdditionalHitCount per spirit
/// skill matches only when read this way). A player's record has the field.
fn parse_hit_tail(packet: &[u8], offset: usize, layout: i32, switch_value: i32, value: i32) -> Option<(usize, i32, i32, i32)> {
    parse_hit_tail_as(packet, offset, layout, switch_value, value)
        .or_else(|| (layout == 4).then(|| parse_hit_tail_as(packet, offset, 6, switch_value, value)).flatten())
}

fn parse_hit_tail_as(packet: &[u8], mut offset: usize, layout: i32, switch_value: i32, value: i32) -> Option<(usize, i32, i32, i32)> {
    let mut field = 0;
    if layout == 4 {
        field = try_read_varint(packet, &mut offset)?;
        if !(1..=25).contains(&field) {
            return None;
        }
    }
    let (mut count, mut damage) = (0, 0i64);
    if switch_value & 0x20 != 0 {
        count = try_read_varint(packet, &mut offset)?;
        if !(1..=25).contains(&count) {
            return None;
        }
        for _ in 0..count {
            let hit = try_read_varint(packet, &mut offset)?;
            if hit < 0 {
                return None;
            }
            damage += i64::from(hit);
        }
        if damage >= i64::from(value) {
            return None;
        }
    }
    let rest = &packet[offset.min(packet.len())..];
    let clean_end = rest.is_empty()
        || (rest.len() >= 2 && rest[1] == 0x00 && (1..=7).contains(&rest[0]))
        || rest.starts_with(&DAMAGE);
    clean_end.then_some((offset, field, count, damage as i32))
}

/// The damage plotter after a record's hit type: the flags byte, the HP a
/// Regeneration hit restored, and the angle byte (absent at the very end).
pub(super) struct Plotter {
    pub(super) flags: u8,
    pub(super) hp: i32,
    pub(super) hp_len: usize,
    pub(super) angle: Option<u8>,
}

pub(super) fn read_plotter(packet: &[u8], offset: usize) -> Option<Plotter> {
    let flags = *packet.get(offset)?;
    let mut at = offset + 1;
    let hp = try_read_varint(packet, &mut at)?;
    Some(Plotter { flags, hp, hp_len: at - offset - 1, angle: packet.get(at).copied() })
}

fn should_use_repeated_hit_damage(switch_value: i32, encoded_damage: i32, multi_hit_count: i32, first_multi_hit_value: Option<i32>, all_match: bool) -> bool {
    let repeated = match first_multi_hit_value {
        Some(v) => v,
        None => return false,
    };
    if switch_value != MULTI_HIT_SWITCH { return false; }
    if multi_hit_count <= 0 || !all_match { return false; }
    // i64: 25 hits of up to 99,999,999 do not fit an i32.
    let main_component = i64::from(encoded_damage) - i64::from(multi_hit_count) * i64::from(repeated);
    if main_component > i64::from(repeated) { return false; }
    encoded_damage / 10 == repeated
}

fn to_hex_range(bytes: &[u8], start: usize, end: usize) -> String {
    let s = start.min(bytes.len());
    let e = end.min(bytes.len());
    bytes[s..e].iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ")
}
