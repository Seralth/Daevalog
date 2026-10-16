// SPDX-License-Identifier: AGPL-3.0-only

//! The enrage countdown beside the boss name: the time left until the boss's
//! timed enrage. It runs to the deadline the server sends as the boss comes
//! into combat (the reading's `target_enrage_ms`); with none, for the enemies
//! the game data gives an enrage (`boss_enrage.json` in the game data
//! folder), it counts that time from the meter's
//! first hit, which can come seconds after the game's own start.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// The enrage time of every enemy on the list that the file gives none.
pub const DEFAULT_SECONDS: i64 = 300;

/// Enemy code to enrage seconds, from {"bosses": [code], "seconds":
/// {"code": seconds}}; empty when the file is missing or reads otherwise.
fn read(path: &Path) -> HashMap<i32, i64> {
    let Some(file) = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else {
        return HashMap::new();
    };
    let seconds = |code: i64| file["seconds"][code.to_string()].as_i64().unwrap_or(DEFAULT_SECONDS);
    file["bosses"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|code| code.as_i64())
        .map(|code| (code as i32, seconds(code)))
        .collect()
}

fn table() -> &'static HashMap<i32, i64> {
    static TABLE: OnceLock<HashMap<i32, i64>> = OnceLock::new();
    TABLE.get_or_init(|| crate::data_dir().map(|dir| read(&dir.join("boss_enrage.json"))).unwrap_or_default())
}

/// The countdown for enemy `code` after `fight_ms` of fight, to the enrage
/// at `server_ms` of fight time (0: the table's): "m:ss" in whole seconds,
/// and its colour as the health bar's (`Theme.pingColors`): 1 above half the
/// time left, 2 from a quarter to half, 3 under a quarter and at 0:00. `None`
/// for an enemy with no timed enrage.
pub fn countdown(code: i32, server_ms: i64, fight_ms: i64) -> Option<(String, i32)> {
    countdown_in(table(), server_ms, code, fight_ms)
}

/// The whole seconds the countdown shows; `None` with no timed enrage.
pub fn seconds_left(code: i32, server_ms: i64, fight_ms: i64) -> Option<i64> {
    left_in(table(), server_ms, code, fight_ms).map(|(secs, _)| secs)
}

fn countdown_in(table: &HashMap<i32, i64>, server_ms: i64, code: i32, fight_ms: i64) -> Option<(String, i32)> {
    let (secs, share) = left_in(table, server_ms, code, fight_ms)?;
    let level = if share > 0.5 { 1 } else if share >= 0.25 { 2 } else { 3 };
    Some((crate::snapshot::clock(secs), level))
}

/// The whole seconds left, counted down, and their share of the enrage time.
fn left_in(table: &HashMap<i32, i64>, server_ms: i64, code: i32, fight_ms: i64) -> Option<(i64, f64)> {
    let total = if server_ms > 0 { server_ms } else { table.get(&code).copied()? * 1000 };
    let left = (total - fight_ms.max(0)).max(0);
    Some(((left + 999) / 1000, left as f64 / total as f64))
}

/// How long the readings must stay on the other side before the mark
/// turns, so a kill about on the enrage does not blink.
const MARK_HOLD_MS: i64 = 3_000;

/// The mark after the time to kill: in time (✓, green) while the kill comes
/// before the enrage, too slow (✗, red) when not. It turns only once the
/// readings have stayed on the other side for `MARK_HOLD_MS`.
#[derive(Debug, Default)]
pub struct KillMark {
    target: i32,
    /// The side shown: in time or not.
    in_time: Option<bool>,
    /// Since when the readings have been on the other side.
    turning: Option<i64>,
}

impl KillMark {
    /// Whether the kill is in time after the reading at `at`, from the
    /// countdown's seconds `left` and the time to kill `kill`. `None` when
    /// either is missing.
    pub fn update(&mut self, at: i64, target: i32, left: Option<i64>, kill: Option<i64>) -> Option<bool> {
        if target != self.target {
            *self = Self { target, ..Self::default() };
        }
        let (Some(left), Some(kill)) = (left, kill) else {
            self.turning = None;
            return None;
        };
        // At 0:00 the enrage has come: no kill is in time then.
        let now = kill <= left && left > 0;
        let shown = *self.in_time.get_or_insert(now);
        if now == shown {
            self.turning = None;
        } else if at - *self.turning.get_or_insert(at) >= MARK_HOLD_MS {
            self.in_time = Some(now);
            self.turning = None;
        }
        self.in_time
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped() -> HashMap<i32, i64> {
        read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data/boss_enrage.json"))
    }

    #[test]
    fn a_boss_on_the_list_counts_down_from_its_time() {
        let table = shipped();
        // A dungeon boss with 300 s.
        assert_eq!(countdown_in(&table, 0, 2300171, 0), Some(("5:00".into(), 1)));
        assert_eq!(countdown_in(&table, 0, 2300171, 400), Some(("5:00".into(), 1)), "whole seconds, counted down");
        assert_eq!(countdown_in(&table, 0, 2300171, 1_000), Some(("4:59".into(), 1)));
        assert_eq!(countdown_in(&table, 0, 2300171, 61_500), Some(("3:59".into(), 1)));
        // Transcendent Bakarma.
        assert_eq!(countdown_in(&table, 0, 2300471, 0).map(|c| c.0), Some("5:00".into()));
    }

    #[test]
    fn the_colour_steps_as_the_health_bars() {
        let table = shipped();
        let level = |ms| countdown_in(&table, 0, 2300171, ms).unwrap().1;
        assert_eq!(level(149_999), 1, "more than half left: green");
        assert_eq!(level(150_000), 2, "half left: yellow");
        assert_eq!(level(225_000), 2, "a quarter left: yellow");
        assert_eq!(level(225_001), 3, "under a quarter: red");
        assert_eq!(countdown_in(&table, 0, 2300171, 400_000), Some(("0:00".into(), 3)), "past it: 0:00 in red");
    }

    #[test]
    fn a_boss_with_another_time_counts_from_it() {
        let table = shipped();
        assert_eq!(countdown_in(&table, 0, 2301014, 0).map(|c| c.0), Some("10:00".into()), "600 s");
        assert_eq!(countdown_in(&table, 0, 2300582, 10_000).map(|c| c.0), Some("3:20".into()), "210 s");
    }

    #[test]
    fn a_boss_off_the_list_and_the_scarecrow_show_nothing() {
        let table = shipped();
        // Neglected Gadioton, Enhanced Harcon, Predator Saraswati, Phantasmal
        // Lakshmi; the Training Scarecrows.
        for code in [2300108, 2300104, 2300403, 2300401, 2300229, 2300919, 2090773] {
            assert_eq!(countdown_in(&table, 0, code, 0), None, "{code}");
        }
        assert_eq!(countdown_in(&table, 0, 0, 0), None, "no target");
    }

    #[test]
    fn a_server_deadline_is_counted_to_instead_of_the_table() {
        // A kill on the 300 s boss: the server's deadline 298.9 s after the
        // first hit, the last hit 234.4 s after it: 1:05 to the deadline,
        // where the table's 300 s from the first hit says 1:06.
        let (deadline, first_hit, killed) = (1_800_000_299_900, 1_800_000_001_000, 1_800_000_235_400);
        let table = shipped();
        let server = deadline - first_hit;
        let left = left_in(&table, server, 2300171, killed - first_hit).map(|l| l.0);
        assert_eq!(left, Some((deadline - killed + 999) / 1000));
        assert_eq!(countdown_in(&table, server, 2300171, killed - first_hit), Some(("1:05".into(), 3)));
        assert_eq!(countdown_in(&table, 0, 2300171, killed - first_hit), Some(("1:06".into(), 3)));
        // An enemy off the list counts down when the server sends a deadline.
        assert_eq!(countdown_in(&table, 120_000, 0, 60_000), Some(("1:00".into(), 2)));
    }

    #[test]
    fn the_mark_says_whether_the_kill_comes_before_the_enrage() {
        let table = shipped();
        // The 300 s boss: 3:18 left at 1:42, 0:38 to kill.
        let left = left_in(&table, 0, 2300171, 102_000).map(|l| l.0);
        assert_eq!(left, Some(198));
        let mut mark = KillMark::default();
        assert_eq!(mark.update(0, 7, left, Some(38)), Some(true), "in time");
        let mut mark = KillMark::default();
        assert_eq!(mark.update(0, 7, Some(55), Some(80)), Some(false), "too slow");
        assert_eq!(mark.update(100, 7, Some(55), None), None, "no time to kill yet");
        assert_eq!(mark.update(200, 7, None, Some(80)), None, "no enrage");
        let mut mark = KillMark::default();
        assert_eq!(mark.update(0, 7, Some(0), Some(0)), Some(false), "killed after the enrage came");
        assert_eq!(mark.update(0, 8, Some(1), Some(1)), Some(true), "at the last second: in time");
    }

    #[test]
    fn the_mark_turns_only_after_3_s_on_the_other_side() {
        let mut mark = KillMark::default();
        let mut at = |ms, kill| mark.update(ms, 7, Some(100), Some(kill));
        assert_eq!(at(0, 98), Some(true));
        // Over by a second: the mark holds, and the kill comes back in time.
        assert_eq!(at(1_000, 101), Some(true));
        assert_eq!(at(2_000, 99), Some(true));
        // Over again: the 3 s start from here, in a row.
        assert_eq!(at(3_000, 102), Some(true));
        assert_eq!(at(5_900, 103), Some(true), "2.9 s");
        assert_eq!(at(6_000, 103), Some(false), "3 s: too slow");
        // Back under for less than 3 s: still too slow.
        assert_eq!(at(7_000, 100), Some(false));
        assert_eq!(at(9_999, 99), Some(false));
        assert_eq!(at(10_000, 101), Some(false));
        // A gap in the time to kill starts the 3 s again.
        assert_eq!(at(11_000, 90), Some(false));
        assert_eq!(mark.update(12_000, 7, Some(100), None), None);
        assert_eq!(mark.update(13_000, 7, Some(100), Some(90)), Some(false));
        assert_eq!(mark.update(16_000, 7, Some(100), Some(90)), Some(true));
        // Another target takes its side at once.
        assert_eq!(mark.update(16_100, 9, Some(100), Some(120)), Some(false));
    }

    #[test]
    fn the_shipped_list_keeps_only_ids_and_the_times_that_are_not_300() {
        let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data/boss_enrage.json")).unwrap();
        let file: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(file.as_object().unwrap().len(), 2, "bosses and seconds, nothing else");
        assert!(file["bosses"].as_array().unwrap().len() > 300);
        assert!(file["seconds"].as_object().unwrap().values().all(|s| s.as_i64() != Some(DEFAULT_SECONDS)));
    }
}
