//! Records built from the field layouts, for tests. No test holds bytes from
//! a capture: those carry character names, entity ids and other live data.
//! The layouts are the parser's own (see each reader); the ids, names,
//! positions and opaque fields here are made up. At the end, the captured
//! segment, ping replies, saved fight and scratch folder the tests share.
//! `a2t-inspect` includes this file for its tests, so it may name only `pub`
//! library items.

use std::path::{Path, PathBuf};

use crate::capture::tcp_chunk::TcpChunk;
use crate::capture::fight_tape::pack_bundle;
use crate::capture::framing::length_value;
use crate::entity::fight_record::FightRecord;

/// One field of a record, as the game writes it.
#[derive(Debug, Clone, Copy)]
pub enum F<'a> {
    /// A varint.
    V(u32),
    U8(u8),
    U16(u16),
    U32(u32),
    I64(i64),
    F32(f32),
    /// `<u8 len> <utf-8>`.
    Str(&'a str),
    Raw(&'a [u8]),
}

pub fn varint(mut v: u32) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return out;
        }
        out.push(b | 0x80);
    }
}

pub fn fields(list: &[F]) -> Vec<u8> {
    let mut out = Vec::new();
    for f in list {
        match *f {
            F::V(v) => out.extend(varint(v)),
            F::U8(v) => out.push(v),
            F::U16(v) => out.extend(v.to_le_bytes()),
            F::U32(v) => out.extend(v.to_le_bytes()),
            F::I64(v) => out.extend(v.to_le_bytes()),
            F::F32(v) => out.extend(v.to_le_bytes()),
            F::Str(s) => {
                out.push(s.len() as u8);
                out.extend(s.as_bytes());
            }
            F::Raw(b) => out.extend_from_slice(b),
        }
    }
    out
}

/// Frame bytes that start with their opcode: `<len varint> <body>`.
pub fn frame(body: &[u8]) -> Vec<u8> {
    let mut out = varint(length_value(body.len()));
    out.extend_from_slice(body);
    out
}

/// `<len> <opcode> <fields>`.
pub fn packet(opcode: [u8; 2], list: &[F]) -> Vec<u8> {
    let mut body = opcode.to_vec();
    body.extend(fields(list));
    frame(&body)
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The bytes of hex digits, either case; whitespace between them is skipped.
pub(crate) fn from_hex(digits: &str) -> Vec<u8> {
    let packed: Vec<u8> = digits.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    assert!(packed.len().is_multiple_of(2), "an odd number of hex digits");
    packed
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).expect("ascii"), 16).expect("hex digits"))
        .collect()
}

/// A bundle frame around `inner`: `FF FF <size u32> <lz4>`.
pub fn bundle(inner: &[u8]) -> Vec<u8> {
    pack_bundle(inner).expect("a bundle")
}

/// A `04 38` hit, in the order `read_hit_record` reads it.
#[derive(Debug, Clone, Default)]
pub struct Hit {
    pub target: u32,
    /// Low nibble the layout; 0x02 a plotter; 0x04 a value; 0x10/0x20 more hits.
    pub switch: u32,
    pub flag: u32,
    pub actor: u32,
    pub skill: u32,
    pub uid: u8,
    pub hit_type: u32,
    /// `<flags u8> <restored HP varint> <angle u8>`, with switch bit 0x02.
    pub plotter: Option<(u8, u32, u8)>,
    /// The skill effect (skill x 100 + n) and a count, which the parser
    /// steps over (layout 6 over 7 of their bytes: the last is read as the
    /// leading 0).
    pub effect: u32,
    pub count: u32,
    /// The varints after them: the actor's scalar and the value, then the
    /// additional hits.
    pub values: Vec<u32>,
    /// Bytes after the values (a ward's record, built with `fields`).
    pub tail: Vec<u8>,
}

impl Hit {
    /// The bytes after the opcode.
    pub fn fields(&self) -> Vec<u8> {
        let mut list = vec![
            F::V(self.target),
            F::V(self.switch),
            F::V(self.flag),
            F::V(self.actor),
            F::U32(self.skill),
            F::U8(self.uid),
            F::V(self.hit_type),
        ];
        if let Some((flags, hp, angle)) = self.plotter {
            list.extend([F::U8(flags), F::V(hp), F::U8(angle)]);
        }
        list.extend([F::U32(self.effect), F::U32(self.count)]);
        list.extend(self.values.iter().map(|&v| F::V(v)));
        let mut out = fields(&list);
        out.extend_from_slice(&self.tail);
        out
    }

    /// From the `04 38` on, without the length varint.
    pub fn record(&self) -> Vec<u8> {
        let mut body = vec![0x04, 0x38];
        body.extend(self.fields());
        body
    }

    pub fn packet(&self) -> Vec<u8> {
        frame(&self.record())
    }
}

/// A Sorcerer's Flame Arrow on 40001 from 1500: layout 6, a Power Shard hit
/// from behind, 900 damage.
pub fn shard_hit() -> Hit {
    Hit {
        target: 40_001,
        switch: 0x16,
        actor: 1_500,
        skill: 16_040_000,
        uid: 0xB1,
        hit_type: 2,
        plotter: Some((0x80, 0, 1)),
        effect: 1_604_000_017,
        count: 1,
        values: vec![10_500, 900],
        ..Hit::default()
    }
}

/// A plain layout 4 hit of `damage` by `actor` on `target`; `uid` keeps hits
/// apart.
pub fn plain_hit(actor: u32, target: u32, skill: u32, damage: u32, uid: u8) -> Hit {
    let values = vec![10_000, damage, 1, 0];
    Hit { target, switch: 0x04, actor, skill, uid, hit_type: 2, effect: skill * 100 + 11, count: 1, values, ..Hit::default() }
}

/// A `05 38` effect tick: `<target> <effect> <actor> <n> <code u32> <amount>`
/// and the fields after it.
pub fn tick(target: u32, effect: u8, actor: u32, n: u32, code: u32, amount: u32, rest: &[F]) -> Vec<u8> {
    let mut list = vec![F::V(target), F::U8(effect), F::V(actor), F::V(n), F::U32(code), F::V(amount)];
    list.extend_from_slice(rest);
    packet([0x05, 0x38], &list)
}

/// A `05 38` immune: `<target> 30 <target> <n> <code u32> <attacker> <skill u32>`.
pub fn immune(target: u32, n: u32, code: u32, attacker: u32, skill: u32) -> Vec<u8> {
    packet([0x05, 0x38], &[F::V(target), F::U8(0x30), F::V(target), F::V(n), F::U32(code), F::V(attacker), F::U32(skill)])
}

/// The `end` of an abnormal that never ends (`abnormal::NEVER`).
pub const NEVER: i64 = 4_102_412_400_000;

/// An abnormal as an add, a change or a list entry carries it.
#[derive(Debug, Clone, Copy)]
pub struct Abnormal {
    pub flags: u8,
    pub instance: u32,
    pub abnormal: u32,
    pub length: i64,
    pub end: i64,
    pub caster: u32,
    pub level: u8,
    pub skill: Option<u32>,
    /// The byte before the position (2: applied again); `None` where a list
    /// entry has none.
    pub byte: Option<u8>,
    pub position: [f32; 3],
}

impl Abnormal {
    /// From the flags on, as an add, a change and a spawn's list hold it.
    pub fn fields(&self) -> Vec<u8> {
        let mut list = vec![F::U8(self.flags), F::V(self.instance), F::U32(self.abnormal), F::I64(self.length), F::I64(self.end)];
        if self.flags & 0x01 != 0 {
            list.push(F::V(self.caster));
        }
        list.push(F::U8(self.level));
        if let Some(skill) = self.skill {
            list.push(F::U32(skill));
        }
        if let Some(b) = self.byte {
            list.push(F::U8(b));
        }
        list.extend(self.position.map(F::F32));
        fields(&list)
    }

    /// `2a 38 <entity> 01 ...`.
    pub fn added(&self, entity: u32) -> Vec<u8> {
        let mut body = vec![0x2A, 0x38];
        body.extend(varint(entity));
        body.push(0x01);
        body.extend(self.fields());
        frame(&body)
    }

    /// `2b 38 <entity> ...`.
    pub fn changed(&self, entity: u32) -> Vec<u8> {
        let mut body = vec![0x2B, 0x38];
        body.extend(varint(entity));
        body.extend(self.fields());
        frame(&body)
    }
}

/// `2c 38 <entity> <n> (<kind> <instance> <reason> [kind 7: <by> <skill u32> <effect u32>])*`.
pub fn removed(entity: u32, list: &[(u8, u32, u8, Option<(u32, u32, u32)>)]) -> Vec<u8> {
    let mut f = vec![F::V(entity), F::U8(list.len() as u8)];
    for &(kind, instance, reason, by) in list {
        f.extend([F::U8(kind), F::V(instance), F::U8(reason)]);
        if let Some((who, skill, effect)) = by {
            f.extend([F::V(who), F::U32(skill), F::U32(effect)]);
        }
    }
    packet([0x2C, 0x38], &f)
}

/// `4a 36 <entity> <n varint> (<stat u16> <value i32>)* <8 bytes>`: your own stats.
pub fn stats(entity: u32, values: &[(u16, i32)]) -> Vec<u8> {
    let mut f = vec![F::V(entity), F::V(values.len() as u32)];
    for &(stat, value) in values {
        f.extend([F::U16(stat), F::U32(value as u32)]);
    }
    f.push(F::Raw(&[0; 8]));
    packet([0x4A, 0x36], &f)
}

/// `21 36 <u32 load> <u32 map> <u32> <u32> <x y z heading f32> <u32>
/// <7 bytes> <u8> <u16>`: a map load, 49 bytes framed.
pub fn map_load(load: u32, map: u32, kind: u32, tail: u8) -> Vec<u8> {
    packet(
        [0x21, 0x36],
        &[
            F::U32(load),
            F::U32(map),
            F::U32(0x0012_3456),
            F::U32(0),
            F::F32(1000.0),
            F::F32(-2000.0),
            F::F32(100.0),
            F::F32(90.0),
            F::U32(kind),
            F::Raw(&[0; 7]),
            F::U8(tail),
            F::U16(0),
        ],
    )
}

/// `00 8D <id> <groups> [<n> (<type> <u32>)*] [<n> (<type> <u64>)*]`.
pub fn hp_record(id: u32, small: Option<&[(u8, u32)]>, large: Option<&[(u8, u64)]>) -> Vec<u8> {
    let groups = u8::from(small.is_some()) | (u8::from(large.is_some()) << 1);
    let mut f = vec![F::V(id), F::U8(groups)];
    let mut longs = Vec::new();
    if let Some(list) = small {
        f.push(F::U8(list.len() as u8));
        for &(kind, v) in list {
            f.extend([F::U8(kind), F::U32(v)]);
        }
    }
    if let Some(list) = large {
        for &(kind, v) in list {
            longs.push((kind, v.to_le_bytes()));
        }
    }
    let mut out = fields(&f);
    if large.is_some() {
        out.push(longs.len() as u8);
        for (kind, v) in &longs {
            out.push(*kind);
            out.extend_from_slice(v);
        }
    }
    let mut body = vec![0x00, 0x8D];
    body.extend(out);
    frame(&body)
}

/// A `41 36` spawn, in the order the spawn readers find its fields:
/// `<id> <kind> 00 <gate> [<len> <name>] <code u24> 00 40 02 <x y z f32>
/// <heading f32> <2 bytes> 01 <hp> <max hp> <filler> 06 <n> <abnormals> <end>`.
#[derive(Debug, Clone, Default)]
pub struct Spawn<'a> {
    pub id: u32,
    pub kind: u8,
    /// The inline name (a summon's owner, an effect's target).
    pub name: Option<&'a str>,
    pub code: u32,
    pub hp: u32,
    /// Bytes between the HP and the abnormal list.
    pub filler: usize,
    /// The entity's abnormals (`abnormal::listed`).
    pub abnormals: Vec<Abnormal>,
    /// What follows the list: `spawn_caster`, `spawn_owner` or `effect_parent`.
    pub end: Vec<u8>,
}

impl Spawn<'_> {
    /// The bytes after the opcode.
    pub fn fields(&self) -> Vec<u8> {
        let mut f = vec![F::V(self.id), F::U8(self.kind), F::U8(0)];
        match self.name {
            Some(name) => f.extend([F::U8(0x01), F::Str(name)]),
            None => f.push(F::U8(0x00)),
        }
        let code = self.code.to_le_bytes();
        f.extend([F::Raw(&code[..3]), F::Raw(&[0x00, 0x40, 0x02])]);
        f.extend([F::F32(1500.0), F::F32(-2500.0), F::F32(120.0), F::F32(90.0), F::Raw(&[0x00, 0x00])]);
        f.extend([F::U8(0x01), F::V(self.hp), F::V(self.hp)]);
        let filler = vec![0u8; self.filler];
        f.push(F::Raw(&filler));
        let mut out = fields(&f);
        out.extend([0x06, self.abnormals.len() as u8]);
        for a in &self.abnormals {
            out.extend(a.fields());
        }
        out.extend_from_slice(&self.end);
        out
    }

    pub fn packet(&self) -> Vec<u8> {
        let mut body = vec![0x41, 0x36];
        body.extend(self.fields());
        frame(&body)
    }
}

/// After a spawn's list: `07 02 <06|01> <caster u32>`, a ground spell's
/// caster (`ground_spell_caster`).
pub fn spawn_caster(kind: u8, caster: u32) -> Vec<u8> {
    fields(&[F::U8(0x07), F::U8(0x02), F::U8(kind), F::U32(caster), F::Raw(&[0x02, 0xCD, 0x00, 0x28, 0x00])])
}

/// After a summon's list: `07 02 06` and the owner block that follows its
/// `parent_key`: `<parent u32> <legion u32> <u16 0> <server u16> <len>
/// <legion name>` (`legion_block_owner`).
pub fn spawn_owner(parent: u32, legion: u32, server: u16, legion_name: &str) -> Vec<u8> {
    fields(&[
        F::Raw(&[0x07, 0x02, 0x06]),
        F::U32(parent),
        F::U32(legion),
        F::U16(0),
        F::U16(server),
        F::Str(legion_name),
        F::Raw(&[0x01, 0x00, 0x02, 0x00]),
    ])
}

/// A pet's owner block, as the spawn reader searches for it: `06 <owner u32>
/// <legion 7 u32> <pad u16> <server u16> <len> <bytes>`.
pub fn owner_block(owner: u32, pad: u16, server: u16, len: u8, bytes: &[u8]) -> Vec<u8> {
    fields(&[F::U8(0x06), F::U32(owner), F::U32(7), F::U16(pad), F::U16(server), F::U8(len), F::Raw(bytes)])
}

/// A skill effect's record ends `06 <parent u32> 00 <u8> 00 00 00 00`
/// (`note_effect_parent`).
pub fn effect_parent(parent: u32) -> Vec<u8> {
    fields(&[F::U8(0x06), F::U32(parent), F::U8(0), F::U8(0x2D), F::U32(0)])
}

/// `33 36 <id> <mask1 u32> <mask2 u8> <len> <name> <server u16> <class u32>
/// <u8> <level u32>` and four u32 more: your self record.
pub fn self_record(id: u32, name: &str, server: u16, class: u32, level: u32) -> Vec<u8> {
    packet(
        [0x33, 0x36],
        &[
            F::V(id),
            F::U32(0x0100_0000),
            F::U8(0x37),
            F::Str(name),
            F::U16(server),
            F::U32(class),
            F::U8(1),
            F::U32(level),
            F::U32(500),
            F::U32(500),
            F::U32(level),
            F::U32(0),
        ],
    )
}

/// `49 36 <2 bytes> <count varint> (<stat u16> <value i32>)* <8 bytes>`: the whole stat sheet.
pub fn stat_sheet(values: &[(u16, i32)]) -> Vec<u8> {
    let mut f = vec![F::U16(0), F::V(values.len() as u32)];
    for &(stat, value) in values {
        f.extend([F::U16(stat), F::U32(value as u32)]);
    }
    f.push(F::Raw(&[0; 8]));
    packet([0x49, 0x36], &f)
}

/// `45 36 <id> <mask1 u32> <mask2 u8> [mask2 & 1: <len> <name>]` and
/// `rest`: another player's record.
pub fn player_record(id: u32, name: Option<&str>, rest: &[u8]) -> Vec<u8> {
    let mut f = vec![F::V(id), F::U32(0)];
    match name {
        Some(name) => f.extend([F::U8(0x07), F::Str(name)]),
        None => f.push(F::U8(0x06)),
    }
    f.push(F::Raw(rest));
    packet([0x45, 0x36], &f)
}

/// One record of a party roster (`02 97`, see `read_party_list`).
#[derive(Debug, Clone, Copy)]
pub enum Slot<'a> {
    /// A vacant slot: zeros but for its number.
    Vacant(u8),
    Member(Member<'a>),
}

impl<'a> Slot<'a> {
    pub fn member(self) -> Member<'a> {
        match self {
            Slot::Member(m) => m,
            Slot::Vacant(n) => panic!("slot {n} is vacant"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Member<'a> {
    pub mask: u8,
    pub slot: u8,
    /// The account id; its top u16 is the world id.
    pub dbid: u64,
    pub name: &'a str,
    pub class: u32,
    pub level: u32,
    pub gear: u32,
    /// Bytes before the world id repeats (the stretch is not fixed).
    pub extra: &'a [u8],
    pub tag: u8,
    pub combat_power: u64,
    /// The record's tail, up to the next record.
    pub tail: &'a [u8],
}

/// `02 97 <key u32> <party name> <size u8> <dungeon u32> <2 bytes> <leader
/// u64> <3 bytes> <count varint>` and the slots.
pub fn roster(key: u32, party_name: &str, dungeon: u32, leader: u64, slots: &[Slot]) -> Vec<u8> {
    let mut f = vec![
        F::U32(key),
        F::Str(party_name),
        F::U8(5),
        F::U32(dungeon),
        F::Raw(&[0x00, 0x03]),
    ];
    let leader = leader.to_le_bytes();
    f.push(F::Raw(&leader));
    f.extend([F::Raw(&[0x01, 0x01, 0x02]), F::V(slots.len() as u32)]);
    let mut out = fields(&f);
    for slot in slots {
        match slot {
            Slot::Vacant(n) => {
                out.extend([0x00, *n]);
                out.extend([0u8; 21]);
                out.push(0x04);
                out.extend([0u8; 11]);
            }
            Slot::Member(m) => out.extend(roster_member(m, (m.dbid >> 48) as u16)),
        }
    }
    let mut body = vec![0x02, 0x97];
    body.extend(out);
    frame(&body)
}

/// One member record of the roster's layout, on `world`: the account id's
/// world, or another one for a character moved to another world.
pub fn roster_member(m: &Member, world: u16) -> Vec<u8> {
    fields(&[
        F::U8(m.mask),
        F::U8(m.slot),
        F::Raw(&m.dbid.to_le_bytes()),
        F::Str(m.name),
        F::U32(m.class),
        F::U32(m.level),
        F::U32(m.gear),
        F::Raw(m.extra),
        F::U16(world),
        F::U16(0x1069),
        F::U8(m.tag),
        F::Raw(&m.combat_power.to_le_bytes()),
        F::Raw(m.tail),
    ])
}

/// A party finder room list (`01 97`, see `capture::player_facts`) written the
/// way the game lays it out: bools share a bit byte, low bit first, and a new
/// byte goes where a bool finds the last one used up.
#[derive(Default)]
pub struct RoomWriter {
    pub bytes: Vec<u8>,
    bits_at: usize,
    left: u8,
}

impl RoomWriter {
    pub fn put(&mut self, bytes: &[u8]) {
        self.bytes.extend(bytes);
    }

    fn text(&mut self, s: &str) {
        self.bytes.push(s.len() as u8);
        self.bytes.extend(s.as_bytes());
    }

    fn bit(&mut self, v: bool) {
        if self.left == 0 {
            self.bytes.push(0);
            self.bits_at = self.bytes.len() - 1;
            self.left = 8;
        }
        self.bytes[self.bits_at] |= u8::from(v) << (8 - self.left);
        self.left -= 1;
    }

    /// A room's head up to its slot count; number 0 is the empty room the
    /// list sends when you sit in none.
    pub fn room(&mut self, number: u32, title: &str, leader: u64, join: u8, state: u8) {
        self.put(&number.to_le_bytes());
        self.text(title);
        self.put(&[if number == 0 { 0 } else { 5 }]);
        self.put(&(if number == 0 { 0u32 } else { 600_012 }).to_le_bytes());
        self.put(&[0, if number == 0 { 0 } else { 3 }]);
        self.put(&leader.to_le_bytes());
        self.bit(number != 0);
        self.put(&[join, state]);
    }

    /// A member's slot. mask: 0x02 ready, 0x04 home world, 0x08 current
    /// world, 0x10 self-revive items.
    #[allow(clippy::too_many_arguments)]
    pub fn member(&mut self, slot: u8, mask: u8, account: u64, name: &str, class: u32, il: u32, home: u16, role: u8, cp: u64) {
        self.put(&[mask, slot]);
        self.put(&account.to_le_bytes());
        self.text(name);
        for v in [class, 45, il] {
            self.put(&v.to_le_bytes());
        }
        if mask & 0x02 != 0 {
            self.bit(true);
        }
        self.bit(true);
        if mask & 0x04 != 0 {
            self.put(&home.to_le_bytes());
        }
        if mask & 0x08 != 0 {
            self.put(&home.to_le_bytes());
        }
        self.put(&[role]);
        self.put(&cp.to_le_bytes());
        self.put(&[0]);
        if mask & 0x10 != 0 {
            self.put(&3u64.to_le_bytes());
        }
        self.put(&[1, 1]);
    }

    /// An open slot asking for `role`.
    pub fn open(&mut self, slot: u8, role: u8) {
        self.put(&[0, slot]);
        self.put(&[0; 8]);
        self.text("");
        self.put(&[0; 12]);
        self.bit(false);
        self.put(&[role]);
        self.put(&[0; 8]);
        self.put(&[0, 0, 0]);
    }
}

/// An account id: a character number and the world it was made on.
pub fn account(char_id: u64, made_on: u16) -> u64 {
    char_id | u64::from(made_on) << 48
}

/// A room list, from its opcode on: a waiting room (a ready leader, a member
/// who moved from 2201 to 2202, open slots asking for a healer and anyone)
/// and a running room, whose members have no current world. 30 is a Cleric,
/// 22 an Elementalist, 8 a Gladiator.
pub fn two_rooms() -> Vec<u8> {
    let mut w = RoomWriter::default();
    w.put(&[0x01, 0x97, 0, 0]);
    w.put(&600_012u32.to_le_bytes());
    w.room(0, "", 0, 2, 0);
    w.put(&[0, 0, 2]);
    w.room(101, "75K+ 3B", account(0x3d60d, 2201), 1, 0);
    w.put(&[5]);
    w.member(1, 0x0e, account(0x3d60d, 2201), "Other", 30, 1913, 2201, 3, 81_398);
    w.member(2, 0x0c, account(0x51a33, 2201), "Moved", 30, 1500, 2202, 2, 60_123);
    w.open(3, 3);
    w.open(4, 4);
    w.open(5, 4);
    w.put(&[0]);
    w.room(102, "Leaving now.", account(0x50e09, 1201), 2, 2);
    w.put(&[5]);
    w.member(1, 0x14, account(0x50e09, 1201), "Third", 22, 861, 1201, 4, 45_961);
    w.member(2, 0x04, account(0x4b002, 1201), "Fourth", 8, 1403, 1201, 1, 67_893);
    for slot in 3..=5 {
        w.open(slot, 4);
    }
    w.put(&[0]);
    w.bytes
}

/// Grow a spawn's filler until its frame is `len` bytes long.
pub fn spawn_of_length(mut spawn: Spawn, len: usize) -> Vec<u8> {
    let base = spawn.packet().len();
    assert!(base <= len, "{base} > {len}");
    spawn.filler += len - base;
    // The length varint may have grown a byte.
    spawn.filler -= spawn.packet().len() - len;
    let out = spawn.packet();
    assert_eq!(out.len(), len);
    out
}

impl Abnormal {
    /// An abnormal on `entity` for good, cast by `caster` (a spawn's own
    /// list holds one, cast by itself).
    pub fn endless(caster: u32) -> Abnormal {
        Abnormal {
            flags: 0x11,
            instance: 1,
            abnormal: 10_000_001,
            length: -1,
            end: NEVER,
            caster,
            level: 1,
            skill: None,
            byte: Some(2),
            position: [100.0, 200.0, 300.0],
        }
    }
}

/// A monster's spawn (code 2920548) 141 bytes long framed, so its length
/// varint is `8f 01`: after a `01`, the bytes `41 36` that follow read as
/// a one-letter name "A".
pub fn spawn_8f_01(id: u32) -> Vec<u8> {
    let spawn = Spawn {
        id,
        kind: 0x1C,
        code: 2_920_548,
        hp: 100,
        abnormals: vec![Abnormal::endless(id)],
        end: effect_parent(30_600),
        ..Spawn::default()
    };
    let out = spawn_of_length(spawn, 141);
    assert_eq!(out[..4], [0x8F, 0x01, 0x41, 0x36]);
    out
}

/// A Fire Spirit (2920113) spawning for `owner`: its owner block names the
/// owner and a legion, its list holds its passive (161002304) at the owner's
/// summon level, 12.
pub fn spirit_spawn(id: u32, owner: u32) -> Vec<u8> {
    let passive = Abnormal { abnormal: 161_002_304, caster: owner, level: 12, ..Abnormal::endless(owner) };
    Spawn {
        id,
        kind: 0x5F,
        code: 2_920_113,
        hp: 10_000,
        filler: 50,
        abnormals: vec![passive],
        end: spawn_owner(owner, 100, 1201, "Legion"),
        ..Spawn::default()
    }
    .packet()
}

/// A Melee Training Scarecrow (2399011) spawning.
pub fn scarecrow_spawn(id: u32) -> Vec<u8> {
    Spawn {
        id,
        kind: 0x04,
        code: 2_399_011,
        hp: 119_700,
        filler: 50,
        abnormals: vec![Abnormal { byte: Some(0), ..Abnormal::endless(id) }],
        end: fields(&[F::U8(0x01), F::U8(0x00), F::U32(10), F::U32(0x0012_3456), F::U8(0)]),
        ..Spawn::default()
    }
    .packet()
}

/// What the capture hands over for one segment: `data` from port `from` to
/// port `to` on `device`, captured at `at`.
pub fn segment(device: Option<&str>, from: u16, to: u16, data: &[u8], at: i64) -> TcpChunk {
    TcpChunk {
        src_port: from,
        dst_port: to,
        data: data.to_vec(),
        device_name: device.map(String::from),
        captured_at_ms: at,
        src_ip: None,
        dst_ip: None,
        tcp_seq: None,
        tcp_ack: 0,
    }
}

/// The ping reply's mark, and year one in Unix ms: the .NET wall time a
/// reply echoes counts from there.
const PING_REPLY: [u8; 4] = [0x03, 0x36, 0x00, 0x00];
pub const YEAR_ONE_MS: i64 = 62_135_596_800_000;

/// A ping reply frame: `18`, the mark, the echoed time as it is on the wire,
/// then `tail`.
pub fn reply_frame(echoed: i64, tail: i64) -> Vec<u8> {
    [&[0x18][..], &PING_REPLY, &echoed.to_le_bytes(), &tail.to_le_bytes()].concat()
}

/// A server payload with one ping reply that echoes `echoed_unix_ms` as .NET
/// wall time.
pub fn reply_payload(echoed_unix_ms: i64) -> Vec<u8> {
    [&PING_REPLY[..], &(echoed_unix_ms + YEAR_ONE_MS).to_le_bytes()].concat()
}

/// A saved fight from `start` for `duration` ms with nobody in it: boss "B",
/// target 1, 1 damage.
pub fn fight(id: &str, start: i64, duration: i64) -> FightRecord {
    // Only what a saved file must hold; the rest reads as its default.
    let details = serde_json::json!({ "skills": [], "battleTime": duration, "targetId": 1, "totalTargetDamage": 1 });
    serde_json::from_value(serde_json::json!({
        "id": id, "startTimeMs": start, "durationMs": duration, "targetId": 1, "bossName": "B",
        "totalDamage": 1, "actors": [], "jobs": [], "details": details,
    }))
    .expect("a fight record")
}

/// An empty folder of one test's own, `daevalog-<tag>.<pid>` in the temp
/// folder, removed with all it holds when dropped. One a crashed run left
/// behind is cleared first.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("daevalog-{tag}.{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch folder");
        Scratch(path)
    }

    /// Writes `bytes` at `rel`, making the folders on the way. Gives the path.
    pub fn put(&self, rel: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("folders");
        std::fs::write(&path, bytes).expect("scratch file");
        path
    }

    pub fn has(&self, rel: &str) -> bool {
        self.0.join(rel).exists()
    }

    pub fn read(&self, rel: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(rel)).ok()
    }
}

impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
