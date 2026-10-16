// SPDX-License-Identifier: AGPL-3.0-only

//! `Details`, the QML singleton behind the Details window: one player, or
//! every player, in the live fight, on one target or all of them; or a fight
//! saved in History (`open_saved`), as 1.x opens one from History. The view
//! is the backend's (`daevalog::combat::details_view`), built from the same
//! calls the 1.x Details page makes, on a thread of its own and queued onto
//! the Qt thread as JSON. Every number and rule is the backend's; this only
//! picks, and names classes by their theme key.

use core::pin::Pin;
use std::sync::{Arc, Mutex};

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use daevalog::combat::data_storage::DataStorage;
use daevalog::combat::details_view::{DetailsView, Selection, Words};
use daevalog::combat::timeline_tracks::{TrackNames, Tracks};
use daevalog::combat::dps_calculator::DetailsSource;

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
        #[qproperty(QString, view, READ, NOTIFY)]
        #[qproperty(bool, ready, READ, NOTIFY)]
        #[qproperty(i32, player, READ, NOTIFY)]
        #[qproperty(i32, target, READ, NOTIFY)]
        #[qproperty(QString, hover, READ, NOTIFY)]
        type Details = super::DetailsRust;

        /// Opens on a player (0 every player, -1 you, else every player
        /// when the meter does not know you) and the target the meter's row
        /// stands for: the meter's target, its name, and whether TRAIN
        /// shows every dummy.
        #[qinvokable]
        fn open(self: Pin<&mut Details>, player: i32, meter_target: i32, meter_target_named: bool, train_all: bool);

        /// Opens on a fight saved in History, by its id: on `player` (-1
        /// you, 0 every player). It stays on that fight until `open`.
        #[qinvokable]
        fn open_saved(self: Pin<&mut Details>, fight: &QString, player: i32);

        /// Another player; 0 for every player.
        #[qinvokable]
        fn pick_player(self: Pin<&mut Details>, player: i32);

        /// Another target; 0 for every target.
        #[qinvokable]
        fn pick_target(self: Pin<&mut Details>, target: i32);

        /// Reads the fight again.
        #[qinvokable]
        fn refresh(self: Pin<&mut Details>);

        /// Reads the meter's hover card for the row of `player`, into
        /// `hover`: {"player", "skills": [{"name", "job", "dot",
        /// "damage"}], "total"}.
        #[qinvokable]
        fn read_hover(self: Pin<&mut Details>, player: i32);
    }

    impl cxx_qt::Threading for Details {}
}

/// What the meter feeds Details from: set when the live capture or the
/// replay starts.
#[derive(Clone)]
struct Source {
    details: DetailsSource,
    storage: Arc<DataStorage>,
}

static SOURCE: Mutex<Option<Source>> = Mutex::new(None);

/// Hands Details the meter's storage and the targets on screen.
pub fn set_source(details: DetailsSource, storage: Arc<DataStorage>) {
    *SOURCE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Source { details, storage });
}

fn source() -> Option<Source> {
    SOURCE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Your character's name as the meter knows it; `None` before it does.
pub fn your_name() -> Option<String> {
    source()?.storage.local_character_name().filter(|n| !n.trim().is_empty())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlayerPick {
    All,
    You,
    Id(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetPick {
    All,
    Id(i32),
    /// The target the meter's row stands for, settled once the fight has
    /// targets.
    Opening { meter_target: i32, named: bool, train_all: bool },
}

#[derive(Debug, Clone, PartialEq)]
struct Request {
    player: PlayerPick,
    target: TargetPick,
    words: Words,
    /// A saved fight's id; `None` for the live fight.
    fight: Option<String>,
}

/// A view and what it was built for.
#[derive(Debug, Clone, PartialEq)]
struct Computed {
    json: String,
    player: i32,
    target: i32,
    /// The opening target, once the fight had targets to settle it on.
    settled: Option<Option<i32>>,
}

pub struct DetailsRust {
    /// The view as JSON; empty until the first one.
    view: QString,
    /// The view is of the latest pick and reading.
    ready: bool,
    /// The player shown, 0 for every player.
    player: i32,
    /// The target shown, 0 for every target.
    target: i32,
    request: Option<Request>,
    /// Bumped by every pick and refresh; a view of an older one is dropped.
    generation: u64,
    running: bool,
    /// The hover card as JSON; empty until the first.
    hover: QString,
    /// Bumped by every hover read; an older answer is dropped.
    hover_reading: u64,
}

impl Default for DetailsRust {
    fn default() -> Self {
        Self {
            view: QString::default(),
            ready: false,
            player: 0,
            target: 0,
            request: None,
            generation: 0,
            running: false,
            hover: QString::default(),
            hover_reading: 0,
        }
    }
}

/// The skill names Details builds, in the window's language.
fn words() -> Words {
    let settings = crate::settings_store::store().values();
    let language = crate::i18n::resolve(settings.language.as_deref());
    Words {
        unnamed_skill: crate::i18n::text(language, "skills.fallback"),
        dot: crate::i18n::text(language, "skills.dot"),
    }
}

impl qobject::Details {
    fn open(self: Pin<&mut Self>, player: i32, meter_target: i32, meter_target_named: bool, train_all: bool) {
        let player = match player {
            0 => PlayerPick::All,
            p if p < 0 => PlayerPick::You,
            p => PlayerPick::Id(p),
        };
        let target = TargetPick::Opening { meter_target, named: meter_target_named, train_all };
        self.ask(Request { player, target, words: words(), fight: None });
    }

    fn open_saved(self: Pin<&mut Self>, fight: &QString, player: i32) {
        let player = match player {
            0 => PlayerPick::All,
            p if p < 0 => PlayerPick::You,
            p => PlayerPick::Id(p),
        };
        self.ask(Request { player, target: TargetPick::All, words: words(), fight: Some(fight.to_string()) });
    }

    fn pick_player(self: Pin<&mut Self>, player: i32) {
        let Some(mut request) = self.request.clone() else { return };
        request.player = if player == 0 { PlayerPick::All } else { PlayerPick::Id(player) };
        self.ask(request);
    }

    fn pick_target(self: Pin<&mut Self>, target: i32) {
        let Some(mut request) = self.request.clone() else { return };
        request.target = if target == 0 { TargetPick::All } else { TargetPick::Id(target) };
        self.ask(request);
    }

    fn refresh(self: Pin<&mut Self>) {
        let Some(request) = self.request.clone() else { return };
        self.ask(request);
    }

    fn read_hover(mut self: Pin<&mut Self>, player: i32) {
        self.as_mut().rust_mut().hover_reading += 1;
        let reading = self.hover_reading;
        let Some(source) = source() else { return };
        let words = words();
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let (skills, total) = source.details.reader().hover_view(player, &words);
            let mut json = serde_json::json!({ "player": player, "skills": skills, "total": total });
            theme_jobs(&mut json);
            let json = json.to_string();
            let _ = qt.queue(move |mut details| {
                if details.hover_reading == reading {
                    details.as_mut().rust_mut().hover = QString::from(&json);
                    details.as_mut().hover_changed();
                }
            });
        });
    }

    fn ask(mut self: Pin<&mut Self>, mut request: Request) {
        // In the language shown now: a refresh after a language change
        // writes "Skill {code}" and "- DOT" anew, as 1.x redraws.
        request.words = words();
        {
            let mut rust = self.as_mut().rust_mut();
            rust.request = Some(request);
            rust.generation += 1;
        }
        self.as_mut().set_ready_value(false);
        if !self.running {
            self.start();
        }
    }

    fn start(mut self: Pin<&mut Self>) {
        let Some(request) = self.request.clone() else { return };
        let generation = self.generation;
        self.as_mut().rust_mut().running = true;
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let computed = match &request.fight {
                Some(id) => compute_saved(id, &request),
                None => source().map(|s| compute(&s, &request)),
            };
            let _ = qt.queue(move |details| details.land(generation, computed));
        });
    }

    fn land(mut self: Pin<&mut Self>, generation: u64, computed: Option<Computed>) {
        self.as_mut().rust_mut().running = false;
        if generation != self.generation {
            self.start();
            return;
        }
        if let Some(computed) = computed {
            if let (Some(settled), Some(request)) = (computed.settled, self.as_mut().rust_mut().request.as_mut()) {
                if matches!(request.target, TargetPick::Opening { .. }) {
                    request.target = settled.map_or(TargetPick::All, TargetPick::Id);
                }
            }
            if self.player != computed.player {
                self.as_mut().rust_mut().player = computed.player;
                self.as_mut().player_changed();
            }
            if self.target != computed.target {
                self.as_mut().rust_mut().target = computed.target;
                self.as_mut().target_changed();
            }
            self.as_mut().rust_mut().view = QString::from(&computed.json);
            self.as_mut().view_changed();
        }
        self.set_ready_value(true);
    }

    fn set_ready_value(mut self: Pin<&mut Self>, ready: bool) {
        if self.ready != ready {
            self.as_mut().rust_mut().ready = ready;
            self.as_mut().ready_changed();
        }
    }
}

/// Reads the fight as `request` picks it.
fn compute(source: &Source, request: &Request) -> Computed {
    let reader = source.details.reader();
    let context = reader.get_details_context();
    let player = match request.player {
        PlayerPick::All => None,
        PlayerPick::Id(id) => Some(id),
        // The meter's storage, which is the one before a zone change while
        // boss mode holds a fight: your id from then.
        PlayerPick::You => source
            .details
            .storage()
            .local_player_id()
            .and_then(|id| i32::try_from(id).ok())
            .filter(|id| context.actors.iter().any(|a| a.actor_id == *id)),
    };
    let (target, settled) = match request.target {
        TargetPick::All => (None, None),
        TargetPick::Id(id) => (Some(id), None),
        TargetPick::Opening { meter_target, named, train_all } => {
            let target = reader.details_opening_target(&context, meter_target, named, train_all);
            (target, (!context.targets.is_empty()).then_some(target))
        }
    };
    let view = reader.details_view_from(&context, &Selection { player, target }, &request.words, &mut |call| reader.details_call(call));
    let tracks = reader.timeline_tracks(&view, &track_names());
    let start = view.clock_start;
    let mut value: serde_json::Value = serde_json::from_str(&to_json(view, &tracks)).unwrap_or_default();
    // The title's start time and difficulty, as 1.x's.
    value["live"] = crate::history::live_title(start, source.details.storage().current_dungeon_id());
    Computed { json: value.to_string(), player: player.unwrap_or(0), target: target.unwrap_or(0), settled }
}

/// Reads a saved fight as `request` picks it: one target, so the target pick
/// changes nothing. The view says when the fight was fought.
fn compute_saved(id: &str, request: &Request) -> Option<Computed> {
    let player = match request.player {
        PlayerPick::All => 0,
        PlayerPick::You => -1,
        PlayerPick::Id(id) => id,
    };
    let (view, tracks, title) = crate::history::saved_details(id, player, &track_names())?;
    let (player, target) = (view.player, view.target);
    let mut value: serde_json::Value = serde_json::from_str(&to_json(view, &tracks)).unwrap_or_default();
    value["saved"] = serde_json::json!({ "id": id, "date": title["date"], "tier": title["tier"] });
    Some(Computed { json: value.to_string(), player, target, settled: None })
}

/// The names of buffs, debuffs and stats in the window's language, read
/// once per language.
fn track_names() -> Arc<TrackNames> {
    static NAMES: Mutex<Option<(&'static str, Arc<TrackNames>)>> = Mutex::new(None);
    let language = crate::i18n::resolve(crate::settings_store::store().values().language.as_deref());
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, read)) = names.as_ref().filter(|(read, _)| *read == language) {
        return read.clone();
    }
    let read = Arc::new(crate::data_dir().map(|dir| TrackNames::load(&dir, language)).unwrap_or_default());
    *names = Some((language, read.clone()));
    read
}

/// The view as QML reads it, with the timeline's buff and stat tracks:
/// classes by their theme key, and without each player's chart line, which
/// the window does not draw.
fn to_json(mut view: DetailsView, tracks: &Tracks) -> String {
    view.chart.actors.clear();
    let mut value = serde_json::to_value(&view).unwrap_or_default();
    value["tracks"] = serde_json::to_value(tracks).unwrap_or_default();
    theme_jobs(&mut value);
    value.to_string()
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

#[cfg(test)]
mod tests {
    use super::*;
    use daevalog::capture::ping_tracker::PingTracker;
    use daevalog::combat::dps_calculator::DpsCalculator;
    use daevalog::entity::damage_packet::ParsedDamagePacket;
    use daevalog::i18n::lookup::{NpcLookup, SkillLookup};

    const BOSS: i32 = 700;
    const ADD: i32 = 701;

    fn hit(storage: &DataStorage, actor: i32, target: i32, skill: i32, damage: i32, at: i64) {
        let mut packet = ParsedDamagePacket::default();
        packet.set_timestamp(at);
        packet.set_skill_code(skill);
        packet.set_damage(damage);
        packet.set_actor_id(actor);
        packet.set_target_id(target);
        daevalog::clock::set_override(Some(at));
        storage.append_damage(packet);
    }

    /// You (2) and another Elementalist (3) on a boss and an add.
    fn fight() -> (Source, DpsCalculator) {
        let storage = Arc::new(DataStorage::new());
        for (id, name) in [(2, "Me"), (3, "Other")] {
            storage.note_player_spawn(id);
            storage.append_nickname(id, name);
        }
        storage.set_local_player_id(Some(2));
        let npcs = NpcLookup::new();
        npcs.load_from_json(r#"{"9001":{"name":"Boss","isBoss":true},"9002":{"name":"Add"}}"#);
        storage.append_mob(BOSS, 9001);
        storage.register_boss(BOSS);
        storage.append_mob(ADD, 9002);
        // Elementalist skills: the class comes from the skill code.
        for s in 0..10 {
            hit(&storage, 2, BOSS, 16_010_000, 500, 1_000 + s * 1_000);
            hit(&storage, 3, BOSS, 16_020_000, 300, 1_500 + s * 1_000);
        }
        hit(&storage, 2, ADD, 16_010_000, 200, 5_000);
        let mut meter = DpsCalculator::new(storage.clone(), Arc::new(SkillLookup::new()), Arc::new(npcs), Arc::new(PingTracker::new()));
        meter.set_target_selection_mode("bossTargets");
        meter.get_dps();
        daevalog::clock::set_override(None);
        (Source { details: meter.details_source(), storage }, meter)
    }

    fn view(json: &str) -> serde_json::Value {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn opening_from_the_meter_shows_its_target_and_settles_on_it() {
        let (source, _meter) = fight();
        let request = Request {
            player: PlayerPick::You,
            target: TargetPick::Opening { meter_target: BOSS, named: true, train_all: true },
            words: Words::default(),
            fight: None,
        };
        let computed = compute(&source, &request);
        assert_eq!((computed.player, computed.target, computed.settled), (2, BOSS, Some(Some(BOSS))));
        let v = view(&computed.json);
        assert_eq!(v["damage"]["stats"]["damage"], 5_000);
        // BOSS lists the meter's targets only (rule 14): the boss, not the add.
        assert_eq!(v["targets"].as_array().unwrap().len(), 1, "the boss");
        assert_eq!(v["players"][0]["job"], "spiritmaster", "classes by theme key");
        assert!(v["chart"]["actors"].as_array().unwrap().is_empty());
        assert_eq!(v["chart"]["player"].as_array().unwrap().len(), 101);
    }

    #[test]
    fn every_player_on_every_target() {
        let (source, _meter) = fight();
        let request = Request { player: PlayerPick::All, target: TargetPick::All, words: Words::default(), fight: None };
        let computed = compute(&source, &request);
        assert_eq!((computed.player, computed.target, computed.settled), (0, 0, None));
        let v = view(&computed.json);
        // Every target in BOSS is the meter's boss (rule 14): the add's 200 is not in it.
        assert_eq!(v["damage"]["stats"]["damage"], 8_000);
        assert_eq!(v["damage"]["all"], serde_json::Value::Null, "no picked player, no split");
        assert_eq!(v["players"].as_array().unwrap().iter().map(|p| p["id"].as_i64().unwrap()).collect::<Vec<_>>(), [2, 3]);
        // The title: when the fight's first hit came; no dungeon, no
        // difficulty.
        assert!(v["live"]["date"].as_str().is_some_and(|d| !d.is_empty() && d != "-"), "{}", v["live"]);
        assert!(v["live"]["tier"].is_null());
    }

    #[test]
    fn the_title_names_the_dungeons_difficulty_and_the_start() {
        let title = crate::history::live_title(1_767_380_400_000, 600_002);
        assert!(title["date"].as_str().is_some_and(|d| d.starts_with("2026-01-0")), "{title}");
        assert!(title["tier"].as_str().is_some_and(|t| !t.is_empty()), "Krao Cave's Exploration: {title}");
        let none = crate::history::live_title(0, 0);
        assert_eq!((none["date"].as_str(), none["tier"].is_null()), (Some(""), true));
        assert!(crate::history::live_title(0, 10)["tier"].is_null(), "a dungeon with no difficulty");
    }

    #[test]
    fn you_unknown_shows_every_player() {
        let (source, _meter) = fight();
        source.storage.set_local_player_id(None);
        let request = Request { player: PlayerPick::You, target: TargetPick::All, words: Words::default(), fight: None };
        assert_eq!(compute(&source, &request).player, 0);
    }

    /// Spirit's Benediction on you (2), put on by you: `<len> 2a 38
    /// <entity> 01 <flags> <instance 300> <abnormal u32> <length i64> <end
    /// i64> <caster> <level> <skill u32> <u8> <x y z f32>`.
    fn benediction_on() -> Vec<u8> {
        let mut b = vec![0x00, 0x2A, 0x38, 0x02, 0x01, 0x13, 0xAC, 0x02];
        b.extend(161_900_001u32.to_le_bytes());
        b.extend(10_000i64.to_le_bytes());
        b.extend(1_800_000_020_000i64.to_le_bytes());
        b.extend([0x02, 0x02]);
        b.extend(16_190_000u32.to_le_bytes());
        b.push(0x00);
        for v in [-1500.5f32, 2500.25, 120.0] {
            b.extend(v.to_le_bytes());
        }
        b[0] = b.len() as u8 + 3;
        b
    }

    /// And off: `2c 38 <entity> <n> <kind> <instance> <reason 1>`.
    fn benediction_off() -> Vec<u8> {
        vec![0x0C, 0x2C, 0x38, 0x02, 0x01, 0x00, 0xAC, 0x02, 0x01]
    }

    #[test]
    fn the_timeline_has_the_picked_players_buffs() {
        let (source, _meter) = fight();
        source.storage.note_abnormal(3_000, &benediction_on());
        source.storage.note_abnormal(6_000, &benediction_off());
        let tracks = |player| {
            let request = Request { player, target: TargetPick::Id(BOSS), words: Words::default(), fight: None };
            view(&compute(&source, &request).json)["tracks"].clone()
        };
        // The boss fight starts at your first hit, 1 s.
        assert_eq!(
            tracks(PlayerPick::You),
            serde_json::json!({ "buffs": [{ "name": "Spirit's Benediction", "on": "self", "debuff": false, "spans": [[2_000, 5_000]] }], "stats": [] })
        );
        assert_eq!(tracks(PlayerPick::Id(3)), serde_json::json!({ "buffs": [], "stats": [] }), "not on the other player");
        assert_eq!(tracks(PlayerPick::All), serde_json::json!({ "buffs": [], "stats": [] }), "no one picked");
    }

    #[test]
    fn skill_names_are_built_in_the_windows_language() {
        let words = Words {
            unnamed_skill: crate::i18n::text("ja", "skills.fallback"),
            dot: crate::i18n::text("ja", "skills.dot"),
        };
        let (source, _meter) = fight();
        let computed = compute(&source, &Request { player: PlayerPick::Id(3), target: TargetPick::Id(BOSS), words: words.clone(), fight: None });
        let v = view(&computed.json);
        let name = v["damage"]["skills"][0]["name"].as_str().unwrap().to_string();
        assert_eq!(name, words.unnamed_skill.replace("{code}", "16020000"), "no skill table: the code, in Japanese");
        assert_ne!(words.unnamed_skill, Words::default().unnamed_skill);
    }
}
