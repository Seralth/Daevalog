// SPDX-License-Identifier: AGPL-3.0-only

//! `History`, the QML singleton behind the History window: the saved fights
//! in the data folder, filtered and grouped, and the chosen fight with its
//! game records. The list, the fight and every rule are the backend's
//! (`daevalog::history::history_view`), read from the same answers the 1.x
//! History page reads, on a thread of their own and queued onto the Qt
//! thread as JSON. This keeps the filters, picks, and names classes by
//! their theme key.
//!
//! Deleting a fight, uploading it and the share preview are the backend's
//! calls 1.x makes; their outcome lands in `note`.

use core::pin::Pin;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use cxx_qt::{CxxQtThread, CxxQtType, Threading};
use serde_json::json;
use cxx_qt_lib::QString;
use daevalog::combat::details_view::{self, DetailsView, LookupNames, Words};
use daevalog::combat::timeline_tracks::{self, TrackNames, Tracks};
use daevalog::entity::fight_record::{FightRecord, FightSummary};
use daevalog::game_record::files::{Checker, FightStatus};
use daevalog::history::fight_history::FightHistoryManager;
use daevalog::history::history_view::{self, Clock, Dungeons, Filters, FightPlayers, Layout, ListInput, ListNames, ListWords, CLASS_KEYS};
use daevalog::i18n::lookup::{load_language, NpcLookup, SkillLookup};

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QString, list, READ, NOTIFY)]
        #[qproperty(QString, fight, READ, NOTIFY)]
        #[qproperty(QString, chosen, READ, NOTIFY)]
        #[qproperty(bool, ready, READ, NOTIFY)]
        #[qproperty(bool, records_checked, READ, NOTIFY)]
        #[qproperty(QString, layout, READ, WRITE = change_layout, NOTIFY)]
        #[qproperty(bool, show_training, READ, WRITE = change_show_training, NOTIFY)]
        #[qproperty(QString, boss, READ, WRITE = change_boss, NOTIFY)]
        #[qproperty(QString, job, READ, WRITE = change_job, NOTIFY)]
        #[qproperty(QString, date, READ, WRITE = change_date, NOTIFY)]
        #[qproperty(bool, only_records, READ, WRITE = change_only_records, NOTIFY)]
        #[qproperty(QString, note, READ, NOTIFY)]
        #[qproperty(QString, player, READ, NOTIFY)]
        type History = super::HistoryRust;

        fn change_layout(self: Pin<&mut History>, value: QString);
        fn change_show_training(self: Pin<&mut History>, value: bool);
        fn change_boss(self: Pin<&mut History>, value: QString);
        fn change_job(self: Pin<&mut History>, value: QString);
        fn change_date(self: Pin<&mut History>, value: QString);
        fn change_only_records(self: Pin<&mut History>, value: bool);

        /// Reads the saved fights again.
        #[qinvokable]
        fn refresh(self: Pin<&mut History>);

        /// Shows this fight.
        #[qinvokable]
        fn choose(self: Pin<&mut History>, id: QString);

        /// Writes the list and the fight shown again, for a change of
        /// Display Time.
        #[qinvokable]
        fn redraw(self: Pin<&mut History>);

        /// Deletes the fight shown, with the packets kept for it. The window
        /// asks first.
        #[qinvokable]
        fn delete_chosen(self: Pin<&mut History>);

        /// Uploads the fight shown to the signed-in a2tools.app account, on
        /// a thread of its own; the outcome lands in `note`.
        #[qinvokable]
        fn upload_chosen(self: Pin<&mut History>);

        /// Writes what uploading the fight shown would send, and sends
        /// nothing; the outcome lands in `note`.
        #[qinvokable]
        fn preview_chosen(self: Pin<&mut History>);

        /// The fight shown's log on a2tools.app, once uploaded; empty
        /// otherwise, and for a link off the site.
        #[qinvokable]
        fn log_url(self: &History) -> QString;
        /// Shows one player of the fight shown in `player`: their skills,
        /// heals and damage received, as Details shows them picked.
        #[qinvokable]
        fn open_player(self: Pin<&mut History>, id: i32);

        /// Closes the player shown.
        #[qinvokable]
        fn close_player(self: Pin<&mut History>);

        /// A fight was uploaded outside the window (auto-upload): an open
        /// window reads again, so its Upload button becomes Open log, as
        /// 1.x's page on `fight-uploaded`.
        #[qsignal]
        fn uploaded_elsewhere(self: Pin<&mut History>);
    }

    impl cxx_qt::Threading for History {}
}

/// The History object's thread, from its first reading on: an upload made
/// elsewhere tells it through this.
static QT: OnceLock<CxxQtThread<qobject::History>> = OnceLock::new();

/// Tells History a fight was uploaded outside its window.
pub fn uploaded_elsewhere() {
    if let Some(qt) = QT.get() {
        let _ = qt.queue(|history| history.uploaded_elsewhere());
    }
}

/// The 1.x settings History keeps, as 1.x saves them.
const LAYOUT_KEY: &str = "historyViewMode";
const TRAINING_KEY: &str = "historyShowTraining";

/// The names History writes, in one language.
pub struct Lookups {
    language: &'static str,
    skills: Arc<SkillLookup>,
    npcs: Arc<NpcLookup>,
    dungeons: Dungeons,
    list_words: ListWords,
    words: Words,
}

impl Lookups {
    fn new(data: Option<&Path>, language: &'static str) -> Self {
        let (skills, npcs) = (SkillLookup::new(), NpcLookup::new());
        let mut dungeons = Dungeons::default();
        if let Some(data) = data {
            load_language(&skills, &npcs, data, language);
            dungeons = Dungeons::load(data, language);
        }
        let text = |key: &str| crate::i18n::text(language, key);
        Self {
            language,
            skills: Arc::new(skills),
            npcs: Arc::new(npcs),
            dungeons,
            list_words: ListWords {
                open_world: text("history.openWorld"),
                classes: CLASS_KEYS.iter().map(|(job, key)| (job.to_string(), text(&format!("classes.{key}")))).collect(),
            },
            words: Words { unnamed_skill: text("skills.fallback"), dot: text("skills.dot") },
        }
    }

    /// The names, with times of day on the clock Display Time picks now.
    fn names(&self) -> ListNames<'_> {
        let clock = Clock::new(crate::settings_store::clock_24h(), self.language);
        ListNames { npcs: &self.npcs, dungeons: &self.dungeons, words: &self.list_words, zone: None, clock }
    }
}

static LOOKUPS: Mutex<Option<Arc<Lookups>>> = Mutex::new(None);

/// The names in the window's language, loaded once per language.
fn lookups() -> Arc<Lookups> {
    let language = crate::i18n::resolve(crate::settings_store::store().values().language.as_deref());
    let mut held = LOOKUPS.lock().unwrap_or_else(|e| e.into_inner());
    match held.as_ref() {
        Some(l) if l.language == language => l.clone(),
        _ => {
            let fresh = Arc::new(Lookups::new(crate::data_dir().as_deref(), language));
            *held = Some(fresh.clone());
            fresh
        }
    }
}

/// The data folder's saved fights, read once per run: it keeps the summaries.
/// The live feed saves through it too.
pub(crate) fn store() -> Option<&'static (PathBuf, FightHistoryManager)> {
    static STORE: OnceLock<Option<(PathBuf, FightHistoryManager)>> = OnceLock::new();
    STORE
        .get_or_init(|| {
            let dir = crate::settings_store::folder()?;
            let history = FightHistoryManager::new(dir.clone());
            Some((dir, history))
        })
        .as_ref()
}

/// Details on a saved fight (History's Details button): `player` is -1 for
/// you (every player when the fight does not name you), 0 for every player,
/// else that player. With the timeline's tracks from the records the fight
/// was saved with, and the day and time it was fought. `None` when the
/// fight is gone.
/// The player Details shows in a saved fight: -1 is you, by the id the
/// fight was saved with, else by your name; 0 is every player.
fn saved_player(record: &FightRecord, player: i32, you: &str) -> Option<i32> {
    match player {
        p if p < 0 => record
            .actors
            .iter()
            .find(|a| a.actor_id == record.timeline.you)
            .or_else(|| record.actors.iter().find(|a| details_view::saved_is_you(&a.nickname, you)))
            .map(|a| a.actor_id),
        0 => None,
        p => Some(p),
    }
}

pub(crate) fn saved_details(id: &str, player: i32, track_names: &TrackNames) -> Option<(DetailsView, Tracks, serde_json::Value)> {
    let (_, history) = store()?;
    let record = history.load_fight(id).ok()?;
    let lookups = lookups();
    let names = LookupNames { skills: &lookups.skills, npcs: &lookups.npcs, words: &lookups.words };
    let you = your_name();
    let view = match saved_player(&record, player, &you) {
        Some(p) => details_view::saved_player_view(&record, &you, p, &names),
        None => details_view::saved_view(&record, &you, &names),
    };
    let skill = |code: u32| i32::try_from(code).map(|c| lookups.skills.lookup_skill_name(c)).unwrap_or_default();
    let tracks = timeline_tracks::saved_tracks(&record, &view, skill, track_names);
    Some((view, tracks, live_title(record.start_time_ms, record.dungeon_id)))
}

/// The Details title's start and difficulty for the live fight: when its
/// first hit came, as History writes a time, and the difficulty of the
/// dungeon the meter is in, as 1.x's title takes it (`lastDungeonId`).
pub(crate) fn live_title(start_ms: i64, dungeon_id: i32) -> serde_json::Value {
    let lookups = lookups();
    let names = lookups.names();
    json!({
        "date": if start_ms > 0 { names.date(start_ms) } else { String::new() },
        "tier": if dungeon_id != 0 { names.dungeons.tier(dungeon_id).map(|t| t.label) } else { None },
    })
}

/// Your name: the meter's, else the one 1.x last saw.
fn your_name() -> String {
    crate::details::your_name()
        .or_else(|| crate::settings_store::store().raw("dpsMeter.userName"))
        .map(|n| n.trim().to_string())
        .unwrap_or_default()
}

/// What the list is built from: the backend's answers, read together.
#[derive(Clone, Default)]
struct Data {
    fights: Vec<FightSummary>,
    players: Arc<HashMap<String, FightPlayers>>,
    /// `None` until the game records are checked.
    records: Option<HashMap<String, FightStatus>>,
    uploaded: HashSet<String>,
    you: String,
}

/// Where everything is read from.
#[derive(Clone)]
struct Place {
    dir: PathBuf,
    history: &'static FightHistoryManager,
    /// Where the game keeps its records.
    roots: Vec<PathBuf>,
}

fn place() -> Option<Place> {
    let (dir, history) = store()?;
    Some(Place { dir: dir.clone(), history, roots: daevalog::game_record::files::record_roots() })
}

fn read(place: &Place, you: String) -> Data {
    let (fights, players) = place.history.listing();
    let uploaded = daevalog::share::share_status(&place.dir)
        .into_iter()
        .filter(|(_, s)| s.url.is_some())
        .map(|(id, _)| id)
        .collect();
    Data { fights, players, records: None, uploaded, you }
}

fn checker(place: &Place, data: &Data, lookups: &Lookups) -> Checker {
    Checker {
        app_data_dir: place.dir.clone(),
        data_dir: crate::data_dir(),
        skills: lookups.skills.clone(),
        npcs: lookups.npcs.clone(),
        fights: data.fights.clone(),
        roots: place.roots.clone(),
        zone: None,
    }
}

/// The list as the window reads it.
fn list_json(data: &Data, filters: &Filters, lookups: &Lookups) -> String {
    let none = HashMap::new();
    let input = ListInput {
        fights: &data.fights,
        players: &data.players,
        records: data.records.as_ref().unwrap_or(&none),
        uploaded: &data.uploaded,
        you: &data.you,
    };
    let list = history_view::list(&input, filters, &lookups.names());
    let mut value = serde_json::to_value(&list).unwrap_or_default();
    // Classes by their theme key, for the icons.
    for group in value["groups"].as_array_mut().into_iter().flatten() {
        for fight in group["fights"].as_array_mut().into_iter().flatten() {
            for job in fight["jobs"].as_array_mut().into_iter().flatten() {
                *job = crate::snapshot::class_key(job.as_str().unwrap_or_default()).into();
            }
        }
    }
    for choice in value["jobs"].as_array_mut().into_iter().flatten() {
        choice["icon"] = crate::snapshot::class_key(choice["value"].as_str().unwrap_or_default()).into();
    }
    value.to_string()
}

/// The first fight the list shows.
/// The fight to show after a reading: the one shown, read again, since its
/// numbers, In progress, upload and game records can have changed (else a
/// fight saved as finished stays In progress with Upload hidden); else the list's first; `None`
/// when the list is empty.
fn to_show<'a>(chosen: &str, mut fights: impl Iterator<Item = &'a str>, list: &str) -> Option<String> {
    if !chosen.is_empty() && fights.any(|id| id == chosen) {
        return Some(chosen.to_string());
    }
    first_shown(list)
}

fn first_shown(list: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(list).ok()?;
    value["groups"].as_array()?.iter().find_map(|g| g["fights"][0]["id"].as_str().map(str::to_string))
}

/// One fight as the window reads it; `None` when it is gone.
fn fight_json(place: &Place, data: &Data, id: &str, lookups: &Lookups) -> Option<String> {
    let record = place.history.load_fight(id).ok()?;
    let views = checker(place, data, lookups).views(id);
    let names = LookupNames { skills: &lookups.skills, npcs: &lookups.npcs, words: &lookups.words };
    let view = history_view::fight(&record, &data.you, data.uploaded.contains(id), place.history.is_in_progress(id), &views, &names, &lookups.names());
    let mut value = serde_json::to_value(&view).unwrap_or_default();
    theme_jobs(&mut value);
    Some(value.to_string())
}

fn theme_jobs(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                match v {
                    serde_json::Value::String(job) if key == "job" => *job = crate::snapshot::class_key(job).to_string(),
                    _ => theme_jobs(v),
                }
            }
        }
        serde_json::Value::Array(list) => list.iter_mut().for_each(theme_jobs),
        _ => {}
    }
}

pub struct HistoryRust {
    /// The list as JSON; empty until the first reading.
    list: QString,
    /// The chosen fight as JSON; empty with none.
    fight: QString,
    chosen: QString,
    /// The list is read.
    ready: bool,
    /// The game records are checked too.
    records_checked: bool,
    /// "dungeon", "grouped" or "list", as 1.x saves it.
    layout: QString,
    show_training: bool,
    boss: QString,
    job: QString,
    date: QString,
    only_records: bool,
    /// An upload's or a preview's outcome for the fight shown, as JSON:
    /// {"kind": "uploading" | "uploaded" | "previewing" | "previewed" |
    /// "failed", ...}; empty with none.
    note: QString,
    /// The player opened in the fight shown, as JSON: {"id", "skills",
    /// "heals", "taken"}; empty with none.
    player: QString,
    data: Option<Arc<Data>>,
    /// Bumped by every reading and every pick; an older answer is dropped.
    reading: u64,
    picking: u64,
}

impl Default for HistoryRust {
    fn default() -> Self {
        let settings = crate::settings_store::store();
        let layout = Layout::from_setting(&settings.raw(LAYOUT_KEY).unwrap_or_default());
        Self {
            list: QString::default(),
            fight: QString::default(),
            chosen: QString::default(),
            ready: false,
            records_checked: false,
            layout: QString::from(layout.setting()),
            show_training: settings.raw(TRAINING_KEY).as_deref() != Some("0"),
            boss: QString::default(),
            job: QString::default(),
            date: QString::default(),
            only_records: false,
            note: QString::default(),
            player: QString::default(),
            data: None,
            reading: 0,
            picking: 0,
        }
    }
}

/// Sets a property to `$value` and signals it, if it changed; true if so.
macro_rules! show {
    ($this:ident, $field:ident, $signal:ident, $value:expr) => {{
        let value = $value;
        let changed = $this.$field != value;
        if changed {
            $this.as_mut().rust_mut().$field = value;
            $this.as_mut().$signal();
        }
        changed
    }};
}

impl qobject::History {
    fn filters(&self) -> Filters {
        Filters {
            layout: Layout::from_setting(&self.layout.to_string()),
            boss: self.boss.to_string(),
            job: self.job.to_string(),
            date: self.date.to_string(),
            show_training: self.show_training,
            only_records: self.only_records,
        }
    }

    fn change_layout(mut self: Pin<&mut Self>, value: QString) {
        let layout = Layout::from_setting(&value.to_string());
        if show!(self, layout, layout_changed, QString::from(layout.setting())) {
            crate::settings_store::store().set_raw(LAYOUT_KEY, layout.setting());
            self.relist();
        }
    }

    fn change_show_training(mut self: Pin<&mut Self>, value: bool) {
        if show!(self, show_training, show_training_changed, value) {
            crate::settings_store::store().set_raw(TRAINING_KEY, if value { "1" } else { "0" });
            self.relist();
        }
    }

    fn change_boss(mut self: Pin<&mut Self>, value: QString) {
        if show!(self, boss, boss_changed, value) {
            self.relist();
        }
    }

    fn change_job(mut self: Pin<&mut Self>, value: QString) {
        if show!(self, job, job_changed, value) {
            self.relist();
        }
    }

    fn change_date(mut self: Pin<&mut Self>, value: QString) {
        if show!(self, date, date_changed, value) {
            self.relist();
        }
    }

    fn change_only_records(mut self: Pin<&mut Self>, value: bool) {
        if show!(self, only_records, only_records_changed, value) {
            self.relist();
        }
    }

    /// The list again from what was read, with the filters as they are.
    fn relist(mut self: Pin<&mut Self>) {
        let Some(data) = self.data.clone() else { return };
        let list = QString::from(&list_json(&data, &self.filters(), &lookups()));
        show!(self, list, list_changed, list);
    }

    fn refresh(mut self: Pin<&mut Self>) {
        let Some(place) = place() else {
            // No data folder: no fights, and no records to check.
            show!(self, ready, ready_changed, true);
            show!(self, records_checked, records_checked_changed, true);
            return;
        };
        let _ = QT.set(self.qt_thread());
        self.as_mut().rust_mut().reading += 1;
        show!(self, records_checked, records_checked_changed, false);
        let reading = self.reading;
        let you = your_name();
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let data = read(&place, you);
            let lookups = lookups();
            let first = Arc::new(data.clone());
            if qt.queue({
                let first = first.clone();
                move |history| history.land(reading, first)
            })
            .is_err()
            {
                return;
            }
            // Checking a new game record replays the fight's packets, so
            // the records come after the list, as in 1.x.
            let mut checked = data;
            checked.records = Some(checker(&place, &checked, &lookups).statuses());
            let _ = qt.queue(move |history| history.land(reading, Arc::new(checked)));
        });
    }

    fn land(mut self: Pin<&mut Self>, reading: u64, data: Arc<Data>) {
        if reading != self.reading {
            return;
        }
        self.as_mut().rust_mut().data = Some(data.clone());
        self.as_mut().relist();
        show!(self, ready, ready_changed, true);
        show!(self, records_checked, records_checked_changed, data.records.is_some());
        let chosen = self.chosen.to_string();
        match to_show(&chosen, data.fights.iter().map(|f| f.id.as_str()), &self.list.to_string()) {
            Some(id) => self.as_mut().pick(id),
            None => {
                show!(self, chosen, chosen_changed, QString::default());
                show!(self, fight, fight_changed, QString::default());
            }
        }
    }

    fn choose(self: Pin<&mut Self>, id: QString) {
        self.pick(id.to_string());
    }

    fn redraw(mut self: Pin<&mut Self>) {
        self.as_mut().relist();
        let chosen = self.chosen.to_string();
        if !chosen.is_empty() {
            self.pick(chosen);
        }
    }

    fn pick(mut self: Pin<&mut Self>, id: String) {
        if self.chosen.to_string() != id {
            show!(self, note, note_changed, QString::default());
            show!(self, player, player_changed, QString::default());
        }
        show!(self, chosen, chosen_changed, QString::from(&id));
        let (Some(place), Some(data)) = (place(), self.data.clone()) else { return };
        self.as_mut().rust_mut().picking += 1;
        let picking = self.picking;
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let json = fight_json(&place, &data, &id, &lookups()).unwrap_or_default();
            let _ = qt.queue(move |mut history| {
                if picking == history.picking {
                    let json = QString::from(&json);
                    show!(history, fight, fight_changed, json);
                }
            });
        });
    }

    fn delete_chosen(mut self: Pin<&mut Self>) {
        let id = self.chosen.to_string();
        let Some(place) = place().filter(|_| !id.is_empty()) else { return };
        show!(self, chosen, chosen_changed, QString::default());
        show!(self, fight, fight_changed, QString::default());
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            if let Err(error) = history_view::delete_fight(&place.dir, place.history, &id) {
                eprintln!("daevalog-dps-meter: {error}");
            }
            let _ = qt.queue(|history| history.refresh());
        });
    }
}

impl qobject::History {
    /// The fight shown, its id, and where it is kept.
    fn chosen_fight(&self) -> Option<(String, Place)> {
        let id = self.chosen.to_string();
        Some(id).filter(|id| !id.is_empty()).zip(place())
    }

    /// An upload or a preview is running for the fight shown.
    fn busy(&self) -> bool {
        let note = self.note.to_string();
        note.contains(r#""kind":"uploading""#) || note.contains(r#""kind":"previewing""#)
    }

    fn upload_chosen(mut self: Pin<&mut Self>) {
        if self.busy() {
            return;
        }
        let Some((id, place)) = self.chosen_fight() else { return };
        show!(self, note, note_changed, QString::from(r#"{"kind":"uploading"}"#));
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let note = match place.history.load_fight(&id) {
                Err(error) => json!({ "kind": "failed", "message": error }),
                Ok(record) => {
                    let language = crate::account::ui_language();
                    let request = daevalog::share::upload(daevalog::account::client(), &place.dir, &language, &record);
                    match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                        Err(error) => json!({ "kind": "failed", "message": error.to_string() }),
                        Ok(runtime) => match runtime.block_on(request) {
                            Ok(result) => json!({ "kind": "uploaded", "url": result.url, "private": result.visibility == "private" }),
                            Err(message) => json!({ "kind": "failed", "message": message }),
                        },
                    }
                }
            };
            let _ = qt.queue(move |mut history| {
                if history.chosen.to_string() == id {
                    show!(history, note, note_changed, QString::from(&note.to_string()));
                }
                // The list's link marks and the Open log button.
                history.refresh();
            });
        });
    }

    fn preview_chosen(mut self: Pin<&mut Self>) {
        if self.busy() {
            return;
        }
        let Some((id, place)) = self.chosen_fight() else { return };
        show!(self, note, note_changed, QString::from(r#"{"kind":"previewing"}"#));
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let result = place.history.load_fight(&id).and_then(|record| {
                let language = crate::account::ui_language();
                daevalog::share::preview_upload(&place.dir, &language, &record, &place.dir.join("share-preview"))
            });
            let note = match result {
                Ok(r) => json!({
                    "kind": "previewed",
                    "sent": (r.bytes / 1024).max(1),
                    "file": r.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                    "dir": r.path.parent().map(|d| d.display().to_string()).unwrap_or_default(),
                }),
                Err(message) => json!({ "kind": "failed", "message": message }),
            };
            let _ = qt.queue(move |mut history| {
                if history.chosen.to_string() == id {
                    show!(history, note, note_changed, QString::from(&note.to_string()));
                }
            });
        });
    }

    fn log_url(&self) -> QString {
        let Some((id, place)) = self.chosen_fight() else { return QString::default() };
        let url = daevalog::share::share_status(&place.dir).remove(&id).and_then(|s| s.url).unwrap_or_default();
        // The link came from the server: only https on a2tools.app is opened.
        QString::from(if daevalog::account::is_site_url(&url) { url.as_str() } else { "" })
    }
}

impl qobject::History {
    fn open_player(self: Pin<&mut Self>, id: i32) {
        let chosen = self.chosen.to_string();
        let (Some(place), Some(data)) = (place(), self.data.clone()) else { return };
        if chosen.is_empty() {
            return;
        }
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let json = player_json(&place, &data, &chosen, id, &lookups()).unwrap_or_default();
            let _ = qt.queue(move |mut history| {
                if history.chosen.to_string() == chosen {
                    show!(history, player, player_changed, QString::from(&json));
                }
            });
        });
    }

    fn close_player(mut self: Pin<&mut Self>) {
        show!(self, player, player_changed, QString::default());
    }
}

/// One player of a saved fight (`details_view::saved_player_view`): their
/// skill rows, heal rows and damage received by mechanic.
fn player_json(place: &Place, data: &Data, fight: &str, id: i32, lookups: &Lookups) -> Option<String> {
    let record = place.history.load_fight(fight).ok()?;
    let names = LookupNames { skills: &lookups.skills, npcs: &lookups.npcs, words: &lookups.words };
    let view = daevalog::combat::details_view::saved_player_view(&record, &data.you, id, &names);
    let mut value = serde_json::json!({
        "id": id,
        "skills": view.damage.skills,
        "heals": view.heal.skills,
        "taken": view.taken.rows,
    });
    theme_jobs(&mut value);
    Some(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reading_shows_the_chosen_fight_again_or_the_first() {
        let list = r#"{"groups":[{"fights":[]},{"fights":[{"id":"b"},{"id":"a"}]}]}"#;
        let ids = || ["a", "b"].into_iter();
        assert_eq!(to_show("a", ids(), list).as_deref(), Some("a"), "the chosen fight, read again");
        assert_eq!(to_show("gone", ids(), list).as_deref(), Some("b"), "deleted: the list's first");
        assert_eq!(to_show("", ids(), list).as_deref(), Some("b"), "none chosen yet");
        assert_eq!(to_show("a", std::iter::empty(), r#"{"groups":[]}"#), None, "no fights");
    }

    /// A saved fight: you (1) and another Elementalist (2) on a boss.
    fn record(id: &str, boss: &str, start: i64, train: bool) -> FightRecord {
        serde_json::from_value(serde_json::json!({
            "id": id, "bossName": boss, "targetId": 700, "startTimeMs": start, "durationMs": 10_000,
            "totalDamage": 9_000, "jobs": ["정령성"], "isTrain": train,
            "details": {"targetId": 700, "maxHp": 0, "totalTargetDamage": 9_000, "battleTime": 10_000, "startTime": 0,
                "skills": [
                    {"actorId": 1, "code": 16010000, "name": "", "time": 10, "dmg": 6_000, "crit": 5, "job": "정령성",
                     "multiHitCount": 0, "multiHitDamage": 0, "parry": 0, "back": 0, "perfect": 0, "double": 0, "regen": 0,
                     "hitTimestamps": [0, 1000]},
                    {"actorId": 2, "code": 16010000, "name": "", "time": 6, "dmg": 3_000, "crit": 0, "job": "정령성",
                     "multiHitCount": 0, "multiHitDamage": 0, "parry": 0, "back": 0, "perfect": 0, "double": 0, "regen": 0,
                     "hitTimestamps": [500]}
                ],
                "pingHistory": [], "healSkills": [],
                "takenSkills": [{"actorId": 2, "code": 1800001, "name": "", "sourceCode": 0,
                    "damage": 400, "hits": 2, "parry": 1}],
                "deaths": [{"actorId": 1, "deaths": 0}, {"actorId": 2, "deaths": 1}]},
            "actors": [
                {"actorId": 1, "nickname": "Me", "job": "정령성"},
                {"actorId": 2, "nickname": "Other", "job": "정령성"}
            ]
        }))
        .unwrap()
    }

    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("daevalog-qml-history-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Folder(dir)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn place_in(folder: &Folder) -> Place {
        let history: &'static FightHistoryManager = Box::leak(Box::new(FightHistoryManager::new(folder.0.clone())));
        Place { dir: folder.0.clone(), history, roots: Vec::new() }
    }

    fn json(text: &str) -> serde_json::Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn the_list_groups_filters_and_ranks_you() {
        let folder = Folder::new("list");
        let place = place_in(&folder);
        place.history.save_fight(&record("auto_700_1", "Boss", 1_000_000, false)).unwrap();
        place.history.save_fight(&record("auto_700_2", "Training Scarecrow", 2_000_000, true)).unwrap();
        let mut data = read(&place, "Me".into());
        data.records = Some(HashMap::new());
        let lookups = Lookups::new(None, "en");

        let list = json(&list_json(&data, &Filters::default(), &lookups));
        assert_eq!(list["shown"], 2);
        let groups = list["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 1, "both in the open world");
        assert_eq!(groups[0]["title"], crate::i18n::text("en", "history.openWorld"));
        let fight = &groups[0]["fights"][1];
        assert_eq!(fight["id"], "auto_700_1", "newest first");
        assert_eq!(fight["jobs"], serde_json::json!(["spiritmaster", "spiritmaster"]), "one class per player, by theme key");
        assert_eq!(fight["you"]["rank"], 1);
        assert_eq!(fight["you"]["perSecond"], 600.0);
        assert_eq!(list["jobs"][0]["icon"], "spiritmaster");

        let no_training = Filters { show_training: false, layout: Layout::List, ..Filters::default() };
        let list = json(&list_json(&data, &no_training, &lookups));
        assert_eq!(list["shown"], 1);
        assert_eq!(list["groups"][0]["title"], "");
        assert_eq!(first_shown(&list.to_string()).as_deref(), Some("auto_700_1"));
    }

    #[test]
    fn a_saved_fight_finds_you_by_its_saved_id_then_by_name() {
        let mut fight = record("auto_700_1", "Boss", 1_000_000, false);
        assert_eq!(saved_player(&fight, -1, "Me"), Some(1), "by name");
        assert_eq!(saved_player(&fight, -1, "Nobody"), None);
        fight.timeline.you = 2;
        assert_eq!(saved_player(&fight, -1, "Me"), Some(2), "the saved id first");
        assert_eq!(saved_player(&fight, 0, "Me"), None, "every player");
        assert_eq!(saved_player(&fight, 7, "Me"), Some(7));
    }

    #[test]
    fn each_filter_and_the_by_boss_layout() {
        let folder = Folder::new("filters");
        let place = place_in(&folder);
        const DAY: i64 = 86_400_000;
        place.history.save_fight(&record("auto_700_1", "Boss A", 10 * DAY, false)).unwrap();
        let mut other: serde_json::Value = serde_json::to_value(record("auto_700_2", "Boss B", 12 * DAY, false)).unwrap();
        other["jobs"] = serde_json::json!(["치유성"]);
        place.history.save_fight(&serde_json::from_value(other).unwrap()).unwrap();
        let mut data = read(&place, "Me".into());
        data.records = Some(HashMap::from([("auto_700_2".to_string(), FightStatus::default())]));
        let lookups = Lookups::new(None, "en");
        let shown = |filters: Filters| -> Vec<String> {
            let list = json(&list_json(&data, &Filters { layout: Layout::List, ..filters }, &lookups));
            list["groups"][0]["fights"].as_array().map_or(vec![], |f| f.iter().map(|f| f["id"].as_str().unwrap().to_string()).collect())
        };
        assert_eq!(shown(Filters::default()), ["auto_700_2", "auto_700_1"]);
        assert_eq!(shown(Filters { boss: "Boss A".into(), ..Filters::default() }), ["auto_700_1"]);
        assert_eq!(shown(Filters { job: "치유성".into(), ..Filters::default() }), ["auto_700_2"]);
        let list = json(&list_json(&data, &Filters::default(), &lookups));
        let newest_day = list["dates"][0].as_str().unwrap().to_string();
        assert_eq!(shown(Filters { date: newest_day, ..Filters::default() }), ["auto_700_2"]);
        assert_eq!(shown(Filters { only_records: true, ..Filters::default() }), ["auto_700_2"]);

        // By boss: a section per boss, each row named by its day.
        let by_boss = json(&list_json(&data, &Filters { layout: Layout::Boss, ..Filters::default() }, &lookups));
        let titles: Vec<&str> = by_boss["groups"].as_array().unwrap().iter().map(|g| g["title"].as_str().unwrap()).collect();
        assert_eq!(titles, ["Boss B", "Boss A"], "newest section first");
    }

    #[test]
    fn a_fight_has_its_tables_and_deaths() {
        let folder = Folder::new("fight");
        let place = place_in(&folder);
        place.history.save_fight(&record("auto_700_1", "Boss", 1_000_000, false)).unwrap();
        let data = read(&place, "Me".into());
        let lookups = Lookups::new(None, "en");
        let fight = json(&fight_json(&place, &data, "auto_700_1", &lookups).unwrap());
        assert_eq!(fight["damage"], 9_000);
        assert_eq!(fight["perSecond"], 900.0);
        assert_eq!(fight["deaths"], 1);
        assert_eq!(fight["received"]["damage"], 400);
        let dps = fight["dps"].as_array().unwrap();
        assert_eq!((dps[0]["name"].as_str(), dps[0]["you"].as_bool()), (Some("Me"), Some(true)));
        assert_eq!((dps[0]["critRate"].as_f64(), dps[0]["hits"].as_i64()), (Some(50.0), Some(10)));
        assert_eq!(dps[0]["job"], "spiritmaster");
        assert_eq!(dps[1]["number"], 1, "numbered by first hit, you not");
        let tank = fight["tank"].as_array().unwrap();
        assert_eq!(tank[0]["id"], 2);
        assert_eq!((tank[0]["amount"].as_i64(), tank[0]["parry"].as_i64(), tank[0]["deaths"].as_i64()), (Some(400), Some(1), Some(1)));
        assert_eq!((tank[1]["id"].as_i64(), tank[1]["deaths"].as_i64()), (Some(1), Some(0)), "you died 0 times, took nothing");
        assert!(fight["records"].as_array().unwrap().is_empty());
        assert!(fight_json(&place, &data, "gone", &lookups).is_none());
    }

    #[test]
    fn deleting_takes_the_fight_and_its_packets() {
        let folder = Folder::new("delete");
        let place = place_in(&folder);
        place.history.save_fight(&record("auto_700_1", "Boss", 1_000_000, false)).unwrap();
        let slices = folder.0.join("slices");
        std::fs::create_dir_all(&slices).unwrap();
        std::fs::write(slices.join("auto_700_1.a2es.gz"), b"x").unwrap();
        history_view::delete_fight(&place.dir, place.history, "auto_700_1").unwrap();
        assert!(read(&place, String::new()).fights.is_empty());
        assert!(!slices.join("auto_700_1.a2es.gz").exists());
        assert!(history_view::delete_fight(&place.dir, place.history, "../settings").is_err());
    }
}
