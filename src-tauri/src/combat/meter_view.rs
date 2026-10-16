//! The rows the meter window draws, from the meter's readings: the rules the
//! 1.x page applied before drawing (public/src/js/core.js `fetchDps` and
//! `buildRowsFromMapObject`, meter.js `getDisplayRows` and `renderRows`), so
//! a front end shows these rows as they come.
//!
//! In order: rows by id, one per name; the last fight's rows kept on screen
//! while the meter has none, and frozen once the fight clock has stood 30 s;
//! TRAIN shows only you; rows sorted by the metric, ranked, cut to the top N
//! with your row always among them; shares and bars over the rows shown.
//!
//! The Heal and Tank tabs, new in 2.0, take their numbers from Details on
//! the fight the meter shows (`party`) and the same order, cut and TRAIN rule.

use std::collections::HashMap;

use serde::Serialize;

use crate::combat::data_storage::UNATTRIBUTED_ID;
use crate::combat::details_view::{HealEntry, PlayerEntry, TankEntry};
use crate::entity::dps_data::DpsData;
use crate::entity::job_class::JobClass;

/// The fight clock stands this long before it shows "grace", "ended" and
/// "idle" (core.js GRACE_ARM_MS and GRACE_MS, battleTime.js idleMs).
const GRACE_MS: i64 = 3_000;
const ENDED_MS: i64 = 30_000;
const IDLE_MS: i64 = 60_000;

/// Rows shown besides yours, until the player picks another number.
pub const DEFAULT_PLAYER_LIMIT: usize = 6;

/// What orders the rows and sizes the bars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Metric {
    #[default]
    Dps,
    Damage,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ViewOptions {
    /// Rows shown; your own row comes on top of these when it is not among
    /// them. 0 means the default.
    pub player_limit: usize,
    /// Your row first, whatever its rank.
    pub pin_you: bool,
    /// Smallest first.
    pub ascending: bool,
    pub metric: Metric,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self { player_limit: DEFAULT_PLAYER_LIMIT, pin_you: false, ascending: false, metric: Metric::Dps }
    }
}

/// The fight clock, as the header shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FightClock {
    /// No rows: no clock.
    #[default]
    Hidden,
    Fighting,
    /// No new damage for 3 s.
    Grace,
    /// No new damage for 30 s: the rows on screen stay as they are.
    Ended,
    /// No new damage for 60 s.
    Idle,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewRow {
    pub id: i32,
    /// The character name, or the id as text while the player has none yet.
    pub name: String,
    /// No name yet: shown as "#<id>".
    pub identifying: bool,
    /// The one row for summons and effects no player owns.
    pub unattributed: bool,
    /// The backend's class name, "Unknown", or empty.
    pub job: String,
    pub you: bool,
    /// Another player's number, shown with their class when other players'
    /// names are hidden; 0 for you and the unattributed row.
    pub number: u32,
    /// Place by the metric, 1 for the most, whatever the order and pinning.
    pub rank: usize,
    /// Whole numbers, as the page read them.
    pub damage: i64,
    pub dps: i64,
    /// Percent of the damage of the rows shown.
    pub share: f64,
    /// Bar length, 0 to 1: the row's metric against the biggest shown.
    pub fill: f64,
    pub combat_power: i64,
    pub active_dps: f64,
    pub last10_dps: f64,
    pub last30_dps: f64,
    pub last60_dps: f64,
    pub hits: i64,
    pub crit_hits: i64,
    pub max_hit: i64,
}

/// The footer's total: every row the mode shows, not only the top N.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Totals {
    pub damage: i64,
    pub dps: i64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Shown {
    /// In display order.
    pub rows: Vec<ViewRow>,
    /// Only with two rows or more.
    pub total: Option<Totals>,
    pub clock: FightClock,
}

/// Turns readings into the rows on screen. Keeps what the page kept between
/// readings: the last reading, the rows kept on screen and the fight clock.
#[derive(Debug, Default)]
pub struct MeterView {
    /// The last reading, and the options it was drawn with. The page redrew
    /// only when the reading changed; 2.0 also when the options do (lowest
    /// first, the player limit), or a new order would wait for the next hit.
    last_reading: Option<DpsData>,
    last_options: Option<ViewOptions>,
    last_battle_ms: i64,
    /// The rows kept on screen: `None` before any, and after a reset.
    kept: Option<Vec<ViewRow>>,
    clock: Clock,
    /// Whether the clock runs between readings.
    clock_on: bool,
    shown: Shown,
}

impl MeterView {
    pub fn new() -> Self {
        Self::default()
    }

    /// The meter was reset or switched modes: nothing kept, clock stopped.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Takes a reading made at `now_ms`. `user_name` is the local
    /// character's name as the meter knows it, empty when unknown.
    pub fn update(&mut self, reading: &DpsData, user_name: &str, now_ms: i64, options: &ViewOptions) -> &Shown {
        // Compared as values: the rows' map is a new one in each reading, in
        // its own order, and the same reading in another order is the same.
        // Not through JSON text: that took about an eighth of the meter's
        // time in a 400 against 400 fight on a minimum-spec CPU.
        if self.last_reading.as_ref() == Some(reading) && self.last_options.as_ref() == Some(options) {
            // The same reading: only the clock moves.
            self.clock.shown(self.clock_on);
            if self.clock_on {
                self.clock.update(now_ms, self.last_battle_ms);
            }
            self.shown.clock = self.clock.state;
            return &self.shown;
        }
        self.last_reading = Some(reading.clone());
        self.last_options = Some(options.clone());
        self.last_battle_ms = reading.battle_time;
        let local_id = reading.local_player_id.filter(|&id| id > 0);

        let rows = build_rows(reading, user_name, local_id);
        let out_of_combat = matches!(self.clock.state, FightClock::Ended | FightClock::Idle);
        let mut drawn = if rows.is_empty() {
            match &self.kept {
                Some(kept) => party_power_now(kept.clone(), reading),
                None => {
                    // Nothing yet: the screen stays as it is, without a clock.
                    self.clock_on = false;
                    self.clock.shown(false);
                    self.shown.clock = self.clock.state;
                    return &self.shown;
                }
            }
        } else if !out_of_combat {
            self.kept = Some(rows.clone());
            rows
        } else if let Some(kept) = self.kept.take() {
            // After a fight the rows stay as they ended; only your own row
            // takes the name the game has given you since.
            let kept = rename_you(&rows, kept, local_id);
            self.kept = Some(kept.clone());
            party_power_now(kept, reading)
        } else {
            self.kept = Some(rows.clone());
            rows
        };

        self.clock_on = !drawn.is_empty();
        self.clock.shown(self.clock_on);
        if self.clock_on {
            self.clock.update(now_ms, reading.battle_time);
        }

        if reading.target_mode == "trainTargets" && !user_name.trim().is_empty() && local_id.is_some() {
            drawn.retain(|row| !row.unattributed && row.name == user_name);
        }
        let total = (drawn.len() > 1).then(|| Totals {
            damage: drawn.iter().map(|r| r.damage).sum(),
            dps: drawn.iter().map(|r| r.dps).sum(),
        });
        self.shown = Shown { rows: on_screen(drawn, options), total, clock: self.clock.state };
        &self.shown
    }
}

/// The page's battle clock (battleTime.js): when the fight time last moved,
/// and what that says about the fight.
#[derive(Debug, Default)]
struct Clock {
    battle_ms: Option<i64>,
    changed_at: i64,
    state: FightClock,
}

impl Clock {
    fn shown(&mut self, on: bool) {
        if !on {
            self.state = FightClock::Hidden;
        }
    }

    fn update(&mut self, now_ms: i64, battle_ms: i64) {
        if self.battle_ms != Some(battle_ms) {
            self.battle_ms = Some(battle_ms);
            self.changed_at = now_ms;
            self.state = FightClock::Fighting;
            return;
        }
        let standing = (now_ms - self.changed_at).max(0);
        self.state = match standing {
            s if s >= IDLE_MS => FightClock::Idle,
            s if s >= ENDED_MS => FightClock::Ended,
            s if s >= GRACE_MS => FightClock::Grace,
            _ => FightClock::Fighting,
        };
    }
}

/// A non-number reached the page as null, which it read as 0.
fn number(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

/// One row per player in the reading, by id, then one per name.
fn build_rows(reading: &DpsData, user_name: &str, local_id: Option<i64>) -> Vec<ViewRow> {
    let mut ids: Vec<i32> = reading.map.keys().copied().filter(|&id| id > 0).collect();
    ids.sort_unstable();

    let mut rows: Vec<ViewRow> = Vec::with_capacity(ids.len());
    // Name -> place in `rows`. Two rows with one name keep the first one's
    // place and the better one's data: a known class first, then a name,
    // then the higher id.
    let mut by_name: HashMap<String, usize> = HashMap::new();
    for id in ids {
        let data = &reading.map[&id];
        let unattributed = id == UNATTRIBUTED_ID;
        let id_text = id.to_string();
        let named = unattributed || (!data.nickname.is_empty() && data.nickname != id_text);
        let name = if named { data.nickname.clone() } else { id_text };
        let row = ViewRow {
            id,
            you: (!unattributed && name == user_name) || local_id == Some(i64::from(id)),
            name,
            identifying: !named,
            unattributed,
            job: data.job.clone(),
            number: data.number,
            rank: 0,
            damage: number(data.amount).trunc() as i64,
            dps: number(data.dps).trunc() as i64,
            share: 0.0,
            fill: 0.0,
            combat_power: data.combat_power,
            active_dps: number(data.active_dps),
            last10_dps: number(data.last10_dps),
            last30_dps: number(data.last30_dps),
            last60_dps: number(data.last60_dps),
            hits: data.hits,
            crit_hits: data.crit_hits,
            max_hit: data.max_hit,
        };
        // The unattributed row's name on the page is its label, which no
        // player has.
        let key = if unattributed { "\u{0}unattributed".to_string() } else { row.name.clone() };
        match by_name.get(&key) {
            None => {
                by_name.insert(key, rows.len());
                rows.push(row);
            }
            Some(&at) if score(&row) >= score(&rows[at]) => rows[at] = row,
            Some(_) => {}
        }
    }
    rows
}

fn score(row: &ViewRow) -> u8 {
    u8::from(!row.job.is_empty()) * 2 + u8::from(!row.identifying)
}

/// Kept rows with your row renamed from this reading.
/// Kept rows take combat power from the reading, as fresh rows do: it is a
/// party fact, joined from the roster as it is now. A player who left keeps
/// their damage on the kept row but loses the power.
fn party_power_now(mut kept: Vec<ViewRow>, reading: &DpsData) -> Vec<ViewRow> {
    for row in &mut kept {
        // By id only under the same name: a member with no row yet is on
        // an id by roster slot, which the next one in that slot takes.
        row.combat_power = reading
            .map
            .get(&row.id)
            .filter(|p| row.identifying || p.nickname == row.name)
            .or_else(|| reading.map.values().find(|p| !row.identifying && p.nickname == row.name))
            .map_or(0, |p| p.combat_power);
    }
    kept
}

fn rename_you(rows: &[ViewRow], kept: Vec<ViewRow>, local_id: Option<i64>) -> Vec<ViewRow> {
    let Some(local_id) = local_id else { return kept };
    let Some(you) = rows.iter().find(|r| i64::from(r.id) == local_id && !r.identifying) else { return kept };
    kept.into_iter()
        .map(|mut row| {
            if i64::from(row.id) == local_id {
                row.name.clone_from(&you.name);
                row.identifying = false;
                row.you = you.you;
            }
            row
        })
        .collect()
}

/// Sorted by `metric`, ranked, and cut to the limit with your row kept.
fn cut<T: Clone>(
    mut rows: Vec<T>,
    options: &ViewOptions,
    metric: impl Fn(&T) -> i64,
    you: impl Fn(&T) -> bool,
    set_rank: impl Fn(&mut T, usize),
) -> Vec<T> {
    // Rank 1 is the most, whichever way the list runs. The sort is
    // stable, so equal rows keep the order they came in.
    rows.sort_by_key(|row| std::cmp::Reverse(metric(row)));
    for (i, row) in rows.iter_mut().enumerate() {
        set_rank(row, i + 1);
    }

    let limit = match options.player_limit {
        0 => DEFAULT_PLAYER_LIMIT,
        n => n,
    };
    let yours = rows.iter().position(|row| you(row));
    let mut shown: Vec<T> = rows.iter().take(limit).cloned().collect();
    if let Some(yours) = yours {
        if options.pin_you {
            shown.retain(|row| !you(row));
            shown.insert(0, rows[yours].clone());
        } else if !shown.iter().any(|row| you(row)) {
            shown.push(rows[yours].clone());
        }
    }
    // Lowest first: the same rows, the other way round; a pinned row
    // stays first.
    if options.ascending {
        let pinned = usize::from(options.pin_you && yours.is_some());
        shown[pinned..].reverse();
    }
    shown
}

/// Sorted, ranked, cut to the limit with your row kept, and measured.
fn on_screen(rows: Vec<ViewRow>, options: &ViewOptions) -> Vec<ViewRow> {
    let metric = |row: &ViewRow| match options.metric {
        Metric::Dps => row.dps,
        Metric::Damage => row.damage,
    };
    let mut shown = cut(rows, options, metric, |row| row.you, |row, rank| row.rank = rank);

    let top = shown.iter().map(metric).fold(1, i64::max);
    let damage: i64 = shown.iter().map(|row| row.damage).sum();
    for row in &mut shown {
        row.share = if damage > 0 { row.damage as f64 / damage as f64 * 100.0 } else { 0.0 };
        row.fill = (metric(row) as f64 / top as f64).clamp(0.0, 1.0);
    }
    shown
}

/// The split view lists Clerics and Chanters with at least this share of
/// the party's healing.
pub const HEALER_SHARE: f64 = 10.0;

/// A row of the Heal or Tank tab, or of the split view's healers.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartyRow {
    pub id: i32,
    /// The character name, or the id as text while the game has given none.
    pub name: String,
    pub identifying: bool,
    pub unattributed: bool,
    /// The backend's class name, or empty.
    pub job: String,
    pub you: bool,
    /// See `PersonalData::number`.
    pub number: u32,
    /// Place by `amount`, 1 for the most, whatever the order and pinning.
    pub rank: usize,
    /// Healing on the Heal tab, damage received on the Tank tab.
    pub amount: i64,
    /// Per second of the fight; `None` with no fight time.
    pub per_second: Option<f64>,
    /// Percent of everyone's.
    pub share: f64,
    /// Bar length, 0 to 1: `amount` against the biggest row shown.
    pub fill: f64,
    /// The damage the player received: the Heal tab's Taken column.
    pub taken: i64,
    pub shield_block: i64,
    pub parry: i64,
    /// The records behind `amount`: healing skill entries on the Heal tab,
    /// attacks received on the Tank tab. With none the numbers show "-".
    pub records: i64,
    /// The attacks behind `taken`, blocked and fully absorbed ones too.
    pub attacks: i64,
}

impl PartyRow {
    fn new(p: &PlayerEntry, (taken, attacks): (i64, i64), records: i64) -> Self {
        Self {
            id: p.id,
            name: p.name.clone(),
            identifying: p.identifying,
            unattributed: p.unattributed,
            job: p.job.clone(),
            you: p.you,
            number: p.number,
            rank: 0,
            amount: p.amount,
            per_second: p.per_second,
            share: p.share,
            fill: 0.0,
            taken,
            shield_block: 0,
            parry: 0,
            records,
            attacks,
        }
    }

    /// A player with none of the tab's number.
    fn none(p: &PlayerEntry, taken: (i64, i64), battle_ms: i64) -> Self {
        Self { amount: 0, per_second: (battle_ms > 0).then_some(0.0), share: 0.0, ..Self::new(p, taken, 0) }
    }
}

/// The Heal and Tank tabs and the split view's healers.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Party {
    /// In display order.
    pub heal: Vec<PartyRow>,
    pub tank: Vec<PartyRow>,
    /// The split view's healers, most healing first.
    pub healers: Vec<PartyRow>,
    /// The footer's group HPS and DTPS; `None` unless two players or more
    /// have some.
    pub heal_total: Option<f64>,
    pub tank_total: Option<f64>,
}

/// A player's name as rows match on it, `None` while the player has none
/// of their own: one player, though the ids differ, as Details matches its
/// players (`details_view::same_name_ids`).
fn match_name(p: &PlayerEntry) -> Option<String> {
    let named = !p.identifying && !p.unattributed && !p.name.trim().is_empty();
    named.then(|| p.name.trim().to_lowercase())
}

/// Where each player first appears in a list, by id and by `match_name`:
/// one lookup per player, where comparing every pair grew with the square
/// of the players and stalled the meter in large fights.
struct FirstEntry {
    by_id: HashMap<i32, usize>,
    by_name: HashMap<String, usize>,
}

impl FirstEntry {
    fn of<'a>(list: impl Iterator<Item = &'a PlayerEntry>) -> Self {
        let mut first = FirstEntry { by_id: HashMap::new(), by_name: HashMap::new() };
        for (i, p) in list.enumerate() {
            first.by_id.entry(p.id).or_insert(i);
            if let Some(name) = match_name(p) {
                first.by_name.entry(name).or_insert(i);
            }
        }
        first
    }

    /// The first entry that is `p`, by id or by name.
    fn of_player(&self, p: &PlayerEntry) -> Option<usize> {
        let by_name = match_name(p).and_then(|name| self.by_name.get(&name).copied());
        self.by_id.get(&p.id).copied().into_iter().chain(by_name).min()
    }
}

/// The Heal and Tank tabs from Details on every player of the fight the
/// meter shows: `players` (DMG's), `healers` (HEAL's party bars) and `tanks`
/// (TAKEN with each player picked), over its time `battle_ms`. Every tab
/// lists every player: those with none of a tab's number get a row with
/// none. Then TRAIN shows only you (`train`), and the rows are sorted, ranked
/// and cut as the DPS tab's; shares stay over everyone, as Details has them.
pub fn party(
    players: &[PlayerEntry],
    healers: &[HealEntry],
    tanks: &[TankEntry],
    battle_ms: i64,
    train: bool,
    options: &ViewOptions,
) -> Party {
    let healer_at = FirstEntry::of(healers.iter().map(|h| &h.player));
    let tank_at = FirstEntry::of(tanks.iter().map(|t| &t.player));
    let taken = |p: &PlayerEntry| tank_at.of_player(p).map_or((0, 0), |i| (tanks[i].player.amount, tanks[i].attacks));
    let mut heal: Vec<PartyRow> = healers.iter().map(|h| PartyRow::new(&h.player, taken(&h.player), h.heals)).collect();
    let mut tank: Vec<PartyRow> = tanks
        .iter()
        .map(|t| PartyRow { shield_block: t.shield_block, parry: t.parry, ..PartyRow::new(&t.player, (t.player.amount, t.attacks), t.attacks) })
        .collect();
    for p in players.iter().filter(|p| !p.unattributed) {
        if healer_at.of_player(p).is_none() {
            heal.push(PartyRow::none(p, taken(p), battle_ms));
        }
        if tank_at.of_player(p).is_none() {
            tank.push(PartyRow::none(p, (0, 0), battle_ms));
        }
    }

    let total = |rows: &[PartyRow]| {
        let with = rows.iter().filter(|r| r.amount > 0).count();
        let sum: i64 = rows.iter().map(|r| r.amount).sum();
        (with > 1 && battle_ms > 0).then(|| sum as f64 / battle_ms as f64 * 1000.0)
    };
    let heal_total = total(&heal);
    let tank_total = total(&tank);

    let cleric = JobClass::Cleric.class_name();
    let chanter = JobClass::Chanter.class_name();
    let healers: Vec<PartyRow> = heal
        .iter()
        .filter(|r| (r.job == cleric || r.job == chanter) && r.amount > 0 && r.share >= HEALER_SHARE)
        .cloned()
        .collect();
    let all = ViewOptions { player_limit: usize::MAX, pin_you: false, ascending: false, metric: Metric::Damage };

    let shown = |mut rows: Vec<PartyRow>, options: &ViewOptions| {
        if train && rows.iter().any(|r| r.you && !r.unattributed) {
            rows.retain(|r| r.you && !r.unattributed);
        }
        let mut rows = cut(rows, options, |r| r.amount, |r| r.you, |r, rank| r.rank = rank);
        let top = rows.iter().map(|r| r.amount).fold(1, i64::max);
        for r in &mut rows {
            r.fill = (r.amount as f64 / top as f64).clamp(0.0, 1.0);
        }
        rows
    };
    Party {
        heal: shown(heal, options),
        tank: shown(tank, options),
        healers: shown(healers, &all),
        heal_total,
        tank_total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_players_first_entry_is_the_first_a_scan_of_every_pair_finds() {
        use crate::combat::details_view::PlayerEntry;
        let entry = |id: i32, name: &str, identifying: bool, unattributed: bool| PlayerEntry {
            id, name: name.into(), identifying, unattributed, job: String::new(), number: 0, you: false, amount: 0, per_second: None, share: 0.0,
        };
        // The pairwise rule the lookup replaced.
        let same = |a: &PlayerEntry, b: &PlayerEntry| {
            a.id == b.id || matches!((match_name(a), match_name(b)), (Some(x), Some(y)) if x == y)
        };
        let names = ["Ari", " ari ", "ARI", "Bo", "", "Cy", "bo"];
        let list: Vec<PlayerEntry> =
            (0..60).map(|i| entry((i % 9) as i32, names[i % names.len()], i % 5 == 0, i % 11 == 0)).collect();
        let first = FirstEntry::of(list.iter());
        let mut askers = list.clone();
        askers.extend((0..14).map(|i| entry(100 + i as i32, names[i % names.len()], i % 3 == 0, false)));
        for p in &askers {
            assert_eq!(first.of_player(p), list.iter().position(|e| same(e, p)), "{p:?}");
        }
    }
    use crate::entity::personal_data::PersonalData;

    fn player(name: &str, job: &str, damage: f64, dps: f64) -> PersonalData {
        let mut p = PersonalData::with_job(name.into(), job.into());
        p.amount = damage;
        p.dps = dps;
        p
    }

    /// A reading with these rows: (id, name, damage), DPS a tenth of the
    /// damage, every row with a class.
    fn reading(mode: &str, battle_ms: i64, rows: &[(i32, &str, f64)]) -> DpsData {
        let mut data = DpsData::new();
        data.target_mode = mode.into();
        data.battle_time = battle_ms;
        for &(id, name, damage) in rows {
            data.map.insert(id, player(name, "검성", damage, damage / 10.0));
        }
        data
    }

    fn ids(shown: &Shown) -> Vec<i32> {
        shown.rows.iter().map(|r| r.id).collect()
    }

    fn eight() -> Vec<(i32, &'static str, f64)> {
        vec![
            (11, "a", 800.0), (12, "b", 700.0), (13, "c", 600.0), (14, "d", 500.0),
            (15, "e", 400.0), (16, "f", 300.0), (17, "g", 200.0), (18, "me", 100.0),
        ]
    }

    #[test]
    fn two_rows_with_one_name_become_the_better_one_in_the_first_ones_place() {
        let mut data = reading("bossTargets", 1_000, &[(5, "twin", 900.0), (7, "other", 500.0)]);
        // Same name, higher id, no class yet: the row with a class wins.
        data.map.insert(9, player("twin", "", 100.0, 10.0));
        let mut view = MeterView::new();
        let shown = view.update(&data, "", 0, &ViewOptions::default());
        assert_eq!(ids(shown), vec![5, 7]);

        // Both with a class: the higher id wins, in the first one's place.
        data.map.insert(9, player("twin", "검성", 950.0, 95.0));
        let mut view = MeterView::new();
        let shown = view.update(&data, "", 0, &ViewOptions::default());
        assert_eq!(ids(shown), vec![9, 7]);
        assert_eq!(shown.rows[0].damage, 950);
    }

    #[test]
    fn each_row_keeps_the_players_number() {
        let mut data = reading("bossTargets", 1_000, &[(5, "me", 900.0), (7, "other", 500.0)]);
        data.map.get_mut(&7).unwrap().number = 3;
        let mut view = MeterView::new();
        let shown = view.update(&data, "me", 0, &ViewOptions::default());
        let numbers: Vec<(i32, u32)> = shown.rows.iter().map(|r| (r.id, r.number)).collect();
        assert_eq!(numbers, vec![(5, 0), (7, 3)]);
    }

    #[test]
    fn a_player_without_a_name_shows_the_id_and_ids_below_one_are_dropped() {
        let mut data = reading("bossTargets", 1_000, &[(0, "zero", 900.0), (-3, "minus", 900.0)]);
        data.map.insert(21, player("21", "검성", 500.0, 50.0));
        data.map.insert(22, player("", "", 400.0, 40.0));
        data.map.insert(UNATTRIBUTED_ID, player("80000000", "Unknown", 300.0, 30.0));
        let mut view = MeterView::new();
        let shown = view.update(&data, "", 0, &ViewOptions::default());
        assert_eq!(ids(shown), vec![21, 22, UNATTRIBUTED_ID]);
        assert!(shown.rows[0].identifying && shown.rows[1].identifying);
        assert_eq!((shown.rows[0].name.as_str(), shown.rows[1].name.as_str()), ("21", "22"));
        let summons = &shown.rows[2];
        assert!(summons.unattributed && !summons.identifying && !summons.you);
    }

    #[test]
    fn you_are_the_bound_id_or_your_name() {
        let mut data = reading("bossTargets", 1_000, &[(5, "me", 900.0), (6, "other", 500.0), (7, "alt", 100.0)]);
        data.local_player_id = Some(7);
        let mut view = MeterView::new();
        let shown = view.update(&data, "me", 0, &ViewOptions::default());
        let you: Vec<i32> = shown.rows.iter().filter(|r| r.you).map(|r| r.id).collect();
        assert_eq!(you, vec![5, 7]);
    }

    #[test]
    fn train_shows_only_you_once_the_meter_knows_you() {
        let rows = [(5, "me", 900.0), (6, "other", 500.0)];
        let mut data = reading("trainTargets", 1_000, &rows);
        let mut view = MeterView::new();
        assert_eq!(ids(view.update(&data, "me", 0, &ViewOptions::default())), vec![5, 6], "no local id yet");

        data.local_player_id = Some(5);
        let mut view = MeterView::new();
        let shown = view.update(&data, "me", 0, &ViewOptions::default());
        assert_eq!(ids(shown), vec![5]);
        assert_eq!(shown.total, None, "one row has no total");

        let mut view = MeterView::new();
        assert_eq!(ids(view.update(&data, "", 0, &ViewOptions::default())), vec![5, 6], "no name yet");

        data.target_mode = "allTargets".into();
        let mut view = MeterView::new();
        assert_eq!(ids(view.update(&data, "me", 0, &ViewOptions::default())), vec![5, 6], "only TRAIN");
    }

    #[test]
    fn the_top_rows_and_yours_below_them() {
        let data = reading("bossTargets", 1_000, &eight());
        let mut view = MeterView::new();
        let shown = view.update(&data, "me", 0, &ViewOptions::default());
        assert_eq!(ids(shown), vec![11, 12, 13, 14, 15, 16, 18]);
        assert_eq!(shown.rows[6].rank, 8);

        let mut view = MeterView::new();
        let options = ViewOptions { player_limit: 8, ..ViewOptions::default() };
        assert_eq!(ids(view.update(&data, "me", 0, &options)), vec![11, 12, 13, 14, 15, 16, 17, 18]);

        // Nobody is you: just the top rows.
        let mut view = MeterView::new();
        assert_eq!(view.update(&data, "", 0, &ViewOptions { player_limit: 0, ..ViewOptions::default() }).rows.len(), 6);
    }

    #[test]
    fn pinned_your_row_comes_first_with_its_own_rank() {
        let data = reading("bossTargets", 1_000, &eight());
        let options = ViewOptions { pin_you: true, ..ViewOptions::default() };
        let mut view = MeterView::new();
        let shown = view.update(&data, "me", 0, &options);
        assert_eq!(ids(shown), vec![18, 11, 12, 13, 14, 15, 16]);
        assert_eq!(shown.rows[0].rank, 8);

        // Within the top rows: first, and no row more.
        let mut view = MeterView::new();
        let shown = view.update(&data, "c", 0, &options);
        assert_eq!(ids(shown), vec![13, 11, 12, 14, 15, 16]);
    }

    #[test]
    fn ranks_follow_the_metric_whichever_way_the_list_runs() {
        let data = reading("bossTargets", 1_000, &[(1, "a", 300.0), (2, "b", 100.0), (3, "c", 300.0), (4, "d", 200.0)]);
        let mut view = MeterView::new();
        let down = view.update(&data, "", 0, &ViewOptions::default()).clone();
        assert_eq!(ids(&down), vec![1, 3, 4, 2]);
        assert_eq!(down.rows.iter().map(|r| r.rank).collect::<Vec<_>>(), vec![1, 2, 3, 4]);

        let mut view = MeterView::new();
        let up = view.update(&data, "", 0, &ViewOptions { ascending: true, ..ViewOptions::default() }).clone();
        assert_eq!(ids(&up), vec![2, 4, 3, 1], "the same rows reversed");
        assert_eq!(up.rows.iter().map(|r| r.rank).collect::<Vec<_>>(), vec![4, 3, 2, 1]);
    }

    #[test]
    fn lowest_first_shows_the_top_rows_reversed_with_their_shares() {
        // A crowd: the top rows the other way round, not the lowest players.
        let data = reading("bossTargets", 1_000, &eight());
        let mut view = MeterView::new();
        let down = view.update(&data, "", 0, &ViewOptions::default()).clone();
        let mut view = MeterView::new();
        let options = ViewOptions { ascending: true, ..ViewOptions::default() };
        let up = view.update(&data, "", 0, &options).clone();
        assert_eq!(ids(&up), vec![16, 15, 14, 13, 12, 11]);
        let shares = |s: &Shown| s.rows.iter().map(|r| (r.id, r.share, r.rank)).collect::<Vec<_>>();
        let mut reversed = shares(&down);
        reversed.reverse();
        assert_eq!(shares(&up), reversed, "the same shares and ranks");

        // Your row below the top rows comes first, as the lowest; pinned,
        // it stays first.
        let mut view = MeterView::new();
        assert_eq!(ids(view.update(&data, "me", 0, &options)), vec![18, 16, 15, 14, 13, 12, 11]);
        let mut view = MeterView::new();
        let pinned = ViewOptions { pin_you: true, ..options };
        assert_eq!(ids(view.update(&data, "c", 0, &pinned)), vec![13, 16, 15, 14, 12, 11]);
    }

    #[test]
    fn new_options_redraw_the_same_reading() {
        // 2.0's Lowest first flips the order between two hits: the same
        // reading must not keep the old order.
        let data = reading("bossTargets", 1_000, &[(1, "a", 300.0), (2, "b", 100.0), (3, "c", 200.0)]);
        let mut view = MeterView::new();
        assert_eq!(ids(view.update(&data, "", 0, &ViewOptions::default())), vec![1, 3, 2]);
        let up = ViewOptions { ascending: true, ..ViewOptions::default() };
        assert_eq!(ids(view.update(&data, "", 0, &up)), vec![2, 3, 1]);
        let two = ViewOptions { player_limit: 2, ..ViewOptions::default() };
        assert_eq!(ids(view.update(&data, "", 0, &two)), vec![1, 3]);
    }

    #[test]
    fn damage_can_order_the_rows_instead_of_dps() {
        let mut data = reading("bossTargets", 1_000, &[(1, "a", 300.0), (2, "b", 100.0)]);
        // b's DPS is higher: a shorter time in the fight.
        data.map.get_mut(&2).unwrap().dps = 90.0;
        let mut view = MeterView::new();
        assert_eq!(ids(view.update(&data, "", 0, &ViewOptions::default())), vec![2, 1]);
        let mut view = MeterView::new();
        let options = ViewOptions { metric: Metric::Damage, ..ViewOptions::default() };
        let shown = view.update(&data, "", 0, &options);
        assert_eq!(ids(shown), vec![1, 2]);
        assert_eq!(shown.rows[1].fill, 100.0 / 300.0);
    }

    #[test]
    fn shares_and_bars_count_only_the_rows_shown_and_the_total_counts_all() {
        let data = reading("bossTargets", 1_000, &eight());
        let mut view = MeterView::new();
        let options = ViewOptions { player_limit: 2, ..ViewOptions::default() };
        let shown = view.update(&data, "", 0, &options);
        assert_eq!(ids(shown), vec![11, 12]);
        assert_eq!(shown.rows[0].share, 800.0 / 1500.0 * 100.0);
        assert_eq!(shown.rows[1].fill, 70.0 / 80.0);
        assert_eq!(shown.total, Some(Totals { damage: 3600, dps: 360 }));
    }

    #[test]
    fn numbers_are_cut_to_whole_ones_and_a_bad_number_reads_as_zero() {
        let mut data = reading("bossTargets", 1_000, &[(1, "a", 0.0)]);
        let a = data.map.get_mut(&1).unwrap();
        a.amount = 1234.9;
        a.dps = f64::NAN;
        let mut view = MeterView::new();
        let shown = view.update(&data, "", 0, &ViewOptions::default());
        assert_eq!((shown.rows[0].damage, shown.rows[0].dps), (1234, 0));
        assert_eq!(shown.rows[0].fill, 0.0, "a metric of 0 against the floor of 1");
    }

    #[test]
    fn the_last_rows_stay_while_the_meter_has_none() {
        let mut view = MeterView::new();
        let options = ViewOptions::default();
        let empty = reading("bossTargets", 0, &[]);
        assert_eq!(view.update(&empty, "", 0, &options).clock, FightClock::Hidden);

        let fight = reading("bossTargets", 5_000, &[(1, "a", 300.0), (2, "b", 100.0)]);
        assert_eq!(view.update(&fight, "", 100, &options).clock, FightClock::Fighting);
        let shown = view.update(&empty, "", 200, &options);
        assert_eq!(ids(shown), vec![1, 2]);
        assert_eq!(shown.clock, FightClock::Fighting);

        view.reset();
        let shown = view.update(&empty, "", 300, &options);
        assert_eq!(shown.clock, FightClock::Hidden);
        assert!(shown.rows.is_empty());
    }

    #[test]
    fn kept_rows_drop_the_combat_power_of_players_who_left() {
        let mut view = MeterView::new();
        let options = ViewOptions::default();
        let mut fight = reading("bossTargets", 5_000, &[(1, "a", 300.0), (2, "b", 100.0)]);
        fight.map.get_mut(&1).unwrap().combat_power = 44_000;
        fight.map.get_mut(&2).unwrap().combat_power = 39_000;
        view.update(&fight, "", 0, &options);
        assert_eq!(view.update(&fight, "", 30_000, &options).clock, FightClock::Ended);

        // b leaves: the reading still has b's damage, without a roster power.
        let mut left = fight.clone();
        left.map.get_mut(&2).unwrap().combat_power = 0;
        left.map.get_mut(&1).unwrap().combat_power = 44_100;
        let shown = view.update(&left, "", 30_100, &options).clone();
        let power = |shown: &Shown, id: i32| shown.rows.iter().find(|r| r.id == id).map(|r| r.combat_power);
        assert_eq!(ids(&shown), vec![1, 2], "the rows stay as they ended");
        assert_eq!((power(&shown, 1), power(&shown, 2)), (Some(44_100), Some(0)));

        // The party ends: the combat data goes, a's row is kept, a's roster
        // row alone is in the reading; b is gone from it.
        let mut alone = reading("bossTargets", 0, &[]);
        let mut a = player("a", "Cleric", 0.0, 0.0);
        a.combat_power = 44_100;
        alone.map.insert(9_000_001, a);
        let shown = view.update(&alone, "", 30_200, &options).clone();
        assert_eq!((power(&shown, 1), power(&shown, 2)), (Some(44_100), Some(0)));
    }

    #[test]
    fn a_kept_row_takes_no_power_from_the_next_member_on_its_id() {
        // Before a fight the rows are the party's, on ids by roster slot. b
        // left and c took the slot: b's kept row must not show c's power.
        let mut view = MeterView::new();
        let options = ViewOptions::default();
        let mut party = reading("bossTargets", 0, &[(1, "a", 0.0), (90_000_005, "b", 0.0)]);
        party.map.get_mut(&90_000_005).unwrap().combat_power = 60_000;
        view.update(&party, "", 0, &options);
        assert_eq!(view.update(&party, "", 30_000, &options).clock, FightClock::Ended);

        let mut swapped = reading("bossTargets", 0, &[(1, "a", 0.0), (90_000_005, "c", 0.0)]);
        swapped.map.get_mut(&90_000_005).unwrap().combat_power = 59_000;
        let shown = view.update(&swapped, "", 30_100, &options);
        let kept: Vec<(&str, i64)> = shown.rows.iter().map(|r| (r.name.as_str(), r.combat_power)).collect();
        assert_eq!(kept, [("a", 0), ("b", 0)]);
    }

    #[test]
    fn the_clock_says_grace_ended_and_idle_as_the_fight_time_stands() {
        let mut view = MeterView::new();
        let options = ViewOptions::default();
        let fight = reading("bossTargets", 5_000, &[(1, "a", 300.0)]);
        for (at, clock) in [(0, FightClock::Fighting), (2_999, FightClock::Fighting), (3_000, FightClock::Grace),
            (30_000, FightClock::Ended), (60_000, FightClock::Idle)] {
            assert_eq!(view.update(&fight, "", at, &options).clock, clock, "at {at}");
        }
        let more = reading("bossTargets", 6_000, &[(1, "a", 400.0)]);
        assert_eq!(view.update(&more, "", 61_000, &options).clock, FightClock::Fighting);
    }

    #[test]
    fn after_a_fight_the_rows_stay_as_they_ended_but_your_name_comes_in() {
        let mut view = MeterView::new();
        let options = ViewOptions::default();
        let mut fight = reading("bossTargets", 5_000, &[(1, "a", 300.0), (2, "2", 100.0)]);
        fight.local_player_id = Some(2);
        view.update(&fight, "", 0, &options);
        assert_eq!(view.update(&fight, "", 30_000, &options).clock, FightClock::Ended);

        // A party member joins and the game names you, with no new damage.
        let mut later = reading("bossTargets", 5_000, &[(1, "a", 300.0), (2, "me", 100.0), (3, "c", 0.0)]);
        later.local_player_id = Some(2);
        let shown = view.update(&later, "me", 30_100, &options).clone();
        assert_eq!(ids(&shown), vec![1, 2]);
        assert_eq!(shown.rows[1].name, "me");
        assert!(!shown.rows[1].identifying && shown.rows[1].you);

        // New damage restarts the clock; the next new reading is drawn.
        let mut next = reading("bossTargets", 1_000, &[(1, "a", 50.0), (3, "c", 20.0)]);
        next.local_player_id = Some(2);
        assert_eq!(ids(view.update(&next, "me", 40_000, &options)), vec![1, 2], "this one still sees the old clock");
        assert_eq!(ids(view.update(&next, "me", 40_100, &options)), vec![1, 2], "the same reading is not redrawn");
        next.map.get_mut(&1).unwrap().amount = 60.0;
        assert_eq!(ids(view.update(&next, "me", 40_200, &options)), vec![1, 3]);
    }

    #[test]
    fn the_same_rows_in_another_map_order_are_the_same_reading() {
        // Each reading is a new map, and its order changes from one to the
        // next. A new fight's first reading after a fight has ended is drawn
        // with the old rows; the same reading in another order then counted
        // as new and was drawn at once, in the same order not until the next
        // hit, so two replays of one capture could differ.
        let mut view = MeterView::new();
        let options = ViewOptions::default();
        let fight = reading("bossTargets", 5_000, &eight());
        view.update(&fight, "", 0, &options);
        assert_eq!(view.update(&fight, "", 30_000, &options).clock, FightClock::Ended);
        let next = || {
            reading("bossTargets", 1_000, &[
                (11, "a", 10.0), (12, "b", 20.0), (13, "c", 30.0), (14, "d", 40.0),
                (15, "e", 50.0), (16, "f", 60.0), (17, "g", 70.0), (18, "me", 80.0),
            ])
        };
        let kept = [11, 12, 13, 14, 15, 16];
        assert_eq!(ids(view.update(&next(), "", 40_000, &options)), kept, "this one still sees the old clock");
        for at in 1..=10 {
            assert_eq!(ids(view.update(&next(), "", 40_000 + at * 100, &options)), kept, "the same reading, {at}");
        }
    }

    fn entry(id: i32, name: &str, job: JobClass, amount: i64, share: f64) -> PlayerEntry {
        PlayerEntry {
            id,
            name: name.into(),
            identifying: false,
            unattributed: false,
            job: job.class_name().into(),
            number: 0,
            you: false,
            amount,
            per_second: Some(amount as f64 / 10.0),
            share,
        }
    }

    fn tank(player: PlayerEntry, shield_block: i64, parry: i64) -> TankEntry {
        TankEntry { player, attacks: 9, shield_block, parry }
    }

    fn heal(player: PlayerEntry, heals: i64) -> HealEntry {
        HealEntry { player, heals }
    }

    fn party_ids(rows: &[PartyRow]) -> Vec<i32> {
        rows.iter().map(|r| r.id).collect()
    }

    #[test]
    fn every_tab_lists_every_player() {
        let players = [
            entry(1, "Ann", JobClass::Gladiator, 900, 60.0),
            entry(2, "Bo", JobClass::Cleric, 300, 20.0),
            entry(3, "Cy", JobClass::Chanter, 300, 20.0),
        ];
        let healers = [heal(entry(2, "Bo", JobClass::Cleric, 800, 80.0), 3), heal(entry(3, "Cy", JobClass::Chanter, 200, 20.0), 1)];
        // Another id of Ann's: one player all the same.
        let tanks = [tank(entry(8, "ann", JobClass::Gladiator, 700, 70.0), 4, 1), tank(entry(3, "Cy", JobClass::Chanter, 300, 30.0), 0, 0)];
        let party = party(&players, &healers, &tanks, 10_000, false, &ViewOptions::default());

        assert_eq!(party_ids(&party.heal), [2, 3, 1]);
        assert_eq!((party.heal[2].amount, party.heal[2].per_second, party.heal[2].taken), (0, Some(0.0), 700));
        assert_eq!(party.heal.iter().map(|r| r.fill).collect::<Vec<_>>(), [1.0, 0.25, 0.0]);
        assert_eq!(party.heal.iter().map(|r| r.rank).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(party_ids(&party.tank), [8, 3, 2]);
        assert_eq!((party.tank[0].shield_block, party.tank[0].parry), (4, 1));
        assert_eq!(party.heal_total, Some(100.0));
        assert_eq!(party.tank_total, Some(100.0));
        assert_eq!(party_ids(&party.healers), [2, 3], "a Cleric and a Chanter with 10% or more");
    }

    #[test]
    fn each_row_counts_the_records_behind_its_numbers() {
        let players = [entry(1, "Ann", JobClass::Gladiator, 900, 60.0), entry(2, "Bo", JobClass::Cleric, 300, 40.0)];
        let healers = [heal(entry(2, "Bo", JobClass::Cleric, 800, 100.0), 3)];
        // Ann blocked every attack: 5 attacks, no damage.
        let tanks = [TankEntry { player: entry(1, "Ann", JobClass::Gladiator, 0, 0.0), attacks: 5, shield_block: 5, parry: 0 }];
        let party = party(&players, &healers, &tanks, 10_000, false, &ViewOptions::default());
        let counts = |rows: &[PartyRow]| rows.iter().map(|r| (r.id, r.amount, r.records, r.attacks)).collect::<Vec<_>>();
        // Heal: Bo's 3 entries, no attack on him; Ann healed nothing, took 5 attacks.
        assert_eq!(counts(&party.heal), [(2, 800, 3, 0), (1, 0, 0, 5)]);
        // Tank: Ann's 5 attacks behind her 0; Bo received none.
        assert_eq!(counts(&party.tank), [(1, 0, 5, 5), (2, 0, 0, 0)]);
    }

    #[test]
    fn heal_and_tank_rows_are_cut_as_the_dps_rows() {
        let mut players: Vec<PlayerEntry> = (1..=8).map(|id| entry(id, &format!("p{id}"), JobClass::Templar, 100, 0.0)).collect();
        players[7].you = true;
        let tanks: Vec<TankEntry> = (1..=8).map(|id| tank(entry(id, &format!("p{id}"), JobClass::Templar, i64::from(9 - id) * 100, 0.0), 0, 0)).collect();
        let mut tanks = tanks;
        tanks[7].player.you = true;
        let healers = [heal(entry(4, "p4", JobClass::Chanter, 50, 5.0), 1)];

        let party = party(&players, &healers, &tanks, 10_000, false, &ViewOptions::default());
        assert_eq!(party_ids(&party.tank), [1, 2, 3, 4, 5, 6, 8], "the top six and you");
        assert_eq!(party.tank[6].rank, 8);
        assert_eq!(party.heal_total, None, "one player healed");
        assert!(party.healers.is_empty(), "a Chanter under 10%");

        let pinned = ViewOptions { pin_you: true, ascending: true, ..ViewOptions::default() };
        let party = super::party(&players, &healers, &tanks, 10_000, false, &pinned);
        assert_eq!(party_ids(&party.tank), [8, 6, 5, 4, 3, 2, 1], "you first, then the top six reversed");

        let party = super::party(&players, &healers, &tanks, 10_000, true, &ViewOptions::default());
        assert_eq!((party_ids(&party.heal), party_ids(&party.tank)), (vec![8], vec![8]), "TRAIN shows only you");
    }
}
