//! Hit records (`04 38`): damage dealt and taken, misses and resists, and
//! the heals that ride on hits.

use std::fmt;

use super::PacketDecoder;
use crate::capture::opcodes::DAMAGE;
use crate::capture::varint::{next_varint, u32_at};
use crate::combat::data_storage::NoDamageHit;
use crate::entity::damage_packet::ParsedDamagePacket;
use crate::entity::skill_group::{self, THEOSTONE_EFFECTS};
use crate::entity::mark_kinds::{self, HitMark};
use crate::entity::taken::{TakenHit, TakenKind};

/// Theostone raw item ids, read as skill codes after `* 10 + 1`.
pub(super) const THEOSTONE_ITEM_IDS: std::ops::RangeInclusive<i64> = 3_000_000..=3_099_999;
/// The skill code of a compact aggregation record; its real skill comes from
/// `HeldTickSkill`.
const COMPACT_AGGREGATE_SKILL: i32 = 99_745_942;
/// Switch `0x36`: layout 6 with both multi-hit bits (`0x30`) set. The
/// repeated-hit fallbacks apply to this switch only.
const MULTI_HIT_SWITCH: i32 = 54;
/// `01 00` before another record of the same frame.
const NEXT_RECORD: [u8; 2] = [0x01, 0x00];
/// `03 00` before the heal a hit gave its actor.
const HEAL_FOLLOWS: [u8; 2] = [0x03, 0x00];
/// A value above this is a misreading, not a hit.
const MAX_VALUE: i32 = 99_999_999;
/// A heal amount above this reads as no heal.
const MAX_HEAL: i32 = 9_999_999;
/// The most additional hits a record names; also the top of a count.
const MAX_EXTRA_HITS: i32 = 25;
/// In a loose tail, an additional hit below this, or above both the value
/// and `EXTRA_HIT_CEILING`, is a misreading.
const MIN_EXTRA_HIT: i32 = 50;
const EXTRA_HIT_CEILING: i32 = 500_000;

/// A record's fields up to its hit type.
struct HitHead {
    target: i32,
    /// Low nibble the layout: 0 and 2 carry no value, 4 to 7 one, and 6
    /// and 7 (bit 0x02) a plotter. Bits 0x10 and 0x20 shape the tail.
    switch: i32,
    actor: i32,
    /// The skill, a Theostone's item id already turned into its effect id.
    code: i32,
    hit_type: i32,
}

impl HitHead {
    fn layout(&self) -> i32 {
        self.switch & 0x0F
    }

    fn has_value(&self) -> bool {
        self.layout() >= 4
    }

    fn is_monster(&self) -> bool {
        skill_group::NPC_SKILLS.contains(&self.code)
    }

    fn is_crit(&self) -> bool {
        self.hit_type & 0xFF == 3
    }
}

/// The two varints at a value's place, as read.
struct ValuePair {
    value: i32,
    /// The first of the two when it was the actor's combat speed, for the
    /// trace line only.
    scalar: Option<i32>,
}

/// The hits a record says its hit set off: how many, and their damage.
#[derive(Default)]
struct ExtraHits {
    hits: i32,
    damage: i64,
}

/// What a tail gives.
struct Tail {
    /// The count the record names, which the aggregate rule looks at even
    /// where fewer hits were taken.
    count: i32,
    extra: ExtraHits,
    /// The value, which a repeated hit on `MULTI_HIT_SWITCH` can replace.
    value: i32,
}

/// What follows the head of a record with a value.
struct HitBody {
    plotter: Option<Plotter>,
    /// The hit's own damage, its additional hits left out.
    damage: i64,
    scalar: Option<i32>,
    extra: ExtraHits,
    heal: i32,
    /// The skill an aggregate hit takes from its bundle's compact record.
    aggregate_skill: Option<i32>,
}

impl PacketDecoder {
    /// Reads a `<length> 04 38` frame: records joined by `01 00`, each filed
    /// as soon as it is read, up to the first that does not read. True when
    /// any was taken. Another opcode goes to the scan for hits inside other
    /// packets when `allow_embedded_scan`. With `require_trusted` (that
    /// scan's own call) only hits of known player skills are taken.
    pub(super) fn read_hit_record(&mut self, packet: &[u8], allow_embedded_scan: bool, require_trusted: bool) -> bool {
        let mut at = 0;
        if next_varint(packet, &mut at).is_none() {
            return false;
        }
        match packet.get(at..at + 2) {
            Some(opcode) if opcode == DAMAGE => at += 2,
            Some(_) if allow_embedded_scan => return self.read_inner_hit(packet),
            _ => return false,
        }
        let mut any = false;
        loop {
            let joined = bytes_at(packet, at, &NEXT_RECORD);
            if joined {
                at += 2;
            }
            // Only the first record may stand without the pair before it.
            if (any && !joined) || at >= packet.len() {
                break;
            }
            match self.read_hit(packet, at, require_trusted) {
                Some(next) => {
                    any = true;
                    at = next;
                }
                None => break,
            }
        }
        any
    }

    /// The id a raw skill's records are filed under (`skill_group::row_skill`).
    pub(super) fn fold_skill_code(&self, raw: i32) -> i32 {
        skill_group::row_skill(raw, &self.skill_lookup)
    }

    /// Whether a code names a skill: it and its row are valid codes, and the
    /// row is a Theostone's or the table names the row or the code.
    pub(super) fn skill_in_table(&self, skill_code: i32) -> bool {
        if !skill_code_in_range(skill_code) {
            return false;
        }
        let row = self.fold_skill_code(skill_code);
        let named = |code| !self.skill_lookup.skill_title(code).is_empty();
        skill_code_in_range(row) && (THEOSTONE_EFFECTS.contains(&row) || named(row) || named(skill_code))
    }

    /// The record at `at`, filed. Gives where the next record may start, or
    /// `None` when the walk stops here: at a record that does not read, or
    /// at one without a value (filed all the same).
    fn read_hit(&mut self, packet: &[u8], mut at: usize, require_trusted: bool) -> Option<usize> {
        let head = self.read_hit_head(packet, &mut at)?;
        if require_trusted && head.is_monster() {
            return None;
        }
        if !head.has_value() {
            self.file_no_value(&head, require_trusted);
            return None;
        }
        let body = self.read_hit_body(packet, &mut at, &head)?;
        if require_trusted && !self.fits_scan(&head, &body) {
            return None;
        }
        self.file_hit(&head, &body);
        Some(at)
    }

    /// Target, switch, flag, actor, skill, uid and hit type, each checked.
    fn read_hit_head(&self, packet: &[u8], at: &mut usize) -> Option<HitHead> {
        let entity = |at: &mut usize| next_varint(packet, at).filter(|&id| self.data_storage.looks_like_entity(id));
        let target = entity(at)?;
        let switch = next_varint(packet, at).filter(|s| matches!(s & 0x0F, 0 | 2 | 4..=7))?;
        next_varint(packet, at)?;
        let actor = entity(at)?;
        let mut code = i64::from(u32_at(packet, *at)?);
        *at += 4;
        if THEOSTONE_ITEM_IDS.contains(&code) {
            code = code * 10 + 1;
        }
        let code = i32::try_from(code).ok().filter(|&c| skill_code_in_range(c))?;
        *at += 1; // the uid
        let hit_type = next_varint(packet, at)?;
        Some(HitHead { target, switch, actor, code, hit_type })
    }

    /// A record without a value counts only as a Miss or a Resist: damage
    /// taken for a monster's skill, else a count on the skill's row where
    /// the actor already did the target damage.
    fn file_no_value(&self, head: &HitHead, require_trusted: bool) {
        let Some(kind) = NoDamageHit::from_hit_type(head.hit_type) else { return };
        if head.is_monster() {
            let kind = match kind {
                NoDamageHit::Miss => TakenKind::Miss,
                NoDamageHit::Resist => TakenKind::Resist,
            };
            let hit = TakenHit::plain(self.record_time(), head.target, head.actor, head.code, 0, kind);
            self.data_storage.append_taken(hit);
            return;
        }
        if require_trusted || head.actor == head.target {
            return;
        }
        let skill = self.fold_skill_code(head.code);
        let counted = self.data_storage.append_no_damage_hit(head.target, head.actor, skill, kind);
        tracing::trace!(
            target: "hit_flags",
            "{} actor={} target={} skill={} damage=- type={} layout={} counted={}",
            crate::clock::now_ms(),
            head.actor,
            head.target,
            skill,
            head.hit_type,
            head.layout(),
            counted
        );
    }

    /// The plotter, the value, the tail and the heal after a head.
    fn read_hit_body(&mut self, packet: &[u8], at: &mut usize, head: &HitHead) -> Option<HitBody> {
        let layout = head.layout();
        let plotter = match layout & 0x02 {
            0 => None,
            _ => Some(read_plotter(packet, *at)?),
        };
        // Over the effect and the count to the varints. Layout 6 stops on
        // the count's top byte, which `read_value_pair` steps past.
        let skip = match layout {
            4 => 8,
            5 => 12,
            6 => 10,
            _ => 14,
        };
        *at += skip + plotter.as_ref().map_or(0, |p| p.hp_len - 1);
        if *at >= packet.len() {
            return None;
        }
        let pair = read_value_pair(packet, at)?;
        if pair.value > MAX_VALUE {
            return None;
        }
        let strict = if head.code != COMPACT_AGGREGATE_SKILL && matches!(layout, 4 | 6) {
            strict_tail(packet, *at, layout, head.switch, pair.value)
        } else {
            None
        };
        let Tail { count, extra, value } = match strict {
            Some((end, extra)) => {
                *at = end;
                Tail { count: extra.hits, extra, value: pair.value }
            }
            None => loose_tail(packet, at, head.switch, &pair),
        };
        // The value already holds what its additional hits did.
        let mut damage = i64::from(value);
        if extra.hits > 0 && extra.damage > 0 && damage > extra.damage {
            damage -= extra.damage;
        }
        let aggregate_skill = match head.code {
            COMPACT_AGGREGATE_SKILL => self.take_compact_skill(head.actor, count, extra.damage, pair.value),
            _ => None,
        };
        if aggregate_skill.is_some() {
            damage = i64::from(pair.value) - extra.damage;
        }
        let mut heal = 0;
        if bytes_at(packet, *at, &HEAL_FOLLOWS) {
            *at += 2;
            heal = next_varint(packet, at).filter(|h| (1..=MAX_HEAL).contains(h)).unwrap_or(0);
        }
        Some(HitBody { plotter, damage, scalar: pair.scalar, extra, heal, aggregate_skill })
    }

    /// The skill of the bundle's compact record, for an aggregate hit by the
    /// same actor that sums more than one hit below its second value. Used
    /// once: the context goes with it.
    fn take_compact_skill(&mut self, actor: i32, count: i32, extra_damage: i64, second: i32) -> Option<i32> {
        let context = self.pending_compact_skill_context.as_ref()?;
        if context.actor_id != actor || count < 2 || extra_damage <= 0 || i64::from(second) <= extra_damage {
            return None;
        }
        let skill = context.skill_raw;
        self.pending_compact_skill_context = None;
        Some(skill)
    }

    /// What the scan for hits inside other packets may take: a known
    /// skill's landed hit, with damage, on someone other than its actor.
    fn fits_scan(&self, head: &HitHead, body: &HitBody) -> bool {
        head.actor != head.target
            && (1..=3).contains(&(head.hit_type & 0xFF))
            && body.damage > 0
            && self.skill_in_table(self.filed_skill(head, body))
    }

    /// The skill a hit goes under: an aggregate hit's from its compact
    /// record, else the row of its own code.
    fn filed_skill(&self, head: &HitHead, body: &HitBody) -> i32 {
        body.aggregate_skill.unwrap_or_else(|| self.fold_skill_code(head.code))
    }

    /// A record with a value: damage taken for a monster's skill, nothing
    /// for a resource restore, damage on someone else, or a heal on oneself.
    fn file_hit(&self, head: &HitHead, body: &HitBody) {
        let plotter = body.plotter.as_ref();
        if head.is_monster() {
            if head.actor != head.target {
                self.data_storage.append_taken(TakenHit {
                    at: self.record_time(),
                    target: head.target,
                    actor: head.actor,
                    skill: head.code,
                    damage: body.damage + body.extra.damage,
                    kind: TakenKind::Hit,
                    flags: plotter.map_or(0, |p| p.flags),
                    angle: plotter.and_then(|p| p.angle).unwrap_or(0),
                    crit: head.is_crit(),
                    restored: plotter.map_or(0, |p| i64::from(p.hp)),
                });
            }
            return;
        }
        // MP a spirit gives its master, or a self-cast restore: neither
        // damage nor healing.
        if skill_group::restores_resource(head.code) {
            return;
        }
        if head.actor == head.target {
            if body.damage > 1 && self.data_storage.seen_as_player(head.actor) {
                let skill = self.filed_skill(head, body);
                self.data_storage.add_heal_event(head.actor, head.actor, skill, body.damage, false, self.record_time());
            }
            return;
        }
        let row = self.damage_row(head, body);
        let (mods, dir) = (plotter.map(|p| Byte(p.flags)), plotter.and_then(|p| p.angle).map(Byte));
        tracing::trace!(
            target: "hit_flags",
            "{} actor={} target={} skill={} damage={} type={} layout={} mods={} dir={} multi={} multi_dmg={} hp={} scalar={}",
            row.when_ms(),
            head.actor,
            head.target,
            row.skill_used(),
            body.damage,
            head.hit_type,
            head.layout(),
            OrDash(mods),
            OrDash(dir),
            body.extra.hits,
            body.extra.damage,
            OrDash(plotter.map(|p| p.hp)),
            OrDash(body.scalar)
        );
        self.data_storage.append_damage(row);
    }

    /// The damage row of a hit on someone else.
    fn damage_row(&self, head: &HitHead, body: &HitBody) -> ParsedDamagePacket {
        let skill = self.filed_skill(head, body);
        let raw = body.aggregate_skill.unwrap_or(head.code);
        let mut row = ParsedDamagePacket::new();
        row.set_timestamp(self.record_time());
        row.set_target_id(head.target);
        row.set_actor_id(head.actor);
        row.set_skill_code(skill);
        row.set_raw_skill_code(raw);
        row.set_flag_bits(specialty_flags(raw));
        row.set_hit_kind(head.hit_type);
        row.set_mark_list(hit_marks(head.is_crit(), body.plotter.as_ref()));
        row.set_damage(body.damage);
        row.set_added_hits(body.extra.hits);
        row.set_added_damage(body.extra.damage);
        row.set_heal_value(body.heal);
        row
    }
}

/// Whether a code can be a skill a record carries.
pub(super) fn skill_code_in_range(skill_code: i32) -> bool {
    (1..=299_999_999).contains(&skill_code)
}

/// The two varints at a value's place: the actor's combat speed, then the
/// value, crits as every other hit. A leading 0 is the count's top byte
/// (layout 6): the pair then starts one varint on, when there is one.
///
/// Read the other way round (a crit's first varint as its value when it
/// looks like one), that "value" is the actor's own combat speed: crits of
/// tiny damage (PvP, shields) then read as thousands each.
fn read_value_pair(packet: &[u8], at: &mut usize) -> Option<ValuePair> {
    let mut first = next_varint(packet, at)?;
    let mut value = next_varint(packet, at)?;
    let mut shifted = false;
    if first == 0 {
        if let Some(third) = next_varint(packet, at) {
            (first, value, shifted) = (value, third, true);
        }
    }
    Some(ValuePair { value, scalar: shifted.then_some(first) })
}

/// The tail in its strict shape: the record's own layout, then for layout 4
/// a spirit's form without the field, read as layout 6 reads (no field).
/// Gives the end and the additional hits.
fn strict_tail(packet: &[u8], at: usize, layout: i32, switch: i32, value: i32) -> Option<(usize, ExtraHits)> {
    let shape = |layout| parse_hit_tail_as(packet, at, layout, switch, value);
    let (end, _field, hits, damage) = shape(layout).or_else(|| if layout == 4 { shape(6) } else { None })?;
    Some((end, ExtraHits { hits, damage: i64::from(damage) }))
}

/// The older reading, for a tail no strict shape fits: on switch `0x3_`
/// one varint left unread, then a count, then up to that many additional
/// hits.
fn loose_tail(packet: &[u8], at: &mut usize, switch: i32, pair: &ValuePair) -> Tail {
    if switch & 0x30 == 0x30 && *at < packet.len() {
        let _ = next_varint(packet, at);
    }
    let count = loose_count(packet, at);
    let ceiling = pair.value.max(EXTRA_HIT_CEILING);
    let mut extra = ExtraHits::default();
    // The value every hit so far had; none once two differ or one is a
    // misreading.
    let mut repeated = None;
    for _ in 0..count.min(MAX_EXTRA_HITS) {
        if marker_at(packet, *at) || bytes_at(packet, *at, &DAMAGE) {
            break;
        }
        let Some(hit) = next_varint(packet, at) else { break };
        if !(MIN_EXTRA_HIT..=ceiling).contains(&hit) {
            // The damage summed so far goes; the hits read stay counted.
            extra.damage = 0;
            repeated = None;
            break;
        }
        repeated = match repeated {
            Some(same) if same != hit => None,
            _ if extra.hits == 0 => Some(hit),
            kept => kept,
        };
        extra.hits += 1;
        extra.damage += i64::from(hit);
    }
    let mut value = pair.value;
    if let (MULTI_HIT_SWITCH, Some(each)) = (switch, repeated) {
        // One hit written for a count of them: they all did the same.
        if extra.hits == 1 && count > 1 {
            extra = ExtraHits { hits: count, damage: i64::from(count) * i64::from(each) };
        }
        // A second value of about ten such hits is their sum, not the hit.
        let sum = i64::from(extra.hits) * i64::from(each);
        if i64::from(pair.value) - sum <= i64::from(each) && pair.value / 10 == each {
            value = each;
        }
    }
    Tail { count, extra, value }
}

/// A loose tail's count: a varint of 0 to 25, else (with no marker after
/// it) the varint after that one. When both are larger the count is 0 and
/// the reading goes back to before them.
fn loose_count(packet: &[u8], at: &mut usize) -> i32 {
    if *at >= packet.len() || marker_at(packet, *at) {
        return 0;
    }
    let start = *at;
    match next_varint(packet, at) {
        Some(n) if n <= MAX_EXTRA_HITS => n,
        Some(_) if !marker_at(packet, *at) => match next_varint(packet, at) {
            Some(n) if n <= MAX_EXTRA_HITS => n,
            Some(_) => {
                *at = start;
                0
            }
            None => 0,
        },
        _ => 0,
    }
}

/// Whether a marker, `<01..07> 00`, starts at `at`.
fn marker_at(packet: &[u8], at: usize) -> bool {
    matches!(packet.get(at..at + 2), Some([1..=7, 0]))
}

/// Whether `bytes` start at `at`.
fn bytes_at(packet: &[u8], at: usize, bytes: &[u8]) -> bool {
    packet.get(at..).is_some_and(|rest| rest.starts_with(bytes))
}

/// The marks of a hit: its flags byte's, then its angle's, then a crit's.
fn hit_marks(crit: bool, plotter: Option<&Plotter>) -> Vec<HitMark> {
    let mut marks = plotter.map_or_else(Vec::new, |p| mark_kinds::from_hit_flags(p.flags));
    match plotter.and_then(|p| p.angle) {
        Some(0x01) => marks.push(HitMark::Back),
        Some(0x02) => marks.push(HitMark::Frontal),
        _ => {}
    }
    if crit {
        marks.push(HitMark::Critical);
    }
    marks
}

/// The Specialty marks a raw skill id carries in its tens, hundreds and
/// thousands digits (raw mod 10,000, over 10): each digit 1 to 5 sets that
/// mark. A 0 or a digit over 5 among them means none; so does a Theostone.
fn specialty_flags(raw: i32) -> [bool; 5] {
    let mut flags = [false; 5];
    if THEOSTONE_EFFECTS.contains(&raw) {
        return flags;
    }
    let mut digits = raw % 10_000 / 10;
    while digits > 0 {
        match digits % 10 {
            d @ 1..=5 => flags[d as usize - 1] = true,
            _ => return [false; 5],
        }
        digits /= 10;
    }
    flags
}

/// A field of a trace line: its value, or `-` when the record has none.
struct OrDash<T>(Option<T>);

impl<T: fmt::Display> fmt::Display for OrDash<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(v) => fmt::Display::fmt(v, f),
            None => f.write_str("-"),
        }
    }
}

/// A flags or angle byte as the trace lines show it, as `0x0a`.
struct Byte(u8);

impl fmt::Display for Byte {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#04x}", self.0)
    }
}

fn parse_hit_tail_as(packet: &[u8], mut offset: usize, layout: i32, switch_value: i32, value: i32) -> Option<(usize, i32, i32, i32)> {
    let mut field = 0;
    if layout == 4 {
        field = next_varint(packet, &mut offset)?;
        if !(1..=25).contains(&field) {
            return None;
        }
    }
    let (mut count, mut damage) = (0, 0i64);
    if switch_value & 0x20 != 0 {
        count = next_varint(packet, &mut offset)?;
        if !(1..=25).contains(&count) {
            return None;
        }
        for _ in 0..count {
            let hit = next_varint(packet, &mut offset)?;
            if hit < 0 {
                return None;
            }
            damage += i64::from(hit);
        }
        if damage >= i64::from(value) {
            return None;
        }
    }
    let rest = &packet[offset.min(packet.len())..];
    let clean_end = rest.is_empty()
        || (rest.len() >= 2 && rest[1] == 0x00 && (1..=7).contains(&rest[0]))
        || rest.starts_with(&DAMAGE);
    clean_end.then_some((offset, field, count, damage as i32))
}

/// The damage plotter after a record's hit type: the flags byte, the HP a
/// Regeneration hit restored, and the angle byte (absent at the very end).
pub(super) struct Plotter {
    pub(super) flags: u8,
    pub(super) hp: i32,
    pub(super) hp_len: usize,
    pub(super) angle: Option<u8>,
}

pub(super) fn read_plotter(packet: &[u8], offset: usize) -> Option<Plotter> {
    let flags = *packet.get(offset)?;
    let mut at = offset + 1;
    let hp = next_varint(packet, &mut at)?;
    Some(Plotter { flags, hp, hp_len: at - offset - 1, angle: packet.get(at).copied() })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{hit_marks, skill_code_in_range, specialty_flags, Plotter, PacketDecoder};
    use crate::built::{self, shard_hit, Abnormal, Hit, Spawn, F};
    use crate::capture::opcodes::DAMAGE;
    use crate::capture::decoder::tests::{reader, spirit_hit};
    use crate::capture::decoder::HeldTickSkill;
    use crate::checks::replay_timeline::HitTap;
    use crate::combat::data_storage::{DataStorage, SkillTotals, TakenTick};
    use crate::entity::mark_kinds::HitMark::{Critical, Double, Frontal, Parry};
    use crate::entity::taken::{TakenHit, TakenKind, TakenStats};
    use crate::game_record::TakeStat;
    use crate::i18n::lookup::{NpcLookup, SkillLookup};

    /// A boss fight on player `YOU`, as ms from the fight's start, built from
    /// the field layout with made-up values: monster hits with and without a
    /// value, damage over time and an immune, the player's own item skills, a
    /// Chanter ward's tick, and the skill-effect entity's spawn naming its boss.
    const YOU: u32 = 5100;
    const BOSS: u32 = 30201;
    const BOSS_EFFECT: u32 = 30202;
    const BOSS_OTHER: u32 = 30203;

    fn on_you(mut hit: Hit) -> Vec<u8> {
        hit.target = YOU;
        hit.packet()
    }

    /// A Chanter ward's record after a hit's value: `01 <skill effect u32>
    /// <count> 00`.
    fn ward(count: u32) -> Vec<u8> {
        built::fields(&[F::V(1), F::U32(1_873_000_211), F::V(count), F::V(0)])
    }

    /// Seven hits with a value (two from the effect entity, three from the
    /// front, one from behind), 20,100 in all, and one immune.
    fn boss_fight() -> Vec<(i64, Vec<u8>)> {
        vec![
            (400, on_you(Hit { switch: 0x06, actor: BOSS, skill: 1_800_510, uid: 1, hit_type: 2, plotter: Some((0, 0, 2)), effect: 180_051_011, count: 3, values: vec![10_000, 1200, 3, 0], ..Hit::default() })),
            (14_000, built::tick(YOU, 0x0a, BOSS, 900, 1_873_000_211, 300, &[F::U32(18_730_002)])),
            (14_000, on_you(Hit { switch: 0x46, actor: BOSS, skill: 1_800_520, uid: 2, hit_type: 2, plotter: Some((0, 0, 2)), effect: 180_052_011, count: 2, values: vec![10_000, 2000], tail: ward(2), ..Hit::default() })),
            (44_500, Spawn {
                id: BOSS_EFFECT,
                kind: 0x1C,
                code: 2_920_258,
                hp: 3_000_000,
                filler: 60,
                abnormals: vec![
                    Abnormal { instance: 1, abnormal: 10_000_001, byte: Some(4), ..Abnormal::endless(BOSS_EFFECT) },
                    Abnormal { instance: 2, abnormal: 10_000_004, byte: None, ..Abnormal::endless(BOSS_EFFECT) },
                ],
                end: built::effect_parent(BOSS),
                ..Spawn::default()
            }
            .packet()),
            (49_000, on_you(Hit { switch: 0x04, actor: BOSS_EFFECT, skill: 1_800_650, uid: 3, hit_type: 2, effect: 180_065_011, count: 1, values: vec![10_000, 3600, 1, 0], ..Hit::default() })),
            (49_000, on_you(Hit { switch: 0x00, actor: BOSS_EFFECT, skill: 1_800_650, uid: 3, hit_type: 2, effect: 180_065_012, count: 1, values: vec![10_000, 1, 0], ..Hit::default() })),
            (49_500, on_you(Hit { switch: 0x00, actor: YOU, skill: 2_011_101, uid: 4, hit_type: 2, effect: 201_110_111, count: 1, values: vec![10_000, 1, 0], ..Hit::default() })),
            (53_000, on_you(Hit { switch: 0x00, actor: YOU, skill: 2_020_101, uid: 5, hit_type: 2, effect: 202_010_111, count: 1, values: vec![10_000, 1, 0], ..Hit::default() })),
            (54_000, on_you(Hit { switch: 0x04, actor: BOSS_EFFECT, skill: 1_800_650, uid: 6, hit_type: 2, effect: 180_065_011, count: 1, values: vec![10_000, 3600, 1, 0], ..Hit::default() })),
            (54_000, on_you(Hit { switch: 0x00, actor: BOSS_EFFECT, skill: 1_800_650, uid: 6, hit_type: 2, effect: 180_065_012, count: 1, values: vec![10_000, 1, 0], ..Hit::default() })),
            (56_000, on_you(Hit { switch: 0x04, actor: YOU, skill: 2_010_101, uid: 7, hit_type: 2, effect: 201_010_111, count: 1, values: vec![10_000, 1700, 1, 0], ..Hit::default() })),
            (56_000, on_you(Hit { switch: 0x04, actor: YOU, skill: 2_010_101, uid: 7, hit_type: 2, effect: 201_010_112, count: 1, values: vec![10_000, 500, 1, 0], ..Hit::default() })),
            (65_800, on_you(Hit { switch: 0x04, actor: BOSS, skill: 1_800_656, uid: 8, hit_type: 2, effect: 180_065_611, count: 1, values: vec![10_000, 5600, 1, 0], ..Hit::default() })),
            (71_500, on_you(Hit { switch: 0x00, actor: BOSS, skill: 1_800_554, uid: 9, hit_type: 2, effect: 180_055_411, count: 2, values: vec![10_000, 2, 0], ..Hit::default() })),
            (71_600, on_you(Hit { switch: 0x00, actor: BOSS, skill: 1_800_551, uid: 10, hit_type: 2, effect: 180_055_021, count: 1, values: vec![10_000, 1, 0], ..Hit::default() })),
            (72_100, on_you(Hit { switch: 0x00, actor: BOSS, skill: 1_800_551, uid: 11, hit_type: 2, effect: 180_055_021, count: 1, values: vec![10_000, 1, 0], ..Hit::default() })),
            (72_500, built::tick(YOU, 0x02, YOU, 1200, 190_000_151, 2600, &[])),
            (72_600, on_you(Hit { switch: 0x00, actor: BOSS, skill: 1_800_551, uid: 12, hit_type: 2, effect: 180_055_021, count: 1, values: vec![10_000, 1, 0], ..Hit::default() })),
            (73_100, on_you(Hit { switch: 0x00, actor: BOSS, skill: 1_800_551, uid: 13, hit_type: 2, effect: 180_055_021, count: 1, values: vec![10_000, 1, 0], ..Hit::default() })),
            (73_500, on_you(Hit { switch: 0x00, actor: BOSS, skill: 1_800_551, uid: 14, hit_type: 2, effect: 180_055_021, count: 1, values: vec![10_000, 1, 0], ..Hit::default() })),
            (74_000, on_you(Hit { switch: 0x00, actor: BOSS, skill: 1_800_551, uid: 15, hit_type: 2, effect: 180_055_021, count: 1, values: vec![10_000, 1, 0], ..Hit::default() })),
            (74_600, on_you(Hit { switch: 0x00, actor: BOSS, skill: 1_800_551, uid: 16, hit_type: 2, effect: 180_055_021, count: 1, values: vec![10_000, 1, 0], ..Hit::default() })),
            (75_100, on_you(Hit { switch: 0x00, actor: BOSS, skill: 1_800_551, uid: 17, hit_type: 2, effect: 180_055_021, count: 1, values: vec![10_000, 1, 0], ..Hit::default() })),
            (75_600, on_you(Hit { switch: 0x00, actor: BOSS, skill: 1_800_551, uid: 18, hit_type: 2, effect: 180_055_021, count: 1, values: vec![10_000, 1, 0], ..Hit::default() })),
            (76_500, built::immune(YOU, 1210, 2001, BOSS_OTHER, 1_800_559)),
            (76_900, on_you(Hit { switch: 0x06, actor: BOSS, skill: 1_800_554, uid: 9, hit_type: 2, plotter: Some((0, 0, 1)), effect: 180_055_421, count: 3, values: vec![10_000, 2700, 3, 0], ..Hit::default() })),
            (76_900, on_you(Hit { switch: 0x00, actor: BOSS, skill: 1_800_554, uid: 9, hit_type: 2, effect: 180_055_422, count: 3, values: vec![10_000, 3, 0], ..Hit::default() })),
            (82_100, on_you(Hit { switch: 0x00, actor: YOU, skill: 2_020_101, uid: 19, hit_type: 0, effect: 202_010_111, count: 1, values: vec![10_000, 1, 0], ..Hit::default() })),
            (82_600, on_you(Hit { switch: 0x00, actor: YOU, skill: 2_011_101, uid: 20, hit_type: 2, effect: 201_110_111, count: 1, values: vec![10_000, 1, 0], ..Hit::default() })),
            (87_200, on_you(Hit { switch: 0x04, actor: YOU, skill: 2_010_101, uid: 21, hit_type: 2, effect: 201_010_111, count: 1, values: vec![10_000, 1700, 1, 0], ..Hit::default() })),
            (87_200, on_you(Hit { switch: 0x04, actor: YOU, skill: 2_010_101, uid: 21, hit_type: 2, effect: 201_010_112, count: 1, values: vec![10_000, 500, 1, 0], ..Hit::default() })),
            (103_400, on_you(Hit { switch: 0x06, actor: BOSS, skill: 1_800_540, uid: 22, hit_type: 2, plotter: Some((0, 0, 2)), effect: 180_054_021, count: 1, values: vec![10_000, 1400, 1, 0], ..Hit::default() })),
        ]
    }

    fn replay(frames: &[(i64, Vec<u8>)], bosses: &[(u32, i32)]) -> Vec<TakenTick> {
        let storage = Arc::new(DataStorage::new());
        for &(id, code) in bosses {
            storage.append_mob(id as i32, code);
        }
        let mut p = PacketDecoder::new(storage.clone(), Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
        for (at, bytes) in frames {
            p.pin_clock(Some(*at));
            assert_eq!(p.decode_buffer(bytes), bytes.len(), "{}", built::to_hex(bytes));
        }
        p.pin_clock(None);
        storage.taken_since(0)
    }

    fn took(ticks: &[TakenTick]) -> TakeStat {
        let mut s = TakenStats::default();
        for t in ticks.iter().filter(|t| t.hit.target == YOU as i32) {
            s.add(&t.hit);
        }
        TakeStat::of_meter(&s)
    }

    fn game(damage: i64, counts: [i64; 12]) -> TakeStat {
        TakeStat { damage, counts }
    }

    /// Counted as the game's TakeStatData: total, accuracy, crit, perfect,
    /// double, front, back, block, miss, immune, iron wall, restoration. The
    /// ward's tick, the hits without a value and your own item skills are
    /// not counted; the effect entity's hits go to its boss.
    #[test]
    fn damage_taken_counts_a_boss_fights_hits_and_immune() {
        let ticks = replay(&boss_fight(), &[(BOSS, 2_310_471)]);
        assert_eq!(took(&ticks), game(20_100, [8, 7, 0, 0, 0, 3, 1, 0, 0, 1, 0, 0]));
        let rapid: Vec<_> = ticks.iter().filter(|t| t.hit.skill == 1_800_650).map(|t| (t.hit.actor, t.source)).collect();
        assert_eq!(rapid, vec![(BOSS_EFFECT as i32, BOSS as i32); 2]);
    }

    /// The game record view replays a saved fight's slice: the damage taken
    /// is counted over the record's own window, past the end of the damage
    /// window too, for a player who dealt the target nothing.
    #[test]
    fn a_slice_replay_counts_the_damage_taken_over_the_record_window() {
        use std::collections::HashSet;

        use crate::game_record::{replay_slice_window, SliceWindow};

        let records: Vec<(i32, Vec<u8>)> = boss_fight().into_iter().map(|(at, bytes)| (at as i32, bytes)).collect();
        let (skills, npcs) = (Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
        let replay_over = |until: i64, taken: (i64, i64)| {
            let w = SliceWindow { records: &records, fight_start_ms: 0, target_id: BOSS as i32, from: 0, until, owner: Some(YOU as i32), game_total: 0, taken };
            replay_slice_window(&w, &skills, &npcs, &HashSet::new()).taken
        };
        let all = replay_over(200_000, (0, 200_000));
        assert_eq!(TakeStat::of_meter(&all), game(20_100, [8, 7, 0, 0, 0, 3, 1, 0, 0, 1, 0, 0]));
        assert_eq!(replay_over(1_000, (0, 200_000)), all, "read on past the damage window");
        let later: Vec<TakenTick> = replay(&boss_fight(), &[]).into_iter().filter(|t| t.hit.at >= 50_000).collect();
        assert!(!later.is_empty() && later.len() < 9);
        assert_eq!(TakeStat::of_meter(&replay_over(200_000, (50_000, 200_000))), took(&later));
    }

    // ----- One frame at a time -----

    /// The shard hit as a layout 4 record, without the plotter.
    fn plain() -> Hit {
        Hit { switch: 0x04, plotter: None, ..shard_hit() }
    }

    /// One `04 38` frame of `parts` one after another.
    fn frame(parts: &[&[u8]]) -> Vec<u8> {
        built::frame(&[&DAMAGE[..], &parts.concat()[..]].concat())
    }

    /// `parts` as the scan for hits inside other packets hands them over.
    fn scanned(parts: &[&[u8]]) -> Vec<u8> {
        [&[0xFF, 0x01][..], &DAMAGE[..], &parts.concat()[..]].concat()
    }

    /// A processor over a fresh storage with the skill table `skills`, the
    /// time pinned at 1,000.
    fn processor(skills: &str) -> (Arc<DataStorage>, PacketDecoder) {
        let names = SkillLookup::new();
        names.load_from_json(skills);
        let storage: Arc<DataStorage> = DataStorage::new().into();
        let p = reader(&storage, names, NpcLookup::new());
        (storage, p)
    }

    /// `read_hit_record` on `bytes` as the frame reader calls it, or as the
    /// scan does when `trusted`: its result, and the `hit_flags` lines as
    /// the flags report prints them.
    fn traced(p: &mut PacketDecoder, bytes: &[u8], trusted: bool) -> (bool, Vec<String>) {
        let tap = HitTap::default();
        let guard = tap.install_for(&["hit_flags"]);
        let read = p.read_hit_record(bytes, false, trusted);
        drop(guard);
        (read, tap.take().into_iter().map(|(_, _, line)| line).collect())
    }

    /// The value after `key=` in a trace line.
    fn field<'a>(line: &'a str, key: &str) -> &'a str {
        line.split(' ').find_map(|w| w.strip_prefix(key)?.strip_prefix('=')).unwrap_or("")
    }

    /// Damage, additional hits and their damage, per stored hit's line.
    fn stored(lines: &[String]) -> Vec<(i64, i32, i64)> {
        let n = |line: &str, key: &str| field(line, key).parse::<i64>().unwrap();
        lines.iter().filter(|l| field(l, "damage") != "-").map(|l| (n(l, "damage"), n(l, "multi") as i32, n(l, "multi_dmg"))).collect()
    }

    /// What a fresh processor makes of one frame of `parts`.
    fn hits_of(parts: &[&[u8]]) -> (bool, Vec<(i64, i32, i64)>) {
        let (_, mut p) = processor("{}");
        let (read, lines) = traced(&mut p, &frame(parts), false);
        (read, stored(&lines))
    }

    /// The trace line of a frame of `hit` alone, which must be taken.
    fn line_of(hit: &Hit) -> String {
        let (_, mut p) = processor("{}");
        let (read, lines) = traced(&mut p, &hit.packet(), false);
        assert!(read && lines.len() == 1, "{hit:?}: {lines:?}");
        lines[0].clone()
    }

    /// Every skill row in the storage, with its target and actor.
    fn rows(storage: &DataStorage) -> Vec<(i32, i32, SkillTotals)> {
        let mut out = Vec::new();
        for (target, fight) in storage.copy_targets() {
            for (actor, data) in fight.actors {
                out.extend(data.skills.into_values().map(|s| (target, actor, s)));
            }
        }
        out.sort_by_key(|(target, actor, s)| (*target, *actor, s.skill_code));
        out
    }

    #[test]
    fn a_shard_arrow_from_behind_files_900() {
        let packet = shard_hit().packet();
        assert_eq!(built::to_hex(&packet), "220438c1b8021600dc0b40c0f400b10280000111199b5f0100000084528407");
        let (storage, mut p) = processor("{}");
        let (read, lines) = traced(&mut p, &packet, false);
        assert!(read);
        assert_eq!(lines, ["1000 actor=1500 target=40001 skill=16040000 damage=900 type=2 layout=6 mods=0x80 dir=0x01 multi=0 multi_dmg=0 hp=0 scalar=10500"]);
        let all = rows(&storage);
        let (target, actor, row) = &all[0];
        assert_eq!((all.len(), *target, *actor, row.skill_code, row.total_damage, row.hit_count), (1, 40_001, 1_500, 16_040_000, 900, 1));
        assert_eq!((row.back_count, row.frontal_count, row.crit_count, row.heal_amount, row.spec_flags), (1, 0, 0, 0, [false; 5]));
        assert_eq!(row.hit_timestamps, [1_000]);

        // No plotter: no mods, angle or HP. No shift: no scalar.
        let line = line_of(&Hit { values: vec![10_500, 6_000, 1], ..plain() });
        assert_eq!(line, "1000 actor=1500 target=40001 skill=16040000 damage=6000 type=2 layout=4 mods=- dir=- multi=0 multi_dmg=0 hp=- scalar=-");
    }

    #[test]
    fn a_frame_needs_its_length_and_the_hit_opcode() {
        let (storage, mut p) = processor("{}");
        assert!(!p.read_hit_record(&[0x80], false, false));
        assert!(!p.read_hit_record(&[0x05, 0x04], false, false));
        assert!(!p.read_hit_record(&[0x80], true, false));
        let tick = built::tick(40_001, 0x0a, 1_500, 30, 1_604_000_011, 500, &[]);
        assert!(!p.read_hit_record(&tick, false, false));
        assert!(rows(&storage).is_empty());
    }

    #[test]
    fn records_after_the_first_need_01_00_before_them() {
        let first = shard_hit().fields();
        let second = Hit { target: 40_002, values: vec![10_500, 500], ..shard_hit() }.fields();
        let pair: &[u8] = &[0x01, 0x00];
        let (_, mut p) = processor("{}");
        let (read, lines) = traced(&mut p, &frame(&[&first, pair, &second]), false);
        assert_eq!((read, stored(&lines)), (true, vec![(900, 0, 0), (500, 0, 0)]));
        assert_eq!((field(&lines[0], "target"), field(&lines[1], "target")), ("40001", "40002"));
        assert_eq!(hits_of(&[&first, &second]), (true, vec![(900, 0, 0)]));
        assert_eq!(hits_of(&[pair, &first]), (true, vec![(900, 0, 0)]));

        // A record that does not read ends the walk; those before it stay.
        let low = Hit { target: 50, ..shard_hit() }.fields();
        assert_eq!(hits_of(&[&first, pair, &low]), (true, vec![(900, 0, 0)]));
        let (storage, mut p) = processor("{}");
        storage.remember_small_id(50);
        let (read, lines) = traced(&mut p, &frame(&[&first, pair, &low]), false);
        assert_eq!((read, stored(&lines)), (true, vec![(900, 0, 0); 2]));
        assert_eq!(hits_of(&[&Hit { target: 99, ..shard_hit() }.fields()]), (false, vec![]));

        // A resist after a hit: the frame is taken and the resist counted.
        let resist = Hit { switch: 0x00, flag: 1, plotter: None, hit_type: 6, values: vec![10_500, 1], ..shard_hit() }.fields();
        let (storage, mut p) = processor("{}");
        let (read, lines) = traced(&mut p, &frame(&[&first, pair, &resist]), false);
        assert!(read);
        assert_eq!(lines[1], "1000 actor=1500 target=40001 skill=16040000 damage=- type=6 layout=0 counted=true");
        assert_eq!(rows(&storage)[0].2.resist_count, 1);
    }

    #[test]
    fn each_field_up_to_the_hit_type_must_hold() {
        let refused = [
            Hit { target: 99, ..shard_hit() },
            Hit { switch: 0x11, ..shard_hit() },
            Hit { switch: 0x08, ..shard_hit() },
            Hit { actor: 99, ..shard_hit() },
            Hit { skill: 0, ..shard_hit() },
            Hit { skill: 300_000_000, ..shard_hit() },
        ];
        for hit in refused {
            assert_eq!(hits_of(&[&hit.fields()]), (false, vec![]), "{hit:?}");
        }
        let skill_of = |skill| field(&line_of(&Hit { skill, ..shard_hit() }), "skill").to_string();
        assert_eq!(skill_of(299_999_999), "299999999");
        // Theostone procs come as item ids, filed under their effect ids.
        assert_eq!(skill_of(3_000_017), "30000171");
        assert_eq!(skill_of(3_099_999), "30999991");
    }

    #[test]
    fn a_monster_skill_s_hit_is_damage_taken() {
        let taken = |hit: &Hit| {
            let (storage, mut p) = processor("{}");
            let (read, lines) = traced(&mut p, &hit.packet(), false);
            assert!(lines.is_empty() && rows(&storage).is_empty());
            (read, storage.taken_since(0).into_iter().map(|t| t.hit).collect::<Vec<_>>())
        };
        let claw = Hit {
            target: 2_001,
            switch: 0x16,
            actor: 50_001,
            skill: 1_229_240,
            uid: 0x21,
            hit_type: 2,
            plotter: Some((0x20, 10, 2)),
            effect: 122_924_011,
            count: 1,
            values: vec![10_000, 60],
            ..Hit::default()
        };
        let hit = TakenHit { at: 1_000, target: 2_001, actor: 50_001, skill: 1_229_240, damage: 60, kind: TakenKind::Hit, flags: 0x20, angle: 2, crit: false, restored: 10 };
        assert_eq!(taken(&claw), (true, vec![hit]));
        // The value holds its additional hits; damage taken counts them all.
        let flurry = Hit { switch: 0x36, plotter: Some((0x01, 0, 2)), values: vec![10_000, 300, 2, 50, 50], ..claw.clone() };
        assert_eq!(taken(&flurry).1[0].damage, 300);
        assert_eq!(taken(&Hit { target: 50_001, ..claw.clone() }), (true, vec![]));
        for skill in [2_999_999, 3_100_000] {
            assert_eq!(taken(&Hit { skill, ..claw.clone() }).1.len(), 1, "{skill}");
        }
        // The scan takes no monster's record, named or not.
        let (storage, mut p) = processor(r#"{"1218705":"M"}"#);
        assert!(!p.read_hit_record(&scanned(&[&Hit { skill: 1_218_705, ..claw }.fields()]), false, true));
        assert!(storage.taken_since(0).is_empty());
    }

    #[test]
    fn marks_come_from_the_flags_the_angle_and_a_crit() {
        let filed = |hit: Hit| {
            let (storage, mut p) = processor("{}");
            let (read, lines) = traced(&mut p, &hit.packet(), false);
            assert!(read);
            (lines[0].clone(), rows(&storage).remove(0).2)
        };
        // A crit by the hit type's low byte; the whole type is kept.
        let (line, row) = filed(Hit { hit_type: 259, ..shard_hit() });
        assert_eq!((field(&line, "type"), row.crit_count, row.back_count), ("259", 1, 1));
        let (line, row) = filed(Hit { plotter: Some((0x0A, 0, 2)), hit_type: 3, ..shard_hit() });
        assert_eq!((field(&line, "damage"), field(&line, "mods"), field(&line, "dir")), ("900", "0x0a", "0x02"));
        assert_eq!((row.parry_count, row.double_count, row.frontal_count, row.crit_count, row.back_count), (1, 1, 1, 1, 0));
        let (line, row) = filed(Hit { plotter: Some((0x00, 0, 3)), ..shard_hit() });
        assert_eq!((field(&line, "mods"), field(&line, "dir")), ("0x00", "0x03"));
        assert_eq!((row.back_count, row.frontal_count, row.crit_count), (0, 0, 0));

        let plotter = Plotter { flags: 0x0A, hp: 0, hp_len: 1, angle: Some(2) };
        assert_eq!(hit_marks(true, Some(&plotter)), [Parry, Double, Frontal, Critical]);
        assert_eq!(hit_marks(true, None), [Critical]);
        assert!(hit_marks(false, None).is_empty());
    }

    #[test]
    fn a_monster_s_miss_or_resist_is_damage_taken_of_0() {
        // The frame is not taken.
        let monster = Hit {
            target: 5_100,
            switch: 0x00,
            actor: 30_001,
            skill: 1_218_700,
            hit_type: 1,
            effect: 121_870_011,
            count: 1,
            values: vec![10_000, 1, 0],
            ..Hit::default()
        };
        for (hit_type, kind) in [(1, TakenKind::Miss), (6, TakenKind::Resist)] {
            let (storage, mut p) = processor("{}");
            assert!(!p.read_hit_record(&Hit { hit_type, ..monster.clone() }.packet(), false, false));
            let got: Vec<_> = storage.taken_since(0).into_iter().map(|t| t.hit).collect();
            assert_eq!(got, [TakenHit::plain(1_000, 5_100, 30_001, 1_218_700, 0, kind)]);
        }
        let (storage, mut p) = processor("{}");
        assert!(!p.read_hit_record(&Hit { hit_type: 2, ..monster }.packet(), false, false));
        assert!(storage.taken_since(0).is_empty());
    }

    #[test]
    fn a_player_s_miss_or_resist_counts_on_a_row_with_damage() {
        // It counts once its actor has damage on the target, which it leaves
        // as it was. The line names the row: the game files 17050240 under
        // 17050000.
        let hit = Hit {
            target: 40_001,
            switch: 0x04,
            actor: 1_500,
            skill: 17_050_240,
            uid: 0x21,
            hit_type: 2,
            effect: 1_705_024_016,
            count: 1,
            values: vec![10_400, 2_000, 1],
            ..Hit::default()
        };
        let resist = Hit { switch: 0x00, flag: 1, hit_type: 6, effect: 1_705_024_019, values: vec![10_400, 1], ..hit.clone() };
        let (storage, mut p) = processor("{}");
        assert!(p.read_hit_record(&hit.packet(), false, false));
        let (read, lines) = traced(&mut p, &resist.packet(), false);
        assert!(!read);
        assert_eq!(lines, ["1000 actor=1500 target=40001 skill=17050000 damage=- type=6 layout=0 counted=true"]);
        let row = &rows(&storage)[0].2;
        assert_eq!((row.resist_count, row.miss_count, row.total_damage, row.hit_count), (1, 0, 2_000, 1));

        let (storage, mut p) = processor("{}");
        let (read, lines) = traced(&mut p, &Hit { hit_type: 1, ..resist.clone() }.packet(), false);
        assert!(!read);
        assert_eq!(lines, ["1000 actor=1500 target=40001 skill=17050000 damage=- type=1 layout=0 counted=false"]);
        assert!(rows(&storage).is_empty());

        // Layout 2 reads as layout 0: nothing after the hit type.
        let (storage, mut p) = processor("{}");
        assert!(p.read_hit_record(&shard_hit().packet(), false, false));
        let two = Hit { switch: 0x02, plotter: None, hit_type: 1, values: vec![10_000, 1, 0], ..shard_hit() };
        let (read, lines) = traced(&mut p, &two.packet(), false);
        assert!(!read && lines[0].ends_with("damage=- type=1 layout=2 counted=true"));
        assert_eq!(rows(&storage)[0].2.miss_count, 1);

        // Nothing for hit type 2, for a record on its own actor, or in the scan.
        for (record, trusted) in [(Hit { hit_type: 2, ..resist.clone() }, false), (Hit { target: 1_500, ..resist.clone() }, false), (resist, true)] {
            let (storage, mut p) = processor("{}");
            assert!(p.read_hit_record(&hit.packet(), false, false));
            let bytes = if trusted { scanned(&[&record.fields()]) } else { record.packet() };
            let (read, lines) = traced(&mut p, &bytes, trusted);
            assert!(!read && lines.is_empty(), "{record:?}");
            let row = &rows(&storage)[0].2;
            assert_eq!((row.resist_count, row.miss_count), (0, 0));
        }
    }

    #[test]
    fn the_value_lies_a_fixed_skip_past_the_hit_type() {
        // The plotter must read whole.
        assert_eq!(hits_of(&[&shard_hit().fields()[..14]]), (false, vec![]));

        let line = line_of(&Hit { switch: 0x05, plotter: None, values: vec![7, 8, 9, 10, 10_500, 900], ..shard_hit() });
        assert_eq!((field(&line, "damage"), field(&line, "layout"), field(&line, "scalar")), ("900", "5", "-"));
        let line = line_of(&Hit { switch: 0x07, values: vec![7, 8, 9, 10_500, 900], ..shard_hit() });
        assert_eq!((field(&line, "damage"), field(&line, "layout"), field(&line, "mods")), ("900", "7", "0x80"));

        // Restored HP of two bytes moves the value one byte on.
        let (storage, mut p) = processor("{}");
        let healed = Hit { plotter: Some((0x20, 200, 2)), values: vec![10_000, 1_000], ..shard_hit() };
        let (_, lines) = traced(&mut p, &healed.packet(), false);
        assert!(lines[0].contains(" damage=1000 type=2 layout=6 mods=0x20 dir=0x02 multi=0 multi_dmg=0 hp=200 scalar=10000"), "{lines:?}");
        let row = &rows(&storage)[0].2;
        assert_eq!((row.regeneration_count, row.frontal_count), (1, 1));

        // The skip runs to the frame's end.
        let whole = shard_hit().fields();
        assert_eq!(hits_of(&[&whole[..whole.len() - 5]]), (false, vec![]));
    }

    #[test]
    fn a_leading_zero_moves_the_pair_one_varint_on() {
        let line = line_of(&Hit { values: vec![0, 10_500, 6_000, 1], ..plain() });
        assert_eq!((field(&line, "damage"), field(&line, "scalar")), ("6000", "10500"));
        // No third varint: no shift.
        let line = line_of(&Hit { values: vec![0, 6_000], ..plain() });
        assert_eq!((field(&line, "damage"), field(&line, "scalar")), ("6000", "-"));
        // A count whose top byte is not 0.
        let line = line_of(&Hit { count: 1 << 24, ..shard_hit() });
        assert_eq!(stored(&[line.clone()]), [(10_500, 0, 0)]);
        assert_eq!(field(&line, "scalar"), "-");
    }

    #[test]
    fn a_layout_6_crit_reads_speed_then_value_as_any_hit() {
        let read = |hit: Hit| {
            let line = line_of(&hit);
            (field(&line, "damage").to_string(), field(&line, "scalar").to_string())
        };
        let pair = |damage: &str, scalar: &str| (damage.to_string(), scalar.to_string());
        // A crit whose varints are `e0 5d 06` is speed 12,000 and 6 damage,
        // not 12,000 damage.
        assert_eq!(read(Hit { hit_type: 3, values: vec![12_000, 6], ..shard_hit() }), pair("6", "12000"));
        assert_eq!(read(Hit { hit_type: 3, values: vec![10_500, 3], ..shard_hit() }), pair("3", "10500"));
        assert_eq!(read(Hit { hit_type: 2, values: vec![10_500, 3], ..shard_hit() }), pair("3", "10500"));
        assert_eq!(read(Hit { hit_type: 3, values: vec![999, 3], ..shard_hit() }), pair("3", "999"));
        assert_eq!(read(Hit { hit_type: 3, values: vec![10_500, 26], ..shard_hit() }), pair("26", "10500"));
        assert_eq!(read(Hit { hit_type: 3, values: vec![10_500, 3, 1], ..plain() }), pair("3", "-"));
    }

    #[test]
    fn a_value_above_99_999_999_is_no_hit() {
        assert_eq!(hits_of(&[&Hit { values: vec![10_500, 100_000_000], ..shard_hit() }.fields()]), (false, vec![]));
        assert_eq!(hits_of(&[&Hit { values: vec![10_500, 99_999_999], ..shard_hit() }.fields()]), (true, vec![(99_999_999, 0, 0)]));
    }

    #[test]
    fn strict_tails_name_the_additional_hits() {
        let one = |hit: Hit| hits_of(&[&hit.fields()]).1;
        let (storage, mut p) = processor("{}");
        let twice = Hit { switch: 0x36, uid: 0x22, hit_type: 3, values: vec![10_500, 1_700, 2, 24, 24], ..shard_hit() };
        let (_, lines) = traced(&mut p, &twice.packet(), false);
        assert_eq!(stored(&lines), [(1_652, 2, 48)]);
        let row = &rows(&storage)[0].2;
        assert_eq!((row.back_count, row.crit_count, row.total_damage), (1, 1, 1_700));
        let one_more = Hit { switch: 0x34, skill: 16_300_000, uid: 0x23, plotter: None, effect: 1_630_000_011, values: vec![10_500, 6_000, 1, 1, 100], ..shard_hit() };
        assert_eq!(one(one_more), [(5_900, 1, 100)]);

        // A spirit's layout 4 has no field before its additional hits.
        assert_eq!(one(spirit_hit(0x24, 16_120_003, 0x24, 1_612_000_121, 2, &[10_500, 500, 1, 10, 2, 0])), [(490, 1, 10)]);
        let (storage, mut p) = processor("{}");
        let (_, lines) = traced(&mut p, &spirit_hit(0x24, 100_033, 0x25, 10_003_111, 1, &[10_500, 240, 3, 3, 3, 3, 1, 0]).packet(), false);
        assert_eq!(stored(&lines), [(231, 3, 9)]);
        assert_eq!(rows(&storage)[0].2.spec_flags, [false, false, true, false, false]);
        assert_eq!(one(spirit_hit(0x04, 16_120_003, 0x24, 1_612_000_121, 3, &[10_500, 450, 3, 0])), [(450, 0, 0)]);

        // Shapes that do not fit, read the older way.
        assert_eq!(one(Hit { switch: 0x26, values: vec![10_500, 100, 2, 50, 50], ..shard_hit() }), [(100, 2, 100)]);
        assert_eq!(one(Hit { values: vec![10_500, 500, 0], ..plain() }), [(500, 0, 0)]);
        assert_eq!(one(Hit { values: vec![10_500, 900, 9], ..shard_hit() }), [(900, 0, 0)]);
    }

    #[test]
    fn a_loose_tail_reads_a_count_of_25_at_most() {
        let after = |values: &[u32], rest: &[u8]| hits_of(&[&Hit { values: values.to_vec(), ..shard_hit() }.fields(), rest]);
        assert_eq!(after(&[10_500, 900, 26, 3, 100, 100, 100, 9], &[]), (true, vec![(600, 3, 300)]));
        assert_eq!(after(&[10_500, 900, 26, 30, 100, 9], &[]), (true, vec![(900, 0, 0)]));
        assert_eq!(after(&[10_500, 900, 26], &[0x02, 0x00, 0x05]), (true, vec![(900, 0, 0)]));
        // A count that does not finish, a negative one, a second that does
        // not finish.
        assert_eq!(after(&[10_500, 900], &[0x80]), (true, vec![(900, 0, 0)]));
        assert_eq!(after(&[10_500, 900], &[0x85, 0x80, 0x80, 0x80, 0x08]), (true, vec![(900, 0, 0)]));
        assert_eq!(after(&[10_500, 900, 26], &[0x80]), (true, vec![(900, 0, 0)]));
        // A marker is no count: the next record still follows it.
        let first = Hit { switch: 0x26, ..shard_hit() }.fields();
        let second = Hit { target: 40_002, ..shard_hit() }.fields();
        assert_eq!(hits_of(&[&first, &[0x01, 0x00], &second]), (true, vec![(900, 0, 0); 2]));
    }

    #[test]
    fn an_additional_hit_out_of_range_drops_their_damage() {
        let after = |switch, values: &[u32], rest: &[u8]| hits_of(&[&Hit { switch, values: values.to_vec(), ..shard_hit() }.fields(), rest]).1;
        assert_eq!(after(0x16, &[10_500, 900, 2, 100, 120, 9], &[]), [(680, 2, 220)]);
        assert_eq!(after(0x16, &[10_500, 900, 2, 100, 100, 9], &[]), [(700, 2, 200)]);
        assert_eq!(after(0x16, &[10_500, 900, 3, 100, 49, 7, 9], &[]), [(900, 1, 0)]);
        assert_eq!(after(0x16, &[10_500, 900, 2, 600_000, 7, 9], &[]), [(900, 0, 0)]);
        assert_eq!(after(0x16, &[10_500, 900_000, 1, 600_000, 9], &[]), [(300_000, 1, 600_000)]);
        // They stop at a marker, at `04 38` and at a varint that does not finish.
        assert_eq!(after(0x16, &[10_500, 900, 3, 100], &[0x03, 0x00, 0x05]), [(800, 1, 100)]);
        assert_eq!(after(0x16, &[10_500, 900, 3, 100], &[0x04, 0x38]), [(800, 1, 100)]);
        assert_eq!(after(0x16, &[10_500, 900, 2, 100], &[0x80]), [(800, 1, 100)]);
        // Switch 0x36 leaves one varint unread first.
        assert_eq!(after(0x36, &[10_500, 900, 7, 2, 100, 100, 9], &[]), [(700, 2, 200)]);
    }

    #[test]
    fn one_hit_written_for_a_count_on_switch_0x36_stands_for_them_all() {
        let after = |values: &[u32], rest: &[u8]| hits_of(&[&Hit { switch: 0x36, values: values.to_vec(), ..shard_hit() }.fields(), rest]).1;
        assert_eq!(after(&[10_500, 900, 7, 4, 100], &[0x03, 0x00, 0x05]), [(500, 4, 400)]);
        assert_eq!(after(&[10_500, 900, 7, 4, 100, 9], &[]), [(900, 1, 0)]);
        // A second value of ten such hits is their sum: the hit is one of them.
        assert_eq!(after(&[10_500, 1_000, 7, 9, 100], &[0x03, 0x00, 0x05]), [(100, 9, 900)]);
        assert_eq!(after(&[10_500, 1_010, 7, 9, 100], &[0x03, 0x00, 0x05]), [(110, 9, 900)]);
        // Not on another switch.
        let other = hits_of(&[&Hit { switch: 0x16, values: vec![10_500, 900, 4, 100], ..shard_hit() }.fields(), &[0x03, 0x00, 0x05]]).1;
        assert_eq!(other, [(800, 1, 100)]);
    }

    #[test]
    fn the_value_loses_only_damage_it_can_hold() {
        assert_eq!(hits_of(&[&Hit { values: vec![10_500, 900, 1, 2_000, 9], ..shard_hit() }.fields()]).1, [(900, 1, 2_000)]);
        let mut values = vec![10_500, 99_999_999, 0, 25];
        values.extend([99_999_999; 25]);
        assert_eq!(hits_of(&[&Hit { switch: 0x36, values, ..shard_hit() }.fields()]).1, [(99_999_999, 25, 2_499_999_975)]);
    }

    #[test]
    fn an_aggregate_hit_takes_its_compact_record_s_skill_once() {
        let context = |actor_id| Some(HeldTickSkill { actor_id, skill_raw: 16_040_000 });
        let sum = |values: &[u32]| Hit { skill: 99_745_942, values: values.to_vec(), ..shard_hit() }.packet();
        let read = |p: &mut PacketDecoder, packet: &[u8]| {
            let (_, lines) = traced(p, packet, false);
            (stored(&lines), field(&lines[0], "skill").to_string())
        };
        let three = sum(&[10_500, 1_000, 3, 100, 100, 100]);
        let (_, mut p) = processor("{}");
        p.pending_compact_skill_context = context(1_500);
        assert_eq!(read(&mut p, &three), (vec![(700, 3, 300)], "16040000".into()));
        assert!(p.pending_compact_skill_context.is_none());
        assert_eq!(read(&mut p, &three), (vec![(700, 3, 300)], "99745942".into()));
        // Another actor's context stays for its own hit.
        p.pending_compact_skill_context = context(1_600);
        assert_eq!(read(&mut p, &three), (vec![(700, 3, 300)], "99745942".into()));
        assert!(p.pending_compact_skill_context.is_some());
        // So does one a single additional hit does not use.
        p.pending_compact_skill_context = context(1_500);
        assert_eq!(read(&mut p, &sum(&[10_500, 1_000, 1, 100])), (vec![(900, 1, 100)], "99745942".into()));
        assert!(p.pending_compact_skill_context.is_some());

        // A crit on switch 0x36: 1,480 of its own and 220 in two additional
        // hits, 1,700 on the skill's row.
        let (storage, mut p) = processor("{}");
        p.pending_compact_skill_context = context(1_500);
        let crit = Hit { skill: 99_745_942, switch: 0x36, uid: 0x22, hit_type: 3, values: vec![10_500, 1_700, 0, 2, 100, 120], ..shard_hit() };
        assert_eq!(read(&mut p, &crit.packet()), (vec![(1_480, 2, 220)], "16040000".into()));
        let row = &rows(&storage)[0].2;
        assert_eq!((row.skill_code, row.total_damage, row.multi_hit_hits, row.multi_hit_damage), (16_040_000, 1_700, 2, 220));
    }

    /// An aggregate hit on its own actor is a heal under the same skill the
    /// hit takes: its compact record's.
    #[test]
    fn an_aggregate_heal_on_oneself_goes_under_the_compact_skill() {
        // 1500 on itself under 99745942: 10500, 1000, then three additional
        // hits of 100.
        let sum = [
            0x21, 0x04, 0x38, 0xDC, 0x0B, 0x16, 0x00, 0xDC, 0x0B, 0x96, 0x00, 0xF2, 0x05, 0x5C, 0x02, 0x80, 0x00,
            0x01, 0x11, 0x19, 0x9B, 0x5F, 0x01, 0x00, 0x00, 0x00, 0x84, 0x52, 0xE8, 0x07, 0x03, 0x64, 0x64, 0x64,
        ];
        let (storage, mut p) = processor("{}");
        storage.name_entity(1_500, "Ossery");
        p.pending_compact_skill_context = Some(HeldTickSkill { actor_id: 1_500, skill_raw: 16_040_000 });
        let (read, lines) = traced(&mut p, &sum, false);
        assert!(read && lines.is_empty() && rows(&storage).is_empty());
        assert!(p.pending_compact_skill_context.is_none());
        let all = storage.heals_between(0, 2_000);
        let heals: Vec<_> = all.iter().flat_map(|(a, s)| s.iter().map(move |(k, v)| (*a, k.0, k.1, v.total_heal))).collect();
        assert_eq!(heals, [(1_500, 16_040_000, false, 700)]);
    }

    #[test]
    fn a_heal_can_ride_on_a_hit() {
        let heals = |parts: &[&[u8]]| {
            let (storage, mut p) = processor("{}");
            assert!(p.read_hit_record(&frame(parts), false, false));
            rows(&storage).into_iter().map(|(target, _, row)| (target, row.heal_amount)).collect::<Vec<_>>()
        };
        let base = shard_hit().fields();
        assert_eq!(heals(&[&base, &[0x03, 0x00, 0xD2, 0x09]]), [(40_001, 1_234)]);
        assert_eq!(heals(&[&base, &[0x03, 0x00], &built::varint(10_000_000)]), [(40_001, 0)]);
        let other = Hit { target: 40_002, ..shard_hit() }.fields();
        assert_eq!(heals(&[&base, &[0x03, 0x00, 0x4D, 0x01, 0x00], &other]), [(40_001, 77), (40_002, 0)]);
    }

    #[test]
    fn the_scan_takes_only_a_known_skill_s_landed_hit() {
        let scan = |skills: &str, parts: &[&[u8]]| {
            let (_, mut p) = processor(skills);
            let (read, lines) = traced(&mut p, &scanned(parts), true);
            (read, stored(&lines))
        };
        let named = r#"{"16040000":"A"}"#;
        assert_eq!(scan(named, &[&shard_hit().fields()]), (true, vec![(900, 0, 0)]));
        let refused = [
            Hit { hit_type: 9, ..shard_hit() },
            Hit { hit_type: 0, ..shard_hit() },
            Hit { target: 1_500, ..shard_hit() },
            Hit { values: vec![10_500, 0], ..shard_hit() },
        ];
        for hit in refused {
            assert_eq!(scan(named, &[&hit.fields()]), (false, vec![]), "{hit:?}");
        }
        assert_eq!(scan("{}", &[&shard_hit().fields()]), (false, vec![]));
        assert_eq!(scan("{}", &[&Hit { skill: 3_000_017, ..shard_hit() }.fields()]), (true, vec![(900, 0, 0)]));
        let unnamed = Hit { target: 40_002, skill: 16_050_000, ..shard_hit() }.fields();
        assert_eq!(scan(named, &[&shard_hit().fields(), &[0x01, 0x00], &unnamed]), (true, vec![(900, 0, 0)]));
    }

    #[test]
    fn restores_and_hits_on_oneself_are_no_damage() {
        // A Water Spirit's MP for its master: taken, nothing filed.
        let water = Hit { target: 2_100, actor: 52_002, skill: 16_990_002, uid: 6, effect: 1_699_000_211, values: vec![10_500, 20, 1, 0], ..plain() };
        let (storage, mut p) = processor("{}");
        let (read, lines) = traced(&mut p, &water.packet(), false);
        assert!(read && lines.is_empty());
        assert!(rows(&storage).is_empty() && storage.heals_between(0, 2_000).is_empty());
        // A Wind Spirit's heal goes under the game's row for it.
        assert_eq!(field(&line_of(&Hit { actor: 52_001, skill: 16_990_003, ..water }), "skill"), "100031");

        // A heal on oneself: a known player's, over 1.
        let own = Hit { target: 2_100, actor: 2_100, skill: 17_010_000, values: vec![10_500, 1_500, 1, 0], ..plain() };
        let heals = |known: bool, hit: &Hit| {
            let (storage, mut p) = processor("{}");
            if known {
                storage.name_entity(2_100, "Ossery");
            }
            let (read, lines) = traced(&mut p, &hit.packet(), false);
            assert!(read && lines.is_empty());
            let all = storage.heals_between(0, 2_000);
            all.iter().flat_map(|(a, s)| s.iter().map(move |(k, v)| (*a, k.0, k.1, v.total_heal))).collect::<Vec<_>>()
        };
        assert_eq!(heals(true, &own), [(2_100, 17_010_000, false, 1_500)]);
        assert!(heals(true, &Hit { values: vec![10_500, 1, 1, 0], ..own.clone() }).is_empty());
        assert!(heals(false, &own).is_empty());
    }

    #[test]
    fn skill_codes_are_checked_and_filed_by_row() {
        assert_eq!([0, 1, 299_999_999, 300_000_000, -5].map(skill_code_in_range), [false, true, true, false, false]);
        let (_, p) = processor("{}");
        assert_eq!((p.fold_skill_code(16_990_003), p.fold_skill_code(16_040_000)), (100_031, 16_040_000));
        assert!(p.skill_in_table(30_000_171));
        for code in [16_040_000, 0, 300_000_000, 16_990_003] {
            assert!(!p.skill_in_table(code), "{code}");
        }
        let (_, p) = processor(r#"{"16040000":"X"}"#);
        assert_eq!(p.fold_skill_code(16_040_007), 16_040_000);
        assert!(p.skill_in_table(16_040_007));
        for table in [r#"{"100031":"W"}"#, r#"{"16990003":"W"}"#] {
            assert!(processor(table).1.skill_in_table(16_990_003), "{table}");
        }
        // A code whose row is 0.
        assert!(!processor(r#"{"0":"Z","5002":"Z"}"#).1.skill_in_table(5_002));
    }

    #[test]
    fn specialty_digits_set_the_spec_flags() {
        assert_eq!(specialty_flags(14_020_450), [false, false, false, true, true]);
        assert_eq!(specialty_flags(13_070_130), [true, false, true, false, false]);
        assert_eq!(specialty_flags(14_340_230), [false, true, true, false, false]);
        assert_eq!(specialty_flags(16_000_120), [true, true, false, false, false]);
        assert_eq!(specialty_flags(17_050_240), [false, true, false, true, false]);
        for raw in [16_001_109, 12_345_670, 16_000_009, 11_010_000, 30_000_171] {
            assert_eq!(specialty_flags(raw), [false; 5], "{raw}");
        }
        let (storage, mut p) = processor("{}");
        assert!(p.read_hit_record(&Hit { skill: 14_020_450, ..shard_hit() }.packet(), false, false));
        assert_eq!(rows(&storage)[0].2.spec_flags, [false, false, false, true, true]);
    }
}
