//! The lists that state a player's Item Level and Combat Power: the party
//! roster and its one-member updates, the party finder and the legion list.

use super::rooms::{room_list, RoomList};
use crate::capture::decoder::{body_after, home_world, listed_server, member_at, COMBAT_POWER_TOP};
use crate::capture::names::clean_name_field;
use crate::capture::opcodes::{LEGION_MEMBERS, PARTY_FINDER, PARTY_MEMBER_0B, PARTY_MEMBER_1F, PARTY_ROSTER};
use crate::capture::varint::{u32_at, u64_at};
use crate::entity::job_class::JobClass;

/// A list that states Item Level and Combat Power.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListKind {
    /// Your party (`02 97`).
    Party,
    /// One member of your party (`0b 97`).
    PartyMember0b,
    /// One member of your party (`1f 97`).
    PartyMember1f,
    /// The party finder's room list (`01 97`).
    Finder,
    /// Your legion's members (`05 8a`).
    Legion,
}

impl ListKind {
    const OPCODES: [(Self, [u8; 2]); 5] = [
        (Self::Party, PARTY_ROSTER),
        (Self::PartyMember0b, PARTY_MEMBER_0B),
        (Self::PartyMember1f, PARTY_MEMBER_1F),
        (Self::Finder, PARTY_FINDER),
        (Self::Legion, LEGION_MEMBERS),
    ];
}

/// What a list states about one member.
#[derive(Debug, Clone, PartialEq)]
pub struct ListMember {
    pub name: String,
    pub account: u64,
    /// The world the member is on: mostly the account's, another one for a
    /// character moved to another world.
    pub world: u16,
    pub class: JobClass,
    pub level: u32,
    pub item_level: u32,
    pub combat_power: u64,
}

/// One list packet read.
#[derive(Debug, Clone, PartialEq)]
pub struct ListRead {
    pub kind: ListKind,
    pub members: Vec<ListMember>,
    /// The party finder's rooms, when the list read field by field. Its
    /// members are then every member of every room, again each time the list
    /// is sent.
    pub rooms: Option<RoomList>,
}

/// Stricter than the roster reader's limits: list records are searched for
/// at every byte, and a looser value lets noise read as a member.
const LEVEL_TOP: u32 = 99;
const ITEM_LEVEL_TOP: u32 = 10_000;

/// The list a whole packet (`<varint len> <opcode> <body>`) is, read. Every
/// record shape in the body is looked for, so a record that does not read
/// costs only itself; a party finder list is read field by field first and
/// searched only when that fails. `now_ms`, the capture time, dates the
/// legion list's last-seen times.
pub fn read_list(packet: &[u8], now_ms: i64) -> Option<ListRead> {
    let (kind, body) = ListKind::OPCODES.iter().find_map(|&(kind, op)| Some((kind, body_after(packet, op)?)))?;
    if kind == ListKind::Finder
        && let Some(rooms) = room_list(body)
    {
        let members = rooms.all().flat_map(|room| room.slots.iter()).filter_map(|s| s.member.as_ref()).filter_map(|m| {
            Some(ListMember {
                name: m.name.clone(),
                account: m.account,
                world: m.home.unwrap_or_else(|| home_world(m.account)),
                class: m.class?,
                level: m.level,
                item_level: m.item_level,
                combat_power: m.combat_power,
            })
        });
        return Some(ListRead { kind, members: members.collect(), rooms: Some(rooms) });
    }
    let mut members = Vec::new();
    let mut i = 0;
    while i < body.len() {
        let read = if kind == ListKind::Legion { legion_member(body, i, now_ms) } else { party_member(body, i).map(|(m, end)| (Some(m), end)) };
        let Some((member, end)) = read else {
            i += 1;
            continue;
        };
        i = end;
        members.extend(member);
    }
    Some(ListRead { kind, members, rooms: None })
}

/// An account id both kinds of list carry: a character number, two zero
/// bytes, then a world in the game's table.
fn plausible_account(id: u64) -> bool {
    (id >> 32) & 0xffff == 0 && listed_server(home_world(id))
}

/// A member record of the roster's layout, which `01 97`, `0b 97` and `1f 97`
/// share, as the roster reads it, held to the census's stricter checks: a
/// plausible account id, a clean name, the limits above and a class.
fn party_member(data: &[u8], at: usize) -> Option<(ListMember, usize)> {
    let (name, entry, end) = member_at(data, at)?;
    let level = u32::try_from(entry.level).ok().filter(|l| (1..=LEVEL_TOP).contains(l))?;
    let item_level = u32::try_from(entry.gear_score).ok().filter(|il| *il <= ITEM_LEVEL_TOP)?;
    if !plausible_account(entry.dbid) || clean_name_field(name.as_bytes()).is_none() {
        return None;
    }
    let member = ListMember {
        name,
        account: entry.dbid,
        world: entry.server_id,
        class: entry.job?,
        level,
        item_level,
        combat_power: entry.combat_power as u64,
    };
    Some((member, end))
}

/// A member record of the legion list (`05 8a`):
///
/// ```text
/// record id     u64
/// account id    u64       character number, 00 00, world made on
/// class         u32
/// name          str
/// level         u32
/// unnamed       u8
/// unnamed       nothing, one byte, or a text with its length then a byte
/// last seen     u64       Unix ms
/// unnamed       u32       0, or a value that looks like a map id
/// item level    u32
/// combat power  u64
/// unnamed       u8        not after the last record
/// ```
///
/// Where the u32 before the Item Level is 0, the Item Level and Combat Power
/// read far above what the same player had in the lists before and after:
/// the record is read and stepped over, with no member. The last-seen time
/// anchors the rest; it must fall in the two years before the capture.
fn legion_member(data: &[u8], at: usize, now_ms: i64) -> Option<(Option<ListMember>, usize)> {
    if u32_at(data, at + 4)? != 0 {
        return None;
    }
    let account = u64_at(data, at + 8).filter(|&id| plausible_account(id))?;
    let class = JobClass::job_from_party_list(u32_at(data, at + 16)?)?;
    let len = usize::from(*data.get(at + 20)?);
    let name = clean_name_field(data.get(at + 21..at + 21 + len)?)?;
    let after_name = at + 21 + len;
    let level = u32_at(data, after_name).filter(|l| (1..=LEVEL_TOP).contains(l))?;
    let seen = (now_ms - 2 * 365 * 86_400_000)..=(now_ms + 86_400_000);
    let stamp = (after_name + 5..after_name + 68).find(|&i| u64_at(data, i).is_some_and(|t| seen.contains(&(t as i64))))?;
    let item_level = u32_at(data, stamp + 12).filter(|il| *il <= ITEM_LEVEL_TOP)?;
    let combat_power = u64_at(data, stamp + 16).filter(|cp| *cp <= COMBAT_POWER_TOP)?;
    let member = (u32_at(data, stamp + 8)? != 0).then(|| ListMember {
        name,
        account,
        world: home_world(account),
        class,
        level,
        item_level,
        combat_power,
    });
    Some((member, stamp + 24))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::built::{account, frame, roster_member, two_rooms, Member};

    /// A roster-layout member record on `world`. `wide` adds the odd byte
    /// after the Item Level.
    #[allow(clippy::too_many_arguments)]
    fn record(slot: u8, char_id: u64, made_on: u16, world: u16, name: &str, class: u32, il: u32, wide: bool, cp: u64) -> Vec<u8> {
        let m = Member {
            mask: 0x0c,
            slot,
            dbid: account(char_id, made_on),
            name,
            class,
            level: 45,
            gear: il,
            extra: if wide { &[0x01] } else { &[] },
            tag: 0x04,
            combat_power: cp,
            tail: &[0x01, 0x01],
        };
        roster_member(&m, world)
    }

    #[allow(clippy::too_many_arguments)]
    fn legion(id: u64, char_id: u64, name: &str, class: u32, odd: &[u8], seen: u64, map: u32, il: u32, cp: u64, last: bool) -> Vec<u8> {
        let mut b = id.to_le_bytes().to_vec();
        b.extend(account(char_id, 1201).to_le_bytes());
        b.extend(class.to_le_bytes());
        b.push(name.len() as u8);
        b.extend(name.as_bytes());
        b.extend(45u32.to_le_bytes());
        b.push(0x02);
        b.extend(odd);
        b.extend(seen.to_le_bytes());
        b.extend(map.to_le_bytes());
        b.extend(il.to_le_bytes());
        b.extend(cp.to_le_bytes());
        if !last {
            b.push(0x00);
        }
        b
    }

    fn read(name: &str, char_id: u64, made_on: u16, world: u16, class: JobClass, il: u32, cp: u64) -> ListMember {
        ListMember { name: name.into(), account: account(char_id, made_on), world, class, level: 45, item_level: il, combat_power: cp }
    }

    // 2201 = `99 08`; class 8 is a Gladiator, 30 a Cleric, 22 an
    // Elementalist, 9 a Templar, 29 a Cleric, 33 a Chanter.

    #[test]
    fn a_party_member_update_gives_its_member() {
        let body = [&PARTY_MEMBER_0B[..], &record(4, 0x45817, 2201, 2201, "Tester", 8, 353, true, 21_250)].concat();
        let got = read_list(&frame(&body), 0).unwrap();
        assert_eq!((got.kind, got.rooms.is_none()), (ListKind::PartyMember0b, true));
        assert_eq!(got.members, [read("Tester", 0x45817, 2201, 2201, JobClass::Gladiator, 353, 21_250)]);
    }

    /// A party finder list that does not read field by field is searched: two
    /// members around an empty slot, and one made on 2201 who moved to 2202.
    #[test]
    fn a_finder_list_that_does_not_read_is_searched_for_members() {
        // A room's head: its dungeon (a made-up id, 600990) and its title.
        let mut body = vec![0x01, 0x97, 0x00, 0x00, 0x9e, 0x2b, 0x09, 0x00, 0x05, b'R', b'o', b'o', b'm', b'.'];
        body.extend(record(1, 0x3d60d, 2201, 2201, "Other", 30, 1913, false, 81_398));
        body.extend([0x00, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0x00]);
        body.extend(record(3, 0x50e09, 1201, 1201, "Third", 22, 861, true, 45_961));
        body.extend(record(4, 0x51a33, 2201, 2202, "Moved", 30, 1500, false, 60_123));
        let got = read_list(&frame(&body), 0).unwrap();
        assert_eq!((got.kind, got.rooms.is_none()), (ListKind::Finder, true));
        assert_eq!(
            got.members,
            [
                read("Other", 0x3d60d, 2201, 2201, JobClass::Cleric, 1913, 81_398),
                read("Third", 0x50e09, 1201, 1201, JobClass::Elementalist, 861, 45_961),
                read("Moved", 0x51a33, 2201, 2202, JobClass::Cleric, 1500, 60_123),
            ]
        );
    }

    /// A party finder list that reads field by field gives every member of
    /// every room, with the world the slot states (another one for a member
    /// who moved), and the rooms.
    #[test]
    fn a_finder_list_that_reads_gives_every_room_member() {
        let got = read_list(&frame(&two_rooms()), 0).unwrap();
        assert_eq!(got.rooms.as_ref().map(|r| r.rooms.len()), Some(2));
        assert_eq!(
            got.members,
            [
                read("Other", 0x3d60d, 2201, 2201, JobClass::Cleric, 1913, 81_398),
                read("Moved", 0x51a33, 2201, 2202, JobClass::Cleric, 1500, 60_123),
                read("Third", 0x50e09, 1201, 1201, JobClass::Elementalist, 861, 45_961),
                read("Fourth", 0x4b002, 1201, 1201, JobClass::Gladiator, 1403, 67_893),
            ]
        );
    }

    /// Legion members with a greeting, with one odd byte, with the u32 before
    /// the Item Level at 0 (no member), and last without its closing byte.
    #[test]
    fn legion_members_are_anchored_on_their_last_seen_time() {
        let now = 1_791_000_000_000i64;
        let seen = (now - 3_600_000) as u64;
        let mut body = LEGION_MEMBERS.to_vec();
        body.extend([0x00, 0x00, 0x04, 0x00]);
        let greeting: Vec<u8> = [&[0x05u8][..], b"Hello"].concat();
        body.extend(legion(0x808cf, 0x4b002, "Fourth", 9, &greeting, seen, 2040, 1403, 67_893, false));
        body.extend(legion(0x80c52, 0x4bd2e, "Fifth", 29, &[0x42], seen, 2010, 843, 37_476, false));
        body.extend(legion(0x80d00, 0x4c000, "Sixth", 13, &[], seen, 0, 2040, 67_000, false));
        body.extend(legion(0x80e00, 0x4c100, "Seventh", 33, &[], seen, 2064, 2116, 95_541, true));
        let got = read_list(&frame(&body), now).unwrap();
        assert_eq!(
            got.members,
            [
                read("Fourth", 0x4b002, 1201, 1201, JobClass::Templar, 1403, 67_893),
                read("Fifth", 0x4bd2e, 1201, 1201, JobClass::Cleric, 843, 37_476),
                read("Seventh", 0x4c100, 1201, 1201, JobClass::Chanter, 2116, 95_541),
            ]
        );
        // A last-seen time outside the two years before the capture reads nothing.
        assert!(read_list(&frame(&body), now + 3 * 365 * 86_400_000).unwrap().members.is_empty());
    }

    #[test]
    fn other_opcodes_and_broken_lengths_are_no_list() {
        assert!(read_list(&frame(&[0x03, 0x97, 1, 2, 3]), 0).is_none());
        assert!(read_list(&[0x80], 0).is_none());
    }
}
