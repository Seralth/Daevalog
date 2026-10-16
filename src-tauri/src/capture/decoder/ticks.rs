//! Damage and heals over time, the compact skill context and damage records inside other packets.

use std::fmt::Write as _;

use super::damage::{skill_code_in_range, THEOSTONE_ITEM_IDS};
use super::world::body_after;
use super::{HeldTickSkill, PacketDecoder};
use crate::capture::opcodes::{DAMAGE, DOT};
use crate::capture::varint::{index_of_bytes, le_u32_at, varint_at, next_varint};
use crate::entity::damage_packet::ParsedDamagePacket;
use crate::entity::skill_group::NPC_SKILLS;
use crate::entity::taken::{TakenHit, TakenKind};

/// `05 38` effect types read for damage taken only: an immune, and damage
/// reflected onto a player (the `04 38` record beside it has hit type 9 and
/// no skill; the tick names the reflecting skill).
const IMMUNE_EFFECT: u32 = 0x30;
const REFLECT_EFFECT: u32 = 0x4A;
/// The code of an immune record that is the game's Immune.
const IMMUNE_CODE: u32 = 2001;

/// The biggest tick amount believed; a larger one is dropped.
const MAX_TICK_AMOUNT: i32 = 99_999_999;
/// How many bytes of a found `04 38` record, from the opcode on, tell it
/// apart from the ones already counted.
const HIT_KEY_LEN: usize = 64;
/// Put in front of a found record so the hit reader can take it as a frame.
/// A two-byte length varint; the reader never checks its value.
const STAND_IN_LENGTH: [u8; 2] = [0xFF, 0x01];
/// The frame before a compact record: length 8, opcode `3B 38` or `3D 38`,
/// body `00 00`.
const COMPACT_MARKERS: [[u8; 5]; 2] = [[0x08, 0x3B, 0x38, 0x00, 0x00], [0x08, 0x3D, 0x38, 0x00, 0x00]];
/// The byte the compact record's actor follows.
const COMPACT_TAG: u8 = 0x38;
/// A compact record naming a smaller actor id is a misread.
const MIN_COMPACT_ACTOR: i32 = 100;

/// What a `05 38` effect byte other than an immune's makes of the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TickKind {
    Damage,
    Heal { over_time: bool },
    Reflect,
}

impl TickKind {
    /// Exact byte values: 0x0B shares a bit with 0x02 and is a heal.
    fn of(effect: u8) -> Option<Self> {
        match u32::from(effect) {
            0x02 | 0x0A => Some(TickKind::Damage),
            0x01 | 0x09 => Some(TickKind::Heal { over_time: false }),
            0x0B => Some(TickKind::Heal { over_time: true }),
            REFLECT_EFFECT => Some(TickKind::Reflect),
            _ => None,
        }
    }
}

/// A `05 38` damage, heal or reflect record, read up to its amount.
struct Tick {
    kind: TickKind,
    target: i32,
    actor: i32,
    skill: i32,
    amount: i32,
}

impl Tick {
    /// The fields after the effect byte: `<actor> <n> <code u32> <amount>`,
    /// the code being the skill times 100 plus an effect number.
    fn read(cursor: &mut Cursor, kind: TickKind, target: i32) -> Option<Self> {
        let actor = cursor.varint()?;
        // A damage tick on its own source is no damage: the game leaves a
        // player's Restore HP tick on themselves out of its record.
        if kind == TickKind::Damage && actor == target {
            return None;
        }
        cursor.varint()?;
        let skill = cursor.u32()? as i32 / 100;
        if !skill_code_in_range(skill) {
            return None;
        }
        let amount = cursor.varint().filter(|n| (1..=MAX_TICK_AMOUNT).contains(n))?;
        Some(Tick { kind, target, actor, skill, amount })
    }

    /// How the target took it, when a monster's own skill dealt it. A
    /// player's skill ticking on a player is never damage taken: the game's
    /// records count none of those.
    fn taken_as(&self) -> Option<TakenKind> {
        if !NPC_SKILLS.contains(&self.skill) || THEOSTONE_ITEM_IDS.contains(&i64::from(self.skill)) {
            return None;
        }
        match self.kind {
            TickKind::Damage => Some(TakenKind::Tick),
            TickKind::Reflect => Some(TakenKind::Reflect),
            TickKind::Heal { .. } => None,
        }
    }
}

/// Fields read one after another; a read past the end gives `None`.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    /// The fields after a frame's opcode, when it is `opcode`.
    fn after(packet: &'a [u8], opcode: [u8; 2]) -> Option<Self> {
        body_after(packet, opcode).map(|bytes| Cursor { bytes, pos: 0 })
    }

    /// Any value it reads, a negative one from five bytes too.
    fn varint(&mut self) -> Option<i32> {
        let got = varint_at(self.bytes, self.pos);
        if got.length < 1 {
            return None;
        }
        self.pos += got.length as usize;
        Some(got.value)
    }

    /// Only a value that is not negative.
    fn unsigned(&mut self) -> Option<i32> {
        next_varint(self.bytes, &mut self.pos)
    }

    fn byte(&mut self) -> Option<u8> {
        let b = *self.bytes.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    fn u32(&mut self) -> Option<u32> {
        self.bytes.get(self.pos..self.pos + 4)?;
        let word = le_u32_at(self.bytes, self.pos);
        self.pos += 4;
        Some(word)
    }
}

/// The attacker and skill of an immune record, read after its effect byte:
/// `<target again> <n> <code u32> <attacker> <skill u32>`. Its skill is a
/// plain skill code, not times 100.
fn immune_source(cursor: &mut Cursor, target: i32) -> Option<(i32, u32)> {
    cursor.unsigned().filter(|&again| again == target)?;
    cursor.unsigned()?;
    let code = cursor.u32()?;
    let attacker = cursor.unsigned()?;
    if code != IMMUNE_CODE {
        return None;
    }
    Some((attacker, cursor.u32()?))
}

/// A found record's leading bytes as text, one to one, for the seen set.
fn hit_key(bytes: &[u8]) -> String {
    let mut key = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(key, "{b:02x}");
    }
    key
}

impl PacketDecoder {
    /// A `05 38` effect record: a damage or heal tick, a reflect or an
    /// immune. Every record counts as it comes; nothing is skipped as seen.
    pub(super) fn read_tick_record(&mut self, packet: &[u8]) {
        let Some(mut cursor) = Cursor::after(packet, DOT) else { return };
        let Some(target) = cursor.varint() else { return };
        let Some(effect) = cursor.byte() else { return };
        let at = self.record_time();
        if u32::from(effect) == IMMUNE_EFFECT {
            self.note_immune(&mut cursor, target, at);
            return;
        }
        let Some(tick) = TickKind::of(effect).and_then(|kind| Tick::read(&mut cursor, kind, target)) else {
            return;
        };
        if let Some(kind) = tick.taken_as() {
            let hit = TakenHit::plain(at, tick.target, tick.actor, tick.skill, tick.amount.into(), kind);
            self.data_storage.append_taken(hit);
        }
        match tick.kind {
            // The skill is the monster's that reflected it: nobody dealt damage.
            TickKind::Reflect => {}
            // The whole heal, overheal too, under the skill as the record names it.
            TickKind::Heal { over_time } => {
                self.data_storage.add_heal_event(tick.actor, tick.target, tick.skill, tick.amount.into(), over_time, at);
            }
            TickKind::Damage => self.count_damage_tick(&tick),
        }
    }

    /// Only the skills on the DoT list deal damage by ticks. The ticks it
    /// keeps out are heals and monsters' hits on players.
    fn count_damage_tick(&mut self, tick: &Tick) {
        if !self.dot_damage_skill_ids.contains(&tick.skill) {
            if self.unknown_dot_skills.insert(tick.skill) {
                tracing::debug!(
                    "Damage tick of skill {} is not on the DoT list (actor {}, target {})",
                    tick.skill,
                    tick.actor,
                    tick.target
                );
            }
            return;
        }
        let mut row = ParsedDamagePacket::new();
        if let Some(ts) = self.override_timestamp {
            row.set_timestamp(ts);
        }
        row.set_dot(true);
        row.set_target_id(tick.target);
        row.set_actor_id(tick.actor);
        row.set_skill_code(tick.skill);
        row.set_damage(tick.amount);
        self.data_storage.append_damage(row);
    }

    /// Only code 2001, the game's Immune, against a monster's skill is kept:
    /// the other codes came on summons, or on players where the game's
    /// records counted no immune.
    fn note_immune(&mut self, cursor: &mut Cursor, target: i32, at: i64) {
        let Some((attacker, skill)) = immune_source(cursor, target) else { return };
        if !i32::try_from(skill).is_ok_and(|code| NPC_SKILLS.contains(&code)) || !self.data_storage.looks_like_entity(attacker) {
            return;
        }
        let hit = TakenHit::plain(at, target, attacker, skill as i32, 0, TakenKind::Immune);
        self.data_storage.append_taken(hit);
    }

    /// The `04 38` hit records anywhere inside `packet`, a frame with another
    /// opcode. A record whose first `HIT_KEY_LEN` bytes were read before is
    /// passed over. True when any record was read.
    pub(super) fn read_inner_hit(&mut self, packet: &[u8]) -> bool {
        if packet.len() < 6 {
            return false;
        }
        let mut at = 0;
        let mut read_any = false;
        while let Some(found) = index_of_bytes(packet, at, &DAMAGE) {
            let counted = self.count_found_hit(packet, found);
            read_any |= counted;
            // A counted record's opcode is not looked at again.
            at = found + if counted { 2 } else { 1 };
        }
        read_any
    }

    /// The record at `packet[at..]`, and all after it, read as a frame of its
    /// own: no scan inside it, and only the hit reader's trusted shape. Its
    /// key goes in the seen set once it is read.
    fn count_found_hit(&mut self, packet: &[u8], at: usize) -> bool {
        let key = hit_key(&packet[at..packet.len().min(at + HIT_KEY_LEN)]);
        if self.seen_embedded_hexes.contains(&key) {
            return false;
        }
        let mut framed = STAND_IN_LENGTH.to_vec();
        framed.extend_from_slice(&packet[at..]);
        let read = self.read_hit_record(&framed, false, true);
        if read {
            self.seen_embedded_hexes.insert(key);
        }
        read
    }

    /// The actor and skill a bundle packet's compact record names.
    pub(super) fn read_held_tick_skill(&self, packet: &[u8]) -> Option<HeldTickSkill> {
        let len = varint_at(packet, 0);
        if len.length < 1 || len.length as usize >= packet.len() {
            return None;
        }
        let body = &packet[len.length as usize..];
        let record = compact_record_in(body)?;
        let skill = &body[record.skill_at..];
        let three = i32::from(skill[0]) | i32::from(skill[1]) << 8 | i32::from(skill[2]) << 16;
        let four = (skill.len() >= 4).then(|| le_u32_at(skill, 0) as i32);
        let code = four.into_iter().chain([three]).find(|&c| self.skill_in_table(c))?;
        Some(HeldTickSkill { actor_id: record.actor, skill_raw: self.fold_skill_code(code) })
    }
}

/// Where a compact record sits in a frame's body.
pub(crate) struct CompactRecord {
    /// The marker's first byte.
    pub marker: usize,
    pub actor: i32,
    /// The skill's first byte; three at least follow from it.
    pub skill_at: usize,
}

/// The compact record in `body`, a frame's bytes after its length varint:
/// after the marker, the first `38` byte, then `<actor varint> <1 byte>
/// <skill, 4 or 3 bytes LE>`. The parser reads its context and the Evidence
/// Slice lifts its bytes (`cast_context`) by this one walk.
pub(crate) fn compact_record_in(body: &[u8]) -> Option<CompactRecord> {
    let marker = body.windows(5).position(|w| COMPACT_MARKERS.iter().any(|m| w == m))?;
    let past_marker = marker + 5;
    let tag = past_marker + body[past_marker..].iter().position(|&b| b == COMPACT_TAG)?;
    // Two bytes at least after the `38`.
    if body.len() < tag + 3 {
        return None;
    }
    let actor = varint_at(body, tag + 1);
    if actor.length < 1 || actor.value < MIN_COMPACT_ACTOR {
        return None;
    }
    // One byte sits between the actor and the skill, unread.
    let skill_at = tag + 1 + actor.length as usize + 1;
    (body.len() >= skill_at + 3).then_some(CompactRecord { marker, actor: actor.value, skill_at })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Arc;

    use crate::built::{self, shard_hit, Hit, F};
    use crate::checks::replay_timeline::HitTap;
    use crate::combat::data_storage::DataStorage;
    use crate::i18n::lookup::{NpcLookup, SkillLookup};

    /// A processor over a fresh storage and `skills`, its clock pinned at `at`.
    fn rig(skills: SkillLookup, at: i64) -> (Arc<DataStorage>, PacketDecoder) {
        let storage: Arc<DataStorage> = DataStorage::new().into();
        let mut p = PacketDecoder::new(Arc::clone(&storage), skills.into(), NpcLookup::new().into());
        p.pin_clock(Some(at));
        (storage, p)
    }

    fn names(json: &str) -> SkillLookup {
        let skills = SkillLookup::new();
        skills.load_from_json(json);
        skills
    }

    /// The shipped `en` skill table.
    fn english() -> SkillLookup {
        let data = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../src/data"));
        let skills = SkillLookup::new();
        crate::i18n::lookup::load_language(&skills, &NpcLookup::new(), data, "en");
        skills
    }

    fn taken(storage: &DataStorage) -> Vec<TakenHit> {
        storage.taken_since(0).into_iter().map(|t| t.hit).collect()
    }

    /// No damage or heal reached the storage.
    fn assert_no_damage_or_heal(storage: &DataStorage, case: &str) {
        assert!(storage.copy_targets().is_empty(), "{case}: damage");
        assert!(storage.heals_between(0, 1_000_000_000).is_empty(), "{case}: heals");
    }

    /// No damage, heal or damage taken reached the storage.
    fn assert_untouched(storage: &DataStorage, case: &str) {
        assert_no_damage_or_heal(storage, case);
        assert!(taken(storage).is_empty(), "{case}: taken");
    }

    /// (hits, damage) `actor` dealt `target` under (skill, is a tick).
    fn dealt(storage: &DataStorage, target: i32, actor: i32, key: (i32, bool)) -> Option<(i32, i64)> {
        let snapshot = storage.copy_targets();
        let row = snapshot.get(&target)?.actors.get(&actor)?.skills.get(&key)?;
        Some((row.hit_count, row.total_damage))
    }

    /// (heal, ticks) by `healer` under (skill, over time), up to 2,000 ms.
    fn healed(storage: &DataStorage, healer: i32, key: (i32, bool)) -> Option<(i64, i32)> {
        let heals = storage.heals_between(0, 2_000);
        let row = heals.get(&healer)?.get(&key)?;
        Some((row.total_heal, row.tick_count))
    }

    /// The lines `tap` collects while `run` runs.
    fn logged(tap: &HitTap, targets: &'static [&'static str], run: impl FnOnce()) -> Vec<(tracing::Level, String)> {
        let guard = tap.install_for(targets);
        run();
        drop(guard);
        tap.take().into_iter().map(|(_, level, line)| (level, line)).collect()
    }

    /// A player's Corrode tick on 40001 from 1500, 150 damage.
    fn corrode() -> Vec<u8> {
        built::tick(40_001, 0x0A, 1_500, 1, 1_615_000_021, 150, &[F::U32(16_150_010)])
    }

    // ----- `05 38` ticks -----

    #[test]
    fn a_tick_needs_its_length_and_the_05_38_opcode() {
        let other_opcode = built::packet(
            [0x04, 0x38],
            &[F::V(5_100), F::U8(0x0A), F::V(30_001), F::V(200), F::U32(120_001_011), F::V(300)],
        );
        for (case, packet) in [("bad length", vec![0x80]), ("one byte after it", vec![0x03, 0x05]), ("04 38", other_opcode)] {
            let (storage, mut p) = rig(SkillLookup::new(), 1_000);
            p.use_tick_skills(HashSet::from([1_200_010]));
            p.read_tick_record(&packet);
            assert_untouched(&storage, case);
        }
    }

    #[test]
    fn a_tick_without_a_target_or_an_effect_is_dropped() {
        for body in [&[0x05, 0x38, 0x80][..], &[0x05, 0x38, 0x88, 0x27]] {
            let (storage, mut p) = rig(SkillLookup::new(), 1_000);
            p.read_tick_record(&built::frame(body));
            assert_untouched(&storage, &format!("{body:02x?}"));
        }
    }

    #[test]
    fn effect_bytes_outside_the_known_ones_are_dropped() {
        for effect in [0x00, 0x03, 0x08, 0x0C, 0x4B] {
            let (storage, mut p) = rig(SkillLookup::new(), 1_000);
            p.use_tick_skills(HashSet::from([1_200_010]));
            p.read_tick_record(&built::tick(5_100, effect, 30_001, 200, 120_001_011, 300, &[F::U32(1_218_705)]));
            assert_untouched(&storage, &format!("effect {effect:#04x}"));
        }
    }

    #[test]
    fn damage_ticks_on_their_own_source_count_nowhere() {
        let (storage, mut p) = rig(SkillLookup::new(), 1_000);
        p.use_tick_skills(HashSet::from([1_900_001, 1_200_010]));
        p.read_tick_record(&built::tick(5_100, 0x02, 5_100, 1_200, 190_000_151, 2_600, &[]));
        // A monster's skill on itself is not taken either.
        p.read_tick_record(&built::tick(30_001, 0x0A, 30_001, 200, 120_001_011, 300, &[]));
        assert_untouched(&storage, "self damage");
    }

    #[test]
    fn a_tick_cut_before_its_amount_is_dropped() {
        let cuts: [&[u8]; 3] = [
            &[0x05, 0x38, 0x88, 0x27, 0x0A, 0x90, 0x4E, 0x80],
            &[0x05, 0x38, 0x88, 0x27, 0x0A, 0x90, 0x4E, 0x01, 0x2B, 0x4B],
            &[0x05, 0x38, 0x88, 0x27, 0x01, 0x90, 0x4E, 0x01, 0x03, 0x2C, 0x53, 0x0B, 0x80],
        ];
        for body in cuts {
            let (storage, mut p) = rig(SkillLookup::new(), 1_000);
            p.use_tick_skills(HashSet::from([1_900_001]));
            p.read_tick_record(&built::frame(body));
            assert_untouched(&storage, &format!("{body:02x?}"));
        }
    }

    #[test]
    fn the_skill_is_the_code_over_a_hundred() {
        for (code, skill) in [(99, None), (100, Some(1)), (2_147_483_647, Some(21_474_836)), (2_147_483_648, None)] {
            let (storage, mut p) = rig(SkillLookup::new(), 1_000);
            p.read_tick_record(&built::tick(5_000, 0x01, 1_500, 1, code, 420, &[]));
            match skill {
                Some(skill) => assert_eq!(healed(&storage, 1_500, (skill, false)), Some((420, 1)), "code {code}"),
                None => assert_untouched(&storage, &format!("code {code}")),
            }
        }
    }

    #[test]
    fn tick_amounts_run_from_one_to_99_999_999() {
        for (amount, read) in [(0, false), (1, true), (99_999_999, true), (100_000_000, false), (u32::MAX, false)] {
            let (storage, mut p) = rig(SkillLookup::new(), 1_000);
            p.read_tick_record(&built::tick(5_000, 0x01, 1_500, 1, 190_000_131, amount, &[]));
            let got = healed(&storage, 1_500, (1_900_001, false));
            assert_eq!(got, read.then_some((i64::from(amount), 1)), "amount {amount}");
        }
    }

    #[test]
    fn a_monster_s_tick_or_reflect_is_damage_taken() {
        let (storage, mut p) = rig(SkillLookup::new(), 25_000);
        p.read_tick_record(&built::tick(5_100, 0x0A, 30_001, 200, 120_001_011, 300, &[F::U32(1_218_705)]));
        assert_eq!(taken(&storage), vec![TakenHit::plain(25_000, 5_100, 30_001, 1_200_010, 300, TakenKind::Tick)]);
        assert!(storage.copy_targets().is_empty());

        let (storage, mut p) = rig(SkillLookup::new(), 25_000);
        let rest = [F::U32(16_010_030), F::U32(1_605_910)];
        p.read_tick_record(&built::tick(5_100, 0x4A, 30_101, 30, 160_591_011, 500, &rest));
        assert_eq!(taken(&storage), vec![TakenHit::plain(25_000, 5_100, 30_101, 1_605_910, 500, TakenKind::Reflect)]);
    }

    /// A Chanter's Protection Circle ticking as damage while it heals is not
    /// damage taken: an 8-digit skill. Nor is a Theostone's.
    #[test]
    fn a_player_s_or_a_theostone_s_tick_is_not_damage_taken() {
        let (storage, mut p) = rig(SkillLookup::new(), 25_000);
        p.read_tick_record(&built::tick(5_100, 0x0A, 30_001, 180, 1_873_000_211, 350, &[F::U32(18_730_002)]));
        assert_untouched(&storage, "Protection Circle");

        let (storage, mut p) = rig(SkillLookup::new(), 25_000);
        p.read_tick_record(&built::tick(5_100, 0x0A, 30_001, 180, 300_100_801, 350, &[]));
        assert!(taken(&storage).is_empty());
    }

    #[test]
    fn a_reflect_of_a_player_skill_counts_nowhere() {
        let (storage, mut p) = rig(SkillLookup::new(), 25_000);
        p.use_tick_skills(HashSet::from([16_040_240]));
        p.read_tick_record(&built::tick(5_100, 0x4A, 30_101, 30, 1_604_024_017, 500, &[]));
        assert_untouched(&storage, "reflect");
    }

    #[test]
    fn heals_keep_the_skill_as_the_record_names_it() {
        let (storage, mut p) = rig(english(), 1_000);
        p.read_tick_record(&built::tick(2_300, 0x01, 2_300, 180, 190_000_131, 7_000, &[]));
        assert_eq!(healed(&storage, 2_300, (1_900_001, false)), Some((7_000, 1)));
        p.read_tick_record(&built::tick(5_000, 0x0B, 1_500, 1, 1_701_000_011, 420, &[]));
        assert_eq!(healed(&storage, 1_500, (17_010_000, true)), Some((420, 1)));
        assert!(taken(&storage).is_empty());

        let (storage, mut p) = rig(english(), 1_000);
        p.read_tick_record(&built::tick(2_300, 0x09, 2_300, 180, 190_000_131, 7_000, &[]));
        assert_eq!(healed(&storage, 2_300, (1_900_001, false)), Some((7_000, 1)));
    }

    #[test]
    fn a_listed_monster_tick_is_damage_and_damage_taken() {
        let (storage, mut p) = rig(english(), 1_000);
        p.use_tick_skills(HashSet::from([1_200_012]));
        p.read_tick_record(&built::tick(2_400, 0x0A, 60_001, 270, 120_001_211, 50, &[F::U32(1_217_330)]));
        assert_eq!(dealt(&storage, 2_400, 60_001, (1_200_012, true)), Some((1, 50)));
        assert_eq!(taken(&storage), vec![TakenHit::plain(1_000, 2_400, 60_001, 1_200_012, 50, TakenKind::Tick)]);
    }

    #[test]
    fn a_listed_player_tick_is_damage_with_a_trace_line() {
        let (storage, mut p) = rig(SkillLookup::new(), 1_000);
        p.use_tick_skills(HashSet::from([16_150_000]));
        let tap = HitTap::default();
        let lines = logged(&tap, &["dot_ticks"], || p.read_tick_record(&corrode()));
        assert_eq!(dealt(&storage, 40_001, 1_500, (16_150_000, true)), Some((1, 150)));
        let traced: Vec<&str> = lines.iter().map(|l| l.1.as_str()).collect();
        assert_eq!(traced, ["1000 actor=1500 target=40001 skill=16150000 damage=150"]);
        assert!(taken(&storage).is_empty());
    }

    #[test]
    fn an_unlisted_damage_tick_is_noted_once() {
        const TICKS: &str = concat!(env!("CARGO_CRATE_NAME"), "::capture::decoder::ticks");
        let (storage, mut p) = rig(SkillLookup::new(), 1_000);
        let tap = HitTap::default();
        let first = logged(&tap, &[TICKS], || p.read_tick_record(&corrode()));
        assert_eq!(first.len(), 1);
        let (level, line) = &first[0];
        assert_eq!(*level, tracing::Level::DEBUG);
        assert!(["16150000", "1500", "40001"].iter().all(|n| line.contains(n)), "{line}");
        assert!(logged(&tap, &[TICKS], || p.read_tick_record(&corrode())).is_empty());
        assert_eq!(p.unknown_dot_skills, HashSet::from([16_150_000]));
        assert_untouched(&storage, "unlisted");
    }

    #[test]
    fn the_same_tick_twice_counts_twice() {
        let (storage, mut p) = rig(SkillLookup::new(), 1_000);
        p.use_tick_skills(HashSet::from([16_150_000]));
        p.read_tick_record(&corrode());
        p.read_tick_record(&corrode());
        assert_eq!(dealt(&storage, 40_001, 1_500, (16_150_000, true)), Some((2, 300)));
    }

    // ----- Immunes -----

    fn immune_taken(packet: &[u8], low_ids: &[i32]) -> Vec<TakenHit> {
        let (storage, mut p) = rig(SkillLookup::new(), 76_500);
        for &id in low_ids {
            storage.remember_small_id(id);
        }
        p.read_tick_record(packet);
        assert_no_damage_or_heal(&storage, "immune");
        taken(&storage)
    }

    #[test]
    fn an_immune_to_a_monster_skill_is_damage_taken_of_nothing() {
        let entry = |attacker, skill| vec![TakenHit::plain(76_500, 5_100, attacker, skill, 0, TakenKind::Immune)];
        assert_eq!(immune_taken(&built::immune(5_100, 1_210, 2001, 30_203, 1_800_559), &[]), entry(30_203, 1_800_559));
        // Theostone codes are monster skills here.
        assert_eq!(immune_taken(&built::immune(5_100, 1_210, 2001, 30_203, 3_000_500), &[]), entry(30_203, 3_000_500));
        assert_eq!(immune_taken(&built::immune(5_100, 1_210, 2001, 50, 1_800_559), &[50]), entry(50, 1_800_559));
    }

    #[test]
    fn other_immune_records_are_dropped() {
        let second_target = built::packet(
            [0x05, 0x38],
            &[F::V(5_100), F::U8(0x30), F::V(5_101), F::V(1_210), F::U32(2001), F::V(30_203), F::U32(1_800_559)],
        );
        let cut = |fields: &[F]| {
            let mut list = vec![F::V(5_100), F::U8(0x30), F::V(5_100), F::V(1_210), F::U32(2001)];
            list.extend_from_slice(fields);
            built::packet([0x05, 0x38], &list)
        };
        let cases = [
            ("code 100000183", built::immune(5_100, 1_210, 100_000_183, 30_203, 1_800_559)),
            ("second target", second_target),
            ("player skill", built::immune(5_100, 1_210, 2001, 30_203, 16_010_000)),
            ("attacker 50", built::immune(5_100, 1_210, 2001, 50, 1_800_559)),
            ("negative attacker", built::immune(5_100, 1_210, 2001, u32::MAX, 1_800_559)),
            ("cut after the code", cut(&[])),
            ("cut after the attacker", cut(&[F::V(30_203), F::U8(0x2F), F::U8(0x79), F::U8(0x1B)])),
        ];
        for (case, packet) in cases {
            assert!(immune_taken(&packet, &[]).is_empty(), "{case}");
        }
    }

    // ----- Hit records inside other packets -----

    const FLAME_ARROW: &str = r#"{"16040000":"Flame Arrow"}"#;

    /// An `E5 A2` frame holding `parts` one after another.
    fn host(parts: &[&[u8]]) -> Vec<u8> {
        let mut body = vec![0xE5, 0xA2];
        for part in parts {
            body.extend_from_slice(part);
        }
        built::frame(&body)
    }

    #[test]
    fn a_hit_inside_another_packet_counts_once() {
        let (storage, mut p) = rig(names(FLAME_ARROW), 1_000);
        let packet = host(&[&shard_hit().record()]);
        assert!(p.read_inner_hit(&packet));
        assert_eq!(dealt(&storage, 40_001, 1_500, (16_040_000, false)), Some((1, 900)));
        assert!(!p.read_inner_hit(&packet));
        assert_eq!(dealt(&storage, 40_001, 1_500, (16_040_000, false)), Some((1, 900)));
    }

    #[test]
    fn an_unread_hit_is_not_marked_seen() {
        let skills = Arc::new(SkillLookup::new());
        let storage: Arc<DataStorage> = DataStorage::new().into();
        let mut p = PacketDecoder::new(Arc::clone(&storage), Arc::clone(&skills), NpcLookup::new().into());
        p.pin_clock(Some(1_000));
        let packet = host(&[&shard_hit().record()]);
        assert!(!p.read_inner_hit(&packet));
        assert_untouched(&storage, "no skill name");
        skills.load_from_json(FLAME_ARROW);
        assert!(p.read_inner_hit(&packet));
        assert_eq!(dealt(&storage, 40_001, 1_500, (16_040_000, false)), Some((1, 900)));
    }

    #[test]
    fn found_hits_are_told_apart_by_their_first_64_bytes() {
        let rec = shard_hit().record();
        assert!(rec.len() < HIT_KEY_LEN);
        let (storage, mut p) = rig(names(FLAME_ARROW), 1_000);
        assert!(p.read_inner_hit(&host(&[&rec, &[0x01]])));
        assert!(p.read_inner_hit(&host(&[&rec, &[0x02]])));
        assert_eq!(dealt(&storage, 40_001, 1_500, (16_040_000, false)), Some((2, 1_800)));

        let (storage, mut p) = rig(names(FLAME_ARROW), 1_000);
        let zeros = [0u8; 64];
        assert!(p.read_inner_hit(&host(&[&rec, &zeros])));
        assert!(!p.read_inner_hit(&host(&[&rec, &zeros, &[0x01]])));
        assert_eq!(dealt(&storage, 40_001, 1_500, (16_040_000, false)), Some((1, 900)));
    }

    #[test]
    fn two_hits_in_one_packet_both_count() {
        let (storage, mut p) = rig(names(FLAME_ARROW), 1_000);
        let other = Hit { target: 40_002, ..shard_hit() };
        assert!(p.read_inner_hit(&host(&[&shard_hit().record(), &other.record()])));
        assert_eq!(dealt(&storage, 40_001, 1_500, (16_040_000, false)), Some((1, 900)));
        assert_eq!(dealt(&storage, 40_002, 1_500, (16_040_000, false)), Some((1, 900)));
    }

    #[test]
    fn a_found_hit_needs_the_trusted_shape() {
        let monster = Hit { skill: 1_200_010, effect: 120_001_011, ..shard_hit() };
        let on_itself = Hit { target: 1_500, ..shard_hit() };
        for (case, hit) in [("monster skill", monster), ("actor on itself", on_itself)] {
            let (storage, mut p) = rig(names(r#"{"16040000":"Combustion","1200010":"Poison"}"#), 1_000);
            assert!(!p.read_inner_hit(&host(&[&hit.record()])), "{case}");
            assert_untouched(&storage, case);
        }
    }

    #[test]
    fn a_packet_under_six_bytes_is_not_scanned() {
        let (storage, mut p) = rig(names(FLAME_ARROW), 1_000);
        assert!(!p.read_inner_hit(&built::frame(&[0xE5])));
        assert!(!p.read_inner_hit(&[0x09, 0xE5, 0x04, 0x38, 0x00]));
        assert_untouched(&storage, "short");
    }

    // ----- Compact skill context -----

    const TABLE: &str = r#"{"12340000":"S","12340011":"S"}"#;

    /// The context `body`, framed, names under the skill table `table`.
    fn context(table: &str, body: &[u8]) -> Option<(i32, i32)> {
        let (_, p) = rig(names(table), 1_000);
        p.read_held_tick_skill(&built::frame(body)).map(|c| (c.actor_id, c.skill_raw))
    }

    /// `E1 E2`, then `marker`, `01 38`, actor 10,000, one byte and `skill`.
    fn compact(marker: [u8; 5], skill: &[u8]) -> Vec<u8> {
        let mut body = vec![0xE1, 0xE2];
        body.extend(marker);
        body.extend([0x01, 0x38, 0x90, 0x4E, 0x07]);
        body.extend_from_slice(skill);
        body
    }

    const MARKER_3B: [u8; 5] = [0x08, 0x3B, 0x38, 0x00, 0x00];
    const MARKER_3D: [u8; 5] = [0x08, 0x3D, 0x38, 0x00, 0x00];
    const SKILL: [u8; 4] = [0x2B, 0x4B, 0xBC, 0x00];

    #[test]
    fn a_compact_record_names_its_actor_and_skill() {
        let named = Some((10_000, 12_340_000));
        assert_eq!(context(TABLE, &compact(MARKER_3B, &SKILL)), named);
        assert_eq!(context(TABLE, &compact(MARKER_3D, &SKILL)), named);
        // The marker as the body's first bytes.
        assert_eq!(context(TABLE, &compact(MARKER_3B, &SKILL)[2..]), named);
    }

    #[test]
    fn the_three_byte_skill_is_tried_after_the_four_byte_one() {
        let named = Some((10_000, 12_340_000));
        assert_eq!(context(TABLE, &compact(MARKER_3B, &[0x2B, 0x4B, 0xBC, 0xFF])), named);
        assert_eq!(context(TABLE, &compact(MARKER_3B, &SKILL[..3])), named);
        assert_eq!(context(TABLE, &compact(MARKER_3B, &SKILL[..2])), None);
    }

    #[test]
    fn a_compact_record_needs_its_marker_and_a_38_after_it() {
        assert_eq!(context(TABLE, &compact([0x08, 0x3B, 0x38, 0x00, 0x01], &SKILL)), None);
        let mut no_tag = vec![0xE1, 0xE2];
        no_tag.extend(MARKER_3B);
        no_tag.extend([0x01, 0x90, 0x4E, 0x07, 0x2B, 0x4B, 0xBC, 0x00]);
        assert_eq!(context(TABLE, &no_tag), None);
        // One byte after the 38; then no byte between the actor and the skill.
        assert_eq!(context(TABLE, &compact(MARKER_3B, &[])[..10]), None);
        assert_eq!(context(TABLE, &compact(MARKER_3B, &[])[..11]), None);
    }

    #[test]
    fn a_compact_actor_under_100_is_a_misread() {
        let mut body = vec![0xE1, 0xE2];
        body.extend(MARKER_3B);
        body.extend([0x01, 0x38, 0x63, 0x07, 0x2B, 0x4B, 0xBC, 0x00]);
        assert_eq!(context(TABLE, &body), None);
    }

    #[test]
    fn only_the_first_38_after_the_marker_is_tried() {
        let mut body = vec![0xE1, 0xE2];
        body.extend(MARKER_3B);
        body.extend([0x38, 0x05, 0x07, 0x2B, 0x4B, 0xBC, 0x00, 0x38, 0x90, 0x4E, 0x07, 0x2B, 0x4B, 0xBC, 0x00]);
        assert_eq!(context(TABLE, &body), None);
    }

    #[test]
    fn a_theostone_code_is_known_without_a_table() {
        assert_eq!(context("{}", &compact(MARKER_3B, &SKILL)), None);
        assert_eq!(context("{}", &compact(MARKER_3B, &[0xFB, 0xC3, 0xC9, 0x01])), Some((10_000, 30_000_123)));
    }

    #[test]
    fn a_bare_length_has_no_compact_record() {
        let (_, p) = rig(names(TABLE), 1_000);
        assert!(p.read_held_tick_skill(&[0x80]).is_none());
        assert!(p.read_held_tick_skill(&[0x04]).is_none());
    }
}
