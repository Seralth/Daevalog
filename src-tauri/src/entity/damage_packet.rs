//! One damage or heal record on its way from the parser to storage.

use std::sync::atomic::{AtomicI64, Ordering};

use super::mark_kinds::HitMark;

/// The id the last record got. Ids rise across the whole process.
static LAST_RECORD: AtomicI64 = AtomicI64::new(0);

#[derive(Debug, Clone)]
pub struct ParsedDamagePacket {
    serial: i64,
    at: i64,
    source: i32,
    victim: i32,
    skill: i32,
    /// The code the record carried before any folding; 0 when the parser set
    /// none, and then it reads as `skill`.
    raw_skill: i32,
    kind: i32,
    amount: i64,
    extra_amount: i64,
    extra_hits: i32,
    healed: i64,
    tick: bool,
    marks: Vec<HitMark>,
    flags: [bool; 5],
}

impl Default for ParsedDamagePacket {
    fn default() -> Self {
        Self::new()
    }
}

impl ParsedDamagePacket {
    /// An empty record with the next id, stamped with this thread's clock
    /// (the replayed time during a replay).
    pub fn new() -> Self {
        let serial = LAST_RECORD.fetch_add(1, Ordering::Relaxed) + 1;
        ParsedDamagePacket {
            serial,
            at: crate::clock::now_ms(),
            source: 0,
            victim: 0,
            skill: 0,
            raw_skill: 0,
            kind: 0,
            amount: 0,
            extra_amount: 0,
            extra_hits: 0,
            healed: 0,
            tick: false,
            marks: Vec::new(),
            flags: [false; 5],
        }
    }

    pub fn id(&self) -> i64 {
        self.serial
    }

    pub fn when_ms(&self) -> i64 {
        self.at
    }
    pub fn set_timestamp(&mut self, ms: i64) {
        self.at = ms;
    }

    pub fn actor_id(&self) -> i32 {
        self.source
    }
    pub fn set_actor_id(&mut self, id: i32) {
        self.source = id;
    }

    pub fn target_id(&self) -> i32 {
        self.victim
    }
    pub fn set_target_id(&mut self, id: i32) {
        self.victim = id;
    }

    pub fn skill_used(&self) -> i32 {
        self.skill
    }
    pub fn set_skill_code(&mut self, code: i32) {
        self.skill = code;
    }

    pub fn raw_skill_code(&self) -> i32 {
        match self.raw_skill {
            0 => self.skill,
            raw => raw,
        }
    }
    pub fn set_raw_skill_code(&mut self, code: i32) {
        self.raw_skill = code;
    }

    pub fn hit_kind(&self) -> i32 {
        self.kind
    }
    pub fn set_hit_kind(&mut self, kind: i32) {
        self.kind = kind;
    }

    pub fn damage(&self) -> i64 {
        self.amount
    }
    pub fn set_damage(&mut self, amount: impl Into<i64>) {
        self.amount = amount.into();
    }

    pub fn added_damage(&self) -> i64 {
        self.extra_amount
    }
    pub fn set_added_damage(&mut self, amount: impl Into<i64>) {
        self.extra_amount = amount.into();
    }

    pub fn added_hits(&self) -> i32 {
        self.extra_hits
    }
    pub fn set_added_hits(&mut self, hits: i32) {
        self.extra_hits = hits;
    }

    /// The main hit and its extra hits together; stops at the i64 limits.
    pub fn damage_with_extras(&self) -> i64 {
        self.amount.saturating_add(self.extra_amount)
    }

    pub fn heal_value(&self) -> i64 {
        self.healed
    }
    pub fn set_heal_value(&mut self, amount: impl Into<i64>) {
        self.healed = amount.into();
    }

    pub fn is_dot(&self) -> bool {
        self.tick
    }
    pub fn set_dot(&mut self, tick: bool) {
        self.tick = tick;
    }

    pub fn mark_list(&self) -> &[HitMark] {
        &self.marks
    }
    pub fn set_mark_list(&mut self, marks: Vec<HitMark>) {
        self.marks = marks;
    }

    pub fn is_crit(&self) -> bool {
        self.marks.contains(&HitMark::Critical)
    }

    pub fn flag_bits(&self) -> &[bool; 5] {
        &self.flags
    }
    pub fn set_flag_bits(&mut self, flags: [bool; 5]) {
        self.flags = flags;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::mark_kinds::HitMark::{Back, Critical, Parry};
    use std::collections::HashSet;

    #[test]
    fn later_records_get_higher_ids() {
        let first = ParsedDamagePacket::new();
        let second = ParsedDamagePacket::new();
        assert!(first.id() >= 1);
        assert!(second.id() > first.id());
    }

    #[test]
    fn records_made_on_many_threads_never_share_an_id() {
        let workers: Vec<_> = (0..8)
            .map(|_| std::thread::spawn(|| (0..1000).map(|_| ParsedDamagePacket::new().id()).collect::<Vec<_>>()))
            .collect();
        let mut seen = HashSet::new();
        for w in workers {
            for id in w.join().unwrap() {
                assert!(seen.insert(id), "id {id} given twice");
            }
        }
        assert_eq!(seen.len(), 8000);
    }

    #[test]
    fn a_new_record_takes_the_pinned_time() {
        crate::clock::set_override(Some(1000));
        let mut rec = ParsedDamagePacket::new();
        crate::clock::set_override(None);
        assert_eq!(rec.when_ms(), 1000);
        rec.set_timestamp(5);
        assert_eq!(rec.when_ms(), 5);
    }

    #[test]
    fn a_new_record_is_empty() {
        let rec = ParsedDamagePacket::new();
        assert_eq!((rec.actor_id(), rec.target_id(), rec.skill_used(), rec.raw_skill_code()), (0, 0, 0, 0));
        assert_eq!((rec.damage(), rec.added_damage(), rec.heal_value(), rec.damage_with_extras()), (0, 0, 0, 0));
        assert_eq!((rec.hit_kind(), rec.added_hits()), (0, 0));
        assert!(!rec.is_dot());
        assert!(!rec.is_crit());
        assert!(rec.mark_list().is_empty());
        assert_eq!(rec.flag_bits(), &[false; 5]);
    }

    #[test]
    fn every_setter_is_read_back_unchanged() {
        let mut rec = ParsedDamagePacket::new();
        let ids = (9063, 52467, 11340000);
        rec.set_actor_id(ids.0);
        rec.set_target_id(ids.1);
        rec.set_skill_code(ids.2);
        assert_eq!((rec.actor_id(), rec.target_id(), rec.skill_used()), ids);

        // Damage type 2 is reached by no capture.
        rec.set_hit_kind(2);
        assert_eq!(rec.hit_kind(), 2);

        // Amounts take any integer that fits in an i64.
        rec.set_damage(1090u32);
        rec.set_added_damage(121i32);
        rec.set_heal_value(7_000_000_000i64);
        assert_eq!([rec.damage(), rec.added_damage(), rec.heal_value()], [1090, 121, 7_000_000_000]);

        rec.set_added_hits(3);
        rec.set_dot(true);
        assert!(rec.is_dot() && rec.added_hits() == 3);

        let marks = vec![Parry, Back, Critical];
        rec.set_mark_list(marks.clone());
        assert_eq!(rec.mark_list(), marks.as_slice());

        let flags = [true, false, true, false, true];
        rec.set_flag_bits(flags);
        assert_eq!(*rec.flag_bits(), flags);
    }

    #[test]
    fn the_raw_code_falls_back_to_the_skill_code() {
        let mut rec = ParsedDamagePacket::new();
        rec.set_skill_code(100011);
        assert_eq!(rec.raw_skill_code(), 100011);
        rec.set_raw_skill_code(16990002);
        assert_eq!(rec.raw_skill_code(), 16990002);
        rec.set_raw_skill_code(0);
        assert_eq!(rec.raw_skill_code(), 100011);
    }

    #[test]
    fn total_damage_adds_the_extra_hits_and_stops_at_the_limits() {
        let total = |main: i64, extra: i64| {
            let mut rec = ParsedDamagePacket::new();
            rec.set_damage(main);
            rec.set_added_damage(extra);
            rec.damage_with_extras()
        };
        assert_eq!(total(1090, 121), 1211);
        assert_eq!(total(i64::MAX, 1), i64::MAX);
        assert_eq!(total(i64::MIN, -1), i64::MIN);
    }

    #[test]
    fn a_hit_is_critical_when_marked_so() {
        let crit = |marks: Vec<HitMark>| {
            let mut rec = ParsedDamagePacket::new();
            rec.set_mark_list(marks);
            rec.is_crit()
        };
        assert!(crit(vec![Back, Critical]));
        assert!(!crit(vec![Back]));
        assert!(!crit(vec![]));
    }
}
