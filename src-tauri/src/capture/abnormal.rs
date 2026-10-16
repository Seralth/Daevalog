//! Buffs, debuffs and stats: the server's abnormal and stat records, and a
//! timeline built from them.
//!
//! ```text
//! <len> 2a 38 <entity> 01 <flags u8> <instance> <abnormal u32> <length i64> <end i64>
//!             <caster> <level u8> [flags & 2: <skill u32>] <u8> <x f32> <y f32> <z f32>   added
//! <len> 2b 38 <entity> <flags u8> <instance> ... the same from <abnormal> on              changed
//! <len> 2c 38 <entity> <n u8> (<kind u8> <instance> <reason u8>
//!             [kind 7: <by entity> <skill u32> <skill effect u32>])*                      removed
//! <len> 4a 36 <entity> <n u8> (<stat u16> <value i32>)* <8 bytes>                         stats changed
//! <len> 49 36 <2 bytes> <n u8> (<stat u16> <value i32>)* <8 bytes>                        all stats
//! <len> 41 36 | 45 36 | 33 36 <entity> ... <n u8> (<entry>)* 07 02 | 0f ...              on at spawn
//!   entry: <flags u8> <instance> <abnormal u32> <length i64> <end i64>
//!          [flags & 1: <caster>] <level u8> [flags & 2: <skill u32>] [<u8>] <x f32> <y f32> <z f32>
//! ```
//!
//! - `abnormal` is a `SkillAbnormal` id; `skill` the skill that applied it;
//!   `level` that skill's level (a passive's base + board + gear level).
//! - `instance` is numbered per entity. A stacking buff is one instance per
//!   stack: Element Unification at 5 stacks is 5 live instances. A stack
//!   past the limit (SkillAbnormal `AbnormalOverlapCount`) pushes out the
//!   oldest, which for Element no record ends. A debuff that several players
//!   keep up is one instance; a change names whoever applied it last.
//! - `end` is the server's clock in ms since the epoch (2100-01-01 00:00 in
//!   Korea when the abnormal never ends, length -1); `length` runs from the
//!   instance's start to `end`. A change that renews a stack keeps the start;
//!   one that applies the abnormal again restarts it (the byte before the
//!   position is then 2).
//! - Reason 1 is the timer running out; kind 7 (reason 11) is a skill taking
//!   the abnormal off, the skill named; reason 5 a passive replaced by its new
//!   level at login. Other reasons are not decoded.
//! - Stats are `EStat` ids; percent stats in hundredths. `4a 36` comes for
//!   your own character only, with the new value of each stat that changed.
//! - A spawn record (`41 36` a summon or monster, `45 36` another player) and
//!   your self record (`33 36`, at each map load) list every abnormal the
//!   entity has on, in the add layout. A spirit's list holds its passive at
//!   the level of its summon skill. See `listed`.
//!
//! Add, change and removal records read to their last byte with this layout.

use std::collections::{BTreeMap, HashMap};

use super::opcodes::{PLAYER_SPAWN, SELF_IDENTITY, SPAWN};
use super::varint::varint_at;

pub const ADDED: [u8; 2] = [0x2A, 0x38];
pub const CHANGED: [u8; 2] = [0x2B, 0x38];
pub const REMOVED: [u8; 2] = [0x2C, 0x38];
pub const STATS_CHANGED: [u8; 2] = [0x4A, 0x36];
pub const ALL_STATS: [u8; 2] = [0x49, 0x36];

/// The `end` of an abnormal that never ends: 2100-01-01 00:00 in Korea
/// (UTC+9).
pub(super) const NEVER: i64 = 4_102_412_400_000;
/// Bytes after the skill: one byte, then the caster's position (x, y, z
/// floats, near the caster, not the target).
const ADD_TAIL: usize = 13;
/// The field after the abnormal list of a spawn record, and of a player's.
const AFTER_SPAWN_LIST: &[u8] = &[0x07, 0x02];
const AFTER_PLAYER_LIST: &[u8] = &[0x0F];
/// The start of 2024 (UTC), before any timed abnormal in a list started.
const EARLIEST: i64 = 1_704_067_200_000;
/// Longer than any timed abnormal in a list lasts.
const LONGEST: i64 = 400 * 86_400_000;
const STATS_TAIL: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub entity: i32,
    pub instance: i32,
    pub abnormal: u32,
    /// From the instance's start to `end_ms`; `None` when it never ends.
    pub length_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub caster: i32,
    pub level: u8,
    pub skill: Option<u32>,
    /// A change that applied the abnormal again (its start moves).
    pub restarted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    pub instance: i32,
    pub reason: u8,
    /// Kind 7: who took it off, with which skill and skill effect.
    pub by: Option<(i32, u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
    Added(Applied),
    Changed(Applied),
    Removed { entity: i32, list: Vec<Removal> },
    /// `entity` is `None` for the whole sheet, which names nobody: it is yours.
    Stats { entity: Option<i32>, values: Vec<(u16, i32)> },
    /// Every abnormal on `entity` as it comes into view; `own` for your self
    /// record. A caster of 0 is one the entry does not name.
    Listed { entity: i32, own: bool, list: Vec<Applied> },
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn varint(&mut self) -> Option<i32> {
        let v = varint_at(self.b, self.at);
        if v.length <= 0 {
            return None;
        }
        self.at += v.length as usize;
        Some(v.value)
    }
    fn bytes<const N: usize>(&mut self) -> Option<[u8; N]> {
        let out = self.b.get(self.at..self.at + N)?.try_into().ok()?;
        self.at += N;
        Some(out)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.bytes::<1>()?[0])
    }
    fn left(&self) -> usize {
        self.b.len().saturating_sub(self.at)
    }
}

/// Read one framed packet (its length varint included). `None` for any other
/// packet, and for a record that does not end where the layout says.
pub fn parse(packet: &[u8]) -> Option<Record> {
    let len = varint_at(packet, 0);
    if len.length <= 0 {
        return None;
    }
    let o = len.length as usize;
    let op: [u8; 2] = packet.get(o..o + 2)?.try_into().ok()?;
    let mut r = Reader { b: packet, at: o + 2 };
    match op {
        ADDED | CHANGED => {
            let entity = r.varint()?;
            if op == ADDED && r.u8()? != 1 {
                return None;
            }
            let flags = r.u8()?;
            let instance = r.varint()?;
            let abnormal = u32::from_le_bytes(r.bytes()?);
            let length = i64::from_le_bytes(r.bytes()?);
            let end = i64::from_le_bytes(r.bytes()?);
            let caster = r.varint()?;
            let level = r.u8()?;
            let skill = if flags & 0x02 != 0 { Some(u32::from_le_bytes(r.bytes()?)) } else { None };
            let restarted = r.u8()? == 2;
            if r.left() != ADD_TAIL - 1 {
                return None;
            }
            let applied = Applied {
                entity,
                instance,
                abnormal,
                length_ms: (length >= 0).then_some(length),
                end_ms: (length >= 0 && end != NEVER).then_some(end),
                caster,
                level,
                skill,
                restarted,
            };
            Some(if op == ADDED { Record::Added(applied) } else { Record::Changed(applied) })
        }
        REMOVED => {
            let entity = r.varint()?;
            let n = r.u8()?;
            let mut list = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let kind = r.u8()?;
                let instance = r.varint()?;
                let reason = r.u8()?;
                let by = if kind & 0x07 == 0x07 {
                    let who = r.varint()?;
                    Some((who, u32::from_le_bytes(r.bytes()?), u32::from_le_bytes(r.bytes()?)))
                } else {
                    None
                };
                list.push(Removal { instance, reason, by });
            }
            (r.left() == 0).then_some(Record::Removed { entity, list })
        }
        STATS_CHANGED | ALL_STATS => {
            let entity = if op == STATS_CHANGED {
                Some(r.varint()?)
            } else {
                r.bytes::<2>()?;
                None
            };
            let n = r.u8()?;
            let mut values = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let stat = u16::from_le_bytes(r.bytes()?);
                values.push((stat, i32::from_le_bytes(r.bytes()?)));
            }
            (r.left() == STATS_TAIL).then_some(Record::Stats { entity, values })
        }
        SPAWN | PLAYER_SPAWN | SELF_IDENTITY => {
            let entity = r.varint()?;
            let after = if op == SPAWN { AFTER_SPAWN_LIST } else { AFTER_PLAYER_LIST };
            let list = (r.at..packet.len()).find_map(|at| listed(packet, at, entity, after))?;
            Some(Record::Listed { entity, own: op == SELF_IDENTITY, list })
        }
        _ => None,
    }
}

/// The abnormal list at `at`, if one is there: a count, then that many
/// entries in the add layout from the flags on. The caster is left out when
/// flags bit 0 is clear (only in `45 36`), and the byte before the position
/// is there in some entries and not in others; what decides it is not
/// known (the first entry nearly always has it, later ones mostly not). The
/// list sits behind fields of varying length, so it is found by trying each
/// start and both entry lengths, and kept only when exactly one reading has
/// every entry well formed and ends where `after` follows.
fn listed(b: &[u8], at: usize, entity: i32, after: &[u8]) -> Option<Vec<Applied>> {
    let n = *b.get(at)? as usize;
    if n == 0 || listed_entry(b, at + 1, entity).is_none() {
        return None;
    }
    let mut readings = vec![(Vec::with_capacity(n), at + 1)];
    for k in 0..n {
        let mut next = Vec::new();
        for (list, i) in &readings {
            let Some((entry, position)) = listed_entry(b, *i, entity) else { continue };
            for end in [position + 12, position + 13] {
                let fits = if k + 1 < n {
                    listed_entry(b, end, entity).is_some()
                } else {
                    b.get(end..end + after.len()) == Some(after)
                };
                if fits {
                    let mut list = list.clone();
                    list.push(entry.clone());
                    next.push((list, end));
                }
            }
        }
        if next.is_empty() || next.len() > 4 {
            return None;
        }
        readings = next;
    }
    (readings.len() == 1).then(|| readings.remove(0).0)
}

/// One list entry up to its position, and where the optional byte or the
/// position starts.
fn listed_entry(b: &[u8], at: usize, entity: i32) -> Option<(Applied, usize)> {
    let mut r = Reader { b, at };
    let flags = r.u8()?;
    if !matches!(flags, 0x10 | 0x11 | 0x13) {
        return None;
    }
    let instance = r.varint()?;
    let abnormal = u32::from_le_bytes(r.bytes()?);
    let length = i64::from_le_bytes(r.bytes()?);
    let end = i64::from_le_bytes(r.bytes()?);
    let endless = length == -1 && end == NEVER;
    let timed = (0..=LONGEST).contains(&length) && (EARLIEST..NEVER).contains(&end) && end - length >= EARLIEST;
    if !(1..1_000_000_000).contains(&abnormal) || !(endless || timed) {
        return None;
    }
    let caster = if flags & 0x01 != 0 { r.varint()? } else { 0 };
    let level = r.u8()?;
    let skill = if flags & 0x02 != 0 { Some(u32::from_le_bytes(r.bytes()?)) } else { None };
    let applied = Applied {
        entity,
        instance,
        abnormal,
        length_ms: timed.then_some(length),
        end_ms: timed.then_some(end),
        caster,
        level,
        skill,
        restarted: false,
    };
    Some((applied, r.at))
}

/// Why an abnormal ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// Its timer ran out (reason 1).
    Expired,
    /// A skill took it off (kind 7).
    TakenOff { skill: u32 },
    /// Removed for a reason not decoded.
    Removed(u8),
    /// Applied again by another caster: one debuff that several players
    /// keep up is one instance, its caster the last one to apply it.
    Recast,
    /// Its entity died or left the world (`42 36` flag 3 or 7).
    Gone,
    /// A map load: every entity but you is gone (`21 36`).
    MapLoad,
    /// Left out of a later list of everything on its entity (a spawn or
    /// self record) before its timer ran out: it ended unseen. A record
    /// about it can still come later.
    NotListed,
    /// A newer stack went past the abnormal's stack limit and pushed this,
    /// the oldest, out. For Element no record says so.
    PushedOut,
    /// No record ended it: closed at its timer.
    Unseen,
    /// Still on when the capture ended.
    Open,
}

impl End {
    pub fn label(&self) -> String {
        match self {
            End::Expired => "expired".into(),
            End::TakenOff { skill } => format!("taken off by skill {skill}"),
            End::Removed(5) => "replaced by a new level".into(),
            End::Removed(r) => format!("removed, reason {r}"),
            End::Recast => "applied again by another caster".into(),
            End::Gone => "entity gone".into(),
            End::MapLoad => "map load".into(),
            End::PushedOut => "pushed out past the stack limit".into(),
            End::NotListed => "not in a later spawn record".into(),
            End::Unseen => "no end seen, closed at its timer".into(),
            End::Open => "open".into(),
        }
    }
}

/// One instance of an abnormal on one entity, from its add to its end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    pub entity: i32,
    pub instance: i32,
    pub abnormal: u32,
    pub caster: i32,
    pub skill: Option<u32>,
    pub level: u8,
    /// A passive or other abnormal without a timer.
    pub endless: bool,
    pub start_ms: i64,
    /// `None` while open.
    pub end_ms: Option<i64>,
    pub end: End,
    /// When the server last said it ends, on the server's clock.
    pub server_end_ms: Option<i64>,
}

impl Instance {
    fn from(a: &Applied, start_ms: i64) -> Instance {
        Instance {
            entity: a.entity,
            instance: a.instance,
            abnormal: a.abnormal,
            caster: a.caster,
            skill: a.skill,
            level: a.level,
            endless: a.length_ms.is_none(),
            start_ms,
            end_ms: None,
            end: End::Open,
            server_end_ms: a.end_ms,
        }
    }
}

/// One stat record: the new values of the stats that changed (or all of them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatEvent {
    pub ms: i64,
    pub entity: Option<i32>,
    pub whole_sheet: bool,
    pub values: Vec<(u16, i32)>,
}

/// Each abnormal's stack limit, where more than one, from the game data's
/// `abnormals.json`.
pub fn stack_limits(data_dir: &std::path::Path) -> HashMap<u32, u32> {
    let table: serde_json::Value = std::fs::read_to_string(data_dir.join("abnormals.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    table["abnormals"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(id, a)| Some((id.parse().ok()?, a["stacks"].as_u64()? as u32)))
        .collect()
}

/// Every abnormal instance and stat record of a capture, in capture time.
#[derive(Default, Clone)]
pub struct Timeline {
    live: HashMap<(i32, i32), Instance>,
    pub instances: Vec<Instance>,
    pub stats: Vec<StatEvent>,
    map_loads: usize,
    /// The entity of the last `4a 36`: you.
    own: Option<i32>,
    /// Capture time minus server time at the last add: about 25-120 ms.
    lag_ms: i64,
    swept_ms: i64,
    /// Abnormal id -> the stacks one entity can hold, where more than one.
    stack_limits: HashMap<u32, u32>,
    /// Stacks pushed out at the time of the last read, as (entity,
    /// instance) and index in `instances`: a removal read at the same time
    /// says why instead.
    pushed: Vec<((i32, i32), usize)>,
    pushed_ms: i64,
}

impl Timeline {
    /// The stack limit of each abnormal that stacks (`abnormals.json`).
    pub fn set_stack_limits(&mut self, limits: HashMap<u32, u32>) {
        self.stack_limits = limits;
    }

    /// Read one framed packet received at `ms`.
    pub fn note(&mut self, ms: i64, packet: &[u8]) {
        let len = varint_at(packet, 0);
        if len.length <= 0 {
            return;
        }
        let o = len.length as usize;
        if ms >= self.swept_ms + 1_000 {
            self.swept_ms = ms;
            self.close_unseen(ms);
        }
        if ms != self.pushed_ms {
            self.pushed.clear();
            self.pushed_ms = ms;
        }
        match packet.get(o..o + 2) {
            // Your entity keeps its abnormals over a load; no record ends
            // anyone else's. A new id of yours comes just before the load.
            Some(op) if op == super::opcodes::MAP_LOAD => {
                self.map_loads += 1;
                let own = self.own;
                self.close_where(ms, End::MapLoad, |k| Some(k.0) != own);
                return;
            }
            Some(op) if op == super::opcodes::DEATH => {
                let mut r = Reader { b: packet, at: o + 2 };
                if let (Some(entity), Some(_), Some(flag)) = (r.varint(), r.varint(), r.varint())
                    && (flag == 3 || flag == 7)
                {
                    self.close_where(ms, End::Gone, |k| k.0 == entity);
                }
                return;
            }
            _ => {}
        }
        match parse(packet) {
            Some(Record::Added(a)) => {
                // A reused number ends the old instance first.
                if let Some(old) = self.live.remove(&(a.entity, a.instance)) {
                    self.close(old, ms, End::Removed(0));
                }
                if let (Some(length), Some(end)) = (a.length_ms, a.end_ms) {
                    self.lag_ms = (ms - (end - length)).clamp(0, 1_000);
                }
                if let Some(&limit) = self.stack_limits.get(&a.abnormal) {
                    self.push_out(ms, a.entity, a.abnormal, limit);
                }
                self.live.insert((a.entity, a.instance), Instance::from(&a, ms));
            }
            Some(Record::Changed(a)) => {
                let key = (a.entity, a.instance);
                if self.live.get(&key).is_some_and(|l| l.caster != a.caster) {
                    let mut next = self.live.remove(&key).unwrap();
                    self.close(next.clone(), ms, End::Recast);
                    next.caster = a.caster;
                    next.start_ms = ms;
                    self.live.insert(key, next);
                }
                self.seen(ms, &a);
            }
            Some(Record::Listed { entity, own, list }) => {
                if own {
                    self.own = Some(entity);
                }
                // The list is all the entity has on: the rest ended unseen,
                // at its timer if that ran out already.
                let gone: Vec<_> = self
                    .live
                    .keys()
                    .filter(|k| k.0 == entity && !list.iter().any(|a| a.instance == k.1))
                    .copied()
                    .collect();
                for k in gone {
                    let i = self.live.remove(&k).unwrap();
                    match i.server_end_ms.map(|e| e + self.lag_ms).filter(|&e| e <= ms) {
                        Some(at) => {
                            let at = at.max(i.start_ms);
                            self.close(i, at, End::Unseen);
                        }
                        None => self.close(i, ms, End::NotListed),
                    }
                }
                for a in &list {
                    // A number now used by another abnormal ends the old one.
                    let key = (entity, a.instance);
                    if self.live.get(&key).is_some_and(|l| l.abnormal != a.abnormal) {
                        let old = self.live.remove(&key).unwrap();
                        self.close(old, ms, End::Removed(0));
                    }
                    self.seen(ms, a);
                }
            }
            Some(Record::Removed { entity, list }) => {
                for r in list {
                    let end = match (r.reason, r.by) {
                        (_, Some((_, skill, _))) => End::TakenOff { skill },
                        (1, None) => End::Expired,
                        (reason, None) => End::Removed(reason),
                    };
                    if let Some(live) = self.live.remove(&(entity, r.instance)) {
                        self.close(live, ms, end);
                    } else if let Some(&(_, at)) = self.pushed.iter().find(|p| p.0 == (entity, r.instance)) {
                        self.instances[at].end = end;
                    }
                }
            }
            Some(Record::Stats { entity, values }) => {
                if entity.is_some() {
                    self.own = entity;
                }
                if !values.is_empty() {
                    self.stats.push(StatEvent { ms, entity, whole_sheet: entity.is_none(), values });
                }
            }
            None => {}
        }
    }

    /// An instance a change or a list names. One not seen yet has been on
    /// since before the capture (or before it came into view): it started
    /// `length` before its end.
    fn seen(&mut self, ms: i64, a: &Applied) {
        let key = (a.entity, a.instance);
        if !self.live.contains_key(&key) {
            let start = match (a.length_ms, a.end_ms) {
                (Some(length), Some(end)) => (end - length + self.lag_ms).min(ms),
                _ => ms,
            };
            self.live.insert(key, Instance::from(a, start));
        }
        if let Some(live) = self.live.get_mut(&key) {
            live.level = a.level;
            live.server_end_ms = a.end_ms;
            if a.skill.is_some() {
                live.skill = a.skill;
            }
        }
    }

    fn close(&mut self, mut i: Instance, ms: i64, end: End) {
        i.end_ms = Some(ms);
        i.end = end;
        self.instances.push(i);
    }

    fn close_where(&mut self, ms: i64, end: End, which: impl Fn(&(i32, i32)) -> bool) {
        let keys: Vec<_> = self.live.keys().filter(|k| which(k)).copied().collect();
        for k in keys {
            if let Some(i) = self.live.remove(&k) {
                self.close(i, ms, end);
            }
        }
    }

    /// End the oldest stacks of `abnormal` on `entity` that a new one would
    /// take past `limit`. Element (4 stacks) loses its oldest with no
    /// record: an Element pushed out this way is never named by a record
    /// again, and an Elemental Fusion that comes while it would still be on
    /// takes 4 other Elements, never it. A limit one lower is wrong: the
    /// stack it would push out gets records later.
    fn push_out(&mut self, ms: i64, entity: i32, abnormal: u32, limit: u32) {
        let mut stacks: Vec<(i64, i32)> = self
            .live
            .iter()
            .filter(|(k, i)| k.0 == entity && i.abnormal == abnormal)
            .map(|(k, i)| (i.start_ms, k.1))
            .collect();
        stacks.sort();
        let over = (stacks.len() + 1).saturating_sub(limit as usize);
        for &(_, instance) in &stacks[..over] {
            if let Some(i) = self.live.remove(&(entity, instance)) {
                self.pushed.push(((entity, instance), self.instances.len()));
                self.close(i, ms, End::PushedOut);
            }
        }
    }

    /// Instances 2 s past their end with no record ending them.
    fn close_unseen(&mut self, ms: i64) {
        let lag = self.lag_ms;
        let late: Vec<_> = self
            .live
            .iter()
            .filter_map(|(k, i)| i.server_end_ms.filter(|e| ms > e + lag + 2_000).map(|e| (*k, e + lag)))
            .collect();
        for (k, at) in late {
            if let Some(i) = self.live.remove(&k) {
                let at = at.max(i.start_ms);
                self.close(i, at, End::Unseen);
            }
        }
    }

    pub fn map_loads(&self) -> usize {
        self.map_loads
    }

    /// The instances on at any time from `from_ms` to `to_ms`, the ones
    /// still on included (open, no end), in start order.
    pub fn instances_between(&self, from_ms: i64, to_ms: i64) -> Vec<Instance> {
        let on = |i: &&Instance| i.start_ms <= to_ms && i.end_ms.is_none_or(|e| e >= from_ms);
        let mut out: Vec<Instance> = self.instances.iter().filter(on).cloned().collect();
        out.extend(self.live.values().filter(on).cloned());
        out.sort_by_key(|i| (i.start_ms, i.entity, i.instance));
        out
    }

    /// The stat records from `from_ms` to `to_ms`, and the whole sheet
    /// before `from_ms` that they change.
    pub fn stats_between(&self, from_ms: i64, to_ms: i64) -> Vec<StatEvent> {
        let base = self.stats.iter().rposition(|s| s.whole_sheet && s.ms < from_ms).unwrap_or(0);
        self.stats[base..].iter().filter(|s| s.ms <= to_ms).cloned().collect()
    }

    /// Forgets the instances that ended before `ms`, and the stat records
    /// before the last whole sheet ahead of `ms`.
    pub fn forget_before(&mut self, ms: i64) {
        self.instances.retain(|i| i.end_ms.is_none_or(|e| e >= ms));
        if let Some(base) = self.stats.iter().rposition(|s| s.whole_sheet && s.ms < ms) {
            self.stats.drain(..base);
        }
    }

    /// Every instance, the ones still on included (open, no end).
    pub fn all_instances(&self) -> Vec<Instance> {
        let mut out = self.instances.clone();
        out.extend(self.live.values().cloned());
        out.sort_by_key(|i| (i.start_ms, i.entity, i.instance));
        out
    }
}

/// An abnormal on one entity from one source, its stacks over time: the
/// instances of one abnormal id whose casters resolve to the same owner (a
/// spirit's to its summoner), joined while at least one is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub entity: i32,
    pub abnormal: u32,
    pub owner: i32,
    pub skills: Vec<u32>,
    pub level: u8,
    pub endless: bool,
    pub start_ms: i64,
    pub end_ms: Option<i64>,
    pub end: End,
    /// (time, live instances from then on); the last step is 0 unless open.
    pub stacks: Vec<(i64, u32)>,
}

/// Join instances into tracks. `owner` maps a caster to whoever stands
/// behind it.
pub fn tracks(instances: &[Instance], owner: impl Fn(i32) -> i32) -> Vec<Track> {
    let mut groups: BTreeMap<(i32, u32, i32), Vec<&Instance>> = BTreeMap::new();
    // One added and taken off in the same read was never on.
    for i in instances.iter().filter(|i| i.end_ms != Some(i.start_ms)) {
        groups.entry((i.entity, i.abnormal, owner(i.caster))).or_default().push(i);
    }
    let mut out = Vec::new();
    for ((entity, abnormal, owner), list) in groups {
        let mut events: Vec<(i64, i32, &Instance)> = Vec::new();
        for i in &list {
            events.push((i.start_ms, 1, i));
            if let Some(e) = i.end_ms {
                events.push((e, -1, i));
            }
        }
        events.sort_by_key(|(t, d, _)| (*t, *d));
        let mut live = 0i32;
        let mut current: Option<Track> = None;
        // All changes at one time count as one step, so a stack ended and
        // another started in the same read keeps the track going.
        for step in events.chunk_by(|a, b| a.0 == b.0) {
            let t = step[0].0;
            live = (live + step.iter().map(|e| e.1).sum::<i32>()).max(0);
            if current.is_none() && live == 0 {
                continue;
            }
            let track = current.get_or_insert_with(|| Track {
                entity,
                abnormal,
                owner,
                skills: Vec::new(),
                level: 0,
                endless: false,
                start_ms: t,
                end_ms: None,
                end: End::Open,
                stacks: Vec::new(),
            });
            for &(_, d, i) in step {
                if d > 0 {
                    if let Some(s) = i.skill.filter(|s| !track.skills.contains(s)) {
                        track.skills.push(s);
                    }
                    track.level = track.level.max(i.level);
                    track.endless |= i.endless;
                } else {
                    track.end = i.end;
                }
            }
            if track.stacks.last().is_none_or(|s| s.1 != live as u32) {
                track.stacks.push((t, live as u32));
            }
            if live == 0 {
                let mut done = current.take().unwrap();
                done.end_ms = Some(t);
                out.push(done);
            }
        }
        if let Some(mut open) = current {
            open.end = End::Open;
            out.push(open);
        }
    }
    out.sort_by_key(|t| (t.start_ms, t.entity, t.abnormal));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::built::{self, Abnormal, F, NEVER};

    // Records built from the field layout with made-up values: you are
    // `YOU`, with a Fire Spirit `SPIRIT`.
    const YOU: u32 = 2_800;
    const SPIRIT: u32 = 28_001;
    const OTHER_SPIRIT: u32 = 42_001;
    // An Elementalist, and the spirits that put Elements on them.
    const ELEMENTALIST: u32 = 11_000;
    const E1: u32 = 20_001;
    const E2: u32 = 20_002;
    const E3: u32 = 20_003;
    const E4: u32 = 20_004;
    // A boss, and three players who curse it.
    const BOSS: u32 = 40_041;
    const P1: u32 = 1_300;
    const P2: u32 = 1_400;
    const P3: u32 = 1_500;

    /// An abnormal in the add layout; its caster's position is made up.
    #[allow(clippy::too_many_arguments)]
    fn ab(flags: u8, instance: u32, abnormal: u32, length: i64, end: i64, caster: u32, level: u8, skill: Option<u32>, byte: Option<u8>) -> Abnormal {
        Abnormal { flags, instance, abnormal, length, end, caster, level, skill, byte, position: [-1500.5, 2500.25, 120.0] }
    }

    /// Spirit's Benediction (abnormal 161900001) from skill 16190000 at
    /// level 2, on for 10 s.
    fn benediction_on(on: u32) -> Vec<u8> {
        ab(0x13, if on == YOU { 300 } else { 5 }, 161_900_001, 10_000, 1_800_000_020_000, YOU, 2, Some(16_190_000), Some(0)).added(on)
    }
    /// Instance 300 taken off, reason 1.
    fn benediction_off() -> Vec<u8> {
        built::removed(YOU, &[(0x00, 300, 1, None)])
    }
    /// The stats it changes, on and off.
    fn stats_on() -> Vec<u8> {
        built::stats(YOU, &[(65, 1500), (76, 1000), (379, 3000), (380, 2000)])
    }
    fn stats_off() -> Vec<u8> {
        built::stats(YOU, &[(65, 500), (76, 0), (379, 1000), (380, 0)])
    }
    /// `42 36 <entity> 00 07`: it left the world.
    fn gone(entity: u32) -> Vec<u8> {
        built::packet([0x42, 0x36], &[F::V(entity), F::V(0), F::V(7)])
    }

    #[test]
    fn an_added_buff_names_its_skill_level_and_end() {
        let Some(Record::Added(a)) = parse(&benediction_on(YOU)) else { panic!() };
        assert_eq!(
            a,
            Applied {
                entity: YOU as i32,
                instance: 300,
                abnormal: 161_900_001,
                length_ms: Some(10_000),
                end_ms: Some(1_800_000_020_000),
                caster: YOU as i32,
                level: 2,
                skill: Some(16_190_000),
                restarted: false,
            }
        );
        assert_eq!(
            parse(&benediction_off()),
            Some(Record::Removed { entity: YOU as i32, list: vec![Removal { instance: 300, reason: 1, by: None }] })
        );
    }

    /// Spirit Strike (167100001) at login, at
    /// `level`.
    fn spirit_strike(instance: u32, level: u8) -> Vec<u8> {
        ab(0x11, instance, 167_100_001, -1, NEVER, ELEMENTALIST, level, None, Some(0)).added(ELEMENTALIST)
    }

    #[test]
    fn a_passive_never_ends_and_has_no_skill() {
        let Some(Record::Added(a)) = parse(&spirit_strike(7, 1)) else { panic!() };
        assert_eq!((a.entity, a.instance, a.abnormal, a.length_ms, a.end_ms, a.skill), (ELEMENTALIST as i32, 7, 167_100_001, None, None, None));
    }

    #[test]
    fn a_renewed_stack_keeps_its_start_and_a_reapplied_one_restarts() {
        // Element Unification (167800011) stack 250 renewed by a new stack,
        // 12.5 s after its start.
        let renewed = ab(0x13, 250, 167_800_011, 12_500, 1_800_000_022_000, YOU, 2, Some(16_780_001), Some(0)).changed(YOU);
        let Some(Record::Changed(c)) = parse(&renewed) else { panic!() };
        assert_eq!((c.instance, c.abnormal, c.length_ms, c.level, c.restarted), (250, 167_800_011, Some(12_500), 2, false));
        // The Dimensional Control trigger (163300000) applied again by a
        // spirit's skill.
        let again = ab(0x13, 240, 163_300_000, 3000, 1_800_000_013_000, OTHER_SPIRIT, 1, Some(16_107_000), Some(2)).changed(YOU);
        let Some(Record::Changed(c)) = parse(&again) else { panic!() };
        assert_eq!((c.instance, c.abnormal, c.length_ms, c.caster, c.restarted), (240, 163_300_000, Some(3_000), OTHER_SPIRIT as i32, true));
    }

    /// Elemental Fusion (16300002) by `entity` takes off these Elements.
    fn fusion(entity: u32, instances: [u32; 4]) -> Vec<u8> {
        let by = Some((entity, 16_300_002, 1_630_000_221));
        built::removed(entity, &instances.map(|i| (0x07, i, 11, by)))
    }

    #[test]
    fn a_skill_takes_off_four_elements_at_once() {
        // Elemental Fusion (16300002) uses up four Elements.
        let Some(Record::Removed { entity, list }) = parse(&fusion(YOU, [271, 262, 253, 244])) else { panic!() };
        assert_eq!(entity, YOU as i32);
        let taken: Vec<_> = list.iter().map(|r| (r.instance, r.reason, r.by)).collect();
        let by = Some((YOU as i32, 16_300_002, 1_630_000_221));
        assert_eq!(taken, [(271, 11, by), (262, 11, by), (253, 11, by), (244, 11, by)]);
    }

    #[test]
    fn stat_records_carry_the_new_values() {
        // Spirit's Benediction: PvP and PvE Damage Boost and Tolerance, the
        // values of the game's StatChange rows (+10%, +10%, +20%, +20%).
        assert_eq!(
            parse(&stats_on()),
            Some(Record::Stats { entity: Some(YOU as i32), values: vec![(65, 1500), (76, 1000), (379, 3000), (380, 2000)] })
        );
        assert_eq!(
            parse(&stats_off()),
            Some(Record::Stats { entity: Some(YOU as i32), values: vec![(65, 500), (76, 0), (379, 1000), (380, 0)] })
        );
    }

    #[test]
    fn a_record_cut_short_or_running_long_is_not_read() {
        let mut long = benediction_on(YOU);
        long.push(0);
        long[0] += 1;
        assert_eq!(parse(&long), None);
        let mut short = benediction_off();
        short.pop();
        short[0] -= 1;
        assert_eq!(parse(&short), None);
    }

    #[test]
    fn the_timeline_ends_a_spirits_buff_when_it_leaves() {
        let mut t = Timeline::default();
        t.note(1_000, &benediction_on(YOU));
        // The same buff on the spirit, then its `42 36` flag 7.
        t.note(1_000, &benediction_on(SPIRIT));
        t.note(2_000, &gone(SPIRIT));
        t.note(11_000, &benediction_off());
        let ends: Vec<_> = t.all_instances().iter().map(|i| (i.entity, i.start_ms, i.end_ms, i.end)).collect();
        assert_eq!(ends, [(YOU as i32, 1_000, Some(11_000), End::Expired), (SPIRIT as i32, 1_000, Some(2_000), End::Gone)]);
    }

    /// The boss: one Curse (161400005), instance 290,
    /// applied by `caster` with `skill` at `level`, ending at `end`.
    fn curse(added: bool, caster: u32, level: u8, skill: u32, end: i64) -> Vec<u8> {
        let a = ab(0x13, 290, 161_400_005, 7000, end, caster, level, Some(skill), Some(if added { 0 } else { 2 }));
        if added { a.added(BOSS) } else { a.changed(BOSS) }
    }

    #[test]
    fn a_debuff_kept_up_by_several_players_changes_hands() {
        // The Curse applied by one player, then by the local player, then by
        // a third.
        let mut t = Timeline::default();
        t.note(11_000, &curse(true, P1, 10, 16_140_010, 1_800_000_018_000));
        t.note(33_300, &curse(false, P2, 12, 16_140_140, 1_800_000_040_300));
        t.note(34_800, &curse(false, P3, 12, 16_140_120, 1_800_000_041_800));
        let got: Vec<_> = t.all_instances().iter().map(|i| (i.instance, i.caster, i.start_ms, i.end_ms, i.end)).collect();
        assert_eq!(
            got,
            [
                (290, P1 as i32, 11_000, Some(33_300), End::Recast),
                (290, P2 as i32, 33_300, Some(34_800), End::Recast),
                (290, P3 as i32, 34_800, None, End::Open),
            ]
        );
        // Your share of it, and the track of everyone's Curse together.
        let mine = tracks(&t.all_instances(), |c| c);
        assert_eq!(mine.iter().find(|t| t.owner == P2 as i32).map(|t| (t.start_ms, t.end_ms)), Some((33_300, Some(34_800))));
        let all = tracks(&t.all_instances(), |_| 0);
        assert_eq!(all.iter().map(|t| (t.start_ms, t.end_ms, t.stacks.clone())).collect::<Vec<_>>(), [(11_000, None, vec![(11_000, 1)])]);
    }

    #[test]
    fn a_change_to_an_abnormal_never_seen_added_dates_it_from_its_length() {
        // A Curse change alone, read 50 ms after its start: 7 s long, so it
        // started 7 s before its end on the server's clock.
        let mut t = Timeline::default();
        t.note(1_800_000_033_350, &curse(false, P2, 12, 16_140_140, 1_800_000_040_300));
        let i = &t.all_instances()[0];
        assert_eq!((i.instance, i.caster, i.start_ms, i.server_end_ms), (290, P2 as i32, 1_800_000_033_300, Some(1_800_000_040_300)));
    }

    #[test]
    fn a_map_load_ends_every_abnormal_but_yours() {
        let mut t = Timeline::default();
        t.note(1_000, &stats_on());
        t.note(1_000, &benediction_on(YOU));
        t.note(1_000, &benediction_on(SPIRIT));
        // A map load: map 1010, the third load of the session.
        t.note(1_500, &built::map_load(3, 1010, 2, 0x00));
        let ends: Vec<_> = t.all_instances().iter().map(|i| (i.entity, i.end_ms, i.end)).collect();
        assert_eq!(ends, [(YOU as i32, None, End::Open), (SPIRIT as i32, Some(1_500), End::MapLoad)]);
    }

    /// An Element (163000003) on the Elementalist, 30 s, from a
    /// spirit's skill.
    fn element(instance: u32, end: i64, caster: u32) -> Vec<u8> {
        ab(0x13, instance, 163_000_003, 30_000, end, caster, 1, Some(16_107_000), Some(0)).added(ELEMENTALIST)
    }

    #[test]
    fn a_stack_pushed_out_without_a_record_closes_at_its_timer() {
        // An Element, 30 s. A fifth and sixth Element came before it ended;
        // four newer ones were used up by Elemental Fusion, and no record
        // ended this one.
        let added = 1_800_000_100_500;
        let mut t = Timeline::default();
        t.note(added, &element(110, 1_800_000_130_400, E1));
        t.note(added + 33_000, &benediction_off());
        let i = &t.all_instances()[0];
        // Its server end plus the 100 ms the add came late.
        assert_eq!((i.instance, i.end_ms, i.end), (110, Some(1_800_000_130_500), End::Unseen));
    }

    #[test]
    fn a_fifth_element_pushes_out_the_oldest() {
        // Six Elements (163000003, 4 stacks), each added 100, 60, 90, 80,
        // 70 and 50 ms after its start on the server's clock, then Elemental
        // Fusion takes the four newest.
        let records = [
            (1_800_000_200_000, element(110, 1_800_000_229_900, E1)),
            (1_800_000_205_000, element(120, 1_800_000_234_940, E1)),
            (1_800_000_216_000, element(140, 1_800_000_245_910, E2)),
            (1_800_000_219_000, element(150, 1_800_000_248_920, E2)),
            (1_800_000_226_000, element(160, 1_800_000_255_930, E3)),
            (1_800_000_227_000, element(170, 1_800_000_256_950, E4)),
            (1_800_000_227_100, fusion(ELEMENTALIST, [170, 140, 160, 150])),
        ];
        let read = |limits: HashMap<u32, u32>| {
            let mut t = Timeline::default();
            t.set_stack_limits(limits);
            for (ms, record) in &records {
                t.note(*ms, record);
            }
            t.note(1_800_000_270_000, &benediction_off());
            t.all_instances().iter().map(|i| (i.instance, i.end_ms.unwrap(), i.end)).collect::<Vec<_>>()
        };
        let fusion = End::TakenOff { skill: 16_300_002 };
        assert_eq!(
            read(HashMap::from([(163_000_003, 4)])),
            [
                (110, 1_800_000_226_000, End::PushedOut),
                (120, 1_800_000_227_000, End::PushedOut),
                (140, 1_800_000_227_100, fusion),
                (150, 1_800_000_227_100, fusion),
                (160, 1_800_000_227_100, fusion),
                (170, 1_800_000_227_100, fusion),
            ]
        );
        // Without the limit the two oldest stay on to their timers (plus the
        // 50 ms the last add came late).
        let unlimited = read(HashMap::new());
        assert_eq!(unlimited[..2], [(110, 1_800_000_229_950, End::Unseen), (120, 1_800_000_234_990, End::Unseen)]);
    }

    #[test]
    fn a_removal_read_with_the_push_says_why() {
        // At login: Spirit Strike (167100001) at level 1, 3 and 5; each new
        // level comes with a removal of the last, reason 5, in the same read.
        let mut t = Timeline::default();
        t.set_stack_limits(HashMap::from([(167_100_001, 1)]));
        let replaced = |instances: &[u32]| built::removed(ELEMENTALIST, &instances.iter().map(|&i| (0x00, i, 5, None)).collect::<Vec<_>>());
        for (ms, record) in [
            (700, spirit_strike(7, 1)),
            (800, spirit_strike(17, 3)),
            (800, replaced(&[14, 11, 9, 7])),
            (900, spirit_strike(27, 5)),
            (900, replaced(&[13, 12, 10, 18, 17])),
        ] {
            t.note(ms, &record);
        }
        let ends: Vec<_> = t.all_instances().iter().map(|i| (i.instance, i.level, i.end_ms, i.end)).collect();
        assert_eq!(ends, [(7, 1, Some(800), End::Removed(5)), (17, 3, Some(900), End::Removed(5)), (27, 5, None, End::Open)]);
    }

    /// The Fire Spirit spawns (`41 36`).
    fn spirit_spawn() -> Vec<u8> {
        built::spirit_spawn(SPIRIT, YOU)
    }

    #[test]
    fn a_spirit_spawn_lists_its_passive_at_the_summon_level() {
        // Fire Spirit (161002304) from you: its passive comes at the level
        // of your Summon: Fire Spirit, 12.
        let Some(Record::Listed { entity, own, list }) = parse(&spirit_spawn()) else { panic!() };
        assert_eq!((entity, own), (SPIRIT as i32, false));
        let passive = Applied {
            entity: SPIRIT as i32,
            instance: 1,
            abnormal: 161_002_304,
            length_ms: None,
            end_ms: None,
            caster: YOU as i32,
            level: 12,
            skill: None,
            restarted: false,
        };
        assert_eq!(list, [passive]);
    }

    #[test]
    fn a_player_record_lists_buffs_with_and_without_a_caster() {
        // A player's record (`45 36`) cut to its
        // entity and its list: passives, two 5-minute scrolls that name no
        // caster, and a 30 s buff from a skill. The first entry has the byte
        // before the position, the second has none.
        const PLAYER: u32 = 3_300;
        let passive = |instance, abnormal, level, byte| ab(0x11, instance, abnormal, -1, NEVER, PLAYER, level, None, byte);
        let scroll = |instance, abnormal, end| Abnormal { position: [f32::MAX; 3], ..ab(0x10, instance, abnormal, 300_000, end, 0, 1, None, None) };
        let list = [
            passive(4, 177_200_001, 1, Some(0)),
            passive(7, 177_500_001, 1, None),
            scroll(9, 22_101_031, 1_800_000_300_000),
            scroll(10, 22_101_051, 1_800_000_340_000),
            passive(11, 177_100_001, 5, None),
            passive(12, 177_300_001, 2, None),
            passive(14, 177_400_001, 2, None),
            ab(0x13, 15, 170_900_001, 30_000, 1_800_000_210_000, PLAYER, 3, Some(17_090_000), None),
        ];
        let mut fields = vec![list.len() as u8];
        for a in &list {
            fields.extend(a.fields());
        }
        fields.push(0x0F);
        let player = built::packet([0x45, 0x36], &[F::V(PLAYER), F::Raw(&fields)]);
        let Some(Record::Listed { entity, own, list }) = parse(&player) else { panic!() };
        assert_eq!((entity, own), (PLAYER as i32, false));
        let got: Vec<_> = list.iter().map(|a| (a.instance, a.abnormal, a.length_ms, a.caster, a.level, a.skill)).collect();
        let p = PLAYER as i32;
        assert_eq!(
            got,
            [
                (4, 177_200_001, None, p, 1, None),
                (7, 177_500_001, None, p, 1, None),
                (9, 22_101_031, Some(300_000), 0, 1, None),
                (10, 22_101_051, Some(300_000), 0, 1, None),
                (11, 177_100_001, None, p, 5, None),
                (12, 177_300_001, None, p, 2, None),
                (14, 177_400_001, None, p, 2, None),
                (15, 170_900_001, Some(30_000), p, 3, Some(17_090_000)),
            ]
        );
    }

    #[test]
    fn a_spawn_record_starts_what_it_lists_and_ends_what_it_leaves_out() {
        // Spirit's Benediction on the spirit, then a spawn record of the
        // spirit that lists only the passive: the buff ends there.
        let mut t = Timeline::default();
        t.note(1_000, &benediction_on(SPIRIT));
        t.note(3_000, &spirit_spawn());
        let got: Vec<_> = t.all_instances().iter().map(|i| (i.instance, i.abnormal, i.level, i.start_ms, i.end_ms, i.end)).collect();
        assert_eq!(got, [(5, 161_900_001, 2, 1_000, Some(3_000), End::NotListed), (1, 161_002_304, 12, 3_000, None, End::Open)]);
        // A record that lists the same again keeps the start.
        t.note(4_000, &spirit_spawn());
        assert_eq!(t.all_instances().last().map(|i| (i.start_ms, i.end)), Some((3_000, End::Open)));
    }

    #[test]
    fn stacks_join_into_one_track_per_owner() {
        let inst = |instance, caster, start, end: Option<i64>| Instance {
            entity: 1,
            instance,
            abnormal: 167_800_011,
            caster,
            skill: Some(16_780_001),
            level: 2,
            endless: false,
            start_ms: start,
            end_ms: end,
            end: if end.is_some() { End::Expired } else { End::Open },
            server_end_ms: None,
        };
        // Caster 9 is a spirit of player 1.
        let owner = |c| if c == 9 { 1 } else { c };
        let list = [
            inst(1, 1, 0, Some(10)),
            inst(2, 9, 2, Some(10)),
            inst(3, 1, 5, Some(10)),
            inst(4, 1, 20, None),
            inst(5, 7, 3, Some(4)),
        ];
        let t = tracks(&list, owner);
        let got: Vec<_> = t.iter().map(|t| (t.owner, t.start_ms, t.end_ms, t.stacks.clone())).collect();
        assert_eq!(
            got,
            [
                (1, 0, Some(10), vec![(0, 1), (2, 2), (5, 3), (10, 0)]),
                (7, 3, Some(4), vec![(3, 1), (4, 0)]),
                (1, 20, None, vec![(20, 1)]),
            ]
        );
    }
}
