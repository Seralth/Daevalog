//! Damage, damage over time and the heals that ride on damage records.

use super::{PendingCompactSkillContext, StreamProcessor};
use crate::capture::opcodes::{DAMAGE, DOT};
use crate::capture::varint::{parse_u32_le, read_varint, try_read_varint};
use crate::combat::data_storage::NoDamageHit;
use crate::entity::damage_packet::ParsedDamagePacket;
use crate::entity::special_damage::{self, SpecialDamage};
use crate::entity::taken::{TakenHit, TakenKind};

/// Theostone raw item ids, read as skill codes after `* 10 + 1`.
const THEOSTONE_ITEM_IDS: std::ops::RangeInclusive<i64> = 3_000_000..=3_099_999;
/// 7-digit skill codes are monster (NPC) skills.
const MONSTER_SKILL_CODES: std::ops::RangeInclusive<i64> = 1_000_000..=9_999_999;
/// The skill code of a compact aggregation record; its real skill comes from
/// `PendingCompactSkillContext`.
const COMPACT_AGGREGATE_SKILL: i32 = 99_745_942;
/// `05 38` effect types read for damage taken only: an immune, and damage
/// reflected onto a player (the `04 38` record beside it has hit type 9 and
/// no skill; the tick names the reflecting skill, 2026-10-06, Lakshmi).
const IMMUNE_EFFECT: u32 = 0x30;
const REFLECT_EFFECT: u32 = 0x4A;
/// The code of an immune record that is the game's Immune.
const IMMUNE_CODE: u32 = 2001;
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
        if effect_type == IMMUNE_EFFECT {
            self.parse_immune(packet, target_info.value, offset);
            return;
        }
        // Effect type: 0x02/0x0A = damage; 0x01/0x09 = heal; 0x0B = HoT.
        // 0x00 = status, 0x08 = buff. Exact match, NOT bitmask — 0x0B (HoT) has bit
        // 1 set and would leak through a mask.
        let is_damage = effect_type == 0x02 || effect_type == 0x0A;
        let is_heal = effect_type == 0x01 || effect_type == 0x09 || effect_type == 0x0B;
        let is_reflect = effect_type == REFLECT_EFFECT;
        if !is_damage && !is_heal && !is_reflect {
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

        // A monster's damage over time on a player, or damage reflected onto
        // one, is damage taken. A tick of a player's skill is not: a Chanter
        // skill (18730002) that heals 347 as effect 0x09 also came as effect
        // 0x0A ticks of 347 naming the monster that had just hit, and the
        // game's record counts none of them (2026-10-06, Saraswati). Nor is a
        // Theostone's tick, whose item id is 7 digits too.
        let code = i64::from(skill_code);
        if (is_damage || is_reflect) && MONSTER_SKILL_CODES.contains(&code) && !THEOSTONE_ITEM_IDS.contains(&code) {
            let at = self.override_timestamp.unwrap_or_else(crate::clock::now_ms);
            let kind = if is_reflect { TakenKind::Reflect } else { TakenKind::Tick };
            self.data_storage.append_taken(TakenHit::plain(
                at,
                target_info.value,
                actor_info.value,
                skill_code,
                i64::from(amount_info.value),
                kind,
            ));
        }
        if is_reflect {
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

    /// `05 38 <target> 30 <target> <n> <code u32> <attacker> <skill u32>`:
    /// the target was immune to the attacker's skill. Code 2001 is the
    /// game's Immune (ImmuneCount, two records of 2026-10-05 and -06). In the
    /// check kit's 14 captures code 100000183 came on summons only, and four
    /// records with other codes on players were not 2001.
    fn parse_immune(&mut self, packet: &[u8], target: i32, mut offset: usize) {
        let Some(again) = try_read_varint(packet, &mut offset) else { return };
        if again != target || try_read_varint(packet, &mut offset).is_none() || offset + 4 > packet.len() {
            return;
        }
        let code = parse_u32_le(packet, offset);
        offset += 4;
        let Some(attacker) = try_read_varint(packet, &mut offset) else { return };
        if code != IMMUNE_CODE || offset + 4 > packet.len() {
            return;
        }
        let skill = parse_u32_le(packet, offset) as i64;
        if !MONSTER_SKILL_CODES.contains(&skill) || !self.data_storage.is_plausible_entity_id(attacker) {
            return;
        }
        let at = self.override_timestamp.unwrap_or_else(crate::clock::now_ms);
        self.data_storage.append_taken(TakenHit::plain(at, target, attacker, skill as i32, 0, TakenKind::Immune));
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

            // 7-digit skills are monsters' (NPC) skills: damage taken. A
            // record found inside another packet is left out, as before.
            let monster = MONSTER_SKILL_CODES.contains(&exact_skill_code);
            if monster && require_trusted {
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
                if monster {
                    let kind = match NoDamageHit::from_hit_type(hit_type) {
                        Some(NoDamageHit::Miss) => Some(TakenKind::Miss),
                        Some(NoDamageHit::Resist) => Some(TakenKind::Resist),
                        None => None,
                    };
                    if let Some(kind) = kind {
                        let at = self.override_timestamp.unwrap_or_else(crate::clock::now_ms);
                        let skill = exact_skill_code as i32;
                        self.data_storage.append_taken(TakenHit::plain(at, target_value, actor_value, skill, 0, kind));
                    }
                } else if !require_trusted && actor_value != target_value {
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
                // is a Power Shard hit: one shard leaves the bag per flagged
                // cast (counted against the bag, 2026-10-07).
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
            // the actor's combat speed in hundredths of a percent, 10000 plus the
            // CombatSpeed stat (282). In the check kit's 14 captures it matched
            // the local player's stat on 9,091 of 9,096 hits, the other five
            // within 250 ms of a speed change, and it did not move with Damage
            // Boost or PvE Damage Boost. Players' hits read 10000 to 17534.
            let mut pad = false;
            if first_value == 0 {
                let after_second_offset = offset;
                if let Some(third) = try_read_varint(packet, &mut offset) {
                    first_value = second_value;
                    after_first_offset = after_second_offset;
                    second_value = third;
                    pad = true;
                }
            }

            let first_is_damage = should_treat_first_value_as_damage(first_value, second_value, layout, damage_type as i32);
            // The scalar, for `A2_REPLAY_FLAGS` in the replay report.
            let scalar = (pad && !first_is_damage).then_some(first_value);

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

            if monster {
                // Damage taken: the record's value with its additional hits,
                // the plotter's flags and angle, and the hit type's crit.
                if actor_value != target_value {
                    let damage = i64::from(final_damage) + multi_hit_damage;
                    tracing::trace!(
                        target: "taken_flags",
                        "{} actor={actor_value} target={target_value} skill={exact_skill_code} damage={damage} type={hit_type} layout={layout} mods={} dir={} hp={}",
                        self.override_timestamp.unwrap_or_else(crate::clock::now_ms),
                        raw_mods.map_or("-".to_string(), |m| format!("{m:#04x}")),
                        raw_dir.map_or("-".to_string(), |d| format!("{d:#04x}")),
                        raw_hp.map_or("-".to_string(), |h| h.to_string()),
                    );
                    self.data_storage.append_taken(TakenHit {
                        at: self.override_timestamp.unwrap_or_else(crate::clock::now_ms),
                        target: target_value,
                        actor: actor_value,
                        skill: exact_skill_code as i32,
                        damage,
                        kind: TakenKind::Hit,
                        flags: raw_mods.unwrap_or(0),
                        angle: raw_dir.unwrap_or(0),
                        crit: damage_type == 3,
                        restored: raw_hp.map_or(0, i64::from),
                    });
                }
            } else if crate::entity::skill_group::restores_resource(exact_skill_code as i32) {
                // MP (or another resource) restored, not HP: neither damage
                // nor healing. A Water Spirit's attack sends one of these to
                // its Spiritmaster (16990002, 20 MP), filed under 100011.
            } else if actor_value != target_value {
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
                    "{} actor={actor_value} target={target_value} skill={resolved_skill_code} damage={final_damage} type={hit_type} layout={layout} mods={} dir={} multi={multi_hit_count} multi_dmg={multi_hit_damage} hp={} scalar={}",
                    pdp.timestamp(),
                    raw_mods.map_or("-".to_string(), |m| format!("{m:#04x}")),
                    raw_dir.map_or("-".to_string(), |d| format!("{d:#04x}")),
                    raw_hp.map_or("-".to_string(), |h| h.to_string()),
                    scalar.map_or("-".to_string(), |s| s.to_string()),
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::capture::stream_processor::StreamProcessor;
    use crate::combat::data_storage::{DataStorage, TakenTick};
    use crate::entity::taken::TakenStats;
    use crate::game_record::TakeStat;
    use crate::i18n::lookup::{NpcLookup, SkillLookup};

    /// The frames that name player 9200 with a 7-digit skill or none, in the
    /// windows of three of the game's Damage Analyzer records of 2026-10-06
    /// (the Kernon capture), as ms from the window's start: monster hits with
    /// and without a value, resists, reflects, damage over time and immune
    /// records, the player's own item skills, a Chanter ward's ticks, and the
    /// skill-effect entities' spawns. Bakarma's immune came from an effect
    /// whose spawn carries a character name; that spawn is left out.
    const SARASWATI: &[(i64, &str)] = &[
        (627, "1e0438f0470000f047193429003402cf59181001000000904e0100"),
        (627, "1e0438f0470000f04739b9210035024f5a2c0d01000000904e0100"),
        (2625, "1e0438f0470000f04761ba21003902efcd2c0d01000000904e0100"),
        (22731, "1f0438f0470000f1d4018c9812000902bb96430701000000904e0100"),
        (23728, "180538f0470af1d401bb0113b7a36fdb0212cc1d01"),
        (23728, "290438f0474600f1d401919812000f02000002af98430701000000904e8c040113b7a36f0100"),
        (23728, "1f0438f0470000f1d401919812000f02b098430701000000904e0100"),
        (23728, "1f0438f0470000f1d401919812000f02b198430701000000904e0100"),
        (25825, "180538f0470af1d4018602f3112707b10291981200"),
        (27176, "180538f0470af1d4018b0213b7a36fdb0212cc1d01"),
        (27176, "290438f0474600f1d401aa98120011020000027da2430702000000904e83060113b7a36f0200"),
        (27176, "240438f0470001f1d401aa98120011067ea2430702000000904e01a5f6b9000200"),
        (27525, "240438f0470600f1d401aa98120011020000027da2430703000000904e92060300"),
        (27525, "1f0438f0470000f1d401aa98120011027ea2430703000000904e0300"),
        (27781, "180538f0470af1d4018602f3112707b10291981200"),
        (29775, "180538f0470af1d4018602f3112707b10291981200"),
        (30875, "1e0438f0470000f04705d31e00d502ff6d0a0c01000000904e0100"),
        (32176, "1f0438f0470000f1d40149420f0013028fe4f50501000000904e0100"),
        (32176, "b9014136eac1011c0000708e2c000002176bc6c6e1f7f74500c8d7c53418b142f83e01e0c65be0c65b640000006400000000000000000000000000000000000000b80b00000000000001000000000000000000000000000000000000000602110181969800ffffffffffffffff8075d52abb030000eac1010104176bc6c6e1f7f74500c8d7c5110284969800ffffffffffffffff8075d52abb030000eac10101176bc6c6e1f7f74500c8d7c506716a0000002d00000000"),
        (35325, "180538f0470aeac101990213b7a36fdb0212cc1d01"),
        (35325, "260438f0474400eac101829812000403d392430701000000904edf040113b7a36f0100"),
    ];
    const LAKSHMI: &[(i64, &str)] = &[
        (1127, "1e0438f0470000f04761ba21003502efcd2c0d01000000904e0100"),
        (7475, "1b0538f0474ae7ab011ea36c9209f4032e4bf40016811800"),
        (7475, "1b0538f0474ae7ab011ea36c9209f403a16eff0016811800"),
        (7475, "180538f0470ae7ab01cb0313b7a36fdb0212cc1d01"),
        (7475, "250438f0474400e7ab01000000000009035e6d5f000000000099010113b7a36f0000"),
        (7475, "200438f0470400e7ab01000000000009ef36c7630000000000f4030000"),
        (7626, "1b0538f0474ae7ab011ea36c9209f40330c1f40016811800"),
        (7626, "200438f0470400e7ab01000000000009d1769b5f0000000000f4030000"),
        (8224, "1b0538f0474ae7ab011ea36c9209f4033e72f40016811800"),
        (8224, "200438f0470400e7ab0100000000000943a07c5f0000000000f4030000"),
        (8326, "1b0538f0474ae7ab011ea36c9209f40351c9f90016811800"),
        (8326, "200438f0470400e7ab01000000000009afa392610000000000f4030000"),
        (8382, "1b0538f0474ae7ab011ea36c9209f40330c1f40016811800"),
        (8382, "1b0538f0474ae7ab011ea36c9209f4030159000116811800"),
        (8382, "200438f0470400e7ab01000000000009d1769b5f0000000000f4030000"),
        (8382, "200438f0470400e7ab010000000000096fc422640000000000f4030000"),
        (8476, "1b0538f0474ae7ab011ea36c9209f4039147ff0016811800"),
        (8476, "200438f0470400e7ab01000000000009b0f4b7630000000000f4030000"),
        (9074, "1b0538f0474ae7ab011ea36c9209f4034e99f40016811800"),
        (9074, "200438f0470400e7ab0100000000000983e28b5f0000000000f4030000"),
        (9125, "1e0438f0470000f047ddaf1e0068025fb2fc0b01000000904e0100"),
        (9232, "1b0538f0474ae7ab011ea36c9209f40330c1f40016811800"),
        (9232, "240438f0470600e7ab014881180008020000023580920902000000904e83060200"),
        (9232, "1f0438f0470000e7ab014881180008023680920902000000904e0200"),
        (9232, "200438f0470400e7ab01000000000009d1769b5f0000000000f4030000"),
        (9376, "1b0538f0474ae7ab011ea36c9209f40351c9f90016811800"),
        (9376, "200438f0470400e7ab01000000000009afa392610000000000f4030000"),
        (9584, "1b0538f0474ae7ab011ea36c9209f4032b2df90016811800"),
        (9584, "200438f0470400e7ab01000000000009d7a455610000000000f4030000"),
        (9874, "1b0538f0474ae7ab011ea36c9209f4032e4bf40016811800"),
        (9874, "200438f0470400e7ab01000000000009035e6d5f0000000000f4030000"),
        (9974, "1b0538f0474ae7ab011ea36c9209f4039147ff0016811800"),
        (9974, "1b0538f0474ae7ab011ea36c9209ec020159000116811800"),
        (9974, "200438f0470400e7ab01000000000009b0f4b7630000000000f4030000"),
        (9974, "200438f0470400e7ab010000000000096fc422640000000000ec020000"),
        (10676, "1b0538f0474ae7ab011ea36c9209f4033e72f40016811800"),
        (10676, "200438f0470400e7ab0100000000000943a07c5f0000000000f4030000"),
        (10876, "1b0538f0474ae7ab011ea36c9209f40330c1f40016811800"),
        (10876, "200438f0470400e7ab01000000000009d1769b5f0000000000f4030000"),
        (11075, "1b0538f0474ae7ab011ea36c9209f403a16eff0016811800"),
        (11075, "200438f0470400e7ab01000000000009ef36c7630000000000f4030000"),
        (11574, "1b0538f0474ae7ab011ea36c9209f4036c47f60016811800"),
        (11574, "200438f0470400e7ab010000000000093be633600000000000f4030000"),
        (11727, "1b0538f0474ae7ab011ea36c9209f4039147ff0016811800"),
        (11727, "200438f0470400e7ab01000000000009b0f4b7630000000000f4030000"),
        (11876, "1b0538f0474ae7ab011ea36c9209f40330c1f40016811800"),
        (11876, "200438f0470400e7ab01000000000009d1769b5f0000000000f4030000"),
        (11976, "1b0538f0474ae7ab011ea36c920998030159000116811800"),
        (11976, "200438f0470400e7ab010000000000096fc42264000000000098030000"),
        (12129, "1b0538f0474ae7ab011ea36c9209f403a16eff0016811800"),
        (12129, "200438f0470400e7ab01000000000009ef36c7630000000000f4030000"),
        (12929, "1e0438f0470000f04705d31e007d02ff6d0a0c01000000904e0100"),
        (13274, "1b0538f0474ae7ab011ea36c9209f4032e4bf40016811800"),
        (13274, "200438f0470400e7ab01000000000009035e6d5f0000000000f4030000"),
        (13429, "1b0538f0474ae7ab011ea36c9209f4039147ff0016811800"),
        (13429, "1b0538f0474ae7ab011ea36c9209f403242df90016811800"),
        (13429, "200438f0470400e7ab01000000000009b0f4b7630000000000f4030000"),
        (13429, "200438f0470400e7ab010000000000091ba255610000000000f4030000"),
        (13584, "1b0538f0474ae7ab011ea36c9209ad020159000116811800"),
        (13584, "200438f0470400e7ab010000000000096fc422640000000000ad020000"),
        (13784, "1b0538f0474ae7ab011ea36c9209f40330c1f40016811800"),
        (13784, "200438f0470400e7ab01000000000009d1769b5f0000000000f4030000"),
        (14664, "1b0538f0474ae7ab011ea36c9209f403a16eff0016811800"),
        (14664, "1b0538f0474ae7ab011ea36c9209c1020159000116811800"),
        (14664, "180538f0470ae7ab01810413b7a36fdb0212cc1d01"),
        (14664, "250438f0474400e7ab01000000000009ef36c763000000000099010113b7a36f0000"),
        (14664, "200438f0470400e7ab010000000000096fc422640000000000c1020000"),
        (14682, "1b0538f0474ae7ab011ea36c9209f40330c1f40016811800"),
        (14682, "200438f0470400e7ab01000000000009d1769b5f0000000000f4030000"),
        (14827, "1b0538f0474ae7ab011ea36c9209f4039147ff0016811800"),
        (14827, "200438f0470400e7ab01000000000009b0f4b7630000000000f4030000"),
        (15476, "200438f0470400f047f5ab1e009102bf2bfb0b01000000904eca0d0100"),
        (15476, "200438f0470400f047f5ab1e009102c02bfb0b01000000904ed7030100"),
        (18625, "1f0438f0470000e7ab0149420f000b028fe4f50501000000904e0100"),
    ];
    const BAKARMA: &[(i64, &str)] = &[
        (439, "240438f0470600d5b2013e791b000402000002435cbb0a03000000904ede090300"),
        (14277, "180538f0470ad5b201d30713b7a36fdb0212cc1d01"),
        (14277, "290438f0474600d5b20148791b0008020000022b60bb0a02000000904efe0f0113b7a36f0200"),
        (44578, "bb014136aee1011c0000428f2c00000280e39e4700229fc6004c38466d39a943adf001c08db701c08db701640000006400000000000000000000000000000000000000906500000000000001000000000000000000000000000000000000000602110181969800ffffffffffffffff8075d52abb030000aee101010480e39e4700229fc6004c3846110284969800ffffffffffffffff8075d52abb030000aee1010180e39e4700229fc6004c38460655590000002d00000000"),
        (49076, "210438f0470400aee101ca791b000402f392bb0a01000000904eb31c0100"),
        (49076, "1f0438f0470000aee101ca791b000402f492bb0a01000000904e0100"),
        (49677, "1e0438f0470000f047ddaf1e00d0025fb2fc0b01000000904e0100"),
        (53134, "1e0438f0470000f04705d31e00d102ff6d0a0c01000000904e0100"),
        (54226, "210438f0470400aee101ca791b000c02f392bb0a01000000904eb31c0100"),
        (54226, "1f0438f0470000aee101ca791b000c02f492bb0a01000000904e0100"),
        (56128, "200438f0470400f047f5ab1e00d802bf2bfb0b01000000904eca0d0100"),
        (56128, "200438f0470400f047f5ab1e00d802c02bfb0b01000000904ed7030100"),
        (65827, "210438f0470400d5b201d0791b0012024b95bb0a01000000904e962c0100"),
        (71527, "1f0438f0470000d5b2016a791b001402736dbb0a02000000904e0200"),
        (71627, "1f0438f0470000d5b20167791b001502ed6bbb0a01000000904e0100"),
        (72126, "1f0438f0470000d5b20167791b001902ed6bbb0a01000000904e0100"),
        (72577, "130538f04702f047cc09172c530bcf14"),
        (72677, "1f0438f0470000d5b20167791b001c02ed6bbb0a01000000904e0100"),
        (73134, "1f0438f0470000d5b20167791b001f02ed6bbb0a01000000904e0100"),
        (73585, "1f0438f0470000d5b20167791b002102ed6bbb0a01000000904e0100"),
        (74078, "1f0438f0470000d5b20167791b002502ed6bbb0a01000000904e0100"),
        (74677, "1f0438f0470000d5b20167791b002702ed6bbb0a01000000904e0100"),
        (75178, "1f0438f0470000d5b20167791b002a02ed6bbb0a01000000904e0100"),
        (75626, "1f0438f0470000d5b20167791b002d02ed6bbb0a01000000904e0100"),
        (76579, "180538f04730f047d709d1070000b5f4016f791b00"),
        (76927, "240438f0470600d5b2016a791b0014020000017d6dbb0a03000000904eb0150300"),
        (76927, "1f0438f0470000d5b2016a791b0014027e6dbb0a03000000904e0300"),
        (82176, "1e0438f0470000f04705d31e002d00ff6d0a0c01000000904e0100"),
        (82680, "1e0438f0470000f047ddaf1e002f025fb2fc0b01000000904e0100"),
        (87234, "200438f0470400f047f5ab1e004402bf2bfb0b01000000904eca0d0100"),
        (87234, "200438f0470400f047f5ab1e004402c02bfb0b01000000904ed7030100"),
        (103433, "240438f0470600d5b2015c791b0036020000020568bb0a01000000904e8e0b0100"),
    ];

    fn replay(frames: &[(i64, &str)], bosses: &[(i32, i32)]) -> Vec<TakenTick> {
        let storage = Arc::new(DataStorage::new());
        for &(id, code) in bosses {
            storage.append_mob(id, code);
        }
        let mut p = StreamProcessor::new(storage.clone(), Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
        for (at, hex) in frames {
            let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
            p.set_override_timestamp(Some(*at));
            assert_eq!(p.consume_stream(&bytes), bytes.len(), "{hex}");
        }
        p.set_override_timestamp(None);
        storage.taken_since(0)
    }

    fn took(ticks: &[TakenTick]) -> TakeStat {
        let mut s = TakenStats::default();
        for t in ticks.iter().filter(|t| t.hit.target == 9200) {
            s.add(&t.hit);
        }
        TakeStat::of_meter(&s)
    }

    fn game(damage: i64, counts: [i64; 12]) -> TakeStat {
        TakeStat { damage, counts }
    }

    /// The game's TakeStatData: total, accuracy, crit, perfect, double,
    /// front, back, block, miss, immune, iron wall, restoration.
    #[test]
    fn damage_taken_matches_three_boss_records() {
        // Predator Saraswati: four hits (one from its effect entity 24810,
        // a crit) and three Poison ticks. The resist, the hits without a
        // value and a Chanter ward's ticks are not counted.
        let ticks = replay(SARASWATI, &[(27249, 2_310_403)]);
        assert_eq!(took(&ticks), game(3603, [4, 4, 1, 0, 0, 3, 0, 0, 0, 0, 0, 0]));
        let hunt = ticks.iter().find(|t| t.hit.skill == 1_218_690).unwrap();
        assert_eq!((hunt.hit.actor, hunt.source, hunt.hit.damage, hunt.hit.crit), (24810, 27249, 607, true));
        assert_eq!(ticks.iter().filter(|t| t.hit.skill == 1_200_010).map(|t| t.hit.damage).sum::<i64>(), 915);

        // Phantasmal Lakshmi: one hit and 32 hits of reflected damage, read
        // from their effect 0x4a ticks.
        let ticks = replay(LAKSHMI, &[(21991, 2_310_401)]);
        assert_eq!(took(&ticks), game(16165, [33, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]));

        // Transcendent Bakarma: seven hits (two from its effect entity
        // 28846) and one immune.
        let ticks = replay(BAKARMA, &[(22869, 2_310_471)]);
        assert_eq!(took(&ticks), game(20374, [8, 7, 0, 0, 0, 3, 1, 0, 0, 1, 0, 0]));
        let rapid: Vec<_> = ticks.iter().filter(|t| t.hit.skill == 1_800_650).map(|t| (t.hit.actor, t.source)).collect();
        assert_eq!(rapid, vec![(28846, 22869); 2]);
    }

    /// The game record view replays a saved fight's slice: the damage taken
    /// is counted over the record's own window, past the end of the damage
    /// window too, for a player who dealt the target nothing.
    #[test]
    fn a_slice_replay_counts_the_damage_taken_over_the_record_window() {
        use std::collections::HashSet;

        use crate::game_record::{replay_slice_window, SliceWindow};

        let records: Vec<(i32, Vec<u8>)> = BAKARMA
            .iter()
            .map(|(at, hex)| (*at as i32, (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect()))
            .collect();
        let (skills, npcs) = (Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
        let replay_over = |until: i64, taken: (i64, i64)| {
            let w = SliceWindow { records: &records, fight_start_ms: 0, target_id: 22869, from: 0, until, owner: Some(9200), game_total: 0, taken };
            replay_slice_window(&w, &skills, &npcs, &HashSet::new()).taken
        };
        let all = replay_over(200_000, (0, 200_000));
        assert_eq!(TakeStat::of_meter(&all), game(20374, [8, 7, 0, 0, 0, 3, 1, 0, 0, 1, 0, 0]));
        assert_eq!(replay_over(1_000, (0, 200_000)), all, "read on past the damage window");
        let later: Vec<TakenTick> = replay(BAKARMA, &[]).into_iter().filter(|t| t.hit.at >= 50_000).collect();
        assert!(!later.is_empty() && later.len() < 9);
        assert_eq!(TakeStat::of_meter(&replay_over(200_000, (50_000, 200_000))), took(&later));
    }
}
