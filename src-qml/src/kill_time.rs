// SPDX-License-Identifier: AGPL-3.0-only

//! The time to kill a boss: an estimate from the target's own HP records,
//! kept by the reading loop, so damage the meter does not count (other
//! players', unknown damage over time) counts too. Two paces, by where the
//! fight is (`crowd_map`):
//!
//! - In a dungeon (a party, a raid, a sealed or quest dungeon and the like)
//!   the whole pull's pace (`WholePace`). A boss fight there repeats in
//!   cycles (stagger windows, cooldowns, phases the boss cannot be hurt in),
//!   and an average over whole cycles is the true pace (the renewal reward
//!   theorem); a short window swings with wherever in the cycle it looks, and
//!   the phases still to come are priced in only by an average that holds the
//!   phases behind. A CUSUM test (Page 1954) restarts it on a lasting change.
//! - In a crowd fight (the open world, the Abyss, field events, artifact
//!   sieges) the recent pace (`RecentPace`): players keep arriving and the
//!   damage keeps growing, so the whole pull's pace lags far behind.
//!
//! Both chosen on recorded boss kills: 9 in dungeons, 3 in crowds.

use std::collections::{HashSet, VecDeque};
use std::sync::LazyLock;

/// No time before this much of the pull.
const ETA_MIN_MS: i64 = 5_000;

/// A rise in HP of more than this share of the pull's first HP (a reset, a
/// wipe, a new pull on the same id) starts the pull again; smaller heals do
/// not.
const ETA_NEW_PULL_RISE: f64 = 0.02;

/// The dungeon kinds of the game's table where a crowd fights: the rest are
/// small groups. The open world has no dungeon (0).
const CROWD_KINDS: [&str; 3] = ["abyss", "fieldevent", "abyssartifact"];

static CROWD_DUNGEONS: LazyLock<HashSet<i32>> = LazyLock::new(|| {
    let table: std::collections::HashMap<String, serde_json::Value> =
        serde_json::from_str(include_str!("../../src/data/i18n/dungeons/en.json")).unwrap_or_default();
    table
        .iter()
        .filter(|(_, v)| v.get("type").and_then(|t| t.as_str()).is_some_and(|t| CROWD_KINDS.contains(&t)))
        .filter_map(|(id, _)| id.parse().ok())
        .collect()
});

/// Whether a fight in `dungeon_id` (the reading's; 0 outside a dungeon) is a
/// crowd's.
pub fn crowd_map(dungeon_id: i32) -> bool {
    dungeon_id == 0 || CROWD_DUNGEONS.contains(&dungeon_id)
}

/// The pull's readings, kept by the reading loop, which sees every reading
/// (a fast replay sends the window only some); both paces follow every pull,
/// and `crowd` picks the one shown.
#[derive(Debug, Default)]
pub struct HpTrail {
    target: i32,
    fight_ms: i64,
    first: Option<(i64, i64)>,
    last: Option<(i64, i64)>,
    whole: WholePace,
    recent: RecentPace,
}

impl HpTrail {
    /// The time to kill after the reading at `at`: `fight_ms` is the fight
    /// time, `hp` the target's HP from its records, -1 with none; `crowd`
    /// whether a crowd fights it (`crowd_map`).
    pub fn update(&mut self, at: i64, target: i32, fight_ms: i64, hp: i64, crowd: bool) -> Option<i64> {
        // A new fight on a dead target (the combat record after a kill opens
        // one) has nothing left to kill.
        let dead_again = hp == 0 && fight_ms < self.fight_ms;
        self.fight_ms = fight_ms;
        if self.target != target || hp < 0 || dead_again || self.last.is_some_and(|(t, _)| at < t) {
            self.first = None;
            self.last = None;
            self.target = target;
        }
        if hp >= 0 {
            let new_pull = match (self.first, self.last) {
                (Some((_, start)), Some((_, before))) => (hp - before) as f64 > ETA_NEW_PULL_RISE * start as f64,
                _ => true,
            };
            if new_pull {
                self.first = Some((at, hp));
                self.whole.start(at, hp);
                self.recent.start(at, hp);
            } else if let Some(before) = self.last {
                self.whole.add(before, at, hp);
                self.recent.add(before, at, hp);
            }
            self.last = Some((at, hp));
        }
        self.time_to_kill(crowd)
    }

    fn time_to_kill(&self, crowd: bool) -> Option<i64> {
        let (Some((t0, hp0)), Some((t1, hp1))) = (self.first, self.last) else {
            return None;
        };
        if t1 - t0 < ETA_MIN_MS || hp1 >= hp0 {
            return None;
        }
        let whole_pull = (hp0 - hp1) as f64 / (t1 - t0) as f64;
        let per_ms = if crowd {
            self.recent.per_ms(t1, whole_pull)
        } else {
            self.whole.per_ms().unwrap_or(whole_pull)
        };
        Some((hp1 as f64 / per_ms / 1_000.0).round() as i64)
    }
}

// ---- the dungeon pace -------------------------------------------------------

/// The pace over this last stretch, under `PHASE_SHARE` of the pace the HP
/// fell at so far, is a phase the boss cannot be hurt in (only damage over
/// time still ticks): it still counts in the average, but not in the change
/// test, where it would look like players leaving.
const PHASE_LOOK_MS: i64 = 3_000;
const PHASE_SHARE: f64 = 0.15;

/// The change test's block, and how much of the pull comes first.
const BLOCK_MS: i64 = 10_000;
const WARM_MS: i64 = 10_000;

/// The change test: log(block pace / pace) may drift this far per block
/// before it counts, and the sum that counts sets the alarm here. Strict on
/// purpose: in a dungeon a half-minute slump (a death, the group moving)
/// is not a change, and an average started again on it stays at the slump's
/// pace (on one recorded dungeon boss, 0.25 and 1.0 read 157% long at half
/// HP; these read as the whole pull's pace).
const CHANGE_DRIFT: f64 = 0.40;
const CHANGE_ALARM: f64 = 2.0;

/// Time and HP lost: all of it, and the part outside phases.
#[derive(Debug, Default, Clone, Copy)]
struct Totals {
    all_ms: i64,
    all_loss: i64,
    hurt_ms: i64,
    hurt_loss: i64,
}

impl Totals {
    fn since(self, base: Totals) -> Totals {
        Totals {
            all_ms: self.all_ms - base.all_ms,
            all_loss: self.all_loss - base.all_loss,
            hurt_ms: self.hurt_ms - base.hurt_ms,
            hurt_loss: self.hurt_loss - base.hurt_loss,
        }
    }

    /// HP per ms outside phases, once there is enough of it.
    fn hurt_pace(self) -> Option<f64> {
        (self.hurt_ms >= WARM_MS && self.hurt_loss > 0).then(|| self.hurt_loss as f64 / self.hurt_ms as f64)
    }

    /// HP per ms over all of it, once there is enough of it.
    fn pace(self) -> Option<f64> {
        (self.all_ms >= WARM_MS && self.all_loss > 0).then(|| self.all_loss as f64 / self.all_ms as f64)
    }
}

/// The change test (two-sided CUSUM) and where each sum was last zero.
#[derive(Debug, Default)]
struct Change {
    up: f64,
    down: f64,
    up_from: usize,
    down_from: usize,
}

/// The whole pull's pace, phases included, started again where a lasting
/// change of pace began.
#[derive(Debug, Default)]
struct WholePace {
    /// The readings of the last `PHASE_LOOK_MS`, and the one before them.
    look: VecDeque<(i64, i64)>,
    totals: Totals,
    /// Where the average starts: the pull's start, or the last change.
    base: Totals,
    /// The current block's start time and totals.
    block: (i64, Totals),
    /// The totals at each block's end.
    marks: Vec<Totals>,
    change: Change,
}

impl WholePace {
    fn start(&mut self, at: i64, hp: i64) {
        *self = WholePace { block: (at, Totals::default()), ..WholePace::default() };
        self.look.push_back((at, hp));
    }

    fn add(&mut self, (before_at, before): (i64, i64), at: i64, hp: i64) {
        let (dt, loss) = (at - before_at, (before - hp).max(0));
        let pace = self.totals.since(self.base).hurt_pace();
        self.look.push_back((at, hp));
        // Keep the last reading at or before the look's start too.
        while self.look.get(1).is_some_and(|&(t, _)| t <= at - PHASE_LOOK_MS) {
            self.look.pop_front();
        }
        let &(from_at, from_hp) = self.look.front().expect("pushed above");
        let lately = (from_hp - hp) as f64 / (at - from_at).max(1) as f64;
        self.totals.all_ms += dt;
        self.totals.all_loss += loss;
        if pace.is_none_or(|p| lately >= PHASE_SHARE * p) {
            self.totals.hurt_ms += dt;
            self.totals.hurt_loss += loss;
        }
        if at - self.block.0 >= BLOCK_MS {
            self.close_block(at, pace);
        }
    }

    /// Ends a block and runs the change test on it against `pace`, the pace
    /// before this reading.
    fn close_block(&mut self, at: i64, pace: Option<f64>) {
        let block = self.totals.since(self.block.1);
        self.block = (at, self.totals);
        self.marks.push(self.totals);
        let Some(pace) = pace else { return };
        if block.hurt_ms < BLOCK_MS / 2 || block.hurt_loss <= 0 {
            return;
        }
        let x = (block.hurt_loss as f64 / block.hurt_ms as f64 / pace).ln();
        let c = &mut self.change;
        c.up = (c.up + x - CHANGE_DRIFT).max(0.0);
        c.down = (c.down - x - CHANGE_DRIFT).max(0.0);
        let n = self.marks.len();
        if c.up == 0.0 {
            c.up_from = n;
        }
        if c.down == 0.0 {
            c.down_from = n;
        }
        if c.up > CHANGE_ALARM || c.down > CHANGE_ALARM {
            // The average starts again where the sum that rang last was zero.
            let from = if c.up > CHANGE_ALARM { c.up_from } else { c.down_from };
            self.base = if from > 0 { self.marks[from - 1] } else { Totals::default() };
            self.change = Change { up_from: n, down_from: n, ..Change::default() };
        }
    }

    /// HP per ms since the last change, phases included; before there is
    /// `WARM_MS` of that, outside phases; none before either.
    fn per_ms(&self) -> Option<f64> {
        let since = self.totals.since(self.base);
        since.pace().or_else(|| since.hurt_pace())
    }
}

// ---- the crowd pace ---------------------------------------------------------

/// How far back the recent pace looks: on an open-world boss the damage rises
/// as players arrive and falls when they leave, and the recent pace follows
/// the damage once it holds steady this long.
const RECENT_WINDOW_MS: i64 = 30_000;

/// HP that has not fallen for this long is in a phase the boss cannot be hurt
/// in, or a pause: the time holds at the whole pull's pace then, and the
/// recent pace starts again where the HP falls again, so the pause does not
/// drag it down.
const RECENT_FLAT_MS: i64 = 5_000;

/// The pace of the last `RECENT_WINDOW_MS`, along a straight line fitted to
/// its readings.
#[derive(Debug, Default)]
struct RecentPace {
    /// The readings of the window, and the last one at or before its start.
    readings: VecDeque<(i64, i64)>,
    /// When the HP last fell.
    fell_at: i64,
    /// The last reading before the HP fell again after `RECENT_FLAT_MS` or
    /// more without falling; the recent pace starts there.
    resumed_at: i64,
}

impl RecentPace {
    fn start(&mut self, at: i64, hp: i64) {
        self.readings.clear();
        (self.fell_at, self.resumed_at) = (at, at);
        self.readings.push_back((at, hp));
    }

    fn add(&mut self, (before_at, before): (i64, i64), at: i64, hp: i64) {
        if hp < before {
            if before_at - self.fell_at >= RECENT_FLAT_MS {
                self.resumed_at = before_at;
            }
            self.fell_at = at;
        }
        self.readings.push_back((at, hp));
        while self.readings.get(1).is_some_and(|&(t, _)| at - t >= RECENT_WINDOW_MS) {
            self.readings.pop_front();
        }
    }

    /// HP per ms at `now`: the window's line, or from where the HP fell again
    /// after a pause when that is later; `whole_pull` while the HP is not
    /// falling, when it has fallen again for under `ETA_MIN_MS`, or when the
    /// line does not fall.
    fn per_ms(&self, now: i64, whole_pull: f64) -> f64 {
        let from = (now - RECENT_WINDOW_MS).max(self.resumed_at);
        if now - self.fell_at >= RECENT_FLAT_MS || now - from < ETA_MIN_MS {
            return whole_pull;
        }
        // From the last reading at or before `from`.
        let skip = self.readings.iter().take_while(|&&(t, _)| t <= from).count().saturating_sub(1);
        let recent: Vec<(i64, i64)> = self.readings.range(skip..).copied().collect();
        Some(line_pace(&recent)).filter(|&p| p > 0.0).unwrap_or(whole_pull)
    }
}

/// The HP lost per ms along a straight line fitted to the readings (least
/// squares), so one burst or one late record moves it little; 0 when the
/// line does not fall.
fn line_pace(readings: &[(i64, i64)]) -> f64 {
    let Some(&(end, _)) = readings.last() else {
        return 0.0;
    };
    let n = readings.len() as f64;
    // Times from the last reading, so the squares stay small.
    let x = |t: i64| (t - end) as f64;
    let mean_x = readings.iter().map(|&(t, _)| x(t)).sum::<f64>() / n;
    let mean_hp = readings.iter().map(|&(_, hp)| hp as f64).sum::<f64>() / n;
    let (sxx, sxy) = readings.iter().fold((0.0, 0.0), |(sxx, sxy), &(t, hp)| {
        let dx = x(t) - mean_x;
        (sxx + dx * dx, sxy + dx * (hp as f64 - mean_hp))
    });
    if sxx == 0.0 { 0.0 } else { (-sxy / sxx).max(0.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUNGEON: bool = false;
    const CROWD: bool = true;

    /// Readings every 100 ms, as the reading loop sees them, from `hp(ms)`;
    /// the time to kill after each whole second, by second.
    fn run(trail: &mut HpTrail, target: i32, from_s: i64, to_s: i64, hp: impl Fn(i64) -> i64) -> Vec<Option<i64>> {
        let mut out = Vec::new();
        for ms in (from_s * 1_000..=to_s * 1_000).step_by(100) {
            let eta = trail.update(ms, target, ms, hp(ms), DUNGEON);
            if ms % 1_000 == 0 {
                out.push(eta);
            }
        }
        out
    }

    #[test]
    fn crowd_maps_are_the_open_world_and_the_crowded_dungeon_kinds() {
        assert!(crowd_map(0), "the open world");
        assert!(crowd_map(21), "Chaotic Lower Reshanta (abyss)");
        assert!(crowd_map(500032), "Beritra Air Raid (fieldevent)");
        assert!(!crowd_map(600012), "Urugugu Canyon (party)");
        assert!(!crowd_map(600001), "Krao Cave (party)");
    }

    #[test]
    fn a_steady_pace_gives_the_hp_left_over_it() {
        let mut trail = HpTrail::default();
        // 10,000 HP a second from 1,000,000.
        let eta = run(&mut trail, 7, 0, 30, |ms| 1_000_000 - 10 * ms);
        assert!(eta[..5].iter().all(Option::is_none), "under 5 s of readings");
        assert_eq!(eta[5], Some(95), "950,000 left at 10,000 a second");
        assert_eq!(eta[30], Some(70), "700,000 left");
    }

    #[test]
    fn resets_other_targets_and_the_kill() {
        let mut trail = HpTrail::default();
        let mut read = |s: i64, target: i32, hp: i64| trail.update(s * 1_000, target, s * 1_000, hp, DUNGEON);
        for s in 0..=20 {
            read(s, 7, 1_000_000 - 10_000 * s);
        }
        // A small heal (under 2% of where the pull began) keeps the pull.
        assert_eq!(read(21, 7, 805_000), Some(85), "200,000 lost over 21 s");
        // A reset: a new pull, 5 s of readings first.
        assert_eq!(read(22, 7, 1_000_000), None);
        assert_eq!(read(27, 7, 900_000), Some(45), "100,000 over 5 s");
        // Another target, or no HP records: start again.
        assert_eq!(read(28, 8, 500_000), None);
        assert_eq!(read(29, 8, -1), None);
        assert_eq!(read(34, 8, 400_000), None, "the 5 s start again");
        assert_eq!(read(34, 8, 0), None, "still under 5 s");
        assert_eq!(read(39, 8, 0), Some(0), "the kill");
        assert_eq!(read(44, 8, 0), Some(0), "the kill, held");
    }

    #[test]
    fn a_new_fight_on_a_dead_target_has_no_time_to_kill() {
        for crowd in [DUNGEON, CROWD] {
            let mut trail = HpTrail::default();
            for s in 0..10 {
                trail.update(s * 1_000, 8, s * 1_000, 400_000 - 40_000 * s, crowd);
            }
            assert_eq!(trail.update(10_000, 8, 10_000, 0, crowd), Some(0), "the kill");
            // The combat record after the kill opens a new fight on the dead
            // target: no time to kill there.
            assert_eq!(trail.update(10_100, 8, 0, 0, crowd), None);
            assert_eq!(trail.update(20_000, 8, 9_900, 0, crowd), None);
        }
    }

    /// A boss in a dungeon: 30,000 a second with a 15 s burst at 60,000
    /// every 60 s (stagger and cooldowns), so 37,500 a second on average.
    fn cycles(ms: i64) -> i64 {
        let full = ms / 60_000;
        let into = ms % 60_000;
        let burst = |t: i64| 30 * t + 30 * t.clamp(0, 15_000);
        10_000_000 - full * burst(60_000) - burst(into)
    }

    #[test]
    fn bursts_average_out_over_the_cycle() {
        let mut trail = HpTrail::default();
        let eta = run(&mut trail, 9, 0, 240, cycles);
        // The true time left is the HP left at the cycle's average pace. Over
        // whole cycles the time is right to the second; part-way through one,
        // once a whole cycle is behind, it is never off by more than the burst
        // (inside the first cycle nothing can know the cycle's average yet).
        for s in [60, 120, 180, 240] {
            let truth = (cycles(s * 1_000) as f64 / 37_500.0).round() as i64;
            assert_eq!(eta[s as usize], Some(truth), "at {s} s");
        }
        for s in 60..=240 {
            let truth = cycles(s * 1_000) as f64 / 37_500.0;
            let got = eta[s as usize].unwrap() as f64;
            assert!((got - truth).abs() <= 0.25 * truth + 2.0, "at {s} s: {got} vs {truth:.0}");
        }
    }

    #[test]
    fn a_phase_with_only_ticks_does_not_restart_the_average() {
        let mut trail = HpTrail::default();
        // 40,000 a second for 60 s, a 10 s phase at 1,000 a second (damage
        // over time), then 40,000 again.
        let hp = |ms: i64| {
            let lost = 40 * ms.min(60_000) + ms.clamp(60_000, 70_000) - 60_000 + 40 * (ms - 70_000).max(0);
            10_000_000 - lost
        };
        let eta = run(&mut trail, 3, 0, 120, hp);
        // After the phase the pace counts it: 120 s lost 4,410,000, so
        // 36,750 a second, and 5,590,000 left.
        assert_eq!(eta[120], Some(152));
        assert_eq!(trail.whole.base.all_ms, 0, "no change found");
    }

    #[test]
    fn a_lasting_drop_in_damage_restarts_the_average() {
        let mut trail = HpTrail::default();
        // 40,000 a second for 90 s, then players leave: 10,000 a second.
        let hp = |ms: i64| 20_000_000 - 40 * ms.min(90_000) - 10 * (ms - 90_000).max(0);
        let eta = run(&mut trail, 4, 0, 240, hp);
        // The whole pull's pace would give 11,100,000 / 15,000 = 740 s. The
        // change is found and the time follows the new pace.
        let left = hp(240_000) as f64;
        let got = eta[240].unwrap() as f64;
        assert!((got - left / 10_000.0).abs() <= 0.1 * left / 10_000.0, "{got} s for {left} left");
        assert!(trail.whole.base.all_ms > 0, "the average starts again");
    }

    #[test]
    fn in_a_crowd_the_time_follows_the_pace_of_the_last_30_seconds() {
        let mut trail = HpTrail::default();
        let mut read = |ms: i64, target: i32, hp: i64| trail.update(ms, target, ms, hp, CROWD);
        // 10,000 HP a second from 1,000,000.
        let hp = |s: i64| 1_000_000 - 10_000 * s;
        for s in 0..5 {
            assert_eq!(read(s * 1_000, 7, hp(s)), None, "under 5 s of readings");
        }
        assert_eq!(read(5_000, 7, hp(5)), Some(95), "950,000 left at 10,000 a second");
        for s in 6..=30 {
            read(s * 1_000, 7, hp(s));
        }
        // The pace doubles (players arrive): the line over the last 30 s,
        // part of it at the new pace, is already faster than the whole pull's
        // 14,118 a second (which gives 20).
        let faster = |s: i64| hp(30) - 20_000 * (s - 30);
        for s in 31..=50 {
            read(s * 1_000, 7, faster(s));
        }
        assert_eq!(read(51_000, 7, faster(51)), Some(16), "280,000 left");
        // After 30 s at the new pace, the time is at that pace alone.
        for s in 52..=59 {
            read(s * 1_000, 7, faster(s));
        }
        assert_eq!(read(60_000, 7, faster(60)), Some(5), "100,000 left at 20,000 a second");
        // A pause: from 5 s without a fall the whole pull's pace holds the
        // time, which grows slowly.
        for s in 61..=88 {
            read(s * 1_000, 7, 100_000);
        }
        assert_eq!(read(89_000, 7, 100_000), Some(10), "900,000 over 89 s");
        assert_eq!(read(90_000, 7, 100_000), Some(10), "900,000 over 90 s");
    }

    #[test]
    fn in_a_crowd_the_time_follows_a_drop_and_skips_a_pause() {
        let mut trail = HpTrail::default();
        let mut read = |s: i64, hp: i64| trail.update(s * 1_000, 9, s * 1_000, hp, CROWD);
        // 20,000 a second for 40 s, then players leave: 5,000 a second.
        for s in 0..=40 {
            read(s, 2_000_000 - 20_000 * s);
        }
        for s in 41..70 {
            read(s, 1_200_000 - 5_000 * (s - 40));
        }
        // 30 s at the new pace: the time is at that pace alone, not at the
        // whole pull's 13,571 a second (which gives 77).
        assert_eq!(read(70, 1_050_000), Some(210), "1,050,000 left at 5,000 a second");
        // A pause: from 5 s without a fall, the whole pull's pace.
        for s in 71..89 {
            read(s, 1_050_000);
        }
        assert_eq!(read(89, 1_050_000), Some(98), "950,000 over 89 s");
        // Falling again: the whole pull's pace for 5 s, then the recent pace
        // from where it fell again, without the pause (the whole pull's pace
        // gives 100).
        for s in 90..95 {
            read(s, 1_050_000 - 5_000 * (s - 90));
        }
        assert_eq!(read(95, 1_025_000), Some(205), "1,025,000 left at 5,000 a second");
    }
}
