//! The world map's lists (`01 91`), which the server sends about every
//! second while the player has the world map open, and never otherwise:
//! for the map shown, each field boss on the map's Field Boss list, whether
//! it is up, and its time; then the map's Abyss Bosses, Spacetime Rifts and
//! other objects. Read into the storage's queue for the meter's field boss
//! notices.

use super::world::body_after;
use super::{varint_at, PacketDecoder};
use crate::capture::opcodes::WORLD_MAP_LISTS;

/// A list never holds more entries than this (24 field bosses on a map).
const MAX_ENTRIES: usize = 255;

/// One field boss of the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldBossTime {
    /// Its row in the game's WorldMapFieldNamed table.
    pub row: u32,
    /// Up now: `time_ms` is when it came. Not up: `time_ms` is when it comes
    /// next (after a kill, the kill plus the boss's respawn delay).
    pub spawned: bool,
    /// Unix milliseconds.
    pub time_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldMapList {
    /// The map shown, by its map id (1010, 1110 or 20).
    pub map: u32,
    pub bosses: Vec<FieldBossTime>,
}

/// Reads `body` from `at` on as it goes.
struct Reader<'a> {
    body: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn bytes(&mut self, n: usize) -> Option<&[u8]> {
        let out = self.body.get(self.at..self.at + n)?;
        self.at += n;
        Some(out)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.bytes(1)?[0])
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.bytes(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.bytes(8)?.try_into().ok()?))
    }

    fn varint(&mut self) -> Option<u32> {
        let v = varint_at(self.body, self.at);
        if v.length <= 0 || v.value < 0 {
            return None;
        }
        self.at += v.length as usize;
        u32::try_from(v.value).ok()
    }

    fn count(&mut self) -> Option<usize> {
        Some(self.varint()? as usize).filter(|&n| n <= MAX_ENTRIES)
    }

    /// x, y, z.
    fn place(&mut self) -> Option<()> {
        self.bytes(12).map(|_| ())
    }
}

/// The lists after the opcode: a result (`00 00`), the map, then the field
/// bosses: each a mask (bit 0: a place follows), its row, the place, and its
/// time; whether it is up is a bit of a byte shared by eight entries, which
/// comes before the time of the first of them (bit 0 for it, bit 1 for the
/// next and so on). Then the Abyss Bosses (id, monster, place), the rifts
/// (id, object, rift, place, interactions, opened at) and other objects,
/// read to check that the record ends where the lists do.
pub fn world_map_list(body: &[u8]) -> Option<WorldMapList> {
    let mut r = Reader { body, at: 0 };
    if r.bytes(2)? != [0, 0] {
        return None;
    }
    let map = r.u32()?;
    let n = r.count()?;
    let mut bosses = Vec::with_capacity(n);
    let mut up = 0u8;
    for i in 0..n {
        let mask = r.u8()?;
        if mask > 1 {
            return None;
        }
        let row = r.varint().filter(|&row| row > 0)?;
        if mask == 1 {
            r.place()?;
        }
        if i % 8 == 0 {
            up = r.u8()?;
        }
        let spawned = up >> (i % 8) & 1 == 1;
        bosses.push(FieldBossTime { row, spawned, time_ms: r.u64()? });
    }
    for _ in 0..r.count()? {
        r.varint()?;
        r.u32()?;
        r.place()?;
    }
    for _ in 0..r.count()? {
        r.varint()?;
        r.u32()?;
        r.u32()?;
        r.place()?;
        r.varint()?;
        r.u64()?;
    }
    // No capture has shown an object in the last list, so its layout is
    // not known; an empty one ends the record.
    if r.count()? == 0 && r.at != body.len() {
        return None;
    }
    Some(WorldMapList { map, bosses })
}

impl PacketDecoder {
    /// The world map's lists, queued for the field boss notices.
    pub(super) fn read_world_map(&self, frame: &[u8]) {
        if let Some(list) = body_after(frame, WORLD_MAP_LISTS).and_then(world_map_list) {
            self.data_storage.note_world_map(list);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::processor;
    use super::*;
    use crate::built::{self, varint};

    /// A list for `map` of `bosses` (row, up, unix ms; one up has its place)
    /// and `rifts` rifts.
    fn list_body(map: u32, bosses: &[(u32, bool, u64)], rifts: u32) -> Vec<u8> {
        let mut body = vec![0, 0];
        body.extend(map.to_le_bytes());
        body.extend(varint(bosses.len() as u32));
        let mut shared = 0;
        for (i, &(row, up, ms)) in bosses.iter().enumerate() {
            body.push(up as u8);
            body.extend(varint(row));
            if up {
                body.extend([0x42; 12]);
            }
            if i % 8 == 0 {
                shared = body.len();
                body.push(0);
            }
            body[shared] |= (up as u8) << (i % 8);
            body.extend(ms.to_le_bytes());
        }
        body.push(0);
        body.extend(varint(rifts));
        for i in 0..rifts {
            body.extend(varint(1_632_483 + i));
            body.extend(1_100_011u32.to_le_bytes());
            body.extend(5_000_015u32.to_le_bytes());
            body.extend([0x42; 12]);
            body.extend(varint(254));
            body.extend(1_791_000_000_451u64.to_le_bytes());
        }
        body.push(0);
        body
    }

    /// Twenty bosses: every third one up, so the up bits cross the bytes
    /// shared by entries 1-8, 9-16 and 17-20.
    fn twenty() -> Vec<(u32, bool, u64)> {
        (0..20).map(|i| (101_001 + i, i % 3 == 0, 1_792_400_000_000 + i as u64 * 60_000)).collect()
    }

    #[test]
    fn a_list_gives_each_bosss_row_whether_it_is_up_and_its_time() {
        let bosses = twenty();
        let list = world_map_list(&list_body(1010, &bosses, 3)).unwrap();
        assert_eq!(list.map, 1010);
        let read: Vec<(u32, bool, u64)> = list.bosses.iter().map(|b| (b.row, b.spawned, b.time_ms)).collect();
        assert_eq!(read, bosses);
        let empty = world_map_list(&list_body(20, &[], 0)).unwrap();
        assert_eq!((empty.map, empty.bosses.len()), (20, 0));
    }

    #[test]
    fn a_list_that_does_not_read_to_its_end_is_left_out() {
        let body = list_body(1010, &twenty(), 1);
        assert_eq!(world_map_list(&body[..body.len() - 1]), None, "cut short");
        assert_eq!(world_map_list(&[&body[..], &[0]].concat()), None, "a byte past the lists");
        let mut result = body.clone();
        result[1] = 1;
        assert_eq!(world_map_list(&result), None, "a result that is not 00 00");
        let mut mask = list_body(1010, &[(101_021, false, 1)], 0);
        mask[7] = 2;
        assert_eq!(world_map_list(&mask), None, "a mask with an unknown bit");
    }

    #[test]
    fn an_object_in_the_last_list_does_not_hide_the_bosses() {
        let mut body = list_body(1110, &[(111_021, false, 1_792_400_000_000)], 0);
        *body.last_mut().unwrap() = 1;
        body.extend([7; 9]);
        assert_eq!(world_map_list(&body).unwrap().bosses[0].row, 111_021);
    }

    #[test]
    fn the_decoder_queues_each_list_in_order() {
        let (storage, mut p) = processor();
        let frames = [
            built::frame(&[&WORLD_MAP_LISTS[..], &list_body(1010, &twenty(), 0)].concat()),
            built::frame(&[&WORLD_MAP_LISTS[..], &list_body(20, &[], 0)].concat()),
        ]
        .concat();
        assert_eq!(p.decode_buffer(&frames), frames.len());
        let lists = storage.take_world_map();
        assert_eq!(lists.iter().map(|l| l.map).collect::<Vec<_>>(), [1010, 20]);
        assert_eq!(lists[0].bosses.len(), 20);
        assert!(storage.take_world_map().is_empty(), "taken once");
    }

    /// Run on real captures: `A2_REPLAY_CAPTURE=<packets_*.txt>[:<more>]
    /// cargo test --lib world_map -- --ignored --nocapture`. Every list
    /// reads: the walk prints how many, by map, and the field bosses' rows.
    #[test]
    #[ignore]
    fn real_lists_read() {
        use crate::capture::packet_log::{LogLine, PacketLogReplay};
        use crate::combat::data_storage::DataStorage;
        use crate::i18n::lookup::{NpcLookup, SkillLookup};
        use std::collections::BTreeMap;
        use std::sync::Arc;
        let Ok(paths) = std::env::var("A2_REPLAY_CAPTURE") else {
            return;
        };
        let mut by_map: BTreeMap<u32, usize> = BTreeMap::new();
        let mut rows: BTreeMap<u32, (usize, usize)> = BTreeMap::new();
        for path in paths.split(':') {
            let storage = Arc::new(DataStorage::new());
            let mut replay = PacketLogReplay::new(Arc::clone(&storage), SkillLookup::new().into(), NpcLookup::new().into(), Default::default());
            let text = std::fs::read_to_string(path).unwrap();
            let mut lists = 0;
            for line in text.lines().filter_map(LogLine::parse) {
                replay.feed(&line);
                for list in storage.take_world_map() {
                    lists += 1;
                    *by_map.entry(list.map).or_default() += 1;
                    for boss in list.bosses {
                        let seen = rows.entry(boss.row).or_default();
                        if boss.spawned {
                            seen.0 += 1;
                        } else {
                            seen.1 += 1;
                        }
                    }
                }
            }
            println!("{path}: {lists}");
        }
        println!("by map: {by_map:?}");
        println!("rows (up, not up): {rows:?}");
        assert!(!by_map.is_empty(), "no list read");
    }
}
