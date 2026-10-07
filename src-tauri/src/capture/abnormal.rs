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
//! ```
//!
//! - `abnormal` is a `SkillAbnormal` id; `skill` the skill that applied it;
//!   `level` that skill's level (a passive's base + board + gear level).
//! - `instance` is numbered per entity. A stacking buff is one instance per
//!   stack: Element Unification at 5 stacks is 5 live instances. A debuff
//!   that several players keep up is one instance; a change names whoever
//!   applied it last.
//! - `end` is the server's clock in ms since the epoch (2100-01-01 when the
//!   abnormal never ends, length -1); `length` runs from the instance's start
//!   to `end`. A change that renews a stack keeps the start; one that applies
//!   the abnormal again restarts it (the byte before the position is then 2).
//! - Reason 1 is the timer running out; kind 7 (reason 11) is a skill taking
//!   the abnormal off, the skill named; reason 5 a passive replaced by its new
//!   level at login. Other reasons are not decoded.
//! - Stats are `EStat` ids; percent stats in hundredths. `4a 36` comes for
//!   your own character only, with the new value of each stat that changed.
//!
//! Every add, change and removal record in 19 captures of 2026-10-04 to
//! 2026-10-06 (336,702 adds and changes, 57,886 removals) reads to its last
//! byte with this layout.

use std::collections::{BTreeMap, HashMap};

use super::varint::read_varint;

pub const ADDED: [u8; 2] = [0x2A, 0x38];
pub const CHANGED: [u8; 2] = [0x2B, 0x38];
pub const REMOVED: [u8; 2] = [0x2C, 0x38];
pub const STATS_CHANGED: [u8; 2] = [0x4A, 0x36];
pub const ALL_STATS: [u8; 2] = [0x49, 0x36];

/// The `end` of an abnormal that never ends: 2100-01-01.
const NEVER: i64 = 4_102_444_800_000;
/// Bytes after the skill: one byte, then the entity's position.
const ADD_TAIL: usize = 13;
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
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn varint(&mut self) -> Option<i32> {
        let v = read_varint(self.b, self.at);
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
    let len = read_varint(packet, 0);
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
        _ => None,
    }
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
    /// No record ended it: closed at its timer. A stack pushed out by a
    /// newer one past the stack limit ends this way (Element, twice on
    /// 2026-10-05), earlier than its timer.
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

/// Every abnormal instance and stat record of a capture, in capture time.
#[derive(Default)]
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
}

impl Timeline {
    /// Read one framed packet received at `ms`.
    pub fn note(&mut self, ms: i64, packet: &[u8]) {
        let len = read_varint(packet, 0);
        if len.length <= 0 {
            return;
        }
        let o = len.length as usize;
        if ms >= self.swept_ms + 1_000 {
            self.swept_ms = ms;
            self.close_unseen(ms);
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
                // On since before the capture: it started `length` before its end.
                if !self.live.contains_key(&key) {
                    let start = match (a.length_ms, a.end_ms) {
                        (Some(length), Some(end)) => (end - length + self.lag_ms).min(ms),
                        _ => ms,
                    };
                    self.live.insert(key, Instance::from(&a, start));
                }
                if let Some(live) = self.live.get_mut(&key) {
                    live.level = a.level;
                    live.server_end_ms = a.end_ms;
                    if a.skill.is_some() {
                        live.skill = a.skill;
                    }
                }
            }
            Some(Record::Removed { entity, list }) => {
                for r in list {
                    if let Some(live) = self.live.remove(&(entity, r.instance)) {
                        let end = match (r.reason, r.by) {
                            (_, Some((_, skill, _))) => End::TakenOff { skill },
                            (1, None) => End::Expired,
                            (reason, None) => End::Removed(reason),
                        };
                        self.close(live, ms, end);
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

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    // Records from a capture of 2026-10-06 (the local player is entity 6759,
    // two spirits 71747 and 28113), each named by its capture time.

    /// 21:58:30.225: Spirit's Benediction (abnormal 161900001) from skill
    /// 16190000 at level 3, on for 10 s.
    const BENEDICTION_ON: &str = "342a38e73401138202e165a6091027000000000000bc5cba14a1010000e73403300af7000047e44c47b46a22c700e85a46";
    /// 21:58:40.267: instance 258 taken off, reason 1, 10.04 s later.
    const BENEDICTION_OFF: &str = "0d2c38e7340100820201";
    /// 21:58:30.225 and 21:58:40.267: the stats it changes, on and off.
    const STATS_ON: &str = "294a36e7340441006c0700004c00e80300007b01041000007c01d00700000000000000000000";
    const STATS_OFF: &str = "294a36e734044100840300004c00000000007b01340800007c01000000000000000000000000";

    #[test]
    fn an_added_buff_names_its_skill_level_and_end() {
        let Some(Record::Added(a)) = parse(&hex(BENEDICTION_ON)) else { panic!() };
        assert_eq!(
            a,
            Applied {
                entity: 6759,
                instance: 258,
                abnormal: 161_900_001,
                length_ms: Some(10_000),
                end_ms: Some(1_791_349_120_188),
                caster: 6759,
                level: 3,
                skill: Some(16_190_000),
                restarted: false,
            }
        );
        assert_eq!(
            parse(&hex(BENEDICTION_OFF)),
            Some(Record::Removed { entity: 6759, list: vec![Removal { instance: 258, reason: 1, by: None }] })
        );
    }

    #[test]
    fn a_passive_never_ends_and_has_no_skill() {
        // 2026-10-05 14:57:00.695: Spirit Strike (167100001) at login, level 1.
        let Some(Record::Added(a)) =
            parse(&hex("2f2a38a15801110661bef509ffffffffffffffff8075d52abb030000a15801005e71f0c7196dd6c700dcbf46"))
        else {
            panic!()
        };
        assert_eq!((a.entity, a.instance, a.abnormal, a.length_ms, a.end_ms, a.skill), (11297, 6, 167_100_001, None, None, None));
    }

    #[test]
    fn a_renewed_stack_keeps_its_start_and_a_reapplied_one_restarts() {
        // 21:58:32.070: Element Unification (167800011) stack 254 renewed by a
        // new stack, 12.75 s after its start; stack 269 added.
        let Some(Record::Changed(c)) =
            parse(&hex("332b38e73413fe01cb6c000ace31000000000000f663ba14a1010000e73402e10a00010047e44c47b46a22c700e85a46"))
        else {
            panic!()
        };
        assert_eq!((c.instance, c.abnormal, c.length_ms, c.level, c.restarted), (254, 167_800_011, Some(12_750), 2, false));
        // 21:58:30.172: the Dimensional Control trigger (163300000) applied
        // again by a spirit's skill.
        let Some(Record::Changed(c)) =
            parse(&hex("342b38e73413f601a0c2bb09b80b0000000000003241ba14a1010000fcca0201f8c5f50002b2364a47e3ae1ec709e15b46"))
        else {
            panic!()
        };
        assert_eq!((c.instance, c.abnormal, c.length_ms, c.caster, c.restarted), (246, 163_300_000, Some(3_000), 42364, true));
    }

    #[test]
    fn a_skill_takes_off_four_elements_at_once() {
        // 21:58:33.217: Elemental Fusion (16300002) uses up four Elements.
        let Some(Record::Removed { entity, list }) = parse(&hex(
            "412c38e734040791020be734e2b7f8005dd427610780020be734e2b7f8005dd4276107f9010be734e2b7f8005dd4276107f5010be734e2b7f8005dd42761",
        )) else {
            panic!()
        };
        assert_eq!(entity, 6759);
        let taken: Vec<_> = list.iter().map(|r| (r.instance, r.reason, r.by)).collect();
        let by = Some((6759, 16_300_002, 1_630_000_221));
        assert_eq!(taken, [(273, 11, by), (256, 11, by), (249, 11, by), (245, 11, by)]);
    }

    #[test]
    fn stat_records_carry_the_new_values() {
        // Spirit's Benediction: PvP and PvE Damage Boost and Tolerance, the
        // values of the game's StatChange rows (+10%, +10%, +20%, +20%).
        assert_eq!(
            parse(&hex(STATS_ON)),
            Some(Record::Stats { entity: Some(6759), values: vec![(65, 1900), (76, 1000), (379, 4100), (380, 2000)] })
        );
        assert_eq!(
            parse(&hex(STATS_OFF)),
            Some(Record::Stats { entity: Some(6759), values: vec![(65, 900), (76, 0), (379, 2100), (380, 0)] })
        );
    }

    #[test]
    fn a_record_cut_short_or_running_long_is_not_read() {
        let mut long = hex(BENEDICTION_ON);
        long.push(0);
        long[0] += 1;
        assert_eq!(parse(&long), None);
        let mut short = hex(BENEDICTION_OFF);
        short.pop();
        short[0] -= 1;
        assert_eq!(parse(&short), None);
    }

    #[test]
    fn the_timeline_ends_a_spirits_buff_when_it_leaves() {
        let mut t = Timeline::default();
        t.note(1_000, &hex(BENEDICTION_ON));
        // 21:58:30.225: the same buff on spirit 28113, then its `42 36` flag 7.
        t.note(1_000, &hex("342a38d1db01011303e165a6091027000000000000bc5cba14a1010000e73403300af7000047e44c47b46a22c700e85a46"));
        t.note(2_052, &hex("0b4236d1db010007"));
        t.note(11_042, &hex(BENEDICTION_OFF));
        let ends: Vec<_> = t.all_instances().iter().map(|i| (i.entity, i.start_ms, i.end_ms, i.end)).collect();
        assert_eq!(ends, [(6759, 1_000, Some(11_042), End::Expired), (28113, 1_000, Some(2_052), End::Gone)]);
    }

    #[test]
    fn a_debuff_kept_up_by_several_players_changes_hands() {
        // 2026-10-06, Kernon of the West (48776): one Curse (161400005),
        // instance 293, applied by entity 1168, then by the local player
        // 15740, then by 16129.
        let mut t = Timeline::default();
        t.note(10_974, &hex("352a3888fd020113a502c5c49e09581b000000000000b2a79214a101000090090aea46f6000037cbb3c7f0172dc700e44646"));
        t.note(33_279, &hex("342b3888fd0213a502c5c49e09581b000000000000cefe9214a1010000fc7a0d6c47f6000242f9b6c7d97b2dc700c43d46"));
        t.note(34_816, &hex("342b3888fd0213a502c5c49e09581b000000000000dc049314a1010000817e0d5847f600028a09b5c7229527c700284746"));
        let got: Vec<_> = t.all_instances().iter().map(|i| (i.instance, i.caster, i.start_ms, i.end_ms, i.end)).collect();
        assert_eq!(
            got,
            [
                (293, 1168, 10_974, Some(33_279), End::Recast),
                (293, 15740, 33_279, Some(34_816), End::Recast),
                (293, 16129, 34_816, None, End::Open),
            ]
        );
        // Your share of it, and the track of everyone's Curse together.
        let mine = tracks(&t.all_instances(), |c| c);
        assert_eq!(mine.iter().find(|t| t.owner == 15740).map(|t| (t.start_ms, t.end_ms)), Some((33_279, Some(34_816))));
        let all = tracks(&t.all_instances(), |_| 0);
        assert_eq!(all.iter().map(|t| (t.start_ms, t.end_ms, t.stacks.clone())).collect::<Vec<_>>(), [(10_974, None, vec![(10_974, 1)])]);
    }

    #[test]
    fn a_change_to_an_abnormal_never_seen_added_dates_it_from_its_length() {
        // The 21:15:33.279 Curse change alone: 7 s long, ending at
        // 21:15:40.238 on the server's clock, so it started at 21:15:33.238.
        let mut t = Timeline::default();
        t.note(1_791_346_533_279, &hex("342b3888fd0213a502c5c49e09581b000000000000cefe9214a1010000fc7a0d6c47f6000242f9b6c7d97b2dc700c43d46"));
        let i = &t.all_instances()[0];
        assert_eq!((i.instance, i.caster, i.start_ms, i.server_end_ms), (293, 15740, 1_791_346_533_238, Some(1_791_346_540_238)));
    }

    #[test]
    fn a_map_load_ends_every_abnormal_but_yours() {
        let mut t = Timeline::default();
        t.note(1_000, &hex(STATS_ON));
        t.note(1_000, &hex(BENEDICTION_ON));
        t.note(1_000, &hex("342a38d1db01011303e165a6091027000000000000bc5cba14a1010000e73403300af7000047e44c47b46a22c700e85a46"));
        // 21:58:58.625: map 1010, load 3 of this session.
        t.note(1_500, &hex("34213603000000f20300008bc8da000000000049e14a47ad204dc7004c60465253bd420200000000000000000000000000"));
        let ends: Vec<_> = t.all_instances().iter().map(|i| (i.entity, i.end_ms, i.end)).collect();
        assert_eq!(ends, [(6759, None, End::Open), (28113, Some(1_500), End::MapLoad)]);
    }

    #[test]
    fn a_stack_pushed_out_without_a_record_closes_at_its_timer() {
        // 2026-10-05 14:58:49.395: an Element (163000003) on entity 11297,
        // 30 s. A fifth and sixth Element came before it ended; four newer
        // ones were used up by Elemental Fusion, and no record ended this one.
        let added = 1_791_237_529_395;
        let mut t = Timeline::default();
        t.note(added, &hex("342a38a158011372c32eb7093075000000000000f813140ea1010000919e0101f8c5f5000080f1f1c78035d6c700ccc046"));
        t.note(added + 33_000, &hex(BENEDICTION_OFF));
        let i = &t.all_instances()[0];
        // Its server end, 14:59:19.288, plus the 107 ms the add came late.
        assert_eq!((i.instance, i.end_ms, i.end), (114, Some(1_791_237_559_395), End::Unseen));
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
