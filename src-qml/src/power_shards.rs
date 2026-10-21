// SPDX-License-Identifier: AGPL-3.0-only

//! The Power Shard notice: after your hits used Power Shards, three casts in
//! a row without them put one notice in the tray, "ran out" when the bag
//! says the stack in use is empty, else "off". A hit with a shard again
//! takes it away. Only skills that used a shard in the last ten minutes
//! count, so someone who never turns shards on is never told, and skills
//! that never spend one (spirits, some buffs) never set it off.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use daevalog::capture::decoder::ShardEvent;

use crate::i18n::text;

pub const OFF: &str = "notices.shard.off";
pub const OUT: &str = "notices.shard.out";
pub const TOGGLE: &str = "notices.shard.toggle";

/// The keys this file looks up, for the strings check.
#[cfg(test)]
pub const KEYS: [&str; 3] = [OFF, OUT, TOGGLE];

/// The tray's key: one notice, updated in place.
pub const KEY: &str = "shards";

/// How long a skill's shard hit counts as recent.
const RECENT_MS: i64 = 10 * 60 * 1000;
/// Casts in a row without a shard before the notice.
const PLAIN_CASTS: usize = 3;

/// What the tray should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Off,
    RanOut,
    Back,
}

#[derive(Default)]
pub struct ShardWatch {
    /// By skill: when a hit of it last used a shard.
    used: HashMap<i32, i64>,
    /// The casts (skill, cast byte) without a shard since the last with one.
    plain: Vec<(i32, u8)>,
    /// Shard stacks by item, as the bag last gave them.
    counts: HashMap<u32, u32>,
    /// The stack last spent from.
    in_use: Option<u32>,
    /// The notice is up (or would be, with the setting off).
    shown: bool,
}

impl ShardWatch {
    /// One record; what the tray should do about it.
    pub fn apply(&mut self, event: ShardEvent) -> Option<Change> {
        match event {
            ShardEvent::Count { item, count } => {
                // A stack that grows is loot, not the one in use.
                if self.counts.insert(item, count).is_none_or(|before| count < before) {
                    self.in_use = Some(item);
                }
                None
            }
            ShardEvent::Hit { at, skill, shard: true, .. } => {
                self.used.insert(skill, at);
                self.plain.clear();
                std::mem::take(&mut self.shown).then_some(Change::Back)
            }
            ShardEvent::Hit { at, skill, cast, shard: false } => {
                let recent = self.used.get(&skill).is_some_and(|&used| at - used <= RECENT_MS);
                if self.shown || !recent || self.plain.contains(&(skill, cast)) {
                    return None;
                }
                self.plain.push((skill, cast));
                if self.plain.len() < PLAIN_CASTS {
                    return None;
                }
                self.plain.clear();
                self.shown = true;
                Some(if self.ran_out() { Change::RanOut } else { Change::Off })
            }
        }
    }

    fn ran_out(&self) -> bool {
        self.in_use.and_then(|item| self.counts.get(&item)) == Some(&0)
    }

    /// The setting went off: the notice goes, if up.
    fn hide(&mut self) -> bool {
        std::mem::take(&mut self.shown)
    }
}

/// The notice's title and text in `language`.
pub fn words(change: Change, language: &str) -> (String, String) {
    match change {
        Change::RanOut => (text(language, OUT), String::new()),
        _ => (text(language, OFF), text(language, TOGGLE)),
    }
}

/// The live loop's call, once a tick: your shard records since the last
/// call, and the notice they make when `enabled`.
pub fn tick(storage: &daevalog::combat::data_storage::DataStorage, language: &str, enabled: bool) {
    static WATCH: OnceLock<Mutex<ShardWatch>> = OnceLock::new();
    let events = storage.take_shards();
    let mut watch = WATCH.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner());
    for event in events {
        match watch.apply(event) {
            Some(Change::Back) => crate::notices::remove(KEY),
            Some(change) if enabled => {
                let (title, line) = words(change, language);
                crate::notices::post(KEY, "warning", &title, &line);
            }
            _ => {}
        }
    }
    if !enabled && watch.hide() {
        crate::notices::remove(KEY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUND: u32 = 533_460_001;
    const ARROW: i32 = 16_040_000;
    const BLAST: i32 = 16_050_000;

    fn hit(at: i64, skill: i32, cast: u8, shard: bool) -> ShardEvent {
        ShardEvent::Hit { at, skill, cast, shard }
    }

    /// What `events` make, in order.
    fn run(watch: &mut ShardWatch, events: &[ShardEvent]) -> Vec<Change> {
        events.iter().filter_map(|&e| watch.apply(e)).collect()
    }

    /// Shards on for both skills, as a rotation with them goes.
    fn with_shards() -> ShardWatch {
        let mut watch = ShardWatch::default();
        assert!(run(&mut watch, &[hit(1_000, ARROW, 1, true), hit(2_000, BLAST, 2, true)]).is_empty());
        watch
    }

    #[test]
    fn three_casts_without_shards_after_casts_with_them_say_off_once() {
        let mut watch = with_shards();
        let plain = [hit(5_000, ARROW, 3, false), hit(6_000, BLAST, 4, false), hit(7_000, ARROW, 5, false)];
        assert_eq!(run(&mut watch, &plain[..2]), []);
        assert_eq!(run(&mut watch, &plain[2..]), [Change::Off]);
        assert_eq!(run(&mut watch, &[hit(8_000, ARROW, 6, false), hit(9_000, BLAST, 7, false), hit(9_500, ARROW, 8, false)]), [], "one notice");
        let (title, line) = words(Change::Off, "en");
        assert_eq!((title.as_str(), line.as_str()), ("Power Shards off", "The B key turns them on and off."));
    }

    #[test]
    fn the_hits_of_one_cast_count_once() {
        let mut watch = with_shards();
        let one_cast = [hit(5_000, ARROW, 3, false), hit(5_000, ARROW, 3, false), hit(5_050, ARROW, 3, false)];
        assert_eq!(run(&mut watch, &one_cast), []);
        assert_eq!(run(&mut watch, &[hit(6_000, BLAST, 4, false), hit(7_000, ARROW, 5, false)]), [Change::Off]);
    }

    #[test]
    fn an_empty_stack_says_ran_out() {
        let mut watch = with_shards();
        let spent = [ShardEvent::Count { item: BOUND, count: 2 }, ShardEvent::Count { item: BOUND, count: 1 }, ShardEvent::Count { item: BOUND, count: 0 }];
        assert_eq!(run(&mut watch, &spent), []);
        let plain = [hit(5_000, ARROW, 3, false), hit(6_000, BLAST, 4, false), hit(7_000, ARROW, 5, false)];
        assert_eq!(run(&mut watch, &plain), [Change::RanOut]);
        let (title, line) = words(Change::RanOut, "en");
        assert_eq!((title.as_str(), line.as_str()), ("Power Shards ran out", ""));
    }

    #[test]
    fn shards_left_in_the_stack_in_use_say_off() {
        let mut watch = with_shards();
        // The other kind ran dry long ago; the stack spent from last has some.
        let bag = [ShardEvent::Count { item: 533_460_004, count: 0 }, ShardEvent::Count { item: BOUND, count: 40 }, ShardEvent::Count { item: BOUND, count: 39 }];
        let plain = [hit(5_000, ARROW, 3, false), hit(6_000, BLAST, 4, false), hit(7_000, ARROW, 5, false)];
        assert_eq!(run(&mut watch, &[&bag[..], &plain[..]].concat()), [Change::Off]);
        let mut looted = with_shards();
        let bag = [ShardEvent::Count { item: BOUND, count: 0 }, ShardEvent::Count { item: 533_460_004, count: 5 }, ShardEvent::Count { item: 533_460_004, count: 9 }];
        assert_eq!(run(&mut looted, &[&bag[..], &plain[..]].concat()), [Change::Off], "the newest stack seen in use has some");
    }

    #[test]
    fn without_shard_hits_in_the_last_ten_minutes_nothing_is_said() {
        let mut never = ShardWatch::default();
        let plain: Vec<_> = (0..10).map(|i| hit(1_000 * i, ARROW, i as u8, false)).collect();
        assert_eq!(run(&mut never, &plain), [], "never used shards");
        let mut long_ago = with_shards();
        let later = 2_000 + RECENT_MS + 1;
        let plain = [hit(later, ARROW, 3, false), hit(later + 1_000, BLAST, 4, false), hit(later + 2_000, ARROW, 5, false)];
        assert_eq!(run(&mut long_ago, &plain), []);
    }

    #[test]
    fn skills_that_never_spend_a_shard_do_not_count() {
        let mut watch = with_shards();
        let spirit = 16_100_001;
        let plain = [hit(5_000, spirit, 3, false), hit(6_000, spirit, 4, false), hit(7_000, spirit, 5, false), hit(8_000, ARROW, 6, false)];
        assert_eq!(run(&mut watch, &plain), []);
    }

    #[test]
    fn a_shard_hit_in_between_starts_the_count_over() {
        let mut watch = with_shards();
        let events = [hit(5_000, ARROW, 3, false), hit(6_000, BLAST, 4, false), hit(7_000, ARROW, 5, true), hit(8_000, BLAST, 6, false), hit(9_000, ARROW, 7, false)];
        assert_eq!(run(&mut watch, &events), []);
    }

    #[test]
    fn shard_hits_again_take_the_notice_away() {
        let mut watch = with_shards();
        let plain = [hit(5_000, ARROW, 3, false), hit(6_000, BLAST, 4, false), hit(7_000, ARROW, 5, false)];
        assert_eq!(run(&mut watch, &plain), [Change::Off]);
        assert_eq!(run(&mut watch, &[hit(8_000, BLAST, 6, true), hit(9_000, ARROW, 7, true)]), [Change::Back], "taken away once");
        let again = [hit(10_000, ARROW, 8, false), hit(11_000, BLAST, 9, false), hit(12_000, ARROW, 10, false)];
        assert_eq!(run(&mut watch, &again), [Change::Off], "and said again the next time");
    }

    #[test]
    fn with_the_setting_off_the_notice_goes() {
        let mut watch = with_shards();
        run(&mut watch, &[hit(5_000, ARROW, 3, false), hit(6_000, BLAST, 4, false), hit(7_000, ARROW, 5, false)]);
        assert!(watch.hide());
        assert!(!watch.hide());
    }

    /// The notices a packet log makes, one line each, for a compare by hand:
    /// `A2_SHARD_REPLAY=<packets_*.txt> cargo test --lib power_shards -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn a_packet_log_prints_its_notices() {
        use daevalog::capture::packet_log::{LogLine, PacketLogReplay};
        use daevalog::combat::data_storage::DataStorage;
        use daevalog::i18n::lookup::{NpcLookup, SkillLookup};
        use std::sync::Arc;
        let Ok(file) = std::env::var("A2_SHARD_REPLAY") else { return };
        let storage = Arc::new(DataStorage::new());
        let mut feed = PacketLogReplay::new(storage.clone(), Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()), Default::default());
        let mut watch = ShardWatch::default();
        let (mut used, mut plain) = (0, 0);
        for line in std::fs::read_to_string(file).unwrap().lines() {
            let Some(line) = LogLine::parse(line) else { continue };
            feed.feed(&line);
            let time = line.time_of_day().get(..12).unwrap_or("").to_string();
            for event in storage.take_shards() {
                match event {
                    ShardEvent::Hit { shard: true, .. } => used += 1,
                    ShardEvent::Hit { .. } => plain += 1,
                    ShardEvent::Count { item, count } => println!("BAG {time} {item} {count}"),
                }
                if let Some(change) = watch.apply(event) {
                    println!("SHARD {time} {change:?} after {used} shard hits, {plain} plain");
                }
            }
        }
        println!("END {used} shard hits, {plain} plain");
    }
}
