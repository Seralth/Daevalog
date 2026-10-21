//! Power Shards: whether your own hits use them, and how many you have left.
//! A cast with shards on spends one, and each of its hits carries bit 0x80 in
//! the flags byte; a hit without the flags byte says nothing either way.
//! The count comes from the bag's item update. Read into the storage's queue
//! for the meter's shard notice.

use super::world::body_after;
use super::PacketDecoder;
use crate::capture::opcodes::BAG_ITEM;

/// The flags byte's Power Shard bit.
pub const SHARD_FLAG: u8 = 0x80;

/// The Power Shard items, 5% and Superior 10%: tradeable, bound, Abyss and
/// event stacks.
pub const SHARD_ITEMS: [u32; 8] =
    [513_460_000, 513_460_003, 513_460_005, 533_460_001, 533_460_004, 533_460_006, 563_460_001, 563_460_004];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShardEvent {
    /// A hit of yours with a flags byte, at `at` ms. Hits of one cast share
    /// the skill and the record's `cast` byte.
    Hit { at: i64, skill: i32, cast: u8, shard: bool },
    /// A shard stack in your bag now holds `count` (0: none left).
    Count { item: u32, count: u32 },
}

fn u32_at(body: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(body.get(at..at + 4)?.try_into().ok()?))
}

/// A bag item update for a shard stack: `01 <sub> 00 <slot u32> 00 <u8>
/// 00 00 <item u32> <count u32> ...`; the item and its count.
pub fn bag_count(body: &[u8]) -> Option<(u32, u32)> {
    if body.first() != Some(&1) {
        return None;
    }
    let item = u32_at(body, 11).filter(|item| SHARD_ITEMS.contains(item))?;
    Some((item, u32_at(body, 15)?))
}

impl PacketDecoder {
    /// A shard stack's count, queued for the shard notice.
    pub(super) fn read_shard_count(&self, frame: &[u8]) {
        if let Some((item, count)) = body_after(frame, BAG_ITEM).and_then(bag_count) {
            self.data_storage.note_shard(ShardEvent::Count { item, count });
        }
    }

    /// A hit with a flags byte, queued when it is your own.
    pub(super) fn note_shard_hit(&self, actor: i32, skill: i32, cast: u8, flags: u8) {
        if self.data_storage.local_player_id() == Some(i64::from(actor)) {
            let shard = flags & SHARD_FLAG != 0;
            self.data_storage.note_shard(ShardEvent::Hit { at: self.record_time(), skill, cast, shard });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::processor;
    use super::*;
    use crate::built::{self, shard_hit, Hit};

    /// A bag update for `item` holding `count`.
    fn bag_body(item: u32, count: u32) -> Vec<u8> {
        let mut body = vec![1, 0, 0];
        body.extend(7u32.to_le_bytes());
        body.extend([0, 3, 0, 0]);
        body.extend(item.to_le_bytes());
        body.extend(count.to_le_bytes());
        body.extend([0; 21]);
        body
    }

    #[test]
    fn a_bag_update_gives_a_shard_stacks_count_only() {
        assert_eq!(bag_count(&bag_body(533_460_001, 418)), Some((533_460_001, 418)));
        assert_eq!(bag_count(&bag_body(533_460_004, 0)), Some((533_460_004, 0)), "the last one used");
        assert_eq!(bag_count(&bag_body(600_590_001, 5)), None, "not a shard");
        assert_eq!(bag_count(&bag_body(533_460_001, 5)[..18]), None, "cut short");
    }

    #[test]
    fn only_your_own_hits_with_a_flags_byte_are_queued() {
        let (storage, mut p) = processor();
        storage.set_local_identity_from_game(1_500, None);
        let frames = [
            shard_hit().packet(),
            Hit { uid: 0xB2, plotter: Some((0x00, 0, 1)), ..shard_hit() }.packet(),
            Hit { uid: 0xB3, switch: 0x04, plotter: None, ..shard_hit() }.packet(),
            Hit { actor: 1_600, ..shard_hit() }.packet(),
            built::frame(&[&BAG_ITEM[..], &bag_body(533_460_001, 41)].concat()),
        ]
        .concat();
        p.decode_buffer(&frames);
        let events = storage.take_shards();
        assert_eq!(
            events,
            [
                ShardEvent::Hit { at: 1_000, skill: 16_040_000, cast: 0xB1, shard: true },
                ShardEvent::Hit { at: 1_000, skill: 16_040_000, cast: 0xB2, shard: false },
                ShardEvent::Count { item: 533_460_001, count: 41 },
            ],
            "no flags byte, or another player's hit: nothing"
        );
        assert!(storage.take_shards().is_empty(), "taken once");
    }

    #[test]
    fn before_you_are_known_no_hit_is_queued() {
        let (storage, mut p) = processor();
        p.decode_buffer(&shard_hit().packet());
        assert!(storage.take_shards().is_empty());
    }
}
