use std::collections::HashSet;
use std::sync::Arc;


use crate::combat::data_storage::DataStorage;
use crate::i18n::lookup::{NpcLookup, SkillLookup};

pub use fields::{name_fields, name_fields_unframed, NameField, NameRecord};
pub use super::varint::{
    can_read_varint, find_pattern, parse_u32_le, read_varint, try_read_varint, varint_ending_at, VarIntResult,
};

mod damage;
mod fields;
mod identity;
mod roster;
mod spawn;
mod world;

/// Context for resolving a compact skill aggregation packet.
#[derive(Debug, Clone)]
struct PendingCompactSkillContext {
    actor_id: i32,
    skill_raw: i32,
}

/// Bounded LRU set for deduplication.
struct BoundedHashSet {
    set: Vec<String>,
    max_size: usize,
}

impl BoundedHashSet {
    fn new(max_size: usize) -> Self {
        Self {
            set: Vec::new(),
            max_size,
        }
    }

    fn contains(&self, key: &str) -> bool {
        self.set.iter().any(|s| s == key)
    }

    fn insert(&mut self, key: String) -> bool {
        if self.contains(&key) {
            return false;
        }
        if self.set.len() >= self.max_size {
            self.set.remove(0);
        }
        self.set.push(key);
        true
    }
}

/// The core binary protocol parser for AION 2 game packets.
/// Ported exactly from Kotlin StreamProcessor.
pub struct StreamProcessor {
    data_storage: Arc<DataStorage>,
    skill_lookup: Arc<SkillLookup>,
    npc_lookup: Arc<NpcLookup>,
    seen_embedded_hexes: BoundedHashSet,
    dot_damage_skill_ids: HashSet<i32>,
    /// Damage ticks dropped for a skill not on `dot_damage_skill_ids`, logged
    /// once per skill.
    unknown_dot_skills: HashSet<i32>,
    pending_compact_skill_context: Option<PendingCompactSkillContext>,
    /// When set, override the timestamp on all created packets (for replay mode).
    override_timestamp: Option<i64>,
}

impl StreamProcessor {
    pub fn new(data_storage: Arc<DataStorage>, skill_lookup: Arc<SkillLookup>, npc_lookup: Arc<NpcLookup>) -> Self {
        Self {
            data_storage,
            skill_lookup,
            npc_lookup,
            seen_embedded_hexes: BoundedHashSet::new(16_384),
            dot_damage_skill_ids: HashSet::new(), // loaded lazily
            unknown_dot_skills: HashSet::new(),
            pending_compact_skill_context: None,
            override_timestamp: None,
        }
    }

    pub fn set_dot_skill_ids(&mut self, ids: HashSet<i32>) {
        self.dot_damage_skill_ids = ids;
    }

    /// Set an override timestamp for all packets created by this processor.
    /// Used in replay mode to use capture-time timestamps instead of wall clock.
    ///
    /// This also pins the thread's clock (`crate::clock`), because the processor
    /// is not the only thing that reads "now" while consuming a packet:
    /// `DataStorage` makes idle-reset and zone-reset decisions against it. Those
    /// used to be taken against the replaying machine's wall clock, so a replay
    /// of the same capture could produce different fights depending on when it
    /// was run. The override is thread-local, so a replay on a blocking thread
    /// cannot disturb a live capture running alongside it.
    pub fn set_override_timestamp(&mut self, ts: Option<i64>) {
        self.override_timestamp = ts;
        crate::clock::set_override(ts);
    }

    /// Parse as many complete packets as possible from the buffer.
    /// Returns the number of bytes consumed.
    pub fn consume_stream(&mut self, buffer: &[u8]) -> usize {
        // Framing lives in `capture::framing` so the Evidence Slice builder can
        // split a capture the same way this does. See that module.
        let framing = super::framing::walk(buffer);
        let offset = framing.consumed;

        for frame in &framing.frames {
            match frame.kind {
                super::framing::FrameKind::Bundle => {
                    self.unwrap_bundle(frame.payload(buffer), 1);
                }
                super::framing::FrameKind::Packet => {
                    self.parse_perfect_packet(frame.bytes(buffer));
                    self.scan_embedded_bundles_for_identity(frame.bytes(buffer));
                }
            }
        }

        // The scans below read only what was framed this pass. The tail is an
        // incomplete packet that is kept for the next read; scanning it here
        // read it again on every TCP segment until it completed (a party
        // roster was read 18 times in one millisecond).
        let buffer = &buffer[..offset];
        let regions = super::framing::regions(&framing.frames, offset);
        self.scan_records(buffer, &regions);

        offset
    }

    /// The scans for records that sit anywhere in a packet, each run over one
    /// frame (or stretch between frames) at a time, so no record borrows the
    /// next frame's bytes: a `2a 37` record ending `44 36 33 ..`, read on into
    /// the next frame's length `8f 01` and spawn `41 36`, named entity 51 "A"
    /// (2026-10-06). Each scan still runs over all of `data` before the next.
    fn scan_records(&mut self, data: &[u8], regions: &[std::ops::Range<usize>]) {
        // Embedded 04 8D ownership and loot records.
        for r in regions {
            self.scan_for_embedded_04_8d(&data[r.clone()]);
        }
        if data.len() >= 4 {
            self.scan_for_entity_hp(data);
        }
        // Embedded spawn opcodes (40/41/44/45 36). Player spawns (45 36) also
        // sit mid-packet, where parse_summon_packet at the packet front never
        // looks: party members announced only that way stayed unnamed (#id).
        for r in regions {
            self.scan_for_embedded_40_36(&data[r.clone()]);
        }
        // Bind the local player from the account character-select list, which
        // arrives as plaintext (uncompressed) and can land in a standalone packet.
        for r in regions {
            self.scan_char_list_self(&data[r.clone()]);
        }
        // Mask-driven id↔name records: the self record (33 36) and every other
        // player's record (45 36). This is the primary naming source — it covers
        // the already-loaded case where no login char-list is in the capture.
        for r in regions {
            self.scan_masked_identity(&data[r.clone()]);
        }
        // Party roster (names, levels, gear score, combat power).
        for r in regions {
            self.scan_party_roster(&data[r.clone()]);
        }
    }

    /// Who you are, from compressed bundles that sit inside another packet.
    ///
    /// The game sends your self record (`33 36`: name, server, class, level)
    /// on zone loads and then every few minutes, and those later copies arrive
    /// in a bundle carried inside a larger packet, where the framing never
    /// opens it. A meter started mid-session therefore never learned your
    /// level: five copies went unread in one hour of a capture (2026-10-04),
    /// the player levelling 29 to 30 among them. Only identity is read from
    /// these: what else they hold is left as it was, so no fight changes.
    fn scan_embedded_bundles_for_identity(&self, packet: &[u8]) {
        for bundle in super::framing::embedded_bundles(packet) {
            let walk = super::framing::walk_inner(&bundle.data);
            let regions = super::framing::regions(&walk.frames, bundle.data.len());
            for r in &regions {
                self.scan_masked_identity(&bundle.data[r.clone()]);
            }
            for r in &regions {
                self.scan_party_roster(&bundle.data[r.clone()]);
            }
        }
    }

    /// `depth` is 1 for a bundle in the stream, one more for each bundle it
    /// sits in. Past `MAX_BUNDLE_DEPTH` it is skipped, as the slice builder does.
    fn unwrap_bundle(&mut self, payload: &[u8], depth: usize) {
        // payload starts at FF FF
        // Format: FF FF (2) + decompressed_size (4 LE) + LZ4 compressed data
        if payload.len() < 7 || depth > super::framing::MAX_BUNDLE_DEPTH {
            return;
        }

        let decompressed = match super::framing::decompress_bundle(payload) {
            Some(d) => d,
            None => return,
        };

        // Walk decompressed data as varint-framed inner packets. The walk lives
        // in `capture::framing` so the Evidence Slice builder splits a bundle
        // exactly the way this does.
        self.pending_compact_skill_context = None;

        let walk = super::framing::walk_inner(&decompressed);
        for frame in &walk.frames {
            match frame.kind {
                super::framing::FrameKind::Bundle => {
                    self.unwrap_bundle(frame.payload(&decompressed), depth + 1);
                }
                super::framing::FrameKind::Packet => {
                    let inner_packet = frame.bytes(&decompressed);
                    if let Some(ctx) = self.extract_pending_compact_skill_context(inner_packet) {
                        self.pending_compact_skill_context = Some(ctx);
                    }
                    self.parse_perfect_packet(inner_packet);
                }
            }
        }

        // The same record scans as a stream gets, over the decompressed data.
        let regions = super::framing::regions(&walk.frames, decompressed.len());
        self.scan_records(&decompressed, &regions);

        self.pending_compact_skill_context = None;
    }

    fn parse_perfect_packet(&mut self, packet: &[u8]) -> bool {
        if packet.len() < 3 {
            return false;
        }

        let parsed_damage = self.parsing_damage(packet, true, false);
        let parsed_ownership = self.parse_summon_ownership_packet(packet);
        let parsed_summon = self.parse_summon_packet(packet);
        let parsed_name = self.parse_actor_name_binding_rules(packet)
            || self.parse_loot_attribution_actor_name(packet)
            || self.parsing_nickname(packet);
        let parsed_hp = self.parse_hp_mp_update_packet(packet);
        self.parse_party_scope_packet(packet);
        self.parse_own_record(packet);
        self.parse_death_packet(packet);
        self.parse_zone_change_packet(packet);
        self.parse_map_load_packet(packet);

        if !parsed_damage && !parsed_name && !parsed_summon && !parsed_ownership && !parsed_hp {
            self.parse_dot_packet(packet);
        }

        parsed_damage || parsed_name
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use super::damage::read_plotter;
    use crate::capture::names::exact_name;
    use crate::combat::data_storage::SkillCombatData;
    use crate::entity::damage_packet::ParsedDamagePacket;
    use crate::entity::special_damage::{self, SpecialDamage};

    /// Damage records from a live capture (2026-10-04, target 30001, actor
    /// 1395), each checked against the game's own Damage Analyzer record of
    /// the same fight: switch bit 0x20 marks a hit with additional hits.
    #[test]
    fn additional_hits_are_read_from_the_record_tail() {
        let hex = |s: &str| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect::<Vec<u8>>();
        let parse = |record: &str| {
            let storage = Arc::new(DataStorage::new());
            let mut p = StreamProcessor::new(storage.clone(), Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
            p.set_override_timestamp(Some(1_000));
            let mut packet = vec![0x00, 0x04, 0x38];
            packet.extend(hex(record));
            packet[0] = packet.len() as u8;
            assert!(p.parsing_damage(&packet, false, false), "{record}");
            let combat = storage.get_combat_snapshot();
            let target = combat.values().next().unwrap();
            let skill = target.actors.values().next().unwrap().skills.values().next().unwrap().clone();
            (skill.total_damage, skill.hit_count, skill.multi_hit_count, skill.multi_hit_hits, skill.multi_hit_damage)
        };
        // Layout 6, switch 0x36: 1700 with two additional hits of 24.
        assert_eq!(parse("b1ea013600f30a40c0f4007a038000010b199b5f01000000ac52a40d021818"), (1700, 1, 1, 2, 48));
        // Layout 4, switch 0x34: the layout-4 field, then one additional hit of 89.
        assert_eq!(parse("b1ea013400f30ae0b7f800cd028bd3276101000000ac52d330010159"), (6227, 1, 1, 1, 89));
        // A spirit's layout-4 records (Summon: Wind Spirit, a Wind Spirit basic
        // attack): no layout-4 field, the additional hits right after the value.
        assert_eq!(parse("fe9e0224009e9b01c3f8f5000302792b156002000000ac52e10301060200"), (481, 1, 1, 1, 6));
        assert_eq!(parse("fe9e0224009e9b01c18601001702a7a2980001000000ac52e401030303030100"), (228, 1, 1, 3, 9));
        assert_eq!(parse("fe9e0204009e9b01c3f8f5000302792b156003000000ac52b7030300"), (439, 1, 0, 0, 0));
        // Layout 6, switch 0x16: no additional hits.
        assert_eq!(parse("b1ea011600f30a40c0f40063028000010b199b5f01000000ac52d007"), (976, 1, 0, 0, 0));
    }

    /// Switch 0x36 with 25 additional hits of 99,999,999 each: their sum
    /// does not fit an i32, and a debug build must not overflow on it.
    #[test]
    fn twenty_five_huge_additional_hits_do_not_overflow() {
        let huge = "ffc1d72f"; // 99,999,999
        // The value, then a 0 (no strict tail), then 25 hits.
        let record = format!("b1ea013600f30a40c0f4007a03800001 0b199b5f01000000ac52{huge}0019{}", huge.repeat(25)).replace(' ', "");
        let (storage, mut p) = processor();
        let mut packet = hex(&record);
        packet.splice(0..0, [0x04, 0x38]);
        let len = packet.len() + 2;
        packet.splice(0..0, [(len as u8) | 0x80, (len >> 7) as u8]);
        assert!(p.parsing_damage(&packet, false, false));
        let combat = storage.get_combat_snapshot();
        let skill = combat.values().next().unwrap().actors.values().next().unwrap().skills.values().next().unwrap().clone();
        // The value does not include them, so they add on: 26 hits in all.
        assert_eq!(skill.total_damage, 26 * 99_999_999);
    }

    /// A damage record four bundles deep is read; five deep, it is not, the
    /// same depth the slice builder stops at.
    #[test]
    fn bundles_nest_four_deep_at_most() {
        let frame = |body: &[u8]| {
            let mut v = crate::capture::framing::length_value(body.len());
            let mut out = Vec::new();
            loop {
                let b = (v & 0x7F) as u8;
                v >>= 7;
                if v == 0 {
                    out.push(b);
                    break;
                }
                out.push(b | 0x80);
            }
            out.extend_from_slice(body);
            out
        };
        let bundle = |inner: &[u8]| {
            let mut body = vec![0xFF, 0xFF];
            body.extend_from_slice(&(inner.len() as u32).to_le_bytes());
            body.extend_from_slice(&lz4_flex::compress(inner));
            frame(&body)
        };
        let record = [&[0x04, 0x38][..], &hex("b1ea011600f30a40c0f40063028000010b199b5f01000000ac52d007")].concat();
        for (depth, read) in [(1, true), (4, true), (5, false), (40, false)] {
            let mut stream = frame(&record);
            for _ in 0..depth {
                stream = bundle(&stream);
            }
            let (storage, mut p) = processor();
            p.consume_stream(&stream);
            assert_eq!(!storage.get_combat_snapshot().is_empty(), read, "depth {depth}");
        }
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// Hand one `04 38` record (the bytes after the opcode) to the parser.
    fn feed(p: &mut StreamProcessor, record: &str) -> bool {
        let mut packet = vec![0x00, 0x04, 0x38];
        packet.extend(hex(record));
        packet[0] = packet.len() as u8;
        p.parsing_damage(&packet, false, false)
    }

    #[test]
    fn a_player_record_without_a_name_still_marks_a_player() {
        let (storage, p) = processor();
        // `45 36 <id 9303> <u32> <mask2>`, the name bit clear.
        p.parse_player_spawn_name(&hex("4536d748000000000000"), 2);
        for actor in [9303, 9304] {
            let mut hit = ParsedDamagePacket::new();
            hit.set_actor_id(actor);
            hit.set_target_id(900);
            hit.set_skill_code(14_010_000);
            hit.set_damage(100);
            storage.append_damage(hit);
        }
        let owners = storage.get_summon_data();
        assert_eq!(owners.get(&9303), None, "a player");
        assert_eq!(owners.get(&9304), Some(&crate::combat::data_storage::UNATTRIBUTED_ID));
    }

    fn processor() -> (Arc<DataStorage>, StreamProcessor) {
        let storage = Arc::new(DataStorage::new());
        let mut p = StreamProcessor::new(storage.clone(), Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
        p.set_override_timestamp(Some(1_000));
        (storage, p)
    }

    fn skill_of(storage: &DataStorage, target: i32, actor: i32, skill: i32) -> SkillCombatData {
        let row = crate::entity::skill_group::row_skill(skill, &SkillLookup::new());
        storage.get_combat_snapshot()[&target].actors[&actor].skills[&(row, false)].clone()
    }

    /// Player hits from the captures of 2026-10-04, one per flag the player
    /// side shows: the flags byte after the hit type, then the angle.
    #[test]
    fn hit_flags_are_read_from_player_records() {
        let parse = |record: &str, target: i32, actor: i32, skill: i32| {
            let (storage, mut p) = processor();
            assert!(feed(&mut p, record), "{record}");
            let s = skill_of(&storage, target, actor, skill);
            (s.total_damage, s.crit_count, s.parry_count, s.perfect_count, s.double_count, s.frontal_count)
        };
        // Flags 0x02, the target parried: 106 damage, front.
        assert_eq!(parse("e9de020600c80b9147ff003f02020002aff4b76301000000e4506a0100", 44905, 1480, 16730001), (106, 0, 1, 0, 0, 1));
        // A critical hit the target parried.
        assert_eq!(parse("fcc2010600bf6bc759d1003203020002c711c75101000000904e660100", 24956, 13759, 13720007), (102, 1, 1, 0, 0, 1));
        // Flags 0x04, Perfect, no angle.
        assert_eq!(parse("cac20406009e389147ff00e102040000aff4b76301000000a4587f0100", 74058, 7198, 16730001), (127, 0, 0, 1, 0, 0));
        // Flags 0x08, Double.
        assert_eq!(parse("bfe6020600d23310ffd6006f0208000055a2fb5302000000aa56d20e0200", 45887, 6610, 14090000), (1874, 0, 0, 0, 1, 0));
    }

    /// The flags only monsters' hits on players show in the captures of
    /// 2026-10-04 (`<target> 06 00 <actor> <skill> <uid> <hit type>`, then the
    /// plotter). The parser leaves these records out; the plotter is read
    /// the same way.
    #[test]
    fn hit_flags_of_received_hits() {
        let plotter = |record: &str, at: usize| {
            let p = read_plotter(&hex(record), at).unwrap();
            (special_damage::from_hit_flags(p.flags), p.hp, p.angle)
        };
        use SpecialDamage::*;
        // 0x01 Shield Block: 74 damage.
        assert_eq!(plotter("ca6f0600ca9804b8c112000102010002ebab530701000000904e4a0100", 13), (vec![ShieldBlock], 0, Some(2)));
        // 0x10 Iron Wall.
        assert_eq!(plotter("fe2a0600c68d03b8c112000102100002ebab530701000000904e740100", 13), (vec![IronWall], 0, Some(2)));
        // 0x20 Regeneration, 13 HP back from a hit of 68.
        assert_eq!(plotter("df7e0600a3b103b0b512000302200d02cbf84e0701000000904e440100", 13), (vec![Regeneration], 13, Some(2)));
        // 0x40 Perfect Block, with Shield Block or Parry; the hit does 1.
        assert_eq!(plotter("e9230600ee9e03e4cc120001024100021b09580701000000904e010100", 13), (vec![ShieldBlock, PerfectBlock], 0, Some(2)));
        assert_eq!(plotter("e06d0600aeb1024cba12000202420002bbc5500701000000904e010100", 13), (vec![Parry, PerfectBlock], 0, Some(2)));
    }

    /// Restoration HP is a varint: 177 takes two bytes (`b1 01`). Read as one
    /// byte, as the fixed skip did, the angle came out as Back (the `01`) and
    /// the value one varint early: the actor's scalar 10000 instead of 889.
    #[test]
    fn restoration_hp_is_a_varint() {
        let record = hex("ed080600f0ef04bab51200010220b10102b3fc4e0701000000904ef9060100");
        let p = read_plotter(&record, 13).unwrap();
        assert_eq!((p.flags, p.hp, p.hp_len, p.angle), (0x20, 177, 2, Some(2)));
        // Then 8 bytes, the scalar and the value: 177 HP is 20 % of 889, as on
        // every Regeneration hit with HP in the captures.
        let mut at = 13 + 1 + p.hp_len + 1 + 8;
        assert_eq!(try_read_varint(&record, &mut at), Some(10_000));
        assert_eq!(try_read_varint(&record, &mut at), Some(889));
        // The second two-byte record: 204 HP of 1020.
        let record = hex("ee0d0600849d03bab51200010220cc0101b3fc4e0701000000904efc070100");
        let p = read_plotter(&record, 13).unwrap();
        assert_eq!((p.hp, p.angle), (204, Some(1)));
        let mut at = 13 + 1 + p.hp_len + 1 + 8 + 2;
        assert_eq!(try_read_varint(&record, &mut at), Some(1020));
    }

    /// A Spiritmaster's spirits send `04 38` records to their owner on each
    /// landed attack (2026-10-05 15:37). The Wind Spirit's (16990003) restores
    /// 1.5 % HP: 103 of 6871. The Water Spirit's (16990002) restores 20 MP
    /// (SkillEffect MpHeal 20), same layout; the game files it under 100011,
    /// so it showed as "Fire Spirit: Basic Attack" healing. A self-cast MP
    /// restore (15760007, 30 MP, 15:17:41) went in as a self-heal.
    #[test]
    fn mp_restores_are_not_healing() {
        let (storage, mut p) = processor();
        let mut hit = ParsedDamagePacket::new();
        hit.set_timestamp(1_000);
        hit.set_target_id(16720);
        hit.set_actor_id(10137);
        hit.set_skill_code(16040000);
        hit.set_type(2);
        hit.set_damage(500);
        storage.append_damage(hit.clone());
        hit.set_actor_id(2787);
        storage.append_damage(hit);
        storage.register_confirmed_summon_by_id(61001, 10137);
        storage.register_confirmed_summon_by_id(49482, 10137);

        assert!(feed(&mut p, "994f0400c9dc03333f03010602f7af446501000000ac52670100"));
        assert!(feed(&mut p, "994f0400ca8203323f0301060293af446501000000ac52140100"));
        assert!(feed(&mut p, "e3150400e315877af0005302c7dcef5d01000000865d1e0100"));

        let heals = storage.heals_between(0, 2_000);
        let healed: Vec<_> = heals.iter().flat_map(|(a, s)| s.iter().map(move |(k, v)| (*a, k.0, v.total_heal))).collect();
        assert_eq!(healed, vec![(61001, 100031, 103)]);
        let snapshot = storage.get_combat_snapshot();
        assert!(!snapshot.contains_key(&10137) && !snapshot.contains_key(&2787));
    }

    /// A heal tick from a capture (2026-10-05 17:31:46): target 5041, heal,
    /// actor 5041, effect 190000131, 7351. The effect is abnormal 19000013,
    /// Restore HP, which no skill owns: a full heal (5041's max HP was 6683
    /// and went to full). Its code, 1900001, is no skill but has a name.
    #[test]
    fn a_heal_from_no_skill_has_a_name() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data");
        let skills = Arc::new(SkillLookup::new());
        let npcs = Arc::new(NpcLookup::new());
        crate::i18n::lookup::load_language(&skills, &npcs, &data, "en");
        let storage = Arc::new(DataStorage::new());
        let mut p = StreamProcessor::new(storage.clone(), skills.clone(), npcs);
        p.set_override_timestamp(Some(1_000));
        p.parse_dot_packet(&hex("130538b12701b127b601032c530bb739"));
        let heal = &storage.heals_between(0, 2_000)[&5041][&(1_900_001, false)];
        assert_eq!((heal.total_heal, heal.tick_count), (7351, 1));
        assert_eq!(skills.lookup_skill_name(1_900_001), "Restore HP");
        for lang in ["de", "es", "fr", "ja", "ko", "pt", "ru"] {
            let skills = SkillLookup::new();
            crate::i18n::lookup::load_language(&skills, &NpcLookup::new(), &data, lang);
            assert!(!skills.lookup_skill_name(1_900_001).is_empty(), "{lang}");
        }
    }

    /// A damage tick from a capture (2026-10-05 16:35:12): target 5377 (a
    /// player), actor 26813 (never seen spawning), effect 120001211, 51. The
    /// effect is abnormal 12000121, a Burn many monster skills share and no
    /// skill owns. Its code, 1200012, is no skill but has a name.
    #[test]
    fn a_damage_tick_from_no_skill_has_a_name() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data");
        let skills = Arc::new(SkillLookup::new());
        let npcs = Arc::new(NpcLookup::new());
        crate::i18n::lookup::load_language(&skills, &npcs, &data, "en");
        let storage = Arc::new(DataStorage::new());
        let mut p = StreamProcessor::new(storage.clone(), skills.clone(), npcs);
        p.set_dot_skill_ids(HashSet::from([1_200_012]));
        p.set_override_timestamp(Some(1_000));
        p.parse_dot_packet(&hex("170538812a0abdd1019002bb1227073332931200"));
        let tick = &storage.get_combat_snapshot()[&5377].actors[&26813].skills[&(1_200_012, true)];
        assert_eq!((tick.total_damage, tick.hit_count), (51, 1));
        assert_eq!(skills.lookup_skill_name(1_200_012), "Burn");
        // Poison, Bleed and Burn, magic, physical and the rest.
        for code in [1_200_010, 1_200_011, 1_200_014, 1_200_015, 1_200_016, 1_200_018] {
            assert!(["Poison", "Bleed", "Burn"].contains(&skills.lookup_skill_name(code).as_str()), "{code}");
        }
        for lang in ["de", "es", "fr", "ja", "ko", "pt", "ru"] {
            let skills = SkillLookup::new();
            crate::i18n::lookup::load_language(&skills, &NpcLookup::new(), &data, lang);
            assert!(!skills.lookup_skill_name(1_200_015).is_empty(), "{lang}");
        }
    }

    /// Hit type 1 (Miss) and 6 (Resist) records have no value. They count on
    /// the skill and leave damage, hits and targets as they were.
    #[test]
    fn misses_and_resists_count_without_damage() {
        // A Resist of Divine Punishment, the same cast (uid 5d) as a hit of
        // 1976 on the same target (2026-10-04 00:44:09).
        let (storage, mut p) = processor();
        assert!(feed(&mut p, "a8a3041400be77802a04015d02109aa06501000000dc51b80f010100"));
        assert!(!feed(&mut p, "a8a3040001be77802a04015d06139aa06501000000dc5101c9e1f5050100"));
        let s = skill_of(&storage, 70056, 15294, 17050240);
        assert_eq!((s.total_damage, s.hit_count, s.resist_count, s.miss_count), (1976, 1, 1, 0));
        assert_eq!(storage.get_combat_snapshot()[&70056].total_damage, 1976);

        // A Miss of Dimensional Control (2026-10-04 03:56:02), a skill that
        // never deals damage, after another hit of the same player.
        let miss = "e981020000899803172df9000101079d556101000000904e0100";
        let (storage, mut p) = processor();
        assert!(!feed(&mut p, miss));
        assert!(storage.get_combat_snapshot().is_empty(), "no target from a miss alone");
        let mut hit = ParsedDamagePacket::new();
        hit.set_timestamp(1_000);
        hit.set_target_id(33001);
        hit.set_actor_id(52233);
        hit.set_skill_code(16000000);
        hit.set_type(2);
        hit.set_damage(500);
        storage.append_damage(hit);
        assert!(!feed(&mut p, miss));
        let s = skill_of(&storage, 33001, 52233, 16330007);
        assert_eq!((s.total_damage, s.hit_count, s.miss_count, s.resist_count), (0, 0, 1, 0));
        assert_eq!(storage.get_combat_snapshot()[&33001].total_damage, 500);
    }

    /// Map loads from a live capture (2026-10-04): into Fire Temple, a
    /// teleport inside it, then out to World_L_A.
    #[test]
    fn only_a_map_load_into_the_open_world_ends_the_dungeon() {
        let storage = Arc::new(DataStorage::new());
        let p = StreamProcessor::new(storage.clone(), Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
        let hex = |s: &str| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect::<Vec<u8>>();
        let load = |s: &str| p.parse_map_load_packet(&hex(s));
        storage.set_current_dungeon(600021);
        load("34213601000000d52709003b1a350000000000f7e646460d7fb0c60080b045409da54200000000000000000000004f0000");
        load("34213602000000d5270900d74c390000000000a8f805c610861245008036453ccd24c204000000000000000000004f0000");
        assert_eq!(storage.current_dungeon_id(), 600021);
        load("34213601000000f2030000dd7f3c00000000006868d047d0c62c470098da46fa63284300000000000000000000004f0000");
        assert_eq!(storage.current_dungeon_id(), 0);
    }

    /// A Daeva Hunter recon site is a boss arena of its own, though the Map
    /// table puts it on World_L_A as a layer. A live capture (2026-10-06):
    /// into Watcher Krache's Recon Site (151010), then back to World_L_A.
    #[test]
    fn a_recon_site_is_filed_under_its_map() {
        let storage = Arc::new(DataStorage::new());
        let p = StreamProcessor::new(storage.clone(), Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
        let hex = |s: &str| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect::<Vec<u8>>();
        let load = |s: &str| p.parse_map_load_packet(&hex(s));
        load("34213602000000e24d0200f1eb330000000000d7e3e747d178404710d50947832c1c430c00000000000000000000410000");
        assert_eq!(storage.current_dungeon_id(), 151010);
        load("34213603000000f2030000bac7350000000000ba53dd47c1ef414700ed0947cc6f33430200000000000000000000000000");
        assert_eq!(storage.current_dungeon_id(), 0);
    }

    #[test]
    fn another_players_spirit_is_linked_at_spawn_by_its_caster() {
        let storage = Arc::new(DataStorage::new());
        let mut p = StreamProcessor::new(storage.clone(), Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
        // `41 36 <47324> <mask, kind 0x1F> … <caster anchor> <6332>`, no parent_key, no name.
        let mut spirit = vec![0x41, 0x36, 0xdc, 0xf1, 0x02, 0x1f, 0x10, 0x00, 0xc6];
        spirit.extend([0x22; 24]);
        spirit.extend([0x80, 0x75, 0xd5, 0x2a, 0xbb, 0x03, 0x00, 0x00, 0xbc, 0x31, 0x0c, 0x02]);
        assert!(p.parse_summon_spawn_at(&spirit, 2));
        assert_eq!(storage.get_summon_data().get(&47324), Some(&6332));

        // A mob's caster field is no owner.
        let mut mob = spirit.clone();
        mob[2] = 0xdd;
        mob[5] = 0x0c;
        assert!(!p.parse_summon_spawn_at(&mob, 2));
        assert!(!storage.is_summon(47325));
    }

    #[test]
    fn a_monsters_summon_is_not_the_player_its_spawn_names() {
        let storage = Arc::new(DataStorage::new());
        let npcs = NpcLookup::new();
        npcs.load_from_json(r#"{"2920063":{"name":"Blazing Totem","isBoss":false},"2920149":{"name":"Wind Spirit","isBoss":false}}"#);
        let mut p = StreamProcessor::new(storage.clone(), Arc::new(SkillLookup::new()), Arc::new(npcs));
        // A player, the one the totem burns.
        storage.append_nickname_authoritative(3640, "Abcd");
        let mut hit = ParsedDamagePacket::new();
        hit.set_actor_id(3640);
        hit.set_target_id(900);
        hit.set_skill_code(11_010_000);
        hit.set_damage(100);
        storage.append_damage(hit);
        // Blazing Totem 51395 (2920063) from the scarecrow capture of
        // 2026-10-05, the name it carries made up: kind 0x1C, the u16 mask's
        // name field naming the player, then the NPC code.
        let spawn = |id: &str, code: &str| {
            let mut b = hex("4136");
            b.extend(hex(id));
            b.extend(hex("1c00010441626364"));
            b.extend(hex(code));
            b.extend(hex("0000020e6ccdc607af014800089b46a0411d43d46f0103030000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000006011101819698"));
            b.extend(hex("00ffffffffffffffff8075d52abb030000c3910301020e6ccdc607af014800089b460676700000001e00000000"));
            b
        };
        assert!(!p.parse_summon_spawn_at(&spawn("c39103", "7f8e2c"), 2));
        assert!(!storage.is_summon(51395));
        let mut burn = ParsedDamagePacket::new();
        burn.set_actor_id(51395);
        burn.set_target_id(3640);
        burn.set_skill_code(1_200_012);
        burn.set_damage(1);
        burn.set_dot(true);
        burn.set_timestamp(2_000);
        storage.append_damage(burn);
        assert!(storage.heals_between(0, 10_000).is_empty(), "its Burn on the player is no healing");

        // A spirit's spawn naming its player still links.
        assert!(p.parse_summon_spawn_at(&spawn("c49103", "d58e2c"), 2));
        assert_eq!(storage.get_summon_data().get(&51396), Some(&3640));
    }

    #[test]
    fn a_sorcerers_ground_spell_is_linked_to_its_caster() {
        let storage = Arc::new(DataStorage::new());
        let mut p = StreamProcessor::new(storage.clone(), Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
        let hex = |s: &str| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect::<Vec<u8>>();
        // Bittercold Wind entity 18249 from a party run (krao capture, 2026-10-05):
        // kind 0x1F, buff block naming itself, caster 14143 after `07 02 06`.
        let storm = |id_varint: &str, caster: &str| {
            let mut b = hex("4136");
            b.extend(hex(id_varint));
            b.extend(hex("1F00004B8E2C004002000CF0C7CFA9D0C70090C04672498542642F01A925A9258E0800008E08000000000000000000000000000010E9010064000000F04902000100000000000000A08601000000000090D00300010101110181969800FFFFFFFFFFFFFFFF8075D52ABB030000"));
            b.extend(hex(id_varint));
            b.extend(hex("0102000CF0C7CFA9D0C70090C046070206"));
            b.extend(hex(caster));
            b.extend(hex("02CD002800"));
            b
        };
        assert!(p.parse_summon_spawn_at(&storm("C98E01", "3F370000"), 2));
        assert_eq!(storage.get_summon_data().get(&18249), Some(&14143));

        // A marker naming the spell itself, or a mob, links nothing.
        assert!(!p.parse_summon_spawn_at(&storm("CA8E01", "4A470000"), 2));
        storage.append_mob(30000, 1);
        assert!(!p.parse_summon_spawn_at(&storm("CB8E01", "30750000"), 2));
        assert!(!storage.is_summon(18250) && !storage.is_summon(18251));

        // Some spawns carry `07 02 01` instead of `07 02 06`.
        let mut other = storm("CC8E01", "BF000000");
        let m = other.windows(3).position(|w| w == [0x07, 0x02, 0x06]).unwrap();
        other[m + 2] = 0x01;
        assert!(p.parse_summon_spawn_at(&other, 2));
        assert_eq!(storage.get_summon_data().get(&18252), Some(&191));
    }

    #[test]
    fn names_are_one_to_twelve_letters_or_digits_in_any_script() {
        for name in ["A", "é", "あ", "ApexZ", "Amber1", "Zoë", "Ñandú", "さくら", "桜子", "전사", "Abcdefghijkl"] {
            assert_eq!(exact_name(name.as_bytes()).as_deref(), Some(name), "{name}");
        }
        for field in [
            &b"Abcdefghijklm"[..], // 13 characters
            b"12345",              // no letter
            b"Apex Z",
            b"ApexZ\x06",
            b"\x05ApexZ",
            b"",
            &[0xC3][..], // cut-off UTF-8
        ] {
            assert_eq!(exact_name(field), None, "{field:?}");
        }
    }

    #[test]
    fn an_id_is_read_whole_not_from_its_last_byte() {
        // 13978 = 9A 6D; the 6D alone is 109 and must not win (issue #10).
        assert_eq!(varint_ending_at(&[0x01, 0x9A, 0x6D, 0xE2, 0x07], 3, 0, 100..=99_999), Some(13978));
        // 14957 = ED 74, a loot owner (2026-10-04).
        assert_eq!(varint_ending_at(&[0x01, 0xED, 0x74, 0x18, 0x05], 3, 0, 100..=99_999), Some(14957));
        // A small id is still one byte.
        assert_eq!(varint_ending_at(&[0x01, 0x6D, 0xE2, 0x07], 2, 0, 100..=99_999), Some(109));
        // 8765 = BD 44: 44 alone is 68, below range.
        assert_eq!(varint_ending_at(&[0x01, 0xBD, 0x44, 0xE2, 0x07], 3, 0, 100..=99_999), Some(8765));
    }

    /// The start of a self record from a live capture (2026-10-04): Naicha,
    /// entity 14957 (`ed 74`), server 1304 (`18 05`), class 30 = Cleric, a
    /// byte, level 28. The rest of the record is not needed and not kept.
    #[test]
    fn the_self_record_says_your_server_class_and_level() {
        let storage = Arc::new(DataStorage::new());
        let processor = StreamProcessor::new(storage.clone(), Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
        let hex = "3336ed745e91c12837064e616963686118051e000000011c0000007f0100007f0100001c000000d002040000000000";
        let record: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
        processor.scan_masked_identity(&record);
        let me = storage.local_profile();
        assert_eq!(me.name.as_deref(), Some("Naicha"));
        assert_eq!(storage.local_player_id(), Some(14957));
        assert_eq!((me.server_id, me.class, me.level), (1304, Some(crate::entity::job_class::JobClass::Cleric), Some(28)));
    }

    /// Two frames from a capture (2026-10-06 18:12:55): a `2a 37` record that
    /// ends `44 36 33 7c 42 17 40`, then a mob spawn whose length is `8f 01`.
    /// Read on across the frame end, that is a player record for entity 51
    /// (`33`) with mask2 `8f` and the one-byte name `01 41`, "A".
    fn record_cut_by_its_frame() -> Vec<u8> {
        hex(concat!(
            "1b2a37e880011d034a065afad1bff9d53a4436337c421740",
            "8f014136efb4031c000064902c0000026b519247339aa5c700540a4600d78b3fc7000107076400000064",
            "000000000000000000000000000000000000006400000064000000010000000000000000000000000000",
            "00000000000601110181969800ffffffffffffffff8075d52abb030000efb40301026b519247339aa5c7",
            "00540a46063ed40000002900000000",
        ))
    }

    #[test]
    fn a_record_ends_with_its_frame() {
        let stream = record_cut_by_its_frame();
        let mut bundle = vec![0xFF, 0xFF];
        bundle.extend_from_slice(&(stream.len() as u32).to_le_bytes());
        bundle.extend_from_slice(&lz4_flex::compress(&stream));
        let len = crate::capture::framing::length_value(bundle.len());
        let mut bundled = vec![(len as u8) | 0x80, (len >> 7) as u8];
        bundled.extend_from_slice(&bundle);
        for (how, bytes) in [("in the stream", stream), ("in a bundle", bundled)] {
            let (storage, mut p) = processor();
            p.consume_stream(&bytes);
            assert_eq!(storage.get_nickname(51), None, "{how}");
            // The spawn after it is still read: a mob, code 2920548.
            assert_eq!(storage.mob_code(55919), Some(2_920_548), "{how}");
        }
    }

    /// Four `1d 37` records from a capture (2026-10-05 17:42:14). The second
    /// ends `33 36 33 36`; its last `33 36`, then the third record's length
    /// byte and bytes, read as a self record for entity 16 with a two-letter
    /// name, and the meter took entity 16 for you.
    #[test]
    fn a_self_record_needs_your_server_and_class() {
        let (storage, mut p) = processor();
        p.consume_stream(&hex(
            "101d37f5282703ab13e776e776111d37fc792f031557ff33363336101d37a3132703c6b679be97bc111d37ab262f032250d8722f722f",
        ));
        assert_eq!(storage.local_player_id(), None);
        assert_eq!(storage.get_nickname(16), None);
    }

    /// Shapes from the captures of 2026-10-05: `03 8d <id> 00 00 00 00` and
    /// `42 37 <id>`, entity 4321 (`e1 21`) here.
    #[test]
    fn records_about_you_alone_name_you_mid_zone() {
        let frame = |body: &[u8]| {
            let mut out = vec![crate::capture::framing::length_value(body.len()) as u8];
            out.extend_from_slice(body);
            out
        };
        let (storage, mut p) = processor();
        p.consume_stream(&frame(&[0x03, 0x8D, 0xE1, 0x21, 0, 0, 0, 0]));
        assert_eq!(storage.local_player_id(), None);
        p.consume_stream(&frame(&[0x42, 0x37, 0xE1, 0x21]));
        assert_eq!(storage.local_player_id(), Some(4321));

        // A record one byte longer than its kind is something else.
        let (storage, mut p) = processor();
        for _ in 0..3 {
            p.consume_stream(&frame(&[0x03, 0x8D, 0xE1, 0x21, 0, 0, 0, 0, 0]));
        }
        assert_eq!(storage.local_player_id(), None);
    }

    /// A Sorcerer on Ventus (server 1305) killing a mob, from a player's log
    /// (2026-10-01): `04 8d <mob> <4 bytes> <owner 1454> <server 1305> <name>
    /// <server name>`. The server id used to be matched only as `E0 07` /
    /// `E2 07`, so this owner never got a name. (Whether the name is then bound
    /// depends on 1454 having been seen in combat; `identity_replay` covers that.)
    #[test]
    fn kill_record_names_its_owner_on_any_server() {
        let processor = StreamProcessor::new(
            Arc::new(DataStorage::new()),
            Arc::new(SkillLookup::new()),
            Arc::new(NpcLookup::new()),
        );
        let record = [
            &[0x04, 0x8d, 0xec, 0xde, 0x02, 0x72, 0x28, 0xe9, 0x00, 0xae, 0x0b, 0x19, 0x05, 0x05][..],
            b"ApexZ",
            &[0x06],
            b"Ventus",
            &[0x01, 0x00, 0x00, 0x00],
        ]
        .concat();
        assert!(processor.scan_for_embedded_04_8d(&record));

        // The same record with a name that runs into the next field is not one.
        let mut garbled = record.clone();
        garbled[13] = 0x07;
        assert!(!processor.scan_for_embedded_04_8d(&garbled));
    }
}
