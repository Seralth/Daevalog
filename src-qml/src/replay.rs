// SPDX-License-Identifier: AGPL-3.0-only

//! `--replay`: a packet log fed through the backend's parser,
//! meter and meter view, in real time or as fast as the parser goes. The
//! meter is read every 100 ms of capture time, as the 1.x page polls it, at
//! that capture time, so a replay reads the same rows whatever its speed.
//! With `save`, it saves its boss fights to the data folder as the live meter
//! does, at the same capture times.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use daevalog::capture::packet_log::{LogLine, PacketLogReplay};
use daevalog::capture::ping_tracker::PingTracker;
use daevalog::clock;
use daevalog::combat::data_storage::DataStorage;
use daevalog::combat::dps_calculator::DpsCalculator;
use daevalog::combat::meter_view::MeterView;
use daevalog::history::live_save;
use daevalog::i18n::lookup::{load_language, NpcLookup, SkillLookup};
use daevalog::live_feed::dot_skill_ids;
use daevalog::share::ring;
use parking_lot::Mutex;

use crate::party::PartyFeed;
use crate::settings_store::{MeterApplied, Values};
use crate::snapshot::Snapshot;

/// How often the meter is read, in capture time.
pub const TICK_MS: i64 = 100;

/// In fast mode the window takes at most one reading per this long; the
/// readings in between are dropped, except a fight's last.
const FAST_SEND_EVERY: Duration = Duration::from_millis(40);

#[derive(Debug, Clone, PartialEq)]
pub struct Replay {
    pub file: PathBuf,
    pub fast: bool,
    /// Stop after this time of day, as the log writes it ("HH:MM:SS",
    /// fractions of a second allowed).
    pub until: Option<String>,
    /// The backend's mode id ("bossTargets").
    pub mode: String,
    /// Save the boss fights to the data folder.
    pub save: bool,
}

/// After the last payload, the replay ends as if the meter were closed this
/// much later, as the record check ends a capture.
const QUIT_AFTER_MS: i64 = 60_000;

/// Runs the replay on this thread, with game names in `language` and the
/// meter's `settings`. `send` gets each reading for the window; `true` marks
/// the last. Stops early when `send` returns false.
pub fn run(
    replay: &Replay,
    data: &Path,
    language: &str,
    settings: &Values,
    send: &mut dyn FnMut(Snapshot, bool) -> bool,
) -> Result<(), String> {
    let text = std::fs::read_to_string(&replay.file)
        .map_err(|e| format!("cannot read {}: {e}", replay.file.display()))?;

    let skills = Arc::new(SkillLookup::new());
    let npcs = Arc::new(NpcLookup::new());
    load_language(&skills, &npcs, data, language);
    let storage = Arc::new(DataStorage::new());
    storage.set_abnormal_stack_limits(daevalog::capture::abnormal::stack_limits(data));
    let mut feed = PacketLogReplay::new(storage.clone(), skills.clone(), npcs.clone(), dot_skill_ids(data));
    let mut meter = DpsCalculator::new(storage.clone(), skills, npcs, Arc::new(PingTracker::new()));
    meter.set_target_selection_mode(&replay.mode);
    MeterApplied::default().apply(settings, &storage, &mut meter);
    crate::details::set_source(meter.details_source(), storage.clone());
    let train_all = settings.training_mode == "all";
    let mut view = MeterView::new();
    let mut party = PartyFeed::new(meter.details_source());
    let mut trail = crate::kill_time::HpTrail::default();
    // Locked as the live feed locks it, so both save through the same calls.
    let meter = Mutex::new(meter);
    let saving = if replay.save { crate::history::store() } else { None };
    let mut next_save = 0;
    // The meter and its view as they stand at `at`; `last` at the end.
    let mut read = |at: i64, last: bool| {
        clock::set_override(Some(at));
        let reading = meter.lock().get_dps();
        let you = storage.local_character_name().unwrap_or_default();
        let options = crate::meter::view_options(settings);
        let shown = view.update(&reading, &you, at, &options);
        let mut snapshot = Snapshot::new(&reading, shown, at);
        // A replay reads them always: its views and screenshots open on any tab.
        snapshot.party = party.update(&reading, shown, at, last, train_all, &options, true);
        snapshot.boss_eta = trail.update(at, snapshot.target_id, snapshot.battle_ms, snapshot.boss_live_hp, crate::kill_time::crowd_map(reading.dungeon_id));
        snapshot
    };

    let mut pace = Pace::new(replay.fast);
    let mut out = Outbox::new(replay.fast, send);
    let mut next_tick: Option<i64> = None;
    let mut last_payload: Option<i64> = None;

    for line in text.lines() {
        let Some(line) = LogLine::parse(line) else { continue };
        if replay.until.as_deref().is_some_and(|until| past(line.time_of_day(), until)) {
            break;
        }
        let tick = next_tick.get_or_insert(line.at_ms);
        while *tick <= line.at_ms {
            pace.wait_for(*tick);
            if !out.offer(read(*tick, false)) {
                clock::set_override(None);
                return Ok(());
            }
            *tick += TICK_MS;
        }
        pace.wait_for(line.at_ms);
        feed.feed(&line);
        last_payload = Some(line.at_ms);
        if let Some((dir, history)) = saving {
            // What the live capture keeps in memory for the fights' slices.
            ring::record_at(line.at_ms, line.stream.to_string(), &line.bytes);
            if line.at_ms >= next_save {
                clock::set_override(Some(line.at_ms));
                live_save::auto_save(dir, history, &storage, &meter);
                next_save = line.at_ms + live_save::EVERY.as_millis() as i64;
            }
        }
    }

    // The last reading at the last payload, where the replay report reads
    // the meter too.
    let last = match last_payload {
        Some(at) => read(at, true),
        None => Snapshot::default(),
    };
    if let (Some((_, history)), Some(at)) = (saving, last_payload) {
        clock::set_override(Some(at + QUIT_AFTER_MS));
        live_save::save_before_quit(history, &meter);
    }
    clock::set_override(None);
    out.finish(last);
    Ok(())
}

/// A log's time of day is past `until` when its first `until.len()`
/// characters sort after it, as in the replay report's window.
fn past(time_of_day: &str, until: &str) -> bool {
    time_of_day.get(..until.len()).unwrap_or(time_of_day) > until
}

/// Sleeps so payloads arrive as far apart as they were captured; does
/// nothing in fast mode.
struct Pace {
    start: Option<(Instant, i64)>,
    fast: bool,
}

impl Pace {
    fn new(fast: bool) -> Self {
        Self { start: None, fast }
    }

    fn wait_for(&mut self, at_ms: i64) {
        if self.fast {
            return;
        }
        let (wall, capture) = *self.start.get_or_insert((Instant::now(), at_ms));
        let due = wall + Duration::from_millis((at_ms - capture).max(0) as u64);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }
}

/// Readings on their way to the window. In fast mode they are thinned out,
/// but the window always gets a fight's last reading before the next
/// fight's first, so it shows how each fight ended.
struct Outbox<'a> {
    fast: bool,
    send: &'a mut dyn FnMut(Snapshot, bool) -> bool,
    held: Option<Snapshot>,
    sent_at: Option<Instant>,
}

impl<'a> Outbox<'a> {
    fn new(fast: bool, send: &'a mut dyn FnMut(Snapshot, bool) -> bool) -> Self {
        Self { fast, send, held: None, sent_at: None }
    }

    /// False once the window is gone.
    fn offer(&mut self, reading: Snapshot) -> bool {
        if !self.fast {
            return (self.send)(reading, false);
        }
        if self.held.as_ref().is_some_and(|held| held.target_id != reading.target_id) {
            let fight_end = self.held.take().expect("held");
            if !(self.send)(fight_end, false) {
                return false;
            }
        }
        self.held = Some(reading);
        if self.sent_at.is_none_or(|at| at.elapsed() >= FAST_SEND_EVERY) {
            self.sent_at = Some(Instant::now());
            return (self.send)(self.held.take().expect("held"), false);
        }
        true
    }

    fn finish(mut self, last: Snapshot) {
        if self.held.as_ref().is_some_and(|held| held.target_id != last.target_id) {
            let fight_end = self.held.take().expect("held");
            (self.send)(fight_end, false);
        }
        (self.send)(last, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(target_id: i32, at_ms: i64) -> Snapshot {
        Snapshot { target_id, at_ms, ..Snapshot::default() }
    }

    #[test]
    fn until_compares_the_written_time_of_day() {
        assert!(!past("15:00:12.400000000+00:00", "15:00:12"));
        assert!(past("15:00:13.000000000+00:00", "15:00:12"));
        assert!(past("15:00:12.500+00:00", "15:00:12.4"));
    }

    #[test]
    fn fast_mode_keeps_each_fights_last_reading() {
        let mut got = Vec::new();
        let mut send = |s: Snapshot, done: bool| {
            got.push((s.target_id, s.at_ms, done));
            true
        };
        let mut out = Outbox::new(true, &mut send);
        for (target, at) in [(7, 1), (7, 2), (7, 3), (9, 4), (9, 5)] {
            out.offer(reading(target, at));
        }
        out.finish(reading(9, 6));
        // The first is sent at once; the rest within 40 ms are held, but
        // target 7's last goes out before target 9's first.
        assert_eq!(got, vec![(7, 1, false), (7, 3, false), (9, 6, true)]);
    }

    #[test]
    fn real_time_sends_every_reading() {
        let mut got = 0;
        let mut send = |_: Snapshot, _: bool| {
            got += 1;
            true
        };
        let mut out = Outbox::new(false, &mut send);
        out.offer(reading(7, 1));
        out.offer(reading(7, 2));
        out.finish(reading(7, 3));
        assert_eq!(got, 3);
    }
}
