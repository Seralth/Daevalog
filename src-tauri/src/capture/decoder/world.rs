//! Zone, map, death and HP records, and the party-scope record.

use super::PacketDecoder;
use crate::capture::opcodes::{COMBAT_STATE, DEATH, DEATH_OLD, HP_MP, MAP_LOAD, PARTY_SCOPE, ZONE_CHANGE};
use crate::capture::varint::{index_of_bytes, le_u32_at, varint_at};

/// The `42 36` flag of an entity leaving the world, a spirit unsummoned among
/// others (often over half of them are linked spirits).
const DESPAWN_FLAG: i32 = 7;

/// The `42 36` flag of an entity killed in combat.
const KILLED_FLAG: i32 = 3;

/// Entity ids the HP readers believe.
const ENTITY_IDS: std::ops::RangeInclusive<i32> = 100..=9_999_999;

/// Max HP a party HP record may give.
const MAX_HP_VALUES: std::ops::RangeInclusive<i32> = 1..=50_000_000;

/// Whether the `00 8D` record at `region[i..]`, `end` bytes long, sits right
/// behind a length varint of one byte, else of two, that frames it exactly.
fn behind_its_length(region: &[u8], i: usize, end: usize) -> bool {
    [1usize, 2].into_iter().any(|width| {
        let Some(start) = i.checked_sub(width) else { return false };
        let head = varint_at(region, start);
        head.length == width as i32 && head.value as i64 == end as i64 + 4
    })
}

/// The bytes after a frame's opcode, when the frame opens with `opcode`
/// behind a length varint that reads.
pub(crate) fn body_after(frame: &[u8], opcode: [u8; 2]) -> Option<&[u8]> {
    let head = varint_at(frame, 0);
    if head.length <= 0 {
        return None;
    }
    let rest = frame.get(head.length as usize..)?;
    rest.strip_prefix(&opcode[..])
}

/// `N` varints in a row from the front of `bytes`.
fn varints<const N: usize>(bytes: &[u8]) -> Option<[i32; N]> {
    let mut out = [0; N];
    let mut at = 0usize;
    for slot in &mut out {
        let v = varint_at(bytes, at);
        if v.length <= 0 {
            return None;
        }
        *slot = v.value;
        at += v.length as usize;
    }
    Some(out)
}

impl PacketDecoder {
    /// `06 38 <entity>`: who the party record is about.
    pub(super) fn read_party_scope(&self, frame: &[u8]) {
        let Some(body) = body_after(frame, PARTY_SCOPE) else { return };
        if body.len() < 2 {
            return;
        }
        if let Some([entity]) = varints::<1>(body) {
            self.data_storage.mark_in_party(entity);
        }
    }

    /// `23 36 00 ...`: you changed zone. Entity 0 is you; any other is not.
    pub(super) fn read_zone_change(&self, frame: &[u8]) {
        if body_after(frame, ZONE_CHANGE).and_then(|b| b.first()) == Some(&0) {
            self.data_storage.on_zone_switch();
        }
    }

    /// `21 36 <load counter u32> <map u32> ...`: a map loaded.
    pub(super) fn read_map_load(&self, frame: &[u8]) {
        let Some(body) = body_after(frame, MAP_LOAD) else { return };
        if body.len() >= 8 {
            self.data_storage.note_map_load(le_u32_at(body, 4) as i32);
        }
    }

    /// `42 36 <entity> <varint> <flag>`, and the same under the opcode it
    /// had before the June 2026 update.
    pub(super) fn read_death(&self, frame: &[u8]) {
        let Some(body) = body_after(frame, DEATH).or_else(|| body_after(frame, DEATH_OLD)) else { return };
        let Some([entity, _, flag]) = varints::<3>(body) else { return };
        match flag {
            KILLED_FLAG => self.data_storage.note_death(entity),
            DESPAWN_FLAG => self.data_storage.note_despawn(entity),
            _ => {}
        }
    }

    /// `21 8D <entity> 00 01` into combat, `00 00` out of it, and for a boss
    /// with a timed enrage `01 01 <deadline u64>` into combat with the
    /// deadline in unix ms. No other shape has been seen.
    pub(super) fn read_combat_state(&self, frame: &[u8]) {
        let Some(body) = body_after(frame, COMBAT_STATE) else { return };
        let id = varint_at(body, 0);
        if id.length <= 0 || !ENTITY_IDS.contains(&id.value) {
            return;
        }
        let at = self.record_time();
        match &body[id.length as usize..] {
            [0, on @ (0 | 1)] => self.data_storage.note_combat(id.value, *on == 1, None, at),
            [1, 1, deadline @ ..] if deadline.len() == 8 => {
                let deadline = i64::from_le_bytes(deadline.try_into().expect("8 bytes"));
                self.data_storage.note_combat(id.value, true, Some(deadline), at);
            }
            _ => {}
        }
    }

    /// `1B 92 <entity> <hp> <max hp> ...`: a party member's HP. True when
    /// the record held up and was filed.
    pub(super) fn read_party_hp(&self, frame: &[u8]) -> bool {
        let Some(body) = body_after(frame, HP_MP) else { return false };
        let Some([entity, current, max]) = varints::<3>(body) else { return false };
        if !ENTITY_IDS.contains(&entity) || !MAX_HP_VALUES.contains(&max) {
            return false;
        }
        self.data_storage.remember_enemy_hp(entity, max);
        self.data_storage.note_party_hp(entity, current as i64);
        true
    }

    /// `00 8D` records in one region, each behind its own length varint of
    /// one or two bytes that matches it exactly. Read whole, its HP needs no
    /// ceiling: HP past 100,000,000 counts, where the loose search in
    /// `read_entity_hp` takes it for stray bytes.
    pub(super) fn read_hp_records(&self, region: &[u8]) {
        let mut from = 0;
        while let Some(i) = index_of_bytes(region, from, &[0x00, 0x8D]) {
            let framed = read_hp_record(&region[i..]).filter(|record| behind_its_length(region, i, record.end));
            match framed {
                Some(record) => {
                    if let Some(hp) = record.current_hp {
                        self.data_storage.note_hp(record.id, hp);
                        self.data_storage.update_enemy_hp(record.id, hp as i32);
                    }
                    from = i + record.end;
                }
                None => from = i + 1,
            }
        }
    }

    /// `8D <entity> 02 01 00 <hp u32> 00 00 00 00`, a current HP record,
    /// wherever it sits in `data`, frame ends and packed bundle bytes too.
    pub(super) fn read_entity_hp(&self, data: &[u8]) {
        const SHAPE_TAIL: usize = 3 + 4 + 4;
        let next_8d = |from: usize| data.get(from..)?.iter().position(|&b| b == 0x8D).map(|n| from + n);
        let mut from = 0;
        while let Some(at) = next_8d(from) {
            from = at + 1;
            let id = varint_at(data, at + 1);
            if id.length <= 0 || !ENTITY_IDS.contains(&id.value) {
                continue;
            }
            let after = at + 1 + id.length as usize;
            let fits = data.get(after..after + SHAPE_TAIL).is_some_and(|t| t[..3] == [0x02, 0x01, 0x00] && t[7..] == [0; 4]);
            if !fits {
                continue;
            }
            let hp = le_u32_at(data, after + 3);
            // A match past 100,000,000 can be LZ4 literals read as a record,
            // so it counts only up to the max HP known (from the spawn or a
            // whole `00 8D` record): world bosses have more (810,000,000 on
            // some).
            let spawned_max = || self.data_storage.enemy_max_hp(id.value).is_some_and(|max| i64::from(hp) <= i64::from(max));
            if hp <= 100_000_000 || spawned_max() {
                self.data_storage.update_enemy_hp(id.value, hp as i32);
            }
            from = after + SHAPE_TAIL;
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
    let id = varint_at(data, 2);
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
    use crate::built::{self, frame as pk, F};
    use crate::capture::decoder::tests::{landed, processor};
    use crate::capture::opcodes::SPAWN;
    use crate::combat::data_storage::{DataStorage, RosterEntry};
    use crate::i18n::lookup::{NpcLookup, SkillLookup};

    const YOU: u32 = 4_600;
    const MEMBER: u32 = 3_200;

    /// `00 8D` with current HP alone.
    fn hp(id: u32, current: u64) -> Vec<u8> {
        built::hp_record(id, None, Some(&[(0, current)]))
    }

    /// `1B 92 <id> <hp> <max hp> <mp u32> <max mp u32> <u64> <u32> <u32>
    /// <alive u8>`: a party member's HP.
    fn party_hp(id: u32, current: u32, value: u32, alive: u8) -> Vec<u8> {
        built::packet(
            [0x1B, 0x92],
            &[F::V(id), F::V(current), F::V(8_000), F::U32(3_000), F::U32(3_000), F::Raw(&[0; 8]), F::U32(value), F::U32(150_000), F::U8(alive)],
        )
    }

    /// Feed `(capture ms, packet)` in order; returns the deaths counted.
    fn replay(storage: &Arc<DataStorage>, packets: &[(i64, Vec<u8>)]) -> Vec<(i64, i32, i64)> {
        let mut p = PacketDecoder::new(storage.clone(), Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
        for (at, packet) in packets {
            p.pin_clock(Some(*at));
            p.decode_buffer(packet);
        }
        p.pin_clock(None);
        storage.deaths_since(0).iter().map(|d| (d.at, d.player, d.before)).collect()
    }

    /// Records shaped as the game sends them, each read to its length.
    #[test]
    fn hp_records_are_read_to_their_length() {
        let read = |bytes: Vec<u8>| {
            let r = read_hp_record(&bytes[1..]).unwrap();
            assert_eq!(r.end + 1, bytes.len(), "{bytes:02x?}");
            (r.id, r.current_hp)
        };
        // Current HP alone; current and max HP together (a player nearby).
        assert_eq!(read(hp(YOU, 1_200)), (YOU as i32, Some(1_200)));
        assert_eq!(read(built::hp_record(350, None, Some(&[(0, 4_000), (7, 4_000)]))), (350, Some(4_000)));
        // Your own: a value in the u32 list, then current HP.
        assert_eq!(read(built::hp_record(YOU, Some(&[(3, 60_000)]), Some(&[(0, 7_000)]))), (YOU as i32, Some(7_000)));
        // Your MP only; your max HP among other stats.
        assert_eq!(read(built::hp_record(YOU, Some(&[(1, 2_000)]), None)), (YOU as i32, None));
        let stats = [(8, 2_000), (10, 110_000), (11, 150_000), (12, 100_000), (13, 330_000)];
        assert_eq!(read(built::hp_record(YOU, Some(&stats), Some(&[(7, 6_000)]))), (YOU as i32, None));
    }

    /// Your HP: 4,400, 2,800, 1,300, then 0, and 7,800 three seconds later
    /// at the map load that revived you. One death; a second 0 is the same
    /// one.
    #[test]
    fn your_hp_falling_to_zero_is_one_death() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(YOU as i64));
        let deaths = replay(&s, &[
            (1_800_000_010_000, hp(YOU, 4_400)),
            (1_800_000_010_100, hp(YOU, 2_800)),
            (1_800_000_010_200, hp(YOU, 1_300)),
            (1_800_000_010_300, hp(YOU, 0)),
            (1_800_000_010_330, hp(YOU, 0)),
            (1_800_000_013_300, hp(YOU, 7_800)),
        ]);
        assert_eq!(deaths, vec![(1_800_000_010_300, YOU as i32, 1_300)]);
    }

    /// A party member who died out of your sight:
    /// the party record went from full HP to 0, its last byte 1 to 0, and back
    /// to full 7.3 s later. A `00 8D` record of 0 just after (the bytes the
    /// game sent at this member's next death) is the same death.
    #[test]
    fn a_party_member_dies_in_the_party_record() {
        let s = Arc::new(DataStorage::new());
        let deaths = replay(&s, &[
            (1_800_000_020_000, party_hp(MEMBER, 8_000, 100, 1)),
            (1_800_000_044_200, party_hp(MEMBER, 0, 100, 0)),
            (1_800_000_044_210, hp(MEMBER, 0)),
            (1_800_000_051_500, party_hp(MEMBER, 8_000, 0, 1)),
        ]);
        assert_eq!(deaths, vec![(1_800_000_044_200, MEMBER as i32, 8_000)]);
    }

    /// A monster killed, and a player outside your
    /// party at 0: no death is counted for either.
    #[test]
    fn a_monster_or_a_stranger_at_zero_is_no_death() {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(YOU as i64));
        s.name_entity(MEMBER as i32, "Stranger");
        s.replace_roster(vec![("Member".into(), RosterEntry::default()), ("Other".into(), RosterEntry::default())], true);
        let deaths = replay(&s, &[(1_000, hp(67_000, 1_000)), (2_000, hp(67_000, 0)), (3_000, hp(MEMBER, 7_200)), (4_000, hp(MEMBER, 0))]);
        assert!(deaths.is_empty());
    }

    /// Deaths counted for you after `frame` at 10,000 and a zero HP record
    /// at 11,000: the HP each death fell from.
    fn your_deaths_after(frame: Vec<u8>) -> Vec<i64> {
        let s = Arc::new(DataStorage::new());
        s.set_local_player_id(Some(YOU as i64));
        replay(&s, &[(10_000, frame), (11_000, hp(YOU, 0))]).into_iter().map(|d| d.2).collect()
    }

    #[test]
    fn an_hp_record_counts_only_behind_its_own_length() {
        // You (`F8 23`), one large value: HP 12,345,678.
        let record = [0x00, 0x8D, 0xF8, 0x23, 0x02, 0x01, 0x00, 0x4E, 0x61, 0xBC, 0x00, 0x00, 0x00, 0x00, 0x00];
        let framed: Vec<u8> = [&[0x13][..], &record].concat();
        assert_eq!(your_deaths_after(framed.clone()), vec![12_345_678]);
        // One byte more in the frame: the length no longer fits the record.
        let padded: Vec<u8> = [&[0x14][..], &record, &[0xFF]].concat();
        assert_eq!(your_deaths_after(padded.clone()), Vec::<i64>::new());
        // The entity HP scan still takes the HP from it.
        let (storage, mut p) = processor();
        p.decode_buffer(&padded);
        assert_eq!(storage.enemy_hp_now(YOU as i32), Some(12_345_678));
        // A record behind its own length inside a longer frame.
        let outer: Vec<u8> = [&[0x7E, 0x7E, 0x01][..], &framed, &[0x02]].concat();
        assert_eq!(your_deaths_after(pk(&outer)), vec![12_345_678]);
        // A u32 list long enough to need a two-byte length.
        let long = built::hp_record(YOU, Some(&[(9, 7_000); 30]), Some(&[(0, 2_222)]));
        assert!(long[0] >= 0x80);
        assert_eq!(your_deaths_after(long), vec![2_222]);
    }

    #[test]
    fn entity_hp_past_one_hundred_million_is_dropped() {
        let (storage, mut p) = processor();
        let mut body = vec![0x7E, 0x7E, 0x8D, 0xF8, 0x23, 0x02, 0x01, 0x00];
        body.extend(100_000_001u32.to_le_bytes());
        body.extend([0; 4]);
        p.decode_buffer(&pk(&body));
        assert_eq!(storage.enemy_hp_now(YOU as i32), None);
        body[8..12].copy_from_slice(&100_000_000u32.to_le_bytes());
        p.decode_buffer(&pk(&body));
        assert_eq!(storage.enemy_hp_now(YOU as i32), Some(100_000_000));
    }

    /// A world boss whose spawn gave 810,000,000 max HP: its readings past
    /// 100,000,000 count; loose bytes past its max do not, a whole record
    /// past it raises the max.
    #[test]
    fn entity_hp_past_one_hundred_million_counts_up_to_the_known_max() {
        const BOSS: u32 = 40_000;
        let (storage, mut p) = processor();
        let spawn = built::Spawn { id: BOSS, kind: 0x04, code: 2_900_001, hp: 810_000_000, ..built::Spawn::default() };
        p.decode_buffer(&built::packet(SPAWN, &[F::Raw(&spawn.fields())]));
        assert_eq!(storage.enemy_max_hp(BOSS as i32), Some(810_000_000));
        p.decode_buffer(&hp(BOSS, 400_000_000));
        assert_eq!(storage.enemy_hp_now(BOSS as i32), Some(400_000_000));
        // Loose bytes past the max, as LZ4 literals can read: left out.
        let mut body = vec![0x7E, 0x7E, 0x8D, 0xC0, 0xB8, 0x02, 0x02, 0x01, 0x00];
        body.extend(810_000_001u32.to_le_bytes());
        body.extend([0; 4]);
        p.decode_buffer(&pk(&body));
        assert_eq!(storage.enemy_hp_now(BOSS as i32), Some(400_000_000));
        assert_eq!(storage.enemy_max_hp(BOSS as i32), Some(810_000_000));
        // A whole `00 8D` record past it: the spawn's max was short, so the
        // max rises to it.
        p.decode_buffer(&hp(BOSS, 810_000_001));
        assert_eq!(storage.enemy_hp_now(BOSS as i32), Some(810_000_001));
        assert_eq!(storage.enemy_max_hp(BOSS as i32), Some(810_000_001));
    }

    /// The dungeon the storage files the fight under after each map load.
    fn dungeons_after(start: i32, loads: &[(u32, u32, u32, u8)]) -> Vec<i32> {
        let (storage, mut p) = processor();
        storage.enter_dungeon(start);
        loads
            .iter()
            .map(|&(load, map, kind, tail)| {
                p.decode_buffer(&built::map_load(load, map, kind, tail));
                storage.current_dungeon_id()
            })
            .collect()
    }

    #[test]
    fn the_dungeon_ends_only_on_an_open_world_load() {
        let loads = [(1, 600_021, 0, 0x4f), (2, 600_021, 4, 0x4f), (1, 1_010, 0, 0x4f)];
        assert_eq!(dungeons_after(600_021, &loads), vec![600_021, 600_021, 0]);
    }

    #[test]
    fn sealed_and_quest_dungeons_end_at_the_open_world() {
        let loads = [(8, 310_057, 2, 0x1e), (9, 1_010, 2, 0x00), (2, 840_010, 2, 0x1e), (3, 1_010, 2, 0x00)];
        assert_eq!(dungeons_after(0, &loads), vec![310_057, 0, 840_010, 0]);
    }

    #[test]
    fn a_recon_site_counts_as_its_own_map() {
        assert_eq!(dungeons_after(0, &[(2, 151_010, 0x0c, 0x41), (3, 1_010, 2, 0x00)]), vec![151_010, 0]);
    }

    #[test]
    fn the_party_scope_record_needs_two_bytes_after_its_opcode() {
        let scoped = |frame: &[u8], id: i32| {
            let (storage, mut p) = processor();
            p.decode_buffer(frame);
            storage.claim_loot(900, id, "Velto")
        };
        assert!(scoped(&[0x08, 0x06, 0x38, 0xAE, 0x0B], 1_454));
        assert!(!scoped(&[0x07, 0x06, 0x38, 0x78], 120));
        assert!(scoped(&[0x08, 0x06, 0x38, 0x78, 0x00], 120));
    }

    #[test]
    fn only_a_zone_change_of_entity_zero_is_yours() {
        let reset_at = |body: &[u8]| {
            let (storage, mut p) = processor();
            // A fight at 1,000, then quiet until 9,000.
            landed(&mut p, 1_500, 40_001, 11_010_000, 100);
            assert!(!storage.copy_targets().is_empty());
            p.pin_clock(Some(9_000));
            p.decode_buffer(&pk(body));
            p.pin_clock(None);
            storage.zone_changed_at()
        };
        let mut yours = vec![0x23, 0x36, 0x00];
        yours.extend([0x11; 20]);
        assert_eq!(reset_at(&yours), 9_000);
        yours[2] = 0x01;
        assert_ne!(reset_at(&yours), 9_000);
        assert_ne!(reset_at(&[0x23, 0x36]), 9_000);
    }

    #[test]
    fn a_map_load_takes_its_map_from_the_second_u32() {
        // In the open world with a dungeon queued (a made-up id, on a map
        // of the same id): loading its map enters it.
        let loaded = |frame: &[u8]| {
            let (storage, mut p) = processor();
            p.decode_buffer(&built::map_load(1, 1_010, 0, 0x4f));
            storage.enter_dungeon(600_998);
            assert_eq!(storage.current_dungeon_id(), 0);
            p.decode_buffer(frame);
            storage.current_dungeon_id()
        };
        let body = [0x21, 0x36, 0x01, 0x00, 0x00, 0x00, 0xA6, 0x2B, 0x09, 0x00];
        assert_eq!(loaded(&pk(&body)), 600_998);
        assert_eq!(loaded(&pk(&body[..9])), 0);
        assert_eq!(loaded(&built::map_load(1, 600_998, 0, 0x4f)), 600_998);
    }

    #[test]
    fn a_death_record_kills_on_flag_three_under_either_opcode() {
        let dead = |frame: &[u8]| {
            let (storage, mut p) = processor();
            p.decode_buffer(frame);
            storage.has_died(5_000)
        };
        assert!(dead(&[0x0A, 0x42, 0x36, 0x88, 0x27, 0x00, 0x03]));
        assert!(dead(&[0x0A, 0x41, 0x36, 0x88, 0x27, 0x00, 0x03]));
        assert!(!dead(&[0x0A, 0x42, 0x36, 0x88, 0x27, 0x00, 0x01]));
        assert!(dead(&[0x0A, 0x42, 0x36, 0x88, 0x27, 0x05, 0x03]));
    }

    /// A boss into combat with an enrage deadline, then out. A player's
    /// record into combat and shapes no capture had change nothing.
    #[test]
    fn combat_records_set_the_enrage_and_end_the_fight() {
        const BOSS: u32 = 18_000;
        let state = |id: u32, rest: &[u8]| built::packet(COMBAT_STATE, &[F::V(id), F::Raw(rest)]);
        let deadline: i64 = 301_000;
        let with_deadline = [&[0x01, 0x01][..], &deadline.to_le_bytes()].concat();
        let (storage, mut p) = processor();
        storage.register_boss(BOSS as i32);
        p.decode_buffer(&state(BOSS, &with_deadline));
        landed(&mut p, 1_500, BOSS, 11_010_000, 100);
        assert_eq!(storage.copy_targets()[&(BOSS as i32)].enrage_at, Some(deadline));
        let odd = [
            state(3_000, &[0x00, 0x01]),
            state(BOSS, &[0x00, 0x02]),
            state(BOSS, &[0x01, 0x00]),
            state(BOSS, &with_deadline[..9]),
        ];
        for frame in &odd {
            p.decode_buffer(frame);
        }
        assert!(!storage.fight_ended_by_server(BOSS as i32));
        p.decode_buffer(&state(BOSS, &[0x00, 0x00]));
        assert!(storage.fight_ended_by_server(BOSS as i32));
    }

    #[test]
    fn leaving_the_world_is_no_death() {
        // 5000 a linked summon, 3000 nothing known: flag 7 kills neither.
        let (storage, p) = processor();
        storage.confirm_pet(5_000, 2_000);
        p.read_death(&pk(&[0x42, 0x36, 0x88, 0x27, 0x00, 0x07]));
        p.read_death(&pk(&[0x42, 0x36, 0xB8, 0x17, 0x00, 0x07]));
        assert!(!storage.has_died(5_000) && !storage.has_died(3_000));
    }

    #[test]
    fn party_hp_needs_a_player_id_and_a_sane_max() {
        let (storage, p) = processor();
        assert!(p.read_party_hp(&[0x0C, 0x1B, 0x92, 0x80, 0x19, 0xC0, 0x3E, 0xC0, 0x3E]));
        assert_eq!(storage.enemy_max_hp(3_200), Some(8_000));
        let refused = |fields: &[F]| {
            let (storage, p) = processor();
            let ok = p.read_party_hp(&built::packet([0x1B, 0x92], fields));
            (ok, storage.enemy_max_hp(99), storage.enemy_max_hp(3_200))
        };
        assert_eq!(refused(&[F::V(99), F::V(10), F::V(8_000)]), (false, None, None));
        assert_eq!(refused(&[F::V(3_200), F::V(10), F::V(0)]), (false, None, None));
        assert_eq!(refused(&[F::V(3_200), F::V(10), F::V(50_000_001)]), (false, None, None));
        assert_eq!(refused(&[F::V(3_200), F::Raw(&[0x80])]), (false, None, None));
        assert_eq!(refused(&[]), (false, None, None));
        assert_eq!(refused(&[F::V(3_200), F::V(10), F::V(50_000_000)]), (true, None, Some(50_000_000)));
    }

    /// Frames that stop short, or whose length does not read, are let go
    /// by every reader without a panic.
    #[test]
    fn broken_frames_reach_no_world_reader() {
        let (storage, p) = processor();
        let frames = vec![
            vec![0x80, 0x80],
            pk(&[0x42, 0x36]),
            pk(&[0x42, 0x36, 0x80]),
            pk(&[0x42, 0x36, 0x05, 0x80]),
            pk(&[0x42, 0x36, 0x05, 0x00, 0x80]),
            pk(&[0x1B, 0x92]),
            pk(&[0x21, 0x36, 0x01]),
            pk(&[0x06, 0x38]),
            pk(&[0x21, 0x8D]),
            pk(&[0x21, 0x8D, 0x80]),
            // A death record of entity 5 behind a length that does not read.
            vec![0x80, 0x80, 0x42, 0x36, 0x05, 0x00, 0x03],
        ];
        for f in &frames {
            p.read_party_scope(f);
            p.read_zone_change(f);
            p.read_map_load(f);
            p.read_death(f);
            p.read_combat_state(f);
            assert!(!p.read_party_hp(f));
            p.read_hp_records(f);
            p.read_entity_hp(f);
        }
        assert!(!storage.has_died(5));
        assert_eq!(storage.current_dungeon_id(), 0);
    }
}
