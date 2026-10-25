//! The party finder's room list (`01 97`, the client's `DungeonRoomList_RS`),
//! read field by field.
//!
//! Ints are little-endian; a count is a varint; a text is a varint length and
//! UTF-8. Bools are packed into a shared bit byte, low bit first: when a bool
//! is needed and the byte is used up, a new byte sits right there, so it can
//! fall inside any record.
//!
//! ```text
//! list    u16 result, u32 dungeon picked, room (your own, all zero when
//!         none), count, room x count
//! room    u32 room number, text title, u8 size, u32 dungeon, u8 stage,
//!         u8 faction, u64 leader id, bit public, u8 join type, u8 state,
//!         count, slot x count, count, u32 x count (affixes)
//! slot    u8 field mask (0 = open), u8 slot number, u64 account id, text name,
//!         u32 class, u32 level, u32 Item Level, [bit ready: mask 0x02],
//!         bit, [u16 home world: 0x04], [u16 current world: 0x08], u8 role,
//!         u64 Combat Power, u8 ticket count (always 0), [u64 self-revive
//!         items: 0x10], u8 mentoring role, u8 latency
//! ```
//!
//! Members of a room already running have no current world: a fixed offset
//! from the home world then lands two bytes off, so the record search loses
//! them; read field by field, they read.

use crate::capture::names::clean_name_field;
use crate::capture::varint::next_varint;
use crate::entity::job_class::JobClass;

/// One room list as the party finder sent it.
#[derive(Debug, Clone, PartialEq)]
pub struct RoomList {
    /// The dungeon picked in the window.
    pub dungeon: u32,
    /// The room you sit in, which the list shows apart.
    pub mine: Option<Room>,
    pub rooms: Vec<Room>,
}

impl RoomList {
    /// Your own room first, then the rest.
    pub fn all(&self) -> impl Iterator<Item = &Room> {
        self.mine.iter().chain(&self.rooms)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Room {
    pub number: u32,
    pub title: String,
    pub dungeon: u32,
    pub stage: u8,
    pub faction: u8,
    pub leader: u64,
    pub public: bool,
    pub join: u8,
    pub state: u8,
    pub slots: Vec<RoomSlot>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RoomSlot {
    pub number: u8,
    /// 1 tank, 2 DPS, 3 healer, 4 any; an open slot's is the role it asks for.
    pub role: u8,
    /// `None` for an open slot.
    pub member: Option<RoomMember>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RoomMember {
    pub name: String,
    pub account: u64,
    pub class: Option<JobClass>,
    pub level: u32,
    pub item_level: u32,
    pub home: Option<u16>,
    pub current: Option<u16>,
    pub combat_power: u64,
}

/// Most rooms a list holds, and most slots a room holds, as read; more is
/// taken for a misread.
const ROOMS_MAX: usize = 200;
const SLOTS_MAX: usize = 20;

/// A reader over one packet body that keeps the shared bit byte.
struct Fields<'a> {
    data: &'a [u8],
    at: usize,
    bits: u8,
    left: u8,
}

impl Fields<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let b = self.data.get(self.at..self.at + N)?.try_into().ok()?;
        self.at += N;
        Some(b)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take::<1>()?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take()?))
    }

    fn count(&mut self) -> Option<usize> {
        usize::try_from(next_varint(self.data, &mut self.at)?).ok()
    }

    fn text(&mut self) -> Option<&[u8]> {
        let n = self.count()?;
        let b = self.data.get(self.at..self.at + n)?;
        self.at += n;
        Some(b)
    }

    fn bit(&mut self) -> Option<bool> {
        if self.left == 0 {
            self.bits = self.u8()?;
            self.left = 8;
        }
        let v = (self.bits >> (8 - self.left)) & 1;
        self.left -= 1;
        Some(v == 1)
    }
}

/// A `01 97` body (after the opcode), or `None` when any field does not fit
/// the layout or bytes are left over.
pub(super) fn room_list(body: &[u8]) -> Option<RoomList> {
    let mut f = Fields { data: body, at: 0, bits: 0, left: 0 };
    f.u16()?;
    let dungeon = f.u32()?;
    let mine = room(&mut f)?;
    let n = f.count()?;
    if n > ROOMS_MAX {
        return None;
    }
    let rooms = (0..n).map(|_| room(&mut f)).collect::<Option<Vec<_>>>()?;
    (f.at == body.len()).then(|| RoomList { dungeon, mine: (mine.number != 0).then_some(mine), rooms })
}

fn room(f: &mut Fields) -> Option<Room> {
    let number = f.u32()?;
    let title = String::from_utf8_lossy(f.text()?).into_owned();
    f.u8()?;
    let (dungeon, stage, faction, leader) = (f.u32()?, f.u8()?, f.u8()?, f.u64()?);
    let public = f.bit()?;
    let (join, state) = (f.u8()?, f.u8()?);
    let n = f.count()?;
    if n > SLOTS_MAX {
        return None;
    }
    let mut slots = Vec::with_capacity(n);
    for i in 0..n {
        let s = slot(f)?;
        if usize::from(s.number) != i + 1 {
            return None;
        }
        slots.push(s);
    }
    for _ in 0..f.count()? {
        f.u32()?;
    }
    Some(Room { number, title, dungeon, stage, faction, leader, public, join, state, slots })
}

fn slot(f: &mut Fields) -> Option<RoomSlot> {
    let mask = f.u8()?;
    if mask & !0x1e != 0 {
        return None;
    }
    let number = f.u8()?;
    let account = f.u64()?;
    let raw = f.text()?;
    let name = clean_name_field(raw).unwrap_or_else(|| String::from_utf8_lossy(raw).into_owned());
    let (class, level, item_level) = (f.u32()?, f.u32()?, f.u32()?);
    if mask & 0x02 != 0 {
        f.bit()?;
    }
    f.bit()?;
    let home = if mask & 0x04 != 0 { Some(f.u16()?) } else { None };
    let current = if mask & 0x08 != 0 { Some(f.u16()?) } else { None };
    let role = f.u8()?;
    let combat_power = f.u64()?;
    if f.u8()? != 0 {
        return None;
    }
    if mask & 0x10 != 0 {
        f.u64()?;
    }
    f.u8()?;
    f.u8()?;
    let member = (mask != 0).then(|| RoomMember {
        name,
        account,
        class: JobClass::job_from_party_list(class),
        level,
        item_level,
        home,
        current,
        combat_power,
    });
    Some(RoomSlot { number, role, member })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::built::{account, two_rooms, RoomWriter};

    #[test]
    fn a_room_list_reads_field_by_field() {
        let list = room_list(&two_rooms()[2..]).unwrap();
        assert_eq!((list.dungeon, list.mine.is_none(), list.rooms.len()), (600_012, true, 2));
        let (waiting, running) = (&list.rooms[0], &list.rooms[1]);
        assert_eq!((waiting.number, waiting.title.as_str(), waiting.join, waiting.public), (101, "75K+ 3B", 1, true));
        assert_eq!((waiting.dungeon, waiting.stage, waiting.faction, waiting.leader), (600_012, 0, 3, account(0x3d60d, 2201)));
        assert_eq!(waiting.slots[2], RoomSlot { number: 3, role: 3, member: None });
        let moved = waiting.slots[1].member.as_ref().unwrap();
        assert_eq!((moved.name.as_str(), moved.home, moved.current, moved.class), ("Moved", Some(2202), Some(2202), Some(JobClass::Cleric)));
        assert_eq!(running.state, 2);
        let third = running.slots[0].member.as_ref().unwrap();
        assert_eq!((third.home, third.current, third.combat_power), (Some(1201), None, 45_961));
        assert_eq!(running.slots[1].member.as_ref().unwrap().combat_power, 67_893);
    }

    #[test]
    fn a_list_with_bytes_left_over_or_cut_short_does_not_read() {
        let body = two_rooms();
        assert!(room_list(&[&body[2..], &[0]].concat()).is_none());
        assert!(room_list(&body[2..body.len() - 1]).is_none());
    }

    #[test]
    fn a_slot_out_of_order_or_an_unknown_mask_bit_does_not_read() {
        // One room with one member in `slot` with `mask`.
        let list = |slot: u8, mask: u8| {
            let mut w = RoomWriter::default();
            w.put(&[0, 0, 0, 0, 0, 0]);
            w.room(0, "", 0, 2, 0);
            w.put(&[0, 0, 1]);
            w.room(7, "x", 1, 1, 0);
            w.put(&[1]);
            w.member(slot, mask, 1, "Other", 30, 1, 2201, 1, 1);
            w.put(&[0]);
            w.bytes
        };
        assert!(room_list(&list(1, 0x04)).is_some());
        assert!(room_list(&list(2, 0x04)).is_none());
        assert!(room_list(&list(1, 0x24)).is_none());
    }
}
