//! The party roster (02 97): who is in your party, their levels, gear and
//! combat power. And the leave (1b 97).

use super::world::body_after;
use super::PacketDecoder;
use crate::capture::opcodes::{PARTY_LEAVE, PARTY_ROSTER};
use crate::capture::varint::{varint_at, u16_at, u32_at, u64_at};
use crate::combat::data_storage::RosterEntry;
use crate::entity::job_class::JobClass;

use std::ops::Range;

impl PacketDecoder {
    /// Gives the storage each roster in `data`, or the leave `data` is. The
    /// dungeon and key go in before the members: a roster that breaks up the
    /// party also clears them, and the other order would leave them set.
    /// The pass takes one frame at a time, so a leave and the rosters after
    /// it in one bundle keep their order.
    pub(super) fn read_party_list(&self, data: &[u8]) {
        if let Some(key) = party_left(data) {
            self.data_storage.leave_party(key);
        }
        for roster in rosters(data) {
            tracing::debug!(
                "party roster: {} members, complete {}, dungeon {}",
                roster.members.len(),
                roster.complete,
                roster.dungeon_id
            );
            self.data_storage.enter_dungeon(roster.dungeon_id);
            self.data_storage.note_party_key(roster.key);
            self.data_storage.replace_roster(roster.members, roster.complete);
        }
    }
}

/// The key of the leave record that `region` is: `1b 97 <key u32>`, a
/// frame of its own. Read as "you are out of party `key`", which is inferred,
/// not proven: the key is the party you were in, and a roster of that key
/// rarely comes after it. When one does, it is a late one in the same
/// bundle, followed by the new party's.
fn party_left(region: &[u8]) -> Option<u32> {
    let key: [u8; 4] = body_after(region, PARTY_LEAVE)?.try_into().ok()?;
    Some(u32::from_le_bytes(key))
}

/// A party roster as `PacketDecoder::read_party_list` reads it.
pub(super) struct Roster {
    pub members: Vec<(String, crate::combat::data_storage::RosterEntry)>,
    pub complete: bool,
    /// The party's key, which a leave names.
    pub key: u32,
    pub dungeon_id: i32,
    /// Each member's name field, in member order.
    pub name_fields: Vec<Range<usize>>,
}

/// Bytes of a vacant slot's record up to its name: mask, slot, dbid, name length.
const VACANT_HEADER: usize = 11;

/// Most records a roster holds, and the size byte's top.
const PARTY_MAX: u8 = 12;
/// Longest party or member name, in bytes.
const NAME_MAX: usize = 40;
/// Bytes a record must have left to be read at all.
const RECORD_MIN: usize = 20;
/// Bytes a place must have left to be taken as the next record's start.
const NEXT_MIN: usize = 12;
/// Bytes from a vacant record's start to its tag: mask, slot and 21 zeros.
const VACANT_ZEROS_END: usize = 23;
/// How far past a record's fixed fields the next one may start.
const NEXT_WINDOW: usize = 32;
/// How far past the item level a member's world id may sit.
const WORLD_WINDOW: usize = 10;
const LEVEL_TOP: i32 = 200;
const ITEM_LEVEL_TOP: i32 = 1_000_000;
pub(crate) const COMBAT_POWER_TOP: u64 = 100_000_000;

/// Every roster in `data`, in byte order. Any `02 97` may open one, frame
/// boundaries or not, including one inside a roster already read.
pub(super) fn rosters(data: &[u8]) -> Vec<Roster> {
    let starts = data.windows(2).enumerate().filter(|(_, pair)| *pair == PARTY_ROSTER);
    starts.filter_map(|(at, _)| roster_after_opcode(data, at + 2)).collect()
}

/// Reads the roster body at `body`, the byte after `02 97`. Little-endian:
///
/// `<key u32> <party name: u8 len + UTF-8> <size u8> <dungeon u32> <2 bytes>
/// <leader account id u64> <3 bytes> <count varint>`, then `count` records in
/// slot order, then `00 NN`. A member record is `<mask u8> <slot u8> <account
/// id u64, top u16 = home world> <name: u8 len + UTF-8> <class u32> <level
/// u32> <item level u32> <0 or 1 unknown byte> <world u16> <u16> <tag u8>
/// <combat power u64> <tail: 3 or 11 bytes>`. A vacant one is `00 <slot>`, 21
/// zero bytes, a tag and 11 zero bytes (36 with one more zero before the tag).
fn roster_after_opcode(data: &[u8], body: usize) -> Option<Roster> {
    let key = u32_at(data, body)?;
    let (_, party_name) = short_text(data, body + 4)?;
    let size = *data.get(party_name.end)?;
    if !(1..=PARTY_MAX).contains(&size) {
        return None;
    }
    let dungeon_id = u32_at(data, party_name.end + 1)? as i32;
    // Dungeon, the two unknown bytes, the leader id and three more bytes.
    let count_at = party_name.end + 1 + 4 + 2 + 8 + 3;
    let count = varint_at(data, count_at);
    if count.length < 1 || !(1..=i32::from(PARTY_MAX)).contains(&count.value) {
        return None;
    }

    let mut roster = Roster { members: Vec::new(), complete: false, key, dungeon_id, name_fields: Vec::new() };
    let mut at = count_at + count.length as usize;
    for number in 1..=count.value {
        if data.len().saturating_sub(at) < RECORD_MIN {
            break;
        }
        let slot = data[at + 1];
        // Anything not vacant is read as a member. The mask is left unchecked
        // on purpose: the first record is not found by the next-record search,
        // which is where a zero mask is turned away.
        let resume = if vacant_record(data, at) {
            at + VACANT_HEADER
        } else {
            match member_record(data, at, slot) {
                MemberRead::Read { name, member, field, end } => {
                    roster.members.push((name, member));
                    roster.name_fields.push(field);
                    end
                }
                MemberRead::Broken => break,
                MemberRead::CutShort => return None,
            }
        };
        if number == count.value {
            roster.complete = true;
            break;
        }
        match next_record(data, resume, slot.wrapping_add(1)) {
            Some(next) => at = next,
            None => break,
        }
    }
    (!roster.members.is_empty()).then_some(roster)
}

/// How a member record at a given start reads.
enum MemberRead {
    Read { name: String, member: RosterEntry, field: Range<usize>, end: usize },
    /// A field out of range or missing: the roster stops short here.
    Broken,
    /// `data` ends inside the combat power: no roster at all.
    CutShort,
}

/// The member record for `slot` at `at`, which has at least `RECORD_MIN` bytes left.
fn member_record(data: &[u8], at: usize, slot: u8) -> MemberRead {
    let Some(dbid) = u64_at(data, at + 2) else { return MemberRead::Broken };
    let Some((name, field)) = short_text(data, at + 10) else { return MemberRead::Broken };
    let stats = field.end;
    let (Some(class), Some(level), Some(item_level)) =
        (u32_at(data, stats), u32_at(data, stats + 4), u32_at(data, stats + 8))
    else {
        return MemberRead::Broken;
    };
    let (level, item_level) = (level as i32, item_level as i32);
    if !(1..=LEVEL_TOP).contains(&level) || !(0..=ITEM_LEVEL_TOP).contains(&item_level) {
        return MemberRead::Broken;
    }
    let Some((world_at, world)) = world_field(data, stats + 12, home_world(dbid)) else {
        return MemberRead::Broken;
    };
    // The world, the u16 after it and the tag.
    let power_at = world_at + 2 + 2 + 1;
    let Some(combat_power) = u64_at(data, power_at) else { return MemberRead::CutShort };
    if combat_power > COMBAT_POWER_TOP {
        return MemberRead::Broken;
    }
    let member = RosterEntry {
        slot,
        level,
        gear_score: item_level,
        combat_power: combat_power as i64,
        server_id: world,
        dbid,
        job: JobClass::job_from_party_list(class),
        ..Default::default()
    };
    MemberRead::Read { name: name.to_string(), member, field, end: power_at + 8 }
}

/// A member record of the roster's layout at `at`, as the roster reads it,
/// with where it ends; `None` when it does not read. The other party lists
/// share the layout (`capture::player_facts`).
pub(crate) fn member_at(data: &[u8], at: usize) -> Option<(String, RosterEntry, usize)> {
    if data.len().saturating_sub(at) < RECORD_MIN {
        return None;
    }
    match member_record(data, at, data[at + 1]) {
        MemberRead::Read { name, member, end, .. } => Some((name, member, end)),
        MemberRead::Broken | MemberRead::CutShort => None,
    }
}

/// Where a member's world sits, from `past_stats` (the byte after the item
/// level) on, and the world. Mostly the account id's world; when the account
/// id names another (account 2201, record and the game's server tag
/// 2202), the layout finds it: at `past_stats` or one byte on, with the
/// tag 4 bytes past it.
fn world_field(data: &[u8], past_stats: usize, home: u16) -> Option<(usize, u16)> {
    let laid_out = |p: usize| {
        u16_at(data, p).is_some_and(|world| (1_000..=9_999).contains(&world)) && matches!(data.get(p + 4), Some(3 | 4))
    };
    let at = (past_stats..=past_stats + WORLD_WINDOW)
        .find(|&p| u16_at(data, p) == Some(home))
        .or_else(|| (past_stats..=past_stats + 1).find(|&p| laid_out(p)))?;
    Some((at, u16_at(data, at)?))
}

/// Where the record for `slot` starts, looked for from `from` on, since a
/// record's tail has no fixed length. Places that carry the slot number but
/// read as neither kind of record are passed over.
fn next_record(data: &[u8], from: usize, slot: u8) -> Option<usize> {
    (from..=from + NEXT_WINDOW).find(|&p| {
        p + NEXT_MIN <= data.len() && data[p + 1] == slot && (vacant_record(data, p) || member_start(data, p))
    })
}

/// A vacant slot: mask 0 and 21 zeros after the slot number.
fn vacant_record(data: &[u8], at: usize) -> bool {
    match data.get(at..at + VACANT_ZEROS_END) {
        Some(record) => record[0] == 0 && record[2..].iter().all(|&b| b == 0),
        None => false,
    }
}

/// The first bytes of a member record: a mask, a plausible home world and a name.
fn member_start(data: &[u8], at: usize) -> bool {
    data[at] != 0
        && u64_at(data, at + 2).is_some_and(|dbid| (1..=9_999).contains(&home_world(dbid)))
        && short_text(data, at + 10).is_some()
}

/// The world an account id names: a character number, two zero bytes, then
/// the world the character was made on.
pub(crate) fn home_world(dbid: u64) -> u16 {
    (dbid >> 48) as u16
}

/// A name of 1 to `NAME_MAX` bytes of UTF-8 after its length byte at `at`,
/// with the range of its bytes.
fn short_text(data: &[u8], at: usize) -> Option<(&str, Range<usize>)> {
    let len = usize::from(*data.get(at)?);
    if !(1..=NAME_MAX).contains(&len) {
        return None;
    }
    let field = at + 1..at + 1 + len;
    let text = std::str::from_utf8(data.get(field.clone())?).ok()?;
    Some((text, field))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::built::{self, Member, Slot};
    use crate::capture::decoder::tests::processor;
    use crate::capture::varint::index_of_bytes;
    use crate::combat::data_storage::DataStorage;
    use crate::i18n::lookup::{NpcLookup, SkillLookup};
    use std::sync::Arc;

    const NAMES: [&str; 6] = ["m0", "m1", "m2", "m3", "m4", "m5"];
    const LEADER: u64 = (1201 << 48) | 0x0003_0001;

    /// A filled slot on world 1201 with values that tell the slots apart.
    fn filled(slot: u8) -> Member<'static> {
        Member {
            mask: 0x1e,
            slot,
            dbid: (1201 << 48) | 0x0003_0000 | u64::from(slot),
            name: NAMES[usize::from(slot) % NAMES.len()],
            class: 26,
            level: 40 + u32::from(slot),
            gear: 800 + u32::from(slot),
            extra: &[],
            tag: 4,
            combat_power: 40_000 + u64::from(slot),
            tail: &[0x00, 0x01, 0x02],
        }
    }

    fn party(slots: &[Slot]) -> Vec<u8> {
        built::roster(0x3000, "Party 7", 600_031, LEADER, slots)
    }

    /// Two filled slots, the second as given.
    fn first_then(second: Member) -> Vec<u8> {
        party(&[Slot::Member(filled(1)), Slot::Member(second)])
    }

    /// The slots read and whether the walk completed, for the one roster in `data`.
    fn outcome(data: &[u8]) -> Option<(Vec<u8>, bool)> {
        let found = rosters(data);
        assert!(found.len() <= 1, "{} rosters", found.len());
        found.into_iter().next().map(|r| (r.members.iter().map(|(_, m)| m.slot).collect(), r.complete))
    }

    /// Where the combat power of member `n` (0 is the first) of the one
    /// roster in `data` ends: past its name come class, level and item level
    /// (12 bytes), world, u16 and tag (5), then the combat power (8). Holds
    /// for members with no `extra` bytes.
    fn power_end(data: &[u8], n: usize) -> usize {
        rosters(data)[0].name_fields[n].end + 12 + 5 + 8
    }

    /// Bytes of a `filled` member's tail.
    fn tail_len() -> usize {
        filled(1).tail.len()
    }

    fn spliced(data: &[u8], at: usize, bytes: &[u8]) -> Vec<u8> {
        let mut out = data[..at].to_vec();
        out.extend_from_slice(bytes);
        out.extend_from_slice(&data[at..]);
        out
    }

    /// `data` with the first `from` swapped for `to`, of the same length.
    fn patched(data: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
        assert_eq!(from.len(), to.len());
        let at = index_of_bytes(data, 0, from).expect("pattern");
        let mut out = data.to_vec();
        out[at..at + to.len()].copy_from_slice(to);
        out
    }

    /// `data` with the byte at `at` set to `value`.
    fn with_byte(data: &[u8], at: usize, value: u8) -> Vec<u8> {
        let mut out = data.to_vec();
        out[at] = value;
        out
    }

    /// Offset of the party name's length byte: past the opcode and the key.
    fn party_name_at(data: &[u8]) -> usize {
        index_of_bytes(data, 0, &PARTY_ROSTER).unwrap() + 2 + 4
    }

    /// Offset of the size byte in a `party` frame, after the party name.
    fn size_at(data: &[u8]) -> usize {
        party_name_at(data) + 1 + "Party 7".len()
    }

    fn storage_with(members: &[u8]) -> (Arc<DataStorage>, PacketDecoder) {
        let storage: Arc<DataStorage> = DataStorage::new().into();
        if !members.is_empty() {
            let held = members
                .iter()
                .map(|&s| (NAMES[usize::from(s)].to_string(), RosterEntry { slot: s, gear_score: 700, ..Default::default() }))
                .collect();
            storage.replace_roster(held, true);
        }
        let p = PacketDecoder::new(Arc::clone(&storage), SkillLookup::new().into(), NpcLookup::new().into());
        (storage, p)
    }

    // Finding rosters.

    #[test]
    fn a_roster_frame_reads_as_one_roster() {
        let data = party(&[Slot::Member(filled(1)), Slot::Vacant(2), Slot::Member(filled(3))]);
        let found = rosters(&data);
        assert_eq!(found.len(), 1);
        let r = &found[0];
        assert!(r.complete);
        assert_eq!(r.dungeon_id, 600_031);
        let names: Vec<_> = r.members.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["m1", "m3"]);
        let fields: Vec<_> = r.name_fields.iter().map(|f| &data[f.clone()]).collect();
        assert_eq!(fields, [b"m1".as_slice(), b"m3".as_slice()]);
    }

    #[test]
    fn back_to_back_frames_give_rosters_in_order() {
        let first = party(&[Slot::Member(filled(1))]);
        let second = built::roster(0x3001, "Party 8", 600_002, LEADER, &[Slot::Member(filled(2))]);
        let both = [first.clone(), second.clone()].concat();
        let found = rosters(&both);
        assert_eq!(found.iter().map(|r| r.dungeon_id).collect::<Vec<_>>(), [600_031, 600_002]);
        assert_eq!(&both[found[0].name_fields[0].clone()], b"m1".as_slice());
        let in_second = found[1].name_fields[0].clone();
        assert!(in_second.start > first.len());
        assert_eq!(&both[in_second], b"m2".as_slice());
    }

    #[test]
    fn bytes_in_front_shift_the_name_fields() {
        let data = party(&[Slot::Member(filled(1)), Slot::Member(filled(2))]);
        let plain = rosters(&data)[0].name_fields.clone();
        let shifted = rosters(&spliced(&data, 0, &[0x11, 0x22, 0x33]))[0].name_fields.clone();
        let moved: Vec<_> = plain.iter().map(|f| f.start + 3..f.end + 3).collect();
        assert_eq!(shifted, moved);
    }

    #[test]
    fn an_opcode_before_junk_is_no_roster() {
        let junk = [0x02, 0x97, 0x05, 0xff, 0x13, 0x00, 0x40, 0x41, 0x99, 0x02, 0x97, 0x00];
        assert!(rosters(&junk).is_empty());
        assert!(rosters(&[0x02, 0x97]).is_empty());
    }

    #[test]
    fn a_roster_inside_another_roster_is_found_too() {
        let inner = built::roster(0x3002, "Party 9", 600_021, LEADER, &[Slot::Member(filled(2))]);
        let mut outer_first = filled(1);
        let tail = [&[0x00, 0x01, 0x02][..], &inner].concat();
        outer_first.tail = &tail;
        let data = party(&[Slot::Member(outer_first)]);
        let found = rosters(&data);
        assert_eq!(found.iter().map(|r| r.dungeon_id).collect::<Vec<_>>(), [600_031, 600_021]);
        assert_eq!(found[1].members[0].1.slot, 2);
    }

    #[test]
    fn the_party_name_must_be_1_to_40_bytes_of_utf8() {
        let one = [Slot::Member(filled(1))];
        let named = |name: &str| built::roster(0x3000, name, 600_031, LEADER, &one);
        assert!(rosters(&named("")).is_empty());
        assert!(rosters(&named(&"p".repeat(41))).is_empty());
        assert_eq!(rosters(&named(&"p".repeat(40))).len(), 1);
        assert!(rosters(&patched(&named("zz"), b"zz", &[0xff, 0xfe])).is_empty());
    }

    #[test]
    fn the_size_byte_must_be_1_to_12() {
        let data = party(&[Slot::Member(filled(1))]);
        let at = size_at(&data);
        assert_eq!(data[at], 5);
        for bad in [0, 13] {
            assert!(rosters(&with_byte(&data, at, bad)).is_empty(), "size {bad}");
        }
        for good in [1, 12] {
            assert_eq!(rosters(&with_byte(&data, at, good)).len(), 1, "size {good}");
        }
    }

    #[test]
    fn the_record_count_must_be_1_to_12() {
        let data = party(&[Slot::Member(filled(1))]);
        // After the size byte: dungeon (4), two unknown bytes, leader id (8)
        // and three more bytes, then the count varint.
        let at = size_at(&data) + 1 + 4 + 2 + 8 + 3;
        assert_eq!(data[at], 1);
        for bad in [0, 13] {
            assert!(rosters(&with_byte(&data, at, bad)).is_empty(), "count {bad}");
        }
        // A count above the records present keeps the roster; the walk just
        // stops short.
        assert_eq!(outcome(&with_byte(&data, at, 12)), Some((vec![1], false)));
    }

    // Reading records.

    #[test]
    fn the_first_slot_number_is_taken_as_written() {
        let data = party(&[Slot::Member(filled(3)), Slot::Member(filled(4))]);
        assert_eq!(outcome(&data), Some((vec![3, 4], true)));
        let wrapped = party(&[Slot::Member(Member { level: 45, ..filled(255) }), Slot::Member(filled(0))]);
        assert_eq!(outcome(&wrapped), Some((vec![255, 0], true)));
    }

    #[test]
    fn the_first_record_is_a_member_whatever_its_mask() {
        let data = party(&[Slot::Member(Member { mask: 0, ..filled(1) }), Slot::Member(filled(2))]);
        assert_eq!(outcome(&data), Some((vec![1, 2], true)));
    }

    #[test]
    fn a_member_carries_the_record_values() {
        let odd = Member { name: " m 1 ", class: 22, level: 45, gear: 820, combat_power: 41_000, ..filled(1) };
        let data = party(&[Slot::Member(odd)]);
        let r = &rosters(&data)[0];
        let (name, m) = &r.members[0];
        assert_eq!(name, " m 1 ");
        assert_eq!(&data[r.name_fields[0].clone()], b" m 1 ".as_slice());
        assert_eq!((m.slot, m.level, m.gear_score, m.combat_power), (1, 45, 820, 41_000));
        assert_eq!((m.server_id, m.dbid), (1201, (1201 << 48) | 0x0003_0001));
        assert_eq!(m.job, JobClass::job_from_party_list(22));
    }

    #[test]
    fn names_in_any_script_read() {
        for name in ["가나다", "Ωμέγα", "кот", "a"] {
            let data = party(&[Slot::Member(Member { name, ..filled(1) })]);
            assert_eq!(rosters(&data)[0].members[0].0, name);
        }
    }

    #[test]
    fn the_edges_of_each_value_range_read() {
        for member in [
            Member { gear: 0, ..filled(2) },
            Member { gear: 1_000_000, ..filled(2) },
            Member { level: 1, ..filled(2) },
            Member { level: 200, ..filled(2) },
            Member { combat_power: 100_000_000, ..filled(2) },
            Member { extra: &[0x07; 10], ..filled(2) },
        ] {
            assert_eq!(outcome(&first_then(member)), Some((vec![1, 2], true)), "{member:?}");
        }
    }

    #[test]
    fn a_world_other_than_the_accounts_sits_after_the_item_level_or_one_on() {
        let moved = |extra| {
            let data = party(&[Slot::Member(Member { extra, ..filled(1) }), Slot::Member(filled(2))]);
            patched(&data, &[0xb1, 0x04, 0x69, 0x10], &[0x9a, 0x08, 0x69, 0x10])
        };
        for extra in [&[][..], &[0x07]] {
            let r = &rosters(&moved(extra))[0];
            let first = &r.members[0].1;
            assert_eq!((r.complete, first.server_id, first.combat_power), (true, 2202, 40_001), "{extra:?}");
        }
        assert_eq!(outcome(&moved(&[0x07, 0x07])), None);
    }

    #[test]
    fn short_and_long_tails_lead_to_the_next_member() {
        let long = [0x00, 0x40, 0, 0, 0, 0, 0, 0, 0, 0x01, 0x01];
        let data = party(&[
            Slot::Member(filled(1)),
            Slot::Member(Member { tail: &long, ..filled(2) }),
            Slot::Member(filled(3)),
        ]);
        assert_eq!(outcome(&data), Some((vec![1, 2, 3], true)));
    }

    #[test]
    fn vacant_records_of_35_and_36_bytes_lead_to_the_next_record() {
        let data = party(&[Slot::Member(filled(1)), Slot::Vacant(2), Slot::Member(filled(3))]);
        let vacant = power_end(&data, 0) + tail_len();
        assert_eq!(outcome(&data), Some((vec![1, 3], true)));
        let longer = spliced(&data, vacant + VACANT_ZEROS_END, &[0x00]);
        assert_eq!(outcome(&longer), Some((vec![1, 3], true)));
        let many = party(&[Slot::Member(filled(1)), Slot::Vacant(2), Slot::Vacant(3), Slot::Vacant(4), Slot::Member(filled(5))]);
        assert_eq!(outcome(&many), Some((vec![1, 5], true)));
    }

    #[test]
    fn a_zero_mask_in_a_tail_is_not_the_next_slot() {
        let tail = [0x00, 0x05, 0, 0, 0, 0, 0, 0, 0, 0x01, 0x02];
        let data = party(&[
            Slot::Member(filled(1)),
            Slot::Member(filled(2)),
            Slot::Member(filled(3)),
            Slot::Member(Member { extra: &[0x07], tail: &tail, ..filled(4) }),
            Slot::Member(filled(5)),
        ]);
        let r = &rosters(&data)[0];
        assert!(r.complete);
        let last = &r.members[4].1;
        assert_eq!((last.slot, last.level, last.gear_score, last.combat_power), (5, 45, 805, 40_005));
    }

    #[test]
    fn the_next_record_may_start_32_bytes_on_but_not_33() {
        let tail_32 = [0u8; 32];
        let tail_33 = [0u8; 33];
        let reach = |tail: &[u8]| party(&[Slot::Member(Member { tail, ..filled(1) }), Slot::Member(filled(2))]);
        assert_eq!(outcome(&reach(&tail_32)), Some((vec![1, 2], true)));
        assert_eq!(outcome(&reach(&tail_33)), Some((vec![1], false)));
    }

    #[test]
    fn after_a_vacant_slot_the_window_ends_too() {
        let data = party(&[Slot::Member(filled(1)), Slot::Vacant(2), Slot::Member(filled(3))]);
        let vacant_end = power_end(&data, 0) + tail_len() + 35;
        assert_eq!(outcome(&spliced(&data, vacant_end, &[0; 8])), Some((vec![1, 3], true)));
        assert_eq!(outcome(&spliced(&data, vacant_end, &[0; 9])), Some((vec![1], false)));
    }

    /// Slot 1 followed by a tail that holds a slot 2 header with these name
    /// bytes (length byte first), then the real slot 2.
    fn with_false_start(name: &[u8], second: Member) -> Vec<u8> {
        let dbid = ((1201u64 << 48) | 0x0003_0002).to_le_bytes();
        let tail = [&[0x00, 0x01, 0x02, 0x1e, 0x02][..], &dbid, name].concat();
        party(&[Slot::Member(Member { tail: &tail, ..filled(1) }), Slot::Member(second)])
    }

    #[test]
    fn a_false_start_with_a_bad_name_is_passed_over() {
        for name in [&[0x00][..], &[41], &[0x02, 0xff, 0xfe]] {
            let data = with_false_start(name, filled(2));
            assert_eq!(outcome(&data), Some((vec![1, 2], true)), "{name:02x?}");
            assert_eq!(rosters(&data)[0].members[1].1.combat_power, 40_002);
        }
        // A name of 40 bytes would run past the last record and the end: it
        // starts after the tail's 3 bytes and the false header's 11.
        let last = Member { name: "b", tail: &[], ..filled(2) };
        let data = with_false_start(&[40], last);
        assert!(data.len() < power_end(&data, 0) + 3 + 11 + 40);
        assert_eq!(outcome(&data), Some((vec![1, 2], true)));
    }

    #[test]
    fn a_roster_needs_no_trailer_after_its_last_record() {
        let data = first_then(filled(2));
        assert_eq!(outcome(&data[..power_end(&data, 1)]), Some((vec![1, 2], true)));
    }

    #[test]
    fn ending_inside_a_combat_power_drops_the_roster() {
        let data = first_then(filled(2));
        let end = power_end(&data, 1);
        assert_eq!(outcome(&data[..end - 1]), None);
        // With the tag gone as well.
        assert_eq!(outcome(&data[..end - 9]), None);
    }

    #[test]
    fn a_roster_with_no_member_is_none() {
        assert_eq!(outcome(&party(&[Slot::Vacant(1)])), None);
        assert_eq!(outcome(&party(&[Slot::Member(Member { level: 0, ..filled(1) }), Slot::Member(filled(2))])), None);
        let (storage, p) = storage_with(&[]);
        p.read_party_list(&party(&[Slot::Vacant(1)]));
        assert_eq!(storage.current_dungeon_id(), 0);
        assert!(storage.roster_entries().is_empty());
    }

    // Records that stop the walk, keeping slot 1.

    #[test]
    fn too_few_bytes_at_a_record_start_stop_the_walk() {
        let data = first_then(Member { name: "b", ..filled(2) });
        let start = power_end(&data, 0) + tail_len();
        assert_eq!(outcome(&data[..start + 19]), Some((vec![1], false)));
        assert_eq!(outcome(&data[..start + 12]), Some((vec![1], false)));
    }

    #[test]
    fn a_bad_member_name_stops_the_walk() {
        let long = "p".repeat(41);
        for name in ["", long.as_str()] {
            assert_eq!(outcome(&first_then(Member { name, ..filled(2) })), Some((vec![1], false)), "{name:?}");
        }
        let bad = patched(&first_then(Member { name: "zq", ..filled(2) }), b"zq", &[0xff, 0xfe]);
        assert_eq!(outcome(&bad), Some((vec![1], false)));
        let data = first_then(Member { name: "m2m2m2", ..filled(2) });
        let name_start = rosters(&data)[0].name_fields[1].start;
        assert_eq!(outcome(&data[..name_start + 3]), Some((vec![1], false)));
    }

    #[test]
    fn fewer_than_12_bytes_after_a_name_stop_the_walk() {
        let data = first_then(filled(2));
        let name_end = rosters(&data)[0].name_fields[1].end;
        assert_eq!(outcome(&data[..name_end + 11]), Some((vec![1], false)));
    }

    #[test]
    fn values_out_of_range_stop_the_walk() {
        for member in [
            Member { level: 0, ..filled(2) },
            Member { level: 201, ..filled(2) },
            Member { gear: 1_000_001, ..filled(2) },
            Member { gear: u32::MAX, ..filled(2) },
            Member { combat_power: 100_000_001, ..filled(2) },
            Member { extra: &[0x07; 11], ..filled(2) },
        ] {
            assert_eq!(outcome(&first_then(member)), Some((vec![1], false)), "{member:?}");
        }
    }

    // Applying them.

    #[test]
    fn a_roster_that_ends_the_party_also_ends_its_dungeon() {
        let lone = party(&[Slot::Member(filled(1)), Slot::Vacant(2), Slot::Vacant(3), Slot::Vacant(4), Slot::Vacant(5)]);
        let (storage, p) = storage_with(&[1, 2, 3]);
        p.read_party_list(&lone);
        assert!(storage.roster_entries().is_empty());
        assert_eq!(storage.current_dungeon_id(), 0);

        let (storage, p) = storage_with(&[]);
        p.read_party_list(&lone);
        assert_eq!(storage.current_dungeon_id(), 600_031);
        assert_eq!(storage.roster_entries().keys().map(String::as_str).collect::<Vec<_>>(), ["m1"]);
    }

    #[test]
    fn a_roster_cut_short_adds_to_the_party() {
        let (storage, p) = storage_with(&[1, 2, 3]);
        p.read_party_list(&party(&[
            Slot::Member(filled(1)),
            Slot::Member(Member { level: 0, ..filled(2) }),
            Slot::Member(filled(3)),
        ]));
        let held = storage.roster_entries();
        assert_eq!(held.len(), 3);
        let first = &held["m1"];
        assert_eq!((first.level, first.gear_score, first.combat_power), (41, 801, 40_001));
        assert_eq!(held["m2"].gear_score, 700);
    }

    // Leaving.

    /// `1b 97 <key>`, a frame of its own.
    fn leave(key: u32) -> Vec<u8> {
        built::frame(&[&PARTY_LEAVE[..], &key.to_le_bytes()].concat())
    }

    fn feed(p: &mut PacketDecoder, bytes: &[u8]) {
        assert_eq!(p.decode_buffer(bytes), bytes.len());
    }

    fn held(storage: &DataStorage) -> Vec<String> {
        let mut names: Vec<String> = storage.roster_entries().into_keys().collect();
        names.sort();
        names
    }

    #[test]
    fn a_leave_ends_the_party_it_names_only() {
        let (storage, mut p) = processor();
        feed(&mut p, &party(&[Slot::Member(filled(1)), Slot::Member(filled(2))]));
        feed(&mut p, &leave(0x3001));
        assert_eq!(held(&storage), ["m1", "m2"], "another party's key");
        feed(&mut p, &leave(0x3000));
        assert!(held(&storage).is_empty());
        assert!(!storage.reset_was_asked());
    }

    #[test]
    fn a_leave_and_the_rosters_after_it_in_one_bundle_keep_their_order() {
        let (storage, mut p) = processor();
        let seated = party(&[Slot::Member(filled(1)), Slot::Member(filled(2))]);
        feed(&mut p, &seated);
        feed(&mut p, &built::bundle(&[leave(0x3000), seated].concat()));
        assert_eq!(held(&storage), ["m1", "m2"], "seated again after the leave");
        let other = built::roster(0x3001, "Party 8", 600_002, LEADER, &[Slot::Member(filled(3)), Slot::Member(filled(4))]);
        feed(&mut p, &built::bundle(&[leave(0x3000), other].concat()));
        feed(&mut p, &leave(0x3000));
        assert_eq!(held(&storage), ["m3", "m4"], "the second leave names a party you are out of");
    }
}
