//! Buffs, debuffs and stats as the server reports them
//! (`capture::abnormal`), for Details' timeline: every abnormal instance on
//! every entity, and your stat records, in capture time.

use std::collections::HashMap;

use crate::capture::abnormal::{Instance, StatEvent};

use super::DataStorage;

/// How long the timeline keeps what has ended: longer than any fight
/// Details shows.
const KEEP_MS: i64 = 2 * 3_600_000;
/// How often, in capture time, what is older than `KEEP_MS` goes.
const FORGET_EVERY_MS: i64 = 60_000;

impl DataStorage {
    /// Reads one framed packet for the buff timeline: its abnormal and stat
    /// records, map loads and deaths.
    pub fn note_abnormal(&self, ms: i64, packet: &[u8]) {
        let mut buffs = self.buffs.lock();
        buffs.timeline.note(ms, packet);
        if ms >= buffs.forgot_ms + FORGET_EVERY_MS {
            buffs.forgot_ms = ms;
            buffs.timeline.forget_before(ms - KEEP_MS);
        }
    }

    /// Each abnormal's stack limit, from the game data (`abnormal::stack_limits`).
    pub fn set_abnormal_stack_limits(&self, limits: HashMap<u32, u32>) {
        self.buffs.lock().timeline.set_stack_limits(limits);
    }

    /// The abnormal instances on at any time from `from_ms` to `to_ms`.
    pub fn abnormals_between(&self, from_ms: i64, to_ms: i64) -> Vec<Instance> {
        self.buffs.lock().timeline.instances_between(from_ms, to_ms)
    }

    /// Your stat records from `from_ms` to `to_ms`, with the whole sheet they change.
    pub fn stats_between(&self, from_ms: i64, to_ms: i64) -> Vec<StatEvent> {
        self.buffs.lock().timeline.stats_between(from_ms, to_ms)
    }
}

/// The timeline and when it last forgot.
#[derive(Default, Clone)]
pub(super) struct Buffs {
    pub timeline: crate::capture::abnormal::Timeline,
    pub forgot_ms: i64,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::capture::decoder::PacketDecoder;
    use crate::i18n::lookup::{NpcLookup, SkillLookup};

    use crate::built::{self, Abnormal};

    const YOU: u32 = 2_800;

    /// Spirit's Benediction on you (instance 300), and off 10 s later.
    fn on() -> Vec<u8> {
        Abnormal {
            flags: 0x13,
            instance: 300,
            abnormal: 161_900_001,
            length: 10_000,
            end: 1_800_000_020_000,
            caster: YOU,
            level: 2,
            skill: Some(16_190_000),
            byte: Some(0),
            position: [-1500.5, 2500.25, 120.0],
        }
        .added(YOU)
    }

    fn off() -> Vec<u8> {
        built::removed(YOU, &[(0x00, 300, 1, None)])
    }

    #[test]
    fn a_buff_is_kept_from_on_to_off() {
        let s = DataStorage::new();
        s.note_abnormal(1_000, &on());
        let open = s.abnormals_between(0, 5_000);
        assert_eq!(open.len(), 1);
        assert_eq!((open[0].entity, open[0].instance, open[0].end_ms), (YOU as i32, 300, None));
        s.note_abnormal(11_000, &off());
        let closed = s.abnormals_between(0, 20_000);
        assert_eq!(closed[0].end_ms, Some(11_000));
        assert!(s.abnormals_between(12_000, 20_000).is_empty(), "off before the window");
    }

    #[test]
    fn what_ended_long_ago_is_forgotten() {
        let s = DataStorage::new();
        s.note_abnormal(1_000, &on());
        s.note_abnormal(11_000, &off());
        // Hours later another packet comes; the old buff goes.
        s.note_abnormal(11_000 + KEEP_MS + FORGET_EVERY_MS, &off());
        assert!(s.abnormals_between(0, i64::MAX).is_empty());
    }

    #[test]
    fn the_decoder_hands_every_packet_to_the_timeline() {
        let s = Arc::new(DataStorage::new());
        let mut p = PacketDecoder::new(s.clone(), Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
        p.pin_clock(Some(5_000));
        p.decode_buffer(&on());
        p.pin_clock(None);
        let on = s.abnormals_between(0, 10_000);
        assert_eq!(on.len(), 1);
        assert_eq!(on[0].start_ms, 5_000);
    }
}
