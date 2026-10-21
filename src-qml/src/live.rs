// SPDX-License-Identifier: AGPL-3.0-only

//! Live capture: the backend's live feed (capture helper, dispatcher, game
//! port detection, meter) read every 100 ms through its meter view, as the
//! 1.x page polled it. Its boss fights are saved to the data folder as 1.x
//! saves them: every 30 s, and once more when the program quits. The name
//! setting goes to the backend at the start and takes over the name the game
//! gives, as 1.x's meter does.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant};

use cxx_qt::CxxQtThread;
use daevalog::clock;
use daevalog::combat::dps_calculator::DpsCalculator;
use daevalog::combat::meter_view::MeterView;
use daevalog::config::setting_changes::{apply_character_name, CHARACTER_NAME_KEY};
use daevalog::history::live_save;
use daevalog::live_feed::LiveFeed;

use crate::meter::qobject::Meter;
use crate::party::PartyFeed;
use crate::settings_store::{MeterApplied, Values};
use crate::snapshot::Snapshot;

const TICK: Duration = Duration::from_millis(100);

/// How long quitting waits for the fights to be saved, as 1.x.
const QUIT_SAVE_WAIT: Duration = Duration::from_secs(5);

/// The running feed, for `Capture` and the save when the program quits.
static FEED: OnceLock<Arc<LiveFeed>> = OnceLock::new();
/// Set by Save & Clear and the mode menu: the next reading starts the rows
/// afresh, as 1.x's page did after a reset or a mode change. Without it the
/// meter kept the last fight's rows on screen.
static CLEAR_VIEW: AtomicBool = AtomicBool::new(false);

/// Asks the live loop to start the rows afresh at its next reading.
fn request_clear() {
    CLEAR_VIEW.store(true, Ordering::SeqCst);
}

/// A pending clear, once: the rows and the Heal and Tank tabs' last read go.
fn take_clear(view: &mut MeterView) -> bool {
    let pending = CLEAR_VIEW.swap(false, Ordering::SeqCst);
    if pending {
        view.reset();
    }
    pending
}

/// The live feed, once it runs; never during a replay.
pub fn feed() -> Option<Arc<LiveFeed>> {
    FEED.get().cloned()
}

/// Starts capturing and feeding the window on a thread of its own, with
/// game names in `language`.
pub fn start(qt: CxxQtThread<Meter>, language: &'static str) {
    std::thread::spawn(move || {
        let mut language = language;
        let data = crate::data_dir();
        if data.is_none() {
            eprintln!("daevalog-dps-meter: the game data was not found; skills and monsters go by number");
        }
        let notify = |available: bool| {
            if !available {
                eprintln!("daevalog-dps-meter: packet capture is not available (see the log above)");
            }
            crate::capture::show_available(available);
        };
        // The saved device is the preference before the first packet.
        let device = Some(crate::capture::saved_device()).filter(|d| !d.is_empty());
        let feed = match LiveFeed::start(data.as_deref(), language, crate::capture_helper(), device, notify) {
            Ok(feed) => Arc::new(feed),
            Err(error) => {
                eprintln!("daevalog-dps-meter: {error}");
                let _ = qt.queue(|_| crate::shell::ffi::exit_application(1));
                return;
            }
        };
        apply_character_name(&feed.storage, &crate::capture::saved_name(), false);
        let _ = FEED.set(feed.clone());
        start_auto_save(feed.clone());
        let settings = crate::settings_store::store();
        let mut default_mode = settings.values().default_mode;
        feed.meter.lock().set_target_selection_mode(&default_mode);
        crate::details::set_source(feed.meter.lock().details_source(), feed.storage.clone());

        let mut view = MeterView::new();
        let mut party = PartyFeed::live(feed.meter.lock().details_source());
        let mut trail = crate::snapshot::HpTrail::default();
        let mut applied = MeterApplied::default();
        let mut next = Instant::now();
        loop {
            // Read each time: a change in Settings shows at the next reading.
            let values = settings.values();
            applied.apply(&values, &feed.storage, &mut feed.meter.lock());
            if let Some(changed) = new_language(&mut language, &values) {
                feed.set_language(changed);
            }
            let hour24 = values.uses_24h(crate::settings_store::system_clock_24h());
            crate::pet_souls::tick(&feed.storage, language, hour24);
            crate::power_shards::tick(&feed.storage, language, values.shard_warning);
            crate::world_spawns::tick(language, hour24, values.spawn_notices, &values.server_time_zone);
            if follow_default_mode(&mut default_mode, &values, &mut feed.meter.lock()) {
                request_clear();
            }
            // Before the reading, so a mode picked now is the one it shows.
            let cleared = take_clear(&mut view);
            if cleared {
                party = PartyFeed::live(feed.meter.lock().details_source());
            }
            let reading = feed.meter.lock().get_dps();
            let now = clock::now_ms();
            let you = feed.storage.local_character_name().unwrap_or_default();
            // Against the saved name: a typed name is the backend's too, so
            // it stays until the game names you again (a zone load).
            if let Some(name) = crate::capture::name_from_game(&feed.storage) {
                if crate::capture::saved_name() != name {
                    settings.set_raw(CHARACTER_NAME_KEY, &name);
                }
            }
            let options = crate::meter::view_options(&values);
            let shown = view.update(&reading, &you, now, &options);
            let mut snapshot = Snapshot::new(&reading, shown, now);
            let wanted = crate::meter::party_shown();
            snapshot.party = party.update(&reading, shown, now, false, values.training_mode == "all", &options, wanted);
            snapshot.boss_eta = trail.update(now, snapshot.target_id, snapshot.boss_live_hp);
            let ping = feed.ping.current_ping_ms();
            // Paused, the screen holds while the meter counts on underneath,
            // as a paused stream; a clear or a mode the player picks still
            // shows. The ping stays live.
            let show = !crate::capture::paused() || cleared;
            let shown = qt.queue(move |mut meter| {
                if show {
                    meter.as_mut().show(snapshot, false);
                }
                meter.show_ping(ping);
            });
            // The window is gone.
            if shown.is_err() {
                break;
            }
            next += TICK;
            match next.checked_duration_since(Instant::now()) {
                Some(wait) => std::thread::sleep(wait),
                // Behind: start over from now rather than catch up.
                None => next = Instant::now(),
            }
        }
        feed.stop();
    });
}

/// Saves the boss fights every 30 s, on a thread of its own so the readings
/// go on while it writes, and uploads the finished ones when auto-upload is
/// on.
fn start_auto_save(feed: Arc<LiveFeed>) {
    let Some((dir, history)) = crate::history::store() else { return };
    std::thread::spawn(move || loop {
        std::thread::sleep(live_save::EVERY);
        let saved = live_save::auto_save(dir, history, &feed.storage, &feed.meter);
        crate::account::auto_upload(dir, history, saved.as_ref());
    });
}

/// The mode menu's pick, for the backend at once; the next reading shows it.
/// The setting "Default mode" stays as it is.
pub fn set_mode(mode: &str) {
    if let Some(feed) = FEED.get() {
        if switch_mode(&mut feed.meter.lock(), mode) {
            request_clear();
        }
    }
}

/// Puts the meter in `mode`. True when that changed the mode: only then do
/// the rows start afresh, as 1.x's page.
fn switch_mode(meter: &mut DpsCalculator, mode: &str) -> bool {
    let before = meter.target_selection_mode_id();
    meter.set_target_selection_mode(mode);
    meter.target_selection_mode_id() != before
}

/// The game names' language when Settings > Language changed it from
/// `last`: 1.x loads the names again at once, rather than keeping the
/// start's language.
fn new_language(last: &mut &'static str, values: &Values) -> Option<&'static str> {
    let now = crate::i18n::resolve(values.language.as_deref());
    if now == *last {
        return None;
    }
    *last = now;
    Some(now)
}

/// A new Settings > Default mode switches the running meter, as 1.x's
/// (`settings/panel.js`), not at the next start.
/// `last`: the default mode last seen. True when the rows start afresh.
fn follow_default_mode(last: &mut String, values: &Values, meter: &mut DpsCalculator) -> bool {
    if values.default_mode == *last {
        return false;
    }
    last.clone_from(&values.default_mode);
    switch_mode(meter, last)
}

/// Save & Clear: the backend's reset (1.x's header button), then a save at
/// once, so the fights it ended are in History as finished without waiting
/// for the next auto-save. The meter can be busy reading for a moment, so
/// the save tries again a few times. Nothing during a replay.
pub fn save_and_clear() {
    let Some(feed) = FEED.get().cloned() else { return };
    feed.meter.lock().reset_combat();
    request_clear();
    let Some((dir, history)) = crate::history::store() else { return };
    std::thread::spawn(move || {
        for _ in 0..20 {
            if feed.storage.damage_generation() == 0 {
                return;
            }
            if let Some(saved) = live_save::auto_save(dir, history, &feed.storage, &feed.meter) {
                crate::account::auto_upload(dir, history, Some(&saved));
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });
}

/// Saves every fight as it stands before the program quits. Waits at most
/// `QUIT_SAVE_WAIT`: a stuck disk cannot keep the program open.
pub fn save_before_quit() {
    let (Some(feed), Some((_, history))) = (FEED.get().cloned(), crate::history::store()) else { return };
    let (saved, wait) = mpsc::channel();
    std::thread::spawn(move || {
        live_save::save_before_quit(history, &feed.meter);
        let _ = saved.send(());
    });
    if wait.recv_timeout(QUIT_SAVE_WAIT).is_err() {
        eprintln!("daevalog-dps-meter: the fights were not saved in time; quitting anyway");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meter() -> DpsCalculator {
        use daevalog::capture::ping_tracker::PingTracker;
        use daevalog::i18n::lookup::{NpcLookup, SkillLookup};
        use daevalog::combat::data_storage::DataStorage;
        DpsCalculator::new(
            Arc::new(DataStorage::new()),
            Arc::new(SkillLookup::new()),
            Arc::new(NpcLookup::new()),
            Arc::new(PingTracker::new()),
        )
    }

    #[test]
    fn a_new_language_loads_the_names_once() {
        let mut last = "en";
        let values = Values { language: Some("de".into()), ..Values::default() };
        assert_eq!(new_language(&mut last, &values), Some("de"));
        assert_eq!(new_language(&mut last, &values), None, "once");
        let values = Values { language: Some("en".into()), ..Values::default() };
        assert_eq!(new_language(&mut last, &values), Some("en"), "and back");
    }

    #[test]
    fn a_new_default_mode_switches_the_running_meter() {
        let mut meter = meter();
        let mut last = "bossTargets".to_string();
        meter.set_target_selection_mode(&last);
        let mut values = Values { default_mode: "encounter".into(), ..Values::default() };
        assert!(follow_default_mode(&mut last, &values, &mut meter), "a new mode clears the rows");
        assert_eq!(meter.target_selection_mode_id(), "encounter");
        assert!(!follow_default_mode(&mut last, &values, &mut meter), "once");
        // The mode menu got there first: no clear, as 1.x.
        assert!(switch_mode(&mut meter, "allTargets"));
        values.default_mode = "allTargets".into();
        assert!(!follow_default_mode(&mut last, &values, &mut meter));
        assert!(!switch_mode(&mut meter, "allTargets"), "the same pick again keeps the rows");
        // The menu's pick stays until the default changes again.
        assert!(switch_mode(&mut meter, "bossTargets"));
        assert!(!follow_default_mode(&mut last, &values, &mut meter));
        assert_eq!(meter.target_selection_mode_id(), "bossTargets");
    }

    #[test]
    fn save_and_clear_and_a_mode_change_clear_the_rows_once() {
        let mut view = MeterView::new();
        assert!(!take_clear(&mut view), "nothing asked");
        request_clear();
        assert!(take_clear(&mut view), "the next reading clears");
        assert!(!take_clear(&mut view), "and only that one");
    }
}
