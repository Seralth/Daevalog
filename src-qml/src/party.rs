// SPDX-License-Identifier: AGPL-3.0-only

//! The meter's Heal and Tank tabs and the split view's healers: Details on
//! every player of the fight the meter shows, on the target Details opens on
//! from a meter row, made into rows by the backend's meter view
//! (`meter_view::party`). Details is read every `EVERY_MS` while the fight
//! runs and kept as the fight ended once its clock stands, as the DPS rows
//! are; the rows are cut each reading, so a change in Settings shows at once.

use daevalog::combat::details_view::{HealEntry, PlayerEntry, TankEntry};
use daevalog::combat::dps_calculator::DetailsSource;
use daevalog::combat::meter_view::{self, FightClock, Party, Shown, ViewOptions};
use daevalog::entity::dps_data::DpsData;

/// Details is read this often while a fight runs, in capture time: it asks
/// the backend more than the meter's reading does.
pub const EVERY_MS: i64 = 500;

/// A live re-read may take at most a tenth of the time between re-reads.
const COST_SHARE: i64 = 10;

/// The longest the live rows wait between re-reads in a big fight.
const MAX_GAP_MS: i64 = 5_000;

/// What the rows are made of: Details on every player.
#[derive(Debug, Clone, PartialEq)]
struct Read {
    players: Vec<PlayerEntry>,
    healers: Vec<HealEntry>,
    tanks: Vec<TankEntry>,
    battle_ms: i64,
    train: bool,
}

pub struct PartyFeed {
    source: DetailsSource,
    read: Option<Read>,
    read_at: Option<i64>,
    target: i32,
    /// Whether the last update was wanted: a tab opened reads at once.
    wanted: bool,
    /// True only for the live meter.
    timed: bool,
    /// The time between periodic re-reads; starts at EVERY_MS.
    gap_ms: i64,
}

impl PartyFeed {
    pub fn new(source: DetailsSource) -> Self {
        Self { source, read: None, read_at: None, target: 0, wanted: true, timed: false, gap_ms: EVERY_MS }
    }

    /// As `new`, for the live meter, whose re-reads space out by their cost.
    pub fn live(source: DetailsSource) -> Self {
        Self { timed: true, ..Self::new(source) }
    }

    /// What the last re-read cost in wall time sets the time to the next periodic one.
    fn note_cost(&mut self, cost_ms: i64) {
        self.gap_ms = (cost_ms.saturating_mul(COST_SHARE)).clamp(EVERY_MS, MAX_GAP_MS);
    }

    /// The rows with the meter's reading at `at` and the rows it shows:
    /// Details read again while the fight runs, every `EVERY_MS` (in the
    /// live meter `COST_SHARE` times what the last read cost, between
    /// `EVERY_MS` and `MAX_GAP_MS`), and at once when the meter's target
    /// changes, on `last`, and when `wanted` comes back on; kept once the
    /// clock stands or the meter has no rows, and while nothing on screen
    /// shows them (`wanted` off: reading Details for them took most of the
    /// live loop's time in a 400 against 400 fight). `None`
    /// before the first fight.
    pub fn update(&mut self, reading: &DpsData, shown: &Shown, at: i64, last: bool, train_all: bool, options: &ViewOptions, wanted: bool) -> Option<Party> {
        let running = matches!(shown.clock, FightClock::Fighting | FightClock::Grace) && !reading.map.is_empty();
        let reopened = wanted && !self.wanted;
        self.wanted = wanted;
        let due = last || reopened || reading.target_id != self.target || self.read_at.is_none_or(|t| at - t >= self.gap_ms || at < t);
        if running && due && wanted {
            let started = self.timed.then(std::time::Instant::now);
            self.read = Some(read(&self.source, reading, train_all));
            if let Some(started) = started {
                self.note_cost(i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX));
            }
            self.read_at = Some(at);
            self.target = reading.target_id;
        }
        self.read.as_ref().map(|r| meter_view::party(&r.players, &r.healers, &r.tanks, r.battle_ms, r.train, options))
    }
}

/// Details on every player, on the target it opens on from the meter.
fn read(source: &DetailsSource, reading: &DpsData, train_all: bool) -> Read {
    let reader = source.reader();
    let context = reader.party_context();
    let target = reader.details_opening_target(&context, reading.target_id, !reading.target_name.is_empty(), train_all);
    let view = reader.party_view_from(&context, target);
    Read {
        players: view.players,
        healers: view.healers,
        tanks: view.tanks,
        battle_ms: view.battle_ms,
        train: reading.target_mode == "trainTargets",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use daevalog::capture::ping_tracker::PingTracker;
    use daevalog::combat::data_storage::DataStorage;
    use daevalog::combat::dps_calculator::DpsCalculator;
    use daevalog::combat::meter_view::MeterView;
    use daevalog::entity::damage_packet::ParsedDamagePacket;
    use daevalog::i18n::lookup::{NpcLookup, SkillLookup};

    fn hit(storage: &DataStorage, actor: i32, target: i32, damage: i32, at: i64) {
        let mut p = ParsedDamagePacket::new();
        p.set_actor_id(actor);
        p.set_target_id(target);
        p.set_skill_code(16_010_000);
        p.set_damage(damage);
        p.set_timestamp(at);
        daevalog::clock::set_override(Some(at));
        storage.append_damage(p);
    }

    /// You (id 2, "Me") and a mob 700, on a meter picking targets by `mode`.
    fn me_and_a_mob(mode: &str) -> (Arc<DataStorage>, DpsCalculator) {
        let storage = Arc::new(DataStorage::new());
        storage.note_player_spawn(2);
        storage.append_nickname(2, "Me");
        storage.set_local_player_id(Some(2));
        storage.append_mob(700, 9001);
        let mut meter = DpsCalculator::new(storage.clone(), Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()), Arc::new(PingTracker::new()));
        meter.set_target_selection_mode(mode);
        (storage, meter)
    }

    #[test]
    fn details_are_read_only_while_their_rows_show_and_at_once_when_they_come_back() {
        let (storage, mut meter) = me_and_a_mob("allTargets");
        let mut feed = PartyFeed::new(meter.details_source());
        let mut view = MeterView::new();
        let options = ViewOptions::default();
        let mut step = |at: i64, wanted: bool| {
            hit(&storage, 2, 700, 500, at);
            let reading = meter.get_dps();
            let shown = view.update(&reading, "Me", at, &options).clone();
            let rows = feed.update(&reading, &shown, at, false, false, &options, wanted).is_some();
            (rows, feed.read_at)
        };
        assert_eq!(step(1_000, false), (false, None), "hidden: Details is not read");
        assert_eq!(step(2_000, true), (true, Some(2_000)), "shown: read");
        assert_eq!(step(2_100, true), (true, Some(2_000)), "shown: again only after EVERY_MS");
        assert_eq!(step(2_200, false), (true, Some(2_000)), "hidden: the last rows stay, nothing read");
        assert_eq!(step(2_300, true), (true, Some(2_300)), "shown again: read at once, inside EVERY_MS");
        daevalog::clock::set_override(None);
    }

    #[test]
    fn a_costly_read_spaces_out_the_next_ones() {
        let (storage, mut meter) = me_and_a_mob("allTargets");
        let mut feed = PartyFeed::new(meter.details_source());
        feed.note_cost(200);
        let mut view = MeterView::new();
        let options = ViewOptions::default();
        let mut step = |at: i64| {
            hit(&storage, 2, 700, 500, at);
            let reading = meter.get_dps();
            let shown = view.update(&reading, "Me", at, &options).clone();
            let rows = feed.update(&reading, &shown, at, false, false, &options, true).is_some();
            (rows, feed.read_at)
        };
        assert_eq!(step(1_000), (true, Some(1_000)), "first read, at once");
        assert_eq!(step(1_600), (true, Some(1_000)), "no read inside the gap");
        assert_eq!(step(3_000), (true, Some(3_000)), "read again after the gap");
        daevalog::clock::set_override(None);
    }

    #[test]
    fn the_gap_stays_between_its_limits() {
        let (_storage, meter) = me_and_a_mob("allTargets");
        let mut feed = PartyFeed::new(meter.details_source());
        feed.note_cost(0);
        assert_eq!(feed.gap_ms, EVERY_MS, "a free read waits the usual time");
        feed.note_cost(10_000);
        assert_eq!(feed.gap_ms, MAX_GAP_MS, "a slow read waits the longest");
        feed.note_cost(i64::MAX);
        assert_eq!(feed.gap_ms, MAX_GAP_MS, "no overflow");
    }

    #[test]
    fn a_new_target_reads_at_once_even_when_spaced_out() {
        // ALL always shows every target at once and has no single one.
        let (storage, mut meter) = me_and_a_mob("mostRecent");
        storage.append_mob(701, 9002);
        let mut feed = PartyFeed::new(meter.details_source());
        feed.note_cost(400);
        let mut view = MeterView::new();
        let options = ViewOptions::default();
        let mut step = |at: i64, target: i32| {
            hit(&storage, 2, target, 500, at);
            let reading = meter.get_dps();
            let shown = view.update(&reading, "Me", at, &options).clone();
            let rows = feed.update(&reading, &shown, at, false, false, &options, true).is_some();
            (rows, feed.read_at, reading.target_id)
        };
        assert_eq!(step(1_000, 700), (true, Some(1_000), 700), "first read, at once");
        assert_eq!(step(1_500, 700), (true, Some(1_000), 700), "no read inside the gap");
        assert_eq!(step(1_600, 701), (true, Some(1_600), 701), "a new target reads at once, inside the gap");
        daevalog::clock::set_override(None);
    }
}
