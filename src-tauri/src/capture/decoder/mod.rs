use crate::combat::data_storage::DataStorage;
pub use fields::{name_fields, name_fields_unframed, NameField, NameRecord};
pub use super::varint::{
    varint_fits, index_of_bytes, le_u32_at, varint_at, next_varint, varint_before, VarintRead,
};
mod damage;
mod fields;
mod identity;
mod pet_collection;
mod power_shards;
mod roster;
mod spawn;
mod ticks;
mod world;

pub use pet_collection::{PetCollectionEvent, PetCount};
pub use power_shards::ShardEvent;
pub(crate) use spawn::embedded_spawns;
pub(crate) use ticks::compact_record_in;

use std::collections::{HashSet, VecDeque};
use std::ops::Range;
use std::sync::Arc;

use crate::capture::framing::{self, ChunkForm};
use crate::i18n::lookup::{NpcLookup, SkillLookup};

/// Embedded hit keys held at most; past this the oldest one goes.
const HIT_KEY_CAP: usize = 16_384;

/// The actor and skill a bundle's compact record names, for the aggregate
/// hit that may follow it in the same bundle.
#[derive(Clone)]
struct HeldTickSkill {
    actor_id: i32,
    skill_raw: i32,
}

/// Embedded hits already counted, keyed by their hex. Holds the newest
/// `HIT_KEY_CAP` keys by the order they came in; a lookup does not renew one.
struct HitKeys {
    arrival: VecDeque<String>,
    held: HashSet<String>,
}

impl HitKeys {
    fn new() -> Self {
        HitKeys { arrival: VecDeque::new(), held: HashSet::new() }
    }

    fn contains(&self, key: &str) -> bool {
        self.held.contains(key)
    }

    fn insert(&mut self, key: String) {
        if self.held.contains(&key) {
            return;
        }
        if self.arrival.len() >= HIT_KEY_CAP {
            if let Some(gone) = self.arrival.pop_front() {
                self.held.remove(&gone);
            }
        }
        self.held.insert(key.clone());
        self.arrival.push_back(key);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.held.len()
    }
}

/// Reads the server's stream into a `DataStorage`: frames, bundles and the
/// records that sit loose between or inside them.
pub struct PacketDecoder {
    data_storage: Arc<DataStorage>,
    skill_lookup: Arc<SkillLookup>,
    npc_lookup: Arc<NpcLookup>,
    seen_embedded_hexes: HitKeys,
    dot_damage_skill_ids: HashSet<i32>,
    unknown_dot_skills: HashSet<i32>,
    pending_compact_skill_context: Option<HeldTickSkill>,
    override_timestamp: Option<i64>,
}

/// A record pass run on one region at a time.
type RegionPass = fn(&mut PacketDecoder, &[u8]);

impl PacketDecoder {
    pub fn new(data_storage: Arc<DataStorage>, skill_lookup: Arc<SkillLookup>, npc_lookup: Arc<NpcLookup>) -> Self {
        PacketDecoder {
            data_storage,
            skill_lookup,
            npc_lookup,
            seen_embedded_hexes: HitKeys::new(),
            dot_damage_skill_ids: HashSet::new(),
            unknown_dot_skills: HashSet::new(),
            pending_compact_skill_context: None,
            override_timestamp: None,
        }
    }

    /// Pins the time records are filed under, and "now" on this thread, to a
    /// capture's time; `None` goes back to the wall clock.
    pub fn pin_clock(&mut self, ts: Option<i64>) {
        crate::clock::set_override(ts);
        self.override_timestamp = ts;
    }

    /// When a record happened: the pinned replay time, else now.
    pub(super) fn record_time(&self) -> i64 {
        self.override_timestamp.unwrap_or_else(crate::clock::now_ms)
    }

    /// Reads every whole frame at the front of `buffer` and returns how many
    /// bytes that was. The rest is a frame still arriving: hand it in again
    /// with what follows.
    pub fn decode_buffer(&mut self, buffer: &[u8]) -> usize {
        let walked = framing::walk(buffer);
        for frame in &walked.frames {
            if frame.kind == ChunkForm::Bundle {
                self.open_bundle(frame.payload(buffer), 1);
                continue;
            }
            let whole = frame.bytes(buffer);
            self.read_packet(whole);
            self.peek_into_packet_bundles(whole);
        }
        let done = walked.consumed;
        let regions = framing::regions(&walked.frames, done);
        self.run_record_passes(&buffer[..done], &regions);
        done
    }

    /// The skills whose ticks are damage over time; replaces the old set.
    pub fn use_tick_skills(&mut self, ids: HashSet<i32>) {
        self.dot_damage_skill_ids = ids;
    }

    /// One bundle frame's payload, `depth` bundles down from the stream.
    fn open_bundle(&mut self, payload: &[u8], depth: usize) {
        if depth > framing::NEST_LIMIT {
            return;
        }
        let Some(opened) = framing::inflate_block(payload) else {
            return;
        };
        self.pending_compact_skill_context = None;
        let inner = framing::split_frames(&opened);
        for frame in &inner.frames {
            match frame.kind {
                ChunkForm::Bundle => self.open_bundle(frame.payload(&opened), depth + 1),
                ChunkForm::Packet => {
                    let whole = frame.bytes(&opened);
                    let named = self.read_held_tick_skill(whole);
                    if named.is_some() {
                        self.pending_compact_skill_context = named;
                    }
                    self.read_packet(whole);
                }
            }
        }
        let regions = framing::regions(&inner.frames, opened.len());
        self.run_record_passes(&opened, &regions);
        // A context belongs to its own bundle only.
        self.pending_compact_skill_context = None;
    }

    /// Every reader of a whole frame, in replay order: storage writes depend
    /// on it. Ticks are tried last, and only when none of the hit, summon,
    /// name or party HP readers took the frame.
    fn read_packet(&mut self, frame: &[u8]) {
        if frame.len() < 3 {
            return;
        }
        let when = self.record_time();
        self.data_storage.note_abnormal(when, frame);
        // `|=` does not short-circuit, so every reader runs; only the name
        // chain stops at its first hit.
        let mut taken = self.read_hit_record(frame, true, false);
        taken |= self.read_pet_owner(frame);
        taken |= self.read_pet_spawn(frame);
        taken |= self.read_name_binding(frame)
            || self.read_loot_claim(frame)
            || self.read_name_record(frame);
        taken |= self.read_party_hp(frame);
        self.read_party_scope(frame);
        self.parse_own_record(frame);
        self.read_death(frame);
        self.read_combat_state(frame);
        self.read_zone_change(frame);
        self.read_map_load(frame);
        self.read_pet_collection(frame);
        self.read_shard_count(frame);
        if !taken {
            self.read_tick_record(frame);
        }
    }

    /// Bundles a stream packet carries inside it. Only names and the party
    /// roster are read from them: the hits, spawns and HP there are left out.
    fn peek_into_packet_bundles(&mut self, frame: &[u8]) {
        for found in framing::embedded_bundles(frame) {
            let opened = &found.data;
            let regions = framing::regions(&framing::split_frames(opened).frames, opened.len());
            self.over_regions(opened, &regions, &[|p, r| p.find_masked_self(r), |p, r| p.read_party_list(r)]);
        }
    }

    /// The readers that find records anywhere in `data` rather than at a
    /// frame's start. Each one but the entity HP scan stays inside a region,
    /// and each finishes every region before the next starts.
    fn run_record_passes(&mut self, data: &[u8], regions: &[Range<usize>]) {
        self.over_regions(data, regions, &[|p, r| {
            p.seek_inner_048d(r);
        }]);
        self.read_entity_hp(data);
        self.over_regions(data, regions, &[
            |p, r| p.read_hp_records(r),
            |p, r| p.seek_inner_4036(r),
            |p, r| p.find_self_in_char_list(r),
            |p, r| p.find_masked_self(r),
            |p, r| p.read_party_list(r),
        ]);
    }

    /// Runs each pass over every region of `data` in turn.
    fn over_regions(&mut self, data: &[u8], regions: &[Range<usize>], passes: &[RegionPass]) {
        for pass in passes {
            for region in regions {
                pass(self, &data[region.clone()]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::damage_packet::ParsedDamagePacket;
    use crate::built::frame as pk;
    use crate::combat::data_storage::RosterEntry;
    use crate::entity::job_class::JobClass;

    /// The game tables shipped with the app.
    const DATA_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/data");

    use super::damage::read_plotter;
    use crate::built::{self, shard_hit, Abnormal, Hit, Member, Slot, Spawn, F};
    use crate::capture::names::clean_name_field;
    use crate::combat::data_storage::SkillTotals;
    use crate::entity::mark_kinds::{self, HitMark};
    const MOB: u32 = 40_001;
    const PLAYER: u32 = 1_500;

    /// A fresh storage and a processor over it, with time pinned at 1,000.
    pub(super) fn processor() -> (Arc<DataStorage>, PacketDecoder) {
        let storage: Arc<DataStorage> = DataStorage::new().into();
        let p = reader(&storage, SkillLookup::new(), NpcLookup::new());
        (storage, p)
    }

    /// A processor over `storage` and these tables, time pinned at 1,000.
    pub(super) fn reader(storage: &Arc<DataStorage>, skills: SkillLookup, npcs: NpcLookup) -> PacketDecoder {
        let mut p = PacketDecoder::new(Arc::clone(storage), skills.into(), npcs.into());
        p.pin_clock(Some(1_000));
        p
    }

    /// Switch 0x36 with 25 additional hits of 99,999,999 each: their sum
    /// does not fit an i32, and a debug build must not overflow on it.
    #[test]
    fn twenty_five_huge_additional_hits_do_not_overflow() {
        let huge = 99_999_999;
        // The value, then a 0 (no strict tail), then 25 hits.
        let mut values = vec![10_500, huge, 0, 25];
        values.extend([huge; 25]);
        let packet = Hit { switch: 0x36, values, ..shard_hit() }.packet();
        let (storage, mut p) = processor();
        assert!(p.read_hit_record(&packet, false, false));
        let combat = storage.copy_targets();
        let skill = combat.values().next().unwrap().actors.values().next().unwrap().skills.values().next().unwrap().clone();
        // The value does not include them, so they add on: 26 hits in all.
        assert_eq!(skill.total_damage, 26 * 99_999_999);
    }

    /// A damage record four bundles deep is read; five deep, it is not, the
    /// same depth the slice builder stops at.
    #[test]
    fn bundles_nest_four_deep_at_most() {
        for (depth, read) in [(1, true), (4, true), (5, false), (40, false)] {
            let mut stream = shard_hit().packet();
            for _ in 0..depth {
                stream = built::bundle(&stream);
            }
            let (storage, mut p) = processor();
            p.decode_buffer(&stream);
            assert_eq!(!storage.copy_targets().is_empty(), read, "depth {depth}");
        }
    }

    /// Hand one `04 38` record to the parser.
    fn feed(p: &mut PacketDecoder, hit: &Hit) -> bool {
        p.read_hit_record(&hit.packet(), false, false)
    }

    #[test]
    fn a_player_record_without_a_name_still_marks_a_player() {
        let (storage, p) = processor();
        // `45 36 <id 9303> <u32> <mask2>`, the name bit clear.
        let record = built::player_record(9303, None, &[0]);
        p.read_spawn_name(&record, 3);
        for actor in [9303, 9304] {
            let mut hit = ParsedDamagePacket::new();
            hit.set_actor_id(actor);
            hit.set_target_id(900);
            hit.set_skill_code(14_010_000);
            hit.set_damage(100);
            storage.append_damage(hit);
        }
        let owners = storage.pet_owners();
        assert_eq!(owners.get(&9303), None, "a player");
        assert_eq!(owners.get(&9304), Some(&crate::combat::data_storage::UNATTRIBUTED_ID));
    }

    fn skill_of(storage: &DataStorage, target: i32, actor: i32, skill: i32) -> SkillTotals {
        let row = crate::entity::skill_group::row_skill(skill, &SkillLookup::new());
        storage.copy_targets()[&target].actors[&actor].skills[&(row, false)].clone()
    }

    /// Player hits, one per flag the player side shows: the flags byte after
    /// the hit type, then the angle.
    #[test]
    fn hit_flags_are_read_from_player_records() {
        let parse = |hit: Hit| {
            let (storage, mut p) = processor();
            assert!(feed(&mut p, &hit), "{hit:?}");
            let s = skill_of(&storage, hit.target as i32, hit.actor as i32, hit.skill as i32);
            (s.total_damage, s.crit_count, s.parry_count, s.perfect_count, s.double_count, s.frontal_count)
        };
        let hit = |skill, hit_type, plotter, effect, count, scalar, damage| Hit {
            target: MOB,
            switch: 0x06,
            actor: PLAYER,
            skill,
            uid: 0x31,
            hit_type,
            plotter: Some(plotter),
            effect,
            count,
            values: vec![scalar, damage, count, 0],
            ..Hit::default()
        };
        // Flags 0x02, the target parried: 100 damage, front.
        assert_eq!(parse(hit(16_730_001, 2, (0x02, 0, 2), 1_673_000_111, 1, 10_000, 100)), (100, 0, 1, 0, 0, 1));
        // A critical hit the target parried.
        assert_eq!(parse(hit(13_720_007, 3, (0x02, 0, 2), 1_372_000_711, 1, 10_000, 120)), (120, 1, 1, 0, 0, 1));
        // Flags 0x04, Perfect, no angle.
        assert_eq!(parse(hit(16_730_001, 2, (0x04, 0, 0), 1_673_000_111, 1, 11_000, 130)), (130, 0, 0, 1, 0, 0));
        // Flags 0x08, Double.
        assert_eq!(parse(hit(14_090_000, 2, (0x08, 0, 0), 1_409_000_021, 2, 11_500, 1_800)), (1_800, 0, 0, 0, 1, 0));
    }

    /// A monster's hit on a player, `<target> 06 00 <actor> <skill> <uid>
    /// <hit type>` then the plotter: a two-byte target and a three-byte
    /// actor put the plotter at byte 13.
    fn received(skill: u32, plotter: (u8, u32, u8), damage: u32) -> Hit {
        Hit {
            target: 2_001,
            switch: 0x06,
            actor: 50_001,
            skill,
            uid: 0x01,
            hit_type: 2,
            plotter: Some(plotter),
            effect: skill * 100 + 11,
            count: 1,
            values: vec![10_000, damage, 1, 0],
            ..Hit::default()
        }
    }

    /// The flags only monsters' hits on players show. The parser leaves these
    /// records out; the plotter is read the same way.
    #[test]
    fn hit_flags_of_received_hits() {
        let plotter = |hit: Hit| {
            let p = read_plotter(&hit.fields(), 13).unwrap();
            (mark_kinds::from_hit_flags(p.flags), p.hp, p.angle)
        };
        use HitMark::*;
        // 0x01 Shield Block: 70 damage.
        assert_eq!(plotter(received(1_229_240, (0x01, 0, 2), 70)), (vec![ShieldBlock], 0, Some(2)));
        // 0x10 Iron Wall.
        assert_eq!(plotter(received(1_229_240, (0x10, 0, 2), 110)), (vec![IronWall], 0, Some(2)));
        // 0x20 Regeneration, 10 HP back from a hit of 60.
        assert_eq!(plotter(received(1_226_160, (0x20, 10, 2), 60)), (vec![Regeneration], 10, Some(2)));
        // 0x40 Perfect Block, with Shield Block or Parry; the hit does 1.
        assert_eq!(plotter(received(1_232_100, (0x41, 0, 2), 1)), (vec![ShieldBlock, PerfectBlock], 0, Some(2)));
        assert_eq!(plotter(received(1_227_340, (0x42, 0, 2), 1)), (vec![Parry, PerfectBlock], 0, Some(2)));
    }

    /// Restoration HP is a varint: 200 takes two bytes (`c8 01`). Read as one
    /// byte, as the fixed skip did, the angle came out as Back (the `01`) and
    /// the value one varint early: the actor's scalar 10000 instead of 1000.
    #[test]
    fn restoration_hp_is_a_varint() {
        let record = received(1_226_170, (0x20, 200, 2), 1_000).fields();
        let p = read_plotter(&record, 13).unwrap();
        assert_eq!((p.flags, p.hp, p.hp_len, p.angle), (0x20, 200, 2, Some(2)));
        // Then 8 bytes, the scalar and the value: 200 HP is 20 % of 1,000.
        let mut at = 13 + 1 + p.hp_len + 1 + 8;
        assert_eq!(next_varint(&record, &mut at), Some(10_000));
        assert_eq!(next_varint(&record, &mut at), Some(1_000));
        // A second two-byte record: 240 HP of 1,200.
        let record = received(1_226_170, (0x20, 240, 1), 1_200).fields();
        let p = read_plotter(&record, 13).unwrap();
        assert_eq!((p.hp, p.angle), (240, Some(1)));
        let mut at = 13 + 1 + p.hp_len + 1 + 8 + 2;
        assert_eq!(next_varint(&record, &mut at), Some(1_200));
    }

    /// A Spiritmaster's spirits send `04 38` records to their owner on each
    /// landed attack. The Wind Spirit's (16990003) restores 1.5 % HP. The
    /// Water Spirit's (16990002) restores 20 MP (SkillEffect MpHeal 20), same
    /// layout; the game files it under 100011, so it showed as "Fire Spirit:
    /// Basic Attack" healing. A self-cast MP restore (15760007, 30 MP) went
    /// in as a self-heal.
    #[test]
    fn mp_restores_are_not_healing() {
        const OWNER: u32 = 2_100;
        const OTHER: u32 = 2_200;
        const WIND: u32 = 52_001;
        const WATER: u32 = 52_002;
        let (storage, mut p) = processor();
        let mut hit = ParsedDamagePacket::new();
        hit.set_timestamp(1_000);
        hit.set_target_id(9100);
        hit.set_actor_id(OWNER as i32);
        hit.set_skill_code(16040000);
        hit.set_hit_kind(2);
        hit.set_damage(500);
        storage.append_damage(hit.clone());
        hit.set_actor_id(OTHER as i32);
        storage.append_damage(hit);
        storage.confirm_pet(WIND as i32, OWNER as i32);
        storage.confirm_pet(WATER as i32, OWNER as i32);

        let restore = |target, actor, skill: u32, uid, scalar, value| Hit {
            target,
            switch: 0x04,
            actor,
            skill,
            uid,
            hit_type: 2,
            effect: skill * 100 + 11,
            count: 1,
            values: vec![scalar, value, 1, 0],
            ..Hit::default()
        };
        assert!(feed(&mut p, &restore(OWNER, WIND, 16_990_003, 0x06, 10_500, 105)));
        assert!(feed(&mut p, &restore(OWNER, WATER, 16_990_002, 0x06, 10_500, 20)));
        assert!(feed(&mut p, &restore(OTHER, OTHER, 15_760_007, 0x40, 11_000, 30)));

        let heals = storage.heals_between(0, 2_000);
        let healed: Vec<_> = heals.iter().flat_map(|(a, s)| s.iter().map(move |(k, v)| (*a, k.0, v.total_heal))).collect();
        assert_eq!(healed, vec![(WIND as i32, 100031, 105)]);
        let snapshot = storage.copy_targets();
        assert!(!snapshot.contains_key(&(OWNER as i32)) && !snapshot.contains_key(&(OTHER as i32)));
    }

    /// Hit type 1 (Miss) and 6 (Resist) records have no value. They count on
    /// the skill and leave damage, hits and targets as they were.
    #[test]
    fn misses_and_resists_count_without_damage() {
        // A Resist of Divine Punishment, the same cast (uid) as a hit of
        // 2000 on the same target.
        let (storage, mut p) = processor();
        let hit = Hit {
            target: MOB,
            switch: 0x14,
            actor: PLAYER,
            skill: 17_050_240,
            uid: 0x31,
            hit_type: 2,
            effect: 1_705_024_016,
            count: 1,
            values: vec![10_400, 2_000, 1, 1, 0],
            ..Hit::default()
        };
        assert!(feed(&mut p, &hit));
        let resist = Hit {
            switch: 0x00,
            flag: 1,
            hit_type: 6,
            effect: 1_705_024_019,
            values: vec![10_400, 1],
            tail: built::fields(&[F::U32(99_999_177), F::V(1), F::V(0)]),
            ..hit
        };
        assert!(!feed(&mut p, &resist));
        let s = skill_of(&storage, MOB as i32, PLAYER as i32, 17050240);
        assert_eq!((s.total_damage, s.hit_count, s.resist_count, s.miss_count), (2_000, 1, 1, 0));
        assert_eq!(storage.copy_targets()[&(MOB as i32)].total_damage, 2_000);

        // A Miss of Dimensional Control, a skill
        // that never deals damage, after another hit of the same player.
        let miss = Hit {
            target: MOB,
            actor: PLAYER,
            skill: 16_330_007,
            uid: 0x22,
            hit_type: 1,
            effect: 1_633_000_711,
            count: 1,
            values: vec![10_000, 1, 0],
            ..Hit::default()
        };
        let (storage, mut p) = processor();
        assert!(!feed(&mut p, &miss));
        assert!(storage.copy_targets().is_empty(), "no target from a miss alone");
        let mut hit = ParsedDamagePacket::new();
        hit.set_timestamp(1_000);
        hit.set_target_id(MOB as i32);
        hit.set_actor_id(PLAYER as i32);
        hit.set_skill_code(16000000);
        hit.set_hit_kind(2);
        hit.set_damage(500);
        storage.append_damage(hit);
        assert!(!feed(&mut p, &miss));
        let s = skill_of(&storage, MOB as i32, PLAYER as i32, 16330007);
        assert_eq!((s.total_damage, s.hit_count, s.miss_count, s.resist_count), (0, 0, 1, 0));
        assert_eq!(storage.copy_targets()[&(MOB as i32)].total_damage, 500);
    }

    /// A filled roster slot: world 1201, mask 0x1e, tag 4.
    fn member<'a>(slot: u8, name: &'a str, class: u32, level: u32, gear: u32, combat_power: u64, tail: &'a [u8]) -> Slot<'a> {
        Slot::Member(Member {
            mask: 0x1e,
            slot,
            dbid: (1201 << 48) | 0x0001_0000 | u64::from(slot),
            name,
            class,
            level,
            gear,
            extra: &[],
            tag: 4,
            combat_power,
            tail,
        })
    }

    /// A `0x1C` spawn naming the player "Abcd", its own abnormal cast by the
    /// first totem, and a parent at its end.
    fn totem(id: u32, code: u32) -> Spawn<'static> {
        Spawn {
            id,
            kind: 0x1C,
            name: Some("Abcd"),
            code,
            hp: 3,
            filler: 40,
            abnormals: vec![Abnormal::endless(51_001)],
            end: built::effect_parent(30_500),
        }
    }

    /// Two frames: a `2a 37` record that ends `44 36 33 <4 bytes>`, then a
    /// mob spawn whose length is `8f 01`. Read on across the frame end, that
    /// is a player record for entity 51 (`33`) with mask2 `8f` and the
    /// one-byte name `01 41`, "A".
    fn record_cut_by_its_frame() -> Vec<u8> {
        let mut stream = built::packet(
            [0x2A, 0x37],
            &[F::V(16_100), F::Raw(&[0x01, 0x02, 0x03, 0x04]), F::F32(1.5), F::Raw(&[0x10, 0x20, 0x30, 0x44, 0x36]), F::V(51), F::U32(0x5060_7080)],
        );
        stream.extend(built::spawn_8f_01(56_001));
        stream
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
            p.decode_buffer(&bytes);
            assert_eq!(storage.stored_name(51), None, "{how}");
            // The spawn after it is still read: a mob, code 2920548.
            assert_eq!(storage.enemy_code(56_001), Some(2_920_548), "{how}");
        }
    }

    /// Four `1d 37` records shaped as four real ones. The
    /// second ends `33 36 33 36`; its last `33 36`, then the third record's
    /// length byte and bytes, read as a self record for entity 16 with a
    /// two-letter name, and the meter took entity 16 for you.
    #[test]
    fn a_self_record_needs_your_server_and_class() {
        let moved = |id: u32, kind: u8, rest: &[u8]| built::packet([0x1D, 0x37], &[F::V(id), F::U8(kind), F::U8(0x03), F::Raw(rest)]);
        let mut stream = moved(5_001, 0x27, &[0x10, 0x11, 0x12, 0x13, 0x14, 0x15]);
        stream.extend(moved(5_002, 0x2F, &[0x10, 0x11, 0x12, 0x33, 0x36, 0x33, 0x36]));
        // Length 16, mask2 `27`, then `03` and "éx", then no server or class.
        let third = moved(5_003, 0x27, &[0xC3, 0xA9, b'x', 0xFF, 0xFF, 0xFF]);
        assert_eq!(third[0], 16);
        stream.extend(third);
        stream.extend(moved(5_004, 0x2F, &[0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16]));
        let (storage, mut p) = processor();
        p.decode_buffer(&stream);
        assert_eq!(storage.local_player_id(), None);
        assert_eq!(storage.stored_name(16), None);
    }

    /// Shapes the game sends: `03 8d <id> 00 00 00 00` and
    /// `42 37 <id>`, entity 4321 (`e1 21`) here.
    #[test]
    fn records_about_you_alone_name_you_mid_zone() {
        let frame = |body: &[u8]| {
            let mut out = vec![crate::capture::framing::length_value(body.len()) as u8];
            out.extend_from_slice(body);
            out
        };
        let (storage, mut p) = processor();
        p.decode_buffer(&frame(&[0x03, 0x8D, 0xE1, 0x21, 0, 0, 0, 0]));
        assert_eq!(storage.local_player_id(), None);
        p.decode_buffer(&frame(&[0x42, 0x37, 0xE1, 0x21]));
        assert_eq!(storage.local_player_id(), Some(4321));

        // A record one byte longer than its kind is something else.
        let (storage, mut p) = processor();
        for _ in 0..3 {
            p.decode_buffer(&frame(&[0x03, 0x8D, 0xE1, 0x21, 0, 0, 0, 0, 0]));
        }
        assert_eq!(storage.local_player_id(), None);
    }

    /// A Wind Spirit's Malicious Whirlwind ticks on after the spirit is
    /// unsummoned. The game's Damage Analyzer counts the ticks before the
    /// spirit's `42 36` flag 7 and none after it; so does the meter.
    #[test]
    fn a_spirit_s_ticks_stop_counting_when_it_leaves() {
        const TARGET: u32 = 40_031;
        const SPIRIT: u32 = 26_001;
        const OWNER: u32 = 2_700;
        let (storage, mut p) = processor();
        p.use_tick_skills(HashSet::from([16_001_109]));
        let tick = built::tick(TARGET, 0x0a, SPIRIT, 2_300, 1_600_110_913, 400, &[F::U32(16_001_112)]);
        // The owner's link record to the spirit (16770001).
        let link = Hit {
            target: SPIRIT,
            switch: 0x04,
            actor: OWNER,
            skill: 16_770_001,
            uid: 0x01,
            hit_type: 2,
            effect: 1_677_000_111,
            count: 1,
            values: vec![11_000, 200, 1, 0],
            ..Hit::default()
        };
        feed(&mut p, &link);
        assert_eq!(storage.pet_owners().get(&(SPIRIT as i32)), Some(&(OWNER as i32)));
        p.read_tick_record(&tick);
        p.read_tick_record(&tick);
        p.read_death(&built::packet([0x42, 0x36], &[F::V(SPIRIT), F::V(0), F::V(7)]));
        p.read_tick_record(&tick);
        p.read_tick_record(&tick);
        let (target, spirit) = (TARGET as i32, SPIRIT as i32);
        let ticks = &storage.copy_targets()[&target].actors[&spirit].skills[&(16_001_109, true)];
        assert_eq!((ticks.hit_count, ticks.total_damage), (2, 800));

        // A spirit back under the same id (its link records resume) counts again.
        feed(&mut p, &link);
        p.read_tick_record(&tick);
        let ticks = &storage.copy_targets()[&target].actors[&spirit].skills[&(16_001_109, true)];
        assert_eq!(ticks.hit_count, 3);
    }

    // ----- The stream, bundles and record passes -----

    /// Feeds `bytes` as one read and checks every byte of it was taken.
    fn take_all(p: &mut PacketDecoder, bytes: &[u8]) {
        assert_eq!(p.decode_buffer(bytes), bytes.len(), "{bytes:02x?}");
    }

    /// A plain layout 4 hit of `damage` by `actor` on `target`, fed as a frame.
    pub(super) fn landed(p: &mut PacketDecoder, actor: u32, target: u32, skill: u32, damage: u32) {
        take_all(p, &built::plain_hit(actor, target, skill, damage, 0x01).packet());
    }

    #[test]
    fn the_walk_says_how_many_bytes_were_taken() {
        let (storage, mut p) = processor();
        let mut bytes = pk(&[0x23, 0x36, 0x00]);
        assert_eq!(bytes, [0x07, 0x23, 0x36, 0x00]);
        bytes.extend([0x07, 0x23]);
        assert_eq!(p.decode_buffer(&bytes), 4);
        assert_eq!(p.decode_buffer(&[]), 0);
        // Frames too short for an opcode reach no reader.
        assert_eq!(p.decode_buffer(&[0x04]), 1);
        assert_eq!(p.decode_buffer(&[0x05, 0x23]), 2);
        assert!(storage.copy_targets().is_empty());
        assert!(p.pending_compact_skill_context.is_none());
    }

    #[test]
    fn an_hp_frame_cut_in_two_is_read_once_it_is_whole() {
        let (storage, mut p) = processor();
        let whole = built::hp_record(4_600, None, Some(&[(0, 1_200)]));
        assert_eq!(p.decode_buffer(&whole[..6]), 0);
        assert_eq!(storage.enemy_hp_now(4_600), None);
        take_all(&mut p, &whole);
        assert_eq!(storage.enemy_hp_now(4_600), Some(1_200));
    }

    #[test]
    fn a_bundle_too_short_to_open_is_passed_over() {
        let (storage, mut p) = processor();
        take_all(&mut p, &[0x0A, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00]);
        assert!(storage.copy_targets().is_empty());
    }

    /// An HP record split by a frame end is still seen by the entity HP
    /// scan, which reads past frames: here `0B 7E 7E 00` lands as the HP.
    #[test]
    fn the_entity_hp_scan_reads_across_frame_ends() {
        let (storage, mut p) = processor();
        // `0B 7E 7E 8D C8 01 02 01`, a zero byte, `0B 7E 7E 00 00 00 00 00`.
        let mut stream = pk(&[0x7E, 0x7E, 0x8D, 0xC8, 0x01, 0x02, 0x01]);
        stream.push(0);
        stream.extend(pk(&[0x7E, 0x7E, 0, 0, 0, 0, 0]));
        assert_eq!(stream.len(), 17);
        take_all(&mut p, &stream);
        assert_eq!(storage.enemy_hp_now(200), Some(8_289_803));
    }

    /// A raw LZ4 block that decodes to `content`, with each `(at, from)` of
    /// `copies` sent as a 4-byte match on `content[from..]`. Bytes sent that
    /// way are not in the block, so no reader of the packed bytes finds the
    /// record they belong to.
    fn lz4_with_copies(content: &[u8], copies: &[(usize, usize)]) -> Vec<u8> {
        fn literals(out: &mut Vec<u8>, bytes: &[u8]) {
            let mut extra = bytes.len().saturating_sub(15);
            if bytes.len() >= 15 {
                while extra >= 255 {
                    out.push(255);
                    extra -= 255;
                }
                out.push(extra as u8);
            }
            out.extend_from_slice(bytes);
        }
        let mut out = Vec::new();
        let mut from_byte = 0;
        for &(at, from) in copies {
            let run = &content[from_byte..at];
            out.push((run.len().min(15) as u8) << 4);
            literals(&mut out, run);
            out.extend(((at - from) as u16).to_le_bytes());
            from_byte = at + 4;
        }
        let run = &content[from_byte..];
        out.push((run.len().min(15) as u8) << 4);
        literals(&mut out, run);
        assert_eq!(lz4_flex::decompress(&out, content.len()).unwrap(), content);
        out
    }

    /// A bundle frame holding `frames` behind a filler frame that carries
    /// the 4 bytes after each frame's opcode, which the frames then copy.
    fn bundle_without_plain_records(frames: &[Vec<u8>]) -> Vec<u8> {
        let mut filler = vec![0x7E, 0x7E];
        for f in frames {
            filler.extend_from_slice(&f[3..7]);
        }
        let filler = pk(&filler);
        let mut content = filler.clone();
        let mut copies = Vec::new();
        for (n, f) in frames.iter().enumerate() {
            copies.push((content.len() + 3, 3 + 4 * n));
            content.extend(f);
        }
        content.extend(pk(&[0x7E, 0x7E, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC]));
        let mut payload = vec![0xFF, 0xFF];
        payload.extend((content.len() as u32).to_le_bytes());
        payload.extend(lz4_with_copies(&content, &copies));
        pk(&payload)
    }

    /// A bundle inside a packet is read for names and the roster only: the
    /// self record in it names you, the hit beside it counts for nothing.
    #[test]
    fn a_bundle_inside_a_packet_gives_names_but_no_hits() {
        let (storage, mut p) = processor();
        let you = built::self_record(3_001, "Taveli", 1_202, 30, 30);
        let bundle = bundle_without_plain_records(&[you, shard_hit().packet()]);
        let mut body = vec![0x7E, 0x7E, 0x01];
        body.extend(bundle);
        take_all(&mut p, &pk(&body));
        assert_eq!(who_you_are(&storage), (Some(3_001), "Taveli".into(), 1_202, Some(JobClass::Cleric), Some(30)));
        assert!(storage.copy_targets().is_empty());
    }

    /// The bundle hit that sums a compact record's skill, from actor `actor`.
    fn compact_sum(actor: u32) -> Vec<u8> {
        Hit { actor, skill: 99_745_942, switch: 0x36, uid: 0x22, hit_type: 3, values: vec![10_500, 1_700, 0, 2, 100, 120], ..shard_hit() }.packet()
    }

    /// `08 3B 38 00 00` then `38 <actor> <uid> <skill u32>`: a compact
    /// record naming `actor` and skill 30,000,123.
    fn compact_record(actor: u32) -> Vec<u8> {
        built::packet([0x7E, 0x7E], &[F::Raw(&[0x08, 0x3B, 0x38, 0x00, 0x00, 0x38]), F::V(actor), F::U8(0x05), F::U32(30_000_123), F::U32(0)])
    }

    /// The skills `PLAYER` was credited with on `MOB`.
    fn skills_on_mob(storage: &DataStorage) -> Vec<(i32, i64)> {
        let snapshot = storage.copy_targets();
        let Some(target) = snapshot.get(&(MOB as i32)) else { return Vec::new() };
        let mut out: Vec<_> = target.actors.values().flat_map(|a| a.skills.values().map(|s| (s.skill_code, s.total_damage))).collect();
        out.sort();
        out
    }

    #[test]
    fn a_compact_skill_context_lasts_to_the_end_of_its_bundle() {
        // In one bundle the sum is filed under the compact record's skill.
        let (storage, mut p) = processor();
        let found = p.read_held_tick_skill(&compact_record(PLAYER)).map(|c| (c.actor_id, c.skill_raw));
        let skill = crate::entity::skill_group::row_skill(30_000_123, &SkillLookup::new());
        assert_eq!(found, Some((PLAYER as i32, skill)));
        let mut inner = compact_record(PLAYER);
        inner.extend(compact_sum(PLAYER));
        take_all(&mut p, &built::bundle(&inner));
        assert!(p.pending_compact_skill_context.is_none());
        let together = skills_on_mob(&storage);
        assert_eq!(together.iter().map(|s| s.0).collect::<Vec<_>>(), vec![skill]);

        // In the next bundle it is gone.
        let (storage, mut p) = processor();
        let mut stream = built::bundle(&compact_record(PLAYER));
        stream.extend(built::bundle(&compact_sum(PLAYER)));
        take_all(&mut p, &stream);
        assert_ne!(skills_on_mob(&storage), together);

        // A bundle inside clears the outer one's context too.
        let (storage, mut p) = processor();
        let mut inner = compact_record(PLAYER);
        inner.extend(built::bundle(&[0x07, 0x7E, 0x7E, 0x00]));
        inner.extend(compact_sum(PLAYER));
        take_all(&mut p, &built::bundle(&inner));
        assert_ne!(skills_on_mob(&storage), together);
    }

    // ----- Keys of embedded hits -----

    #[test]
    fn a_hit_key_given_twice_is_held_once() {
        let mut keys = HitKeys::new();
        keys.insert("a".to_string());
        keys.insert("a".to_string());
        assert_eq!(keys.len(), 1);
        assert!(keys.contains("a"));
    }

    #[test]
    fn the_oldest_hit_key_goes_first_when_full() {
        let key = |n: usize| format!("k{n}");
        let mut keys = HitKeys::new();
        for n in 0..=HIT_KEY_CAP {
            keys.insert(key(n));
        }
        assert_eq!(keys.len(), HIT_KEY_CAP);
        assert!(!keys.contains(&key(0)));
        assert!(keys.contains(&key(1)) && keys.contains(&key(HIT_KEY_CAP)));

        // Looking a key up does not make it any younger.
        let mut keys = HitKeys::new();
        for n in 0..HIT_KEY_CAP {
            keys.insert(key(n));
        }
        assert!(keys.contains(&key(0)));
        keys.insert("x".to_string());
        assert!(!keys.contains(&key(0)) && keys.contains("x"));
    }

    // ----- Time -----

    #[test]
    fn pinned_time_moves_the_thread_clock_and_comes_back() {
        let (_, mut p) = processor();
        p.pin_clock(Some(7));
        assert_eq!(crate::clock::now_ms(), 7);
        assert_eq!(p.override_timestamp, Some(7));
        p.pin_clock(None);
        assert!(crate::clock::now_ms() > 1_600_000_000_000);
        assert_eq!(p.override_timestamp, None);
    }

    #[test]
    fn a_new_processor_starts_empty_and_leaves_the_clock() {
        crate::clock::set_override(Some(55));
        let p = PacketDecoder::new(DataStorage::new().into(), SkillLookup::new().into(), NpcLookup::new().into());
        assert_eq!(crate::clock::now_ms(), 55);
        assert_eq!(p.override_timestamp, None);
        assert!(p.dot_damage_skill_ids.is_empty() && p.unknown_dot_skills.is_empty());
        assert!(p.pending_compact_skill_context.is_none());
        crate::clock::set_override(None);
    }

    #[test]
    fn new_dot_skill_ids_replace_the_old_set() {
        let (_, mut p) = processor();
        p.use_tick_skills(HashSet::from([1, 2]));
        p.use_tick_skills(HashSet::from([3]));
        assert_eq!(p.dot_damage_skill_ids, HashSet::from([3]));
    }

    // ----- Records of other readers, fed as a stream -----

    /// What one hit adds to its skill: total, hits, multi-hit records,
    /// multi-hit hits, multi-hit damage.
    fn one_skill_after(hit: Hit) -> (i64, i32, i32, i32, i64) {
        let (storage, mut p) = processor();
        take_all(&mut p, &hit.packet());
        let snapshot = storage.copy_targets();
        let skills: Vec<_> = snapshot.values().flat_map(|t| t.actors.values()).flat_map(|a| a.skills.values()).collect();
        assert_eq!(skills.len(), 1, "{hit:?}");
        let s = skills[0];
        (s.total_damage, s.hit_count, s.multi_hit_count, s.multi_hit_hits, s.multi_hit_damage)
    }

    #[test]
    fn extra_hits_after_the_value_add_to_the_skill() {
        assert_eq!(one_skill_after(shard_hit()), (900, 1, 0, 0, 0));
        let two_more = Hit { switch: 0x36, uid: 0x22, hit_type: 3, values: vec![10_500, 1_700, 2, 24, 24], ..shard_hit() };
        assert_eq!(one_skill_after(two_more), (1_700, 1, 1, 2, 48));
        let one_more = Hit {
            switch: 0x34,
            skill: 16_300_000,
            uid: 0x23,
            plotter: None,
            effect: 1_630_000_011,
            values: vec![10_500, 6_000, 1, 1, 100],
            ..shard_hit()
        };
        assert_eq!(one_skill_after(one_more), (6_000, 1, 1, 1, 100));
        assert_eq!(one_skill_after(spirit_hit(0x24, 16_120_003, 0x24, 1_612_000_121, 2, &[10_500, 500, 1, 10, 2, 0])), (500, 1, 1, 1, 10));
        assert_eq!(one_skill_after(spirit_hit(0x24, 100_033, 0x25, 10_003_111, 1, &[10_500, 240, 3, 3, 3, 3, 1, 0])), (240, 1, 1, 3, 9));
        assert_eq!(one_skill_after(spirit_hit(0x04, 16_120_003, 0x24, 1_612_000_121, 3, &[10_500, 450, 3, 0])), (450, 1, 0, 0, 0));
    }

    /// A spirit's record on 40002 from 20001, without a plotter: its
    /// additional hits follow the value, with no field before them.
    pub(super) fn spirit_hit(switch: u32, skill: u32, uid: u8, effect: u32, count: u32, values: &[u32]) -> Hit {
        Hit { target: 40_002, switch, actor: 20_001, skill, uid, hit_type: 2, effect, count, values: values.to_vec(), ..Hit::default() }
    }

    /// `41 36 <id, 2 bytes> <kind> 00 00 00 01 <name>` and `end`.
    fn named_spawn(id: u32, kind: u8, name: &str, end: &[u8]) -> Vec<u8> {
        let mut body = vec![0x41, 0x36];
        body.extend(built::varint(id));
        assert_eq!(body.len(), 4);
        body.extend([kind, 0x00, 0x00, 0x00, 0x01]);
        body.extend(built::fields(&[F::Str(name), F::Raw(end)]));
        pk(&body)
    }

    /// `06 <owner u32> 07 00 00 00 00 00 E9 03 00`.
    fn owner_block(owner: u32) -> Vec<u8> {
        built::owner_block(owner, 0, 1_001, 0, &[])
    }

    /// `07 02 06 <caster u32>`.
    fn caster_field(caster: u32) -> Vec<u8> {
        built::fields(&[F::Raw(&[0x07, 0x02, 0x06]), F::U32(caster)])
    }

    #[test]
    fn a_summon_spawn_names_the_player_who_owns_it() {
        let (storage, mut p) = processor();
        landed(&mut p, 1_500, 3_000, 11_010_000, 100);
        landed(&mut p, 1_600, 3_000, 11_010_000, 100);
        take_all(&mut p, &named_spawn(5_001, 0x1F, "Orlen", &caster_field(3_000)));
        assert_eq!((storage.stored_name(3_000), storage.stored_name(1_500)), (None, None));

        take_all(&mut p, &named_spawn(5_002, 0x5F, "Orlen", &owner_block(1_500)));
        assert_eq!(storage.pet_owners().get(&5_002), Some(&1_500));
        assert_eq!(storage.stored_name(1_500).as_deref(), Some("Orlen"));

        take_all(&mut p, &named_spawn(5_003, 0x5F, "Pimsa", &caster_field(1_600)));
        assert_eq!(storage.pet_owners().get(&5_003), Some(&1_600));
        assert_eq!(storage.stored_name(1_600).as_deref(), Some("Pimsa"));

        storage.name_entity(1_500, "Quell");
        take_all(&mut p, &named_spawn(5_004, 0x5F, "Orlen", &owner_block(1_500)));
        assert_eq!(storage.pet_owners().get(&5_004), Some(&1_500));
        assert_eq!(storage.stored_name(1_500).as_deref(), Some("Quell"));
    }

    /// A processor over `en` game tables, time pinned at 1,000.
    fn with_tables() -> (Arc<DataStorage>, PacketDecoder) {
        let (skills, npcs) = (SkillLookup::new(), NpcLookup::new());
        crate::i18n::lookup::load_language(&skills, &npcs, std::path::Path::new(DATA_DIR), "en");
        let storage: Arc<DataStorage> = DataStorage::new().into();
        let p = reader(&storage, skills, npcs);
        (storage, p)
    }

    /// The skill name each shipped language other than English gives `code`.
    fn other_language_names(code: i32) -> Vec<String> {
        let data = std::path::Path::new(DATA_DIR);
        ["de", "es", "fr", "ja", "ko", "pt", "ru"]
            .into_iter()
            .map(|lang| {
                let skills = SkillLookup::new();
                crate::i18n::lookup::load_language(&skills, &NpcLookup::new(), data, lang);
                skills.lookup_skill_name(code)
            })
            .collect()
    }

    #[test]
    fn a_restore_tick_with_no_skill_is_named_hp_regen() {
        let (storage, mut p) = with_tables();
        take_all(&mut p, &built::tick(2_300, 0x01, 2_300, 180, 190_000_131, 7_000, &[]));
        let heal = &storage.heals_between(0, 2_000)[&2_300][&(1_900_001, false)];
        assert_eq!((heal.total_heal, heal.tick_count), (7_000, 1));
        assert_eq!(p.skill_lookup.lookup_skill_name(1_900_001), "HP Regen");
        assert!(other_language_names(1_900_001).iter().all(|n| !n.is_empty()));
    }

    #[test]
    fn a_poison_bleed_or_burn_tick_with_no_skill_is_named() {
        let (storage, mut p) = with_tables();
        p.use_tick_skills(HashSet::from([1_200_012]));
        take_all(&mut p, &built::tick(2_400, 0x0a, 60_001, 270, 120_001_211, 50, &[F::U32(1_217_330)]));
        let tick = &storage.copy_targets()[&2_400].actors[&60_001].skills[&(1_200_012, true)];
        assert_eq!((tick.total_damage, tick.hit_count), (50, 1));
        assert_eq!(p.skill_lookup.lookup_skill_name(1_200_012), "Burn");
        for code in [1_200_010, 1_200_011, 1_200_014, 1_200_015, 1_200_016, 1_200_018] {
            let name = p.skill_lookup.lookup_skill_name(code);
            assert!(["Poison", "Bleed", "Burn"].contains(&name.as_str()), "{code}: {name}");
        }
        assert!(other_language_names(1_200_015).iter().all(|n| !n.is_empty()));
    }

    fn roster_member(slot: u8, gear_score: i32, combat_power: i64) -> RosterEntry {
        RosterEntry { slot, gear_score, combat_power, ..Default::default() }
    }

    #[test]
    fn vacant_roster_slots_are_stepped_over() {
        let (storage, mut p) = processor();
        let names = ["Ivra", "Cobb", "Dren", "Esko", "Nalo"];
        let first = names.iter().enumerate().map(|(n, name)| (name.to_string(), roster_member(n as u8 + 1, 800, 40_000))).collect();
        storage.replace_roster(first, true);
        let slots = [
            member(1, "Ivra", 22, 45, 820, 41_000, &[0x00, 0x40, 0, 0, 0, 0, 0, 0, 0, 0x01, 0x01]),
            Slot::Vacant(2),
            Slot::Vacant(3),
            Slot::Vacant(4),
            member(5, "Nalo", 26, 45, 840, 42_000, &[0x00, 0x41, 0, 0, 0, 0, 0, 0, 0, 0x01, 0x01, 0x00, 0x09]),
        ];
        take_all(&mut p, &built::roster(0x1000, "Party name 1", 600_031, (1201 << 48) | 0x0001_0001, &slots));
        let party = storage.roster_entries();
        let mut held: Vec<_> = party.keys().cloned().collect();
        held.sort();
        assert_eq!(held, ["Ivra", "Nalo"]);
        let ivra = &party["Ivra"];
        assert_eq!((ivra.slot, ivra.level, ivra.gear_score, ivra.combat_power), (1, 45, 820, 41_000));
        let nalo = &party["Nalo"];
        assert_eq!((nalo.slot, nalo.level, nalo.gear_score, nalo.combat_power), (5, 45, 840, 42_000));
        assert_eq!(nalo.job, Some(JobClass::Sorcerer));
    }

    #[test]
    fn a_members_tail_does_not_start_another_member() {
        let world = |slot: u8| (2204u64 << 48) | 0x0002_0000 | u64::from(slot);
        let on_world = |slot: u8, name, class, level, gear, combat_power, tag, tail| {
            Slot::Member(Member { mask: 0x1e, slot, dbid: world(slot), name, class, level, gear, extra: &[], tag, combat_power, tail })
        };
        let tail = |a: u8, b: u8| vec![0x00, a, 0, 0, 0, 0, 0, 0, 0, 0x01, b];
        let (t1, t2, t3) = (tail(0x3d, 0x05), tail(0x3e, 0x01), tail(0x4a, 0x01));
        let t4 = tail(0x05, 0x02);
        let mut t5 = tail(0x51, 0x01);
        t5.extend([0x00, 0x09]);
        let slots = [
            on_world(1, "Abra", 36, 22, 325, 15_381, 4, t1.as_slice()),
            on_world(2, "Bexo", 31, 34, 559, 26_608, 1, t2.as_slice()),
            on_world(3, "Calderwyne", 32, 22, 267, 14_716, 3, t3.as_slice()),
            Slot::Member(Member { extra: &[0x07], ..member(4, "Dorvik", 30, 22, 239, 14_027, &t4).member() }),
            member(5, "Elsmora", 22, 38, 707, 33_869, &t5),
        ];
        let (storage, mut p) = processor();
        take_all(&mut p, &built::roster(0x2000, "Party name 2", 600_002, (2204 << 48) | 0x0002_0001, &slots));
        let party = storage.roster_entries();
        assert_eq!(party.len(), 5);
        let last = &party["Elsmora"];
        assert_eq!((last.slot, last.level, last.gear_score, last.combat_power), (5, 38, 707, 33_869));
    }

    /// A `41 36` Wind Spirit spawn whose abnormal was cast by 6400.
    fn spirit(id: u32, kind: u8) -> Vec<u8> {
        Spawn { id, kind, code: 2_920_149, hp: 500, abnormals: vec![Abnormal::endless(6_400)], ..Spawn::default() }.packet()
    }

    #[test]
    fn a_spirit_spawn_links_to_the_caster_of_its_buff() {
        let (storage, mut p) = processor();
        take_all(&mut p, &spirit(47_400, 0x1F));
        assert_eq!(storage.pet_owners().get(&47_400), Some(&6_400));
        take_all(&mut p, &spirit(47_401, 0x0C));
        assert!(!storage.is_pet(47_401));
    }

    /// A processor with an NPC table, and player 2500 ("Abcd", the name the
    /// `totem` spawns carry) in a fight with 900.
    fn abcd_in_a_fight(npcs: &str) -> (Arc<DataStorage>, PacketDecoder) {
        let storage: Arc<DataStorage> = DataStorage::new().into();
        let table = NpcLookup::new();
        table.load_from_json(npcs);
        let mut p = reader(&storage, SkillLookup::new(), table);
        storage.name_entity(2_500, "Abcd");
        landed(&mut p, 2_500, 900, 11_010_000, 100);
        (storage, p)
    }

    #[test]
    fn a_monster_summon_named_after_a_player_is_not_theirs() {
        let npcs = r#"{"2920063":{"name":"Blazing Totem","isBoss":false},"2920149":{"name":"Wind Spirit","isBoss":false}}"#;
        let (storage, mut p) = abcd_in_a_fight(npcs);
        take_all(&mut p, &totem(51_001, 2_920_063).packet());
        assert!(!storage.is_pet(51_001));
        // A Burn tick of 1 from the totem on the player it named.
        p.use_tick_skills(HashSet::from([1_200_012]));
        p.pin_clock(Some(2_000));
        take_all(&mut p, &built::tick(2_500, 0x0a, 51_001, 270, 120_001_211, 1, &[F::U32(1_217_330)]));
        assert!(storage.heals_between(0, 10_000).is_empty());
        take_all(&mut p, &totem(51_002, 2_920_149).packet());
        assert_eq!(storage.pet_owners().get(&51_002), Some(&2_500));
    }

    #[test]
    fn an_effect_code_missing_from_the_table_is_no_summon() {
        let (storage, mut p) = abcd_in_a_fight(r#"{"2920063":{"name":"Blazing Totem","isBoss":false}}"#);
        take_all(&mut p, &totem(51_003, 2_920_999).packet());
        assert!(!storage.is_pet(51_003));
    }

    /// A `41 36` ground spell whose record ends with its caster, `07 02
    /// <how> <caster u32>`.
    fn ground_spell(id: u32, how: u8, caster: u32) -> Vec<u8> {
        Spawn {
            id,
            kind: 0x1F,
            code: 2_920_011,
            hp: 4_777,
            filler: 40,
            abnormals: vec![Abnormal::endless(id)],
            end: built::spawn_caster(how, caster),
            ..Spawn::default()
        }
        .packet()
    }

    #[test]
    fn a_ground_spell_links_to_the_player_who_cast_it() {
        let (storage, mut p) = processor();
        take_all(&mut p, &ground_spell(21_001, 0x06, 2_600));
        assert_eq!(storage.pet_owners().get(&21_001), Some(&2_600));
        take_all(&mut p, &ground_spell(21_002, 0x06, 21_002));
        storage.append_mob(30_000, 1);
        take_all(&mut p, &ground_spell(21_003, 0x06, 30_000));
        assert!(!storage.is_pet(21_002) && !storage.is_pet(21_003));
        take_all(&mut p, &ground_spell(21_004, 0x01, 191));
        assert_eq!(storage.pet_owners().get(&21_004), Some(&191));
    }

    #[test]
    fn a_name_is_up_to_twelve_letters_or_digits_in_any_script() {
        let kept = |name: &str| clean_name_field(name.as_bytes()).is_some_and(|got| got == name);
        assert!(["Q", "Vorta", "Kiro7", "Abcdefghijkl"].into_iter().all(kept));
        // Accents, kana, kanji and hangul.
        assert!(["é", "Brëvo", "Ñemú"].into_iter().all(kept));
        assert!(["ア", "ミカゼ", "風岩", "바람"].into_iter().all(kept));
        let bad: [&[u8]; 7] = [b"Abcdefghijklm", b"12345", b"Ab cd", b"Abcd\x06", b"\x05Abcd", b"", &[0xC3]];
        for bytes in bad {
            assert_eq!(clean_name_field(bytes), None, "{bytes:02x?}");
        }
    }

    #[test]
    fn an_id_read_backwards_takes_all_of_its_bytes() {
        let read = |bytes: &[u8], end| varint_before(bytes, end, 0, 100..=99_999);
        assert_eq!(read(&[0x01, 0x9A, 0x6D, 0xE2, 0x07], 3), Some(13_978));
        assert_eq!(read(&[0x01, 0x98, 0x75, 0x18, 0x05], 3), Some(15_000));
        assert_eq!(read(&[0x01, 0x6D, 0xE2, 0x07], 2), Some(109));
        assert_eq!(read(&[0x01, 0xBD, 0x44, 0xE2, 0x07], 3), Some(8_765));
    }

    #[test]
    fn your_self_record_gives_your_server_class_and_level() {
        let (storage, mut p) = processor();
        take_all(&mut p, &built::self_record(3_001, "Merrow", 1_202, 30, 30));
        assert_eq!(who_you_are(&storage), (Some(3_001), "Merrow".into(), 1_202, Some(JobClass::Cleric), Some(30)));
    }

    /// Your id, name, server, class and level as the storage has them.
    fn who_you_are(storage: &DataStorage) -> (Option<i64>, String, u16, Option<JobClass>, Option<u32>) {
        let profile = storage.self_card();
        (storage.local_player_id(), profile.name.unwrap_or_default(), profile.server_id, profile.class, profile.level)
    }

    #[test]
    fn three_records_about_another_id_replace_a_stale_you() {
        let (storage, mut p) = processor();
        storage.set_local_identity_from_game(5_100, Some("Merrow".into()));
        let mut record = vec![0x10, 0x4A, 0x36, 0xB8, 0x30];
        record.extend([0; 8]);
        let mut two = record.clone();
        two.extend(&record);
        take_all(&mut p, &two);
        assert_eq!(storage.local_player_id(), Some(5_100));
        take_all(&mut p, &record);
        assert_eq!(storage.local_player_id(), Some(6_200));
        assert_eq!(storage.stored_name(6_200), None);
    }

    /// `04 8D <mob> <u32> <owner> <server u16> <06 name> <06 name> <u32 1>`.
    fn kill_record(name_len: u8) -> Vec<u8> {
        let mut bytes = built::fields(&[
            F::Raw(&[0x04, 0x8D]),
            F::V(50_001),
            F::U32(0x00E9_2872),
            F::V(1_600),
            F::U16(1_305),
            F::Str("Rokval"),
            F::Str("Sundre"),
            F::U32(1),
        ]);
        // Byte 13 is the first name's length, after the 3-byte mob varint,
        // the u32, the 2-byte owner varint and the server.
        bytes[13] = name_len;
        bytes
    }

    #[test]
    fn a_kill_record_names_its_owner_on_any_server() {
        let wrapped = |record: Vec<u8>| {
            let mut body = vec![0x7E, 0x7E];
            body.extend(record);
            pk(&body)
        };
        let (storage, mut p) = processor();
        assert!(p.seek_inner_048d(&kill_record(0x06)));
        assert!(!p.seek_inner_048d(&kill_record(0x07)));
        // Fed loose inside a frame, for a player seen fighting.
        landed(&mut p, 1_600, 900, 11_010_000, 100);
        take_all(&mut p, &wrapped(kill_record(0x07)));
        assert_eq!(storage.stored_name(1_600), None);
        take_all(&mut p, &wrapped(kill_record(0x06)));
        assert_eq!(storage.stored_name(1_600).as_deref(), Some("Rokval"));
    }
}
