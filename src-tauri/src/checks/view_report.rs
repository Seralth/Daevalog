//! Replays a capture and prints, every 100 ms of capture time, the meter's
//! reading and the rows `MeterView` makes of it, as the Qt meter reads them.
//!
//! ```text
//! A2_REPLAY_FILE=packets_x.txt   the capture (required)
//! A2_VIEW_MODE=trainTargets      the mode (default bossTargets)
//! A2_VIEW_LIMIT=6                rows shown (default 0: the meter's own)
//! A2_VIEW_PIN=1                  your row first
//! A2_VIEW_ASC=1                  smallest first
//! A2_VIEW_METRIC=damage          order by damage instead of DPS
//! cargo test --lib view_report -- --ignored --nocapture
//! ```
//!
//! Each line is `VIEW {"t", "user", "reading", "shown"}`: `reading` is the
//! reading's JSON as a string, null when it is the same as the last one;
//! `shown` is null when the rows are the same as the last ones.

use std::io::Write;
use std::sync::Arc;

use crate::capture::packet_log::{LogLine, PacketLogReplay};
use crate::capture::ping_tracker::PingTracker;
use crate::combat::data_storage::DataStorage;
use crate::combat::dps_calculator::DpsCalculator;
use crate::combat::meter_view::{MeterView, Metric, ViewOptions};

const TICK_MS: i64 = 100;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// The view's settings, each read by its variable's name through `var`.
fn view_options(var: impl Fn(&str) -> Option<String>) -> ViewOptions {
    ViewOptions {
        player_limit: var("A2_VIEW_LIMIT").and_then(|v| v.parse().ok()).unwrap_or(0),
        pin_you: var("A2_VIEW_PIN").is_some(),
        ascending: var("A2_VIEW_ASC").is_some(),
        metric: if var("A2_VIEW_METRIC").as_deref() == Some("damage") { Metric::Damage } else { Metric::Dps },
    }
}

/// Reads the meter as the Qt meter does and writes one `VIEW` line, leaving
/// out what has not changed since the last read.
struct Reader {
    meter: DpsCalculator,
    storage: Arc<DataStorage>,
    view: MeterView,
    options: ViewOptions,
    last_reading: String,
    last_shown: String,
}

impl Reader {
    fn read(&mut self, at: i64, out: &mut dyn Write) {
        crate::clock::set_override(Some(at));
        let reading = self.meter.get_dps();
        let user = self.storage.local_character_name().unwrap_or_default();
        let shown = self.view.update(&reading, &user, at, &self.options);
        let shown = serde_json::to_string(shown).expect("rows as JSON");
        let reading = serde_json::to_value(&reading).expect("reading as JSON").to_string();
        let reading_field = if reading == self.last_reading {
            "null".to_string()
        } else {
            serde_json::Value::String(reading.clone()).to_string()
        };
        let shown_field = if shown == self.last_shown { "null".to_string() } else { shown.clone() };
        let user = serde_json::Value::String(user).to_string();
        writeln!(out, "VIEW {{\"t\":{at},\"user\":{user},\"reading\":{reading_field},\"shown\":{shown_field}}}")
            .expect("stdout");
        self.last_reading = reading;
        self.last_shown = shown;
    }
}

#[test]
#[ignore]
fn view_report() {
    let path = env("A2_REPLAY_FILE").expect("A2_REPLAY_FILE");
    let mode = env("A2_VIEW_MODE").unwrap_or_else(|| "bossTargets".to_string());
    let (skills, npcs, dot_ids) = super::lookups();
    let storage = Arc::new(DataStorage::new());
    let mut replay = PacketLogReplay::new(Arc::clone(&storage), Arc::clone(&skills), Arc::clone(&npcs), dot_ids);
    let mut meter = DpsCalculator::new(Arc::clone(&storage), skills, npcs, Arc::new(PingTracker::new()));
    meter.set_target_selection_mode(&mode);
    let mut reader = Reader {
        meter,
        storage,
        view: MeterView::new(),
        options: view_options(env),
        last_reading: String::new(),
        last_shown: String::new(),
    };
    let text = std::fs::read_to_string(&path).expect("capture readable");
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    let mut due: Option<i64> = None;
    let mut last = None;
    for line in text.lines().filter_map(LogLine::parse) {
        let mut at = *due.get_or_insert(line.at_ms);
        while at <= line.at_ms {
            reader.read(at, &mut out);
            at += TICK_MS;
        }
        due = Some(at);
        replay.feed(&line);
        last = Some(line.at_ms);
    }
    if let Some(end) = last {
        reader.read(end, &mut out);
    }
    // A reader that closed early (`| head`) is no failure.
    let _ = out.flush();
    crate::clock::set_override(None);
}

#[test]
fn the_view_settings_come_from_the_environment() {
    let plain = view_options(|_| None);
    assert_eq!((plain.player_limit, plain.pin_you, plain.ascending, plain.metric), (0, false, false, Metric::Dps));
    let vars: std::collections::HashMap<&str, &str> =
        [("A2_VIEW_LIMIT", "4"), ("A2_VIEW_PIN", "1"), ("A2_VIEW_ASC", "1"), ("A2_VIEW_METRIC", "damage")].into();
    let set = view_options(|name| vars.get(name).map(|v| v.to_string()));
    assert_eq!((set.player_limit, set.pin_you, set.ascending, set.metric), (4, true, true, Metric::Damage));
}
