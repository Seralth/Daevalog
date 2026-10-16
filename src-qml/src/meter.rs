// SPDX-License-Identifier: AGPL-3.0-only

//! `Meter`, the QML singleton behind the meter window: a list model with one
//! row per player, and the fight time, target and totals around them. The
//! readings come from the backend's meter (`DpsCalculator::get_dps`) and
//! its meter view, which picks and orders the rows, on a thread of their
//! own, queued onto the Qt thread: from live capture, or from a packet log
//! with --replay. The Heal and Tank tabs and the split view's healers come
//! with each reading too (`crate::party`), as JSON.

use core::pin::Pin;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QByteArray, QHash, QHashPair_i32_QByteArray, QList, QModelIndex, QString, QStringList, QVariant};
use daevalog::combat::meter_view::{Party, PartyRow, ViewOptions};

use crate::snapshot::{self, Snapshot};

/// The ⋯ menu's Lowest first, for the threads that order the rows.
static LOWEST_FIRST: AtomicBool = AtomicBool::new(false);

/// Whether the window shows what the Heal and Tank rows feed (those tabs,
/// or the split view's healers), for the live loop. On until the window
/// says otherwise.
static PARTY_SHOWN: AtomicBool = AtomicBool::new(true);

/// Whether the live loop needs the Heal and Tank rows now.
pub fn party_shown() -> bool {
    PARTY_SHOWN.load(Ordering::SeqCst)
}

/// Whether the rows run from the least (the ⋯ menu's Lowest first).
pub fn lowest_first() -> bool {
    LOWEST_FIRST.load(Ordering::SeqCst)
}

/// What the live loop and the replay order the rows by: the settings, and
/// the ⋯ menu's Lowest first.
pub fn view_options(values: &crate::settings_store::Values) -> ViewOptions {
    ViewOptions { ascending: lowest_first(), ..values.view() }
}

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!(<QtCore/QAbstractListModel>);
        type QAbstractListModel;

        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qmodelindex.h");
        type QModelIndex = cxx_qt_lib::QModelIndex;
        include!("cxx-qt-lib/qvariant.h");
        type QVariant = cxx_qt_lib::QVariant;
        include!("cxx-qt-lib/qhash.h");
        type QHash_i32_QByteArray = cxx_qt_lib::QHash<cxx_qt_lib::QHashPair_i32_QByteArray>;
        include!("cxx-qt-lib/qlist.h");
        type QList_i32 = cxx_qt_lib::QList<i32>;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[base = QAbstractListModel]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QString, fight_time)]
        #[qproperty(QString, target)]
        #[qproperty(i32, target_id)]
        #[qproperty(QString, mode)]
        #[qproperty(f64, boss_health)]
        #[qproperty(QString, boss_hp_text)]
        #[qproperty(QString, boss_eta)]
        #[qproperty(QString, enrage)]
        #[qproperty(i32, enrage_level)]
        #[qproperty(QString, kill_mark)]
        #[qproperty(i32, kill_mark_level)]
        #[qproperty(QString, group_total)]
        #[qproperty(QString, group_damage)]
        #[qproperty(QString, party)]
        #[qproperty(QString, ping)]
        #[qproperty(i32, ping_level)]
        #[qproperty(bool, replaying, READ, CONSTANT)]
        #[qproperty(bool, replay_done)]
        #[qproperty(bool, lowest_first, READ, WRITE = change_lowest_first, NOTIFY)]
        #[qproperty(bool, party_shown, READ, WRITE = change_party_shown, NOTIFY)]
        type Meter = super::MeterRust;

        /// Starts the live capture or the --replay feed, once.
        #[qinvokable]
        fn start(self: Pin<&mut Meter>);

        /// Writes the rows and the group totals again, with the settings as
        /// they are now (the tabs' columns, Round DPS).
        #[qinvokable]
        fn redraw(self: Pin<&mut Meter>);

        /// Save & Clear: ends the fights, saves them and starts the meter
        /// afresh. Live only.
        #[qinvokable]
        fn save_and_clear(self: Pin<&mut Meter>);

        /// The footer's mode menu: the backend's mode id ("bossTargets",
        /// "lastHitByMe", "allTargets", "trainTargets", "encounter"); the
        /// next reading shows it. Live only: a replay keeps its --mode.
        #[qinvokable]
        fn pick_mode(self: Pin<&mut Meter>, mode: &QString);

        /// The ⋯ menu's Lowest first: the rows run from the least, for the
        /// session only, as 1.x's footer toggle.
        fn change_lowest_first(self: Pin<&mut Meter>, value: bool);

        /// The window shows the Heal or Tank tab or the split view's
        /// healers: only then does the live loop read Details for them.
        fn change_party_shown(self: Pin<&mut Meter>, value: bool);
    }

    unsafe extern "RustQt" {
        #[inherit]
        #[cxx_name = "beginResetModel"]
        fn begin_reset_model(self: Pin<&mut Meter>);

        #[inherit]
        #[cxx_name = "endResetModel"]
        fn end_reset_model(self: Pin<&mut Meter>);

        #[inherit]
        #[cxx_name = "beginInsertRows"]
        fn begin_insert_rows(self: Pin<&mut Meter>, parent: &QModelIndex, first: i32, last: i32);

        #[inherit]
        #[cxx_name = "endInsertRows"]
        fn end_insert_rows(self: Pin<&mut Meter>);

        #[inherit]
        #[cxx_name = "beginRemoveRows"]
        fn begin_remove_rows(self: Pin<&mut Meter>, parent: &QModelIndex, first: i32, last: i32);

        #[inherit]
        #[cxx_name = "endRemoveRows"]
        fn end_remove_rows(self: Pin<&mut Meter>);

        #[inherit]
        #[cxx_name = "beginMoveRows"]
        fn begin_move_rows(
            self: Pin<&mut Meter>,
            source_parent: &QModelIndex,
            source_first: i32,
            source_last: i32,
            destination_parent: &QModelIndex,
            destination_child: i32,
        ) -> bool;

        #[inherit]
        #[cxx_name = "endMoveRows"]
        fn end_move_rows(self: Pin<&mut Meter>);

        #[inherit]
        fn index(self: &Meter, row: i32, column: i32, parent: &QModelIndex) -> QModelIndex;

        #[inherit]
        #[qsignal]
        #[cxx_name = "dataChanged"]
        fn data_changed(self: Pin<&mut Meter>, top_left: &QModelIndex, bottom_right: &QModelIndex, roles: &QList_i32);
    }

    unsafe extern "RustQt" {
        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &Meter, parent: &QModelIndex) -> i32;

        #[cxx_override]
        fn data(self: &Meter, index: &QModelIndex, role: i32) -> QVariant;

        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &Meter) -> QHash_i32_QByteArray;
    }

    impl cxx_qt::Threading for Meter {}
}

/// The row roles, from Qt::UserRole up, and their names in QML.
#[derive(Clone, Copy)]
#[repr(i32)]
enum Role {
    EntityId = 0x0100,
    Name,
    Job,
    Total,
    Rate,
    Share,
    Fill,
    You,
    Unattributed,
    Identifying,
    Rank,
    Number,
    Texts,
    CombatPower,
    ShareValue,
    TicksOnly,
}

const ROLES: [(Role, &str); 16] = [
    (Role::EntityId, "entityId"),
    (Role::Name, "name"),
    (Role::Job, "job"),
    (Role::Total, "total"),
    (Role::Rate, "rate"),
    (Role::Share, "share"),
    (Role::Fill, "fill"),
    (Role::You, "you"),
    (Role::Unattributed, "unattributed"),
    (Role::Identifying, "identifying"),
    (Role::Rank, "rank"),
    (Role::Number, "number"),
    (Role::Texts, "texts"),
    (Role::CombatPower, "combatPower"),
    (Role::ShareValue, "shareValue"),
    (Role::TicksOnly, "ticksOnly"),
];

/// --print-fights' name for each of the backend's modes.
fn mode_label(id: &str) -> &'static str {
    match id {
        "bossTargets" => "BOSS",
        "lastHitByMe" => "TARGET",
        "allTargets" => "ALL",
        "trainTargets" => "TRAIN",
        "encounter" => "ENC",
        _ => "",
    }
}

pub struct MeterRust {
    fight_time: QString,
    target: QString,
    /// The target's entity id, 0 when the mode has none.
    target_id: i32,
    /// The backend's mode id ("bossTargets"); I18n names it.
    mode: QString,
    boss_health: f64,
    /// The health bar's text: "1.37M / 3.00M", the health left and the most.
    boss_hp_text: QString,
    /// The time to kill ("0:52"), after the enrage countdown; empty before
    /// there is one.
    boss_eta: QString,
    /// The time left until the boss's timed enrage ("4:12"), beside its
    /// name; empty for an enemy with none (`crate::enrage`).
    enrage: QString,
    /// Its colour: `Theme.pingColors`' index, 1 to 3.
    enrage_level: i32,
    /// After the time to kill: "✓" while the kill comes before the enrage,
    /// "✗" when not; empty without both (`crate::enrage::KillMark`).
    kill_mark: QString,
    /// Its colour, and the time to kill's: 1 in time, 3 too slow.
    kill_mark_level: i32,
    group_total: QString,
    /// The group's damage ("3.20M"), beside the group DPS.
    group_damage: QString,
    /// The Heal and Tank tabs and the split view's healers as the window
    /// draws them: {"heal", "tank", "healers": [rows], "healTotal",
    /// "tankTotal"}, each row with its texts for the tab's columns; empty
    /// before the first fight.
    party: QString,
    ping: QString,
    /// The ping's band, for its colour; see `ping_level`.
    ping_level: i32,
    replaying: bool,
    replay_done: bool,
    lowest_first: bool,
    party_shown: bool,
    started: bool,
    /// The reading on screen.
    shown: Snapshot,
    ticker: snapshot::FightTicker,
    kill_check: crate::enrage::KillMark,
    /// Each tab's columns and Round DPS, as the rows on screen show them.
    columns: Vec<&'static str>,
    heal_columns: Vec<&'static str>,
    tank_columns: Vec<&'static str>,
    round_dps: bool,
}

impl Default for MeterRust {
    fn default() -> Self {
        let options = crate::options();
        let settings = crate::settings_store::store().values();
        Self {
            fight_time: QString::from("00:00"),
            target: QString::default(),
            target_id: 0,
            mode: QString::from(&settings.default_mode),
            boss_health: -1.0,
            boss_hp_text: QString::default(),
            boss_eta: QString::default(),
            enrage: QString::default(),
            enrage_level: 1,
            kill_mark: QString::default(),
            kill_mark_level: 1,
            group_total: QString::default(),
            group_damage: QString::default(),
            party: QString::default(),
            ping: QString::default(),
            ping_level: 0,
            replaying: options.replay.is_some(),
            replay_done: false,
            lowest_first: lowest_first(),
            party_shown: party_shown(),
            started: false,
            shown: Snapshot::default(),
            ticker: snapshot::FightTicker::default(),
            kill_check: crate::enrage::KillMark::default(),
            columns: settings.meter_columns.dps,
            heal_columns: settings.meter_columns.heal,
            tank_columns: settings.meter_columns.tank,
            round_dps: settings.round_dps,
        }
    }
}

impl qobject::Meter {
    fn start(mut self: Pin<&mut Self>) {
        if self.started {
            return;
        }
        self.as_mut().rust_mut().started = true;
        let qt = self.qt_thread();
        let options = crate::options();
        let settings = crate::settings_store::store().values();
        // Game names (targets, skills) in the language the window shows.
        let language = crate::i18n::resolve(settings.language.as_deref());
        let Some(replay) = options.replay.clone() else {
            crate::live::start(qt, language);
            return;
        };
        // A replay has no ping; --ping shows one, for a screenshot.
        self.as_mut().show_ping(options.ping);
        std::thread::spawn(move || {
            let result = match crate::data_dir() {
                Some(data) => crate::replay::run(&replay, &data, language, &settings, &mut |reading, done| {
                    qt.queue(move |meter| meter.show(reading, done)).is_ok()
                }),
                None => Err("the game data (src/data) was not found".into()),
            };
            if let Err(error) = result {
                eprintln!("daevalog-dps-meter: replay: {error}");
                let _ = qt.queue(|_| crate::shell::ffi::exit_application(1));
            }
        });
    }

    /// The ping in the footer, and its band; empty when there is none.
    pub fn show_ping(mut self: Pin<&mut Self>, ping_ms: Option<i32>) {
        let text = ping_ms.filter(|&ms| ms >= 0).map_or_else(String::new, |ms| format!("{ms}ms"));
        if self.ping().to_string() != text {
            self.as_mut().set_ping(QString::from(&text));
        }
        let level = ping_level(ping_ms);
        if *self.ping_level() != level {
            self.set_ping_level(level);
        }
    }

    fn redraw(mut self: Pin<&mut Self>) {
        self.as_mut().take_settings();
        let count = self.shown.rows.len() as i32;
        if count > 0 {
            let parent = QModelIndex::default();
            let first = self.index(0, 0, &parent);
            let last = self.index(count - 1, 0, &parent);
            self.as_mut().data_changed(&first, &last, &QList::default());
        }
        let group_total = self.group_text();
        self.as_mut().set_group_total(group_total);
        let party = self.party_text();
        self.as_mut().set_party(party);
    }

    fn save_and_clear(self: Pin<&mut Self>) {
        crate::live::save_and_clear();
    }

    fn pick_mode(self: Pin<&mut Self>, mode: &QString) {
        crate::live::set_mode(&mode.to_string());
    }

    fn change_lowest_first(mut self: Pin<&mut Self>, value: bool) {
        LOWEST_FIRST.store(value, Ordering::SeqCst);
        if self.lowest_first != value {
            self.as_mut().rust_mut().lowest_first = value;
            self.lowest_first_changed();
        }
    }

    fn change_party_shown(mut self: Pin<&mut Self>, value: bool) {
        PARTY_SHOWN.store(value, Ordering::SeqCst);
        if self.party_shown != value {
            self.as_mut().rust_mut().party_shown = value;
            self.party_shown_changed();
        }
    }

    /// The settings the rows' texts follow, read once per reading.
    fn take_settings(mut self: Pin<&mut Self>) {
        let values = crate::settings_store::store().values();
        let mut rust = self.as_mut().rust_mut();
        rust.columns = values.meter_columns.dps;
        rust.heal_columns = values.meter_columns.heal;
        rust.tank_columns = values.meter_columns.tank;
        rust.round_dps = values.round_dps;
    }

    fn party_text(&self) -> QString {
        match &self.shown.party {
            Some(party) => QString::from(&party_json(
                party,
                &self.heal_columns,
                &self.tank_columns,
                self.round_dps,
                &self.shown.combat_powers,
            )),
            None => QString::default(),
        }
    }

    fn group_text(&self) -> QString {
        match self.shown.group_dps {
            Some(dps) => QString::from(&snapshot::group(dps, self.round_dps)),
            None => QString::default(),
        }
    }

    /// Puts a reading on screen.
    pub fn show(mut self: Pin<&mut Self>, reading: Snapshot, done: bool) {
        self.as_mut().take_settings();
        let printing = crate::options().print_fights;
        if printing && self.shown.target_id != 0 && reading.target_id != self.shown.target_id {
            print_fight(&self.shown);
        }

        let party_changed = self.shown.party != reading.party || self.shown.combat_powers != reading.combat_powers;
        // Each player keeps their row, as 1.x's meter.js keeps a row element
        // per id: rows that left go, rows that changed rank move, new ones
        // come in at their place, and only then the numbers change. A reset
        // made every row again, which in a crowd fight (a TARGET switch, two
        // players trading places) was nearly every reading: the rows
        // flickered. Ids that repeat cannot be followed: reset then.
        let old: Vec<i32> = self.shown.rows.iter().map(|r| r.id).collect();
        let new: Vec<i32> = reading.rows.iter().map(|r| r.id).collect();
        match row_steps(&old, &new) {
            Some(steps) => {
                let parent = QModelIndex::default();
                for step in steps {
                    match step {
                        RowStep::Remove(at) => {
                            self.as_mut().begin_remove_rows(&parent, at as i32, at as i32);
                            self.as_mut().rust_mut().shown.rows.remove(at);
                            self.as_mut().end_remove_rows();
                        }
                        RowStep::Move { from, to } => {
                            if self.as_mut().begin_move_rows(&parent, from as i32, from as i32, &parent, to as i32) {
                                let mut this = self.as_mut().rust_mut();
                                let row = this.shown.rows.remove(from);
                                this.shown.rows.insert(to, row);
                                drop(this);
                                self.as_mut().end_move_rows();
                            }
                        }
                        RowStep::Insert(at) => {
                            self.as_mut().begin_insert_rows(&parent, at as i32, at as i32);
                            let row = reading.rows[at].clone();
                            self.as_mut().rust_mut().shown.rows.insert(at, row);
                            self.as_mut().end_insert_rows();
                        }
                    }
                }
                self.as_mut().rust_mut().shown = reading;
                let count = self.shown.rows.len() as i32;
                if count > 0 {
                    let first = self.index(0, 0, &parent);
                    let last = self.index(count - 1, 0, &parent);
                    self.as_mut().data_changed(&first, &last, &QList::default());
                }
            }
            None => {
                self.as_mut().begin_reset_model();
                self.as_mut().rust_mut().shown = reading;
                self.as_mut().end_reset_model();
            }
        }

        let (at, battle_ms, open) = (self.shown.at_ms, self.shown.battle_ms, self.shown.fight_open);
        let ticked = self.as_mut().rust_mut().ticker.update(at, battle_ms, open);
        let shown = &self.shown;
        let fight_time = QString::from(&snapshot::fight_time(ticked));
        let target = QString::from(&shown.target_name);
        let target_id = shown.target_id;
        let mode = QString::from(&shown.mode);
        let boss_health = shown.boss_health;
        let group_total = self.group_text();
        let group_damage = QString::from(&self.shown.group_damage.map_or_else(String::new, |d| snapshot::damage(d as f64)));
        self.as_mut().set_group_damage(group_damage);
        self.as_mut().set_fight_time(fight_time);
        self.as_mut().set_target(target);
        self.as_mut().set_target_id(target_id);
        self.as_mut().set_mode(mode);
        self.as_mut().set_boss_health(boss_health);
        let hp = self.shown.boss_hp.map_or_else(String::new, |(left, max)| {
            format!("{} / {}", snapshot::damage(left as f64), snapshot::damage(max as f64))
        });
        self.as_mut().set_boss_hp_text(QString::from(&hp));
        let (eta, code, server) = (self.shown.boss_eta, self.shown.target_code, self.shown.enrage_ms);
        self.as_mut().set_boss_eta(QString::from(&eta.map_or_else(String::new, snapshot::clock)));
        let (enrage, level) = crate::enrage::countdown(code, server, ticked).unwrap_or_default();
        self.as_mut().set_enrage(QString::from(&enrage));
        if !enrage.is_empty() {
            self.as_mut().set_enrage_level(level);
        }
        let left = crate::enrage::seconds_left(code, server, ticked);
        let (mark, level) = match self.as_mut().rust_mut().kill_check.update(at, target_id, left, eta) {
            Some(true) => ("\u{2713}", 1),
            Some(false) => ("\u{2717}", 3),
            None => ("", 0),
        };
        self.as_mut().set_kill_mark(QString::from(mark));
        if !mark.is_empty() {
            self.as_mut().set_kill_mark_level(level);
        }
        self.as_mut().set_group_total(group_total);
        if party_changed {
            let party = self.party_text();
            self.as_mut().set_party(party);
        }

        if done {
            if printing {
                print_fight(&self.shown);
            }
            self.as_mut().set_replay_done(true);
        }
    }

    fn row_count(&self, _parent: &QModelIndex) -> i32 {
        self.shown.rows.len() as i32
    }

    fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let rows = &self.shown.rows;
        let Some(row) = usize::try_from(index.row()).ok().and_then(|i| rows.get(i)) else {
            return QVariant::default();
        };
        match role {
            r if r == Role::EntityId as i32 => QVariant::from(&row.id),
            r if r == Role::Name as i32 => QVariant::from(&QString::from(&row.name)),
            r if r == Role::Job as i32 => QVariant::from(&QString::from(row.class)),
            r if r == Role::Total as i32 => QVariant::from(&QString::from(&snapshot::damage(row.damage as f64))),
            r if r == Role::Rate as i32 => QVariant::from(&QString::from(&snapshot::rate(row.dps as f64))),
            r if r == Role::Share as i32 => QVariant::from(&QString::from(&snapshot::share(row.share))),
            r if r == Role::Fill as i32 => QVariant::from(&row.fill),
            r if r == Role::You as i32 => QVariant::from(&row.you),
            r if r == Role::Unattributed as i32 => QVariant::from(&row.unattributed),
            r if r == Role::Identifying as i32 => QVariant::from(&row.identifying),
            r if r == Role::Rank as i32 => QVariant::from(&(row.rank as i32)),
            r if r == Role::Number as i32 => QVariant::from(&(row.number as i32)),
            r if r == Role::Texts as i32 => {
                let texts: QList<QString> =
                    self.columns.iter().map(|key| QString::from(&row.cell(key, self.round_dps))).collect();
                QVariant::from(&QStringList::from(&texts))
            }
            r if r == Role::CombatPower as i32 => QVariant::from(&QString::from(&snapshot::combat_power(row.combat_power))),
            // The share as a number, for the low-share colours.
            r if r == Role::ShareValue as i32 => QVariant::from(&row.share),
            r if r == Role::TicksOnly as i32 => QVariant::from(&row.ticks_only()),
            _ => QVariant::default(),
        }
    }

    fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut roles = QHash::<QHashPair_i32_QByteArray>::default();
        for (role, name) in ROLES {
            roles.insert(role as i32, QByteArray::from(name));
        }
        roles
    }
}

/// The split view's healer columns (MeterWindow.qml `healerColumns`).
const HEALER_COLUMNS: [&str; 3] = ["healed", "hps", "pct"];

/// The Heal and Tank tabs and the split view's healers as the window draws
/// them: each row as `MeterRow` takes it, with its texts for `heal` and
/// `tank`, the tabs' columns, and its combat power from `powers` by name;
/// the footer's group HPS and DTPS.
fn party_json(party: &Party, heal: &[&str], tank: &[&str], round: bool, powers: &HashMap<String, i64>) -> String {
    let rows = |rows: &[PartyRow], keys: &[&str]| -> Vec<serde_json::Value> {
        rows.iter()
            .map(|row| {
                serde_json::json!({
                    "entityId": row.id,
                    "name": row.name,
                    "job": snapshot::class_key(&row.job),
                    "identifying": row.identifying,
                    "unattributed": row.unattributed,
                    "you": row.you,
                    "number": row.number,
                    "rank": row.rank,
                    "fill": row.fill,
                    "total": snapshot::damage(row.amount as f64),
                    "rate": snapshot::party_cell(row, "hps", round),
                    "share": snapshot::share(row.share),
                    "texts": keys.iter().map(|key| snapshot::party_cell(row, key, round)).collect::<Vec<_>>(),
                    "combatPower": snapshot::combat_power(powers.get(&row.name).copied().unwrap_or(0)),
                })
            })
            .collect()
    };
    let total = |v: Option<f64>| v.map_or_else(String::new, |v| if round { snapshot::rate(v) } else { snapshot::whole(v) });
    serde_json::json!({
        "heal": rows(&party.heal, heal),
        "tank": rows(&party.tank, tank),
        "healers": rows(&party.healers, &HEALER_COLUMNS),
        "healTotal": total(party.heal_total),
        "tankTotal": total(party.tank_total),
    })
    .to_string()
}

/// The ping's band, which picks its colour (Theme.pingColors): 0 none yet,
/// 1 up to 80 ms, 2 up to 150 ms, 3 above.
pub fn ping_level(ping_ms: Option<i32>) -> i32 {
    match ping_ms {
        None => 0,
        Some(ms) if ms < 0 => 0,
        Some(0..=80) => 1,
        Some(81..=150) => 2,
        Some(_) => 3,
    }
}

/// --print-fights: a fight as the window shows it, by entity id; names stay
/// out, since the logs hold other players'.
fn print_fight(shown: &Snapshot) {
    use chrono::TimeZone;
    let at = chrono::Local
        .timestamp_millis_opt(shown.at_ms)
        .single()
        .map_or_else(String::new, |t| t.format("%H:%M:%S%.3f").to_string());
    let total: i64 = shown.rows.iter().map(|r| r.damage).sum();
    println!(
        "fight at {at}: target {} \"{}\", {} {}, {} rows, damage {total}, group DPS {}",
        shown.target_id,
        shown.target_name,
        mode_label(&shown.mode),
        snapshot::fight_time(shown.battle_ms),
        shown.rows.len(),
        shown.group_dps.map_or_else(|| "-".into(), |dps| dps.to_string()),
    );
    for row in &shown.rows {
        println!(
            "  #{} {}{} {} damage {} dps {} share {:.1}%",
            row.rank,
            row.id,
            if row.you { " (you)" } else { "" },
            if row.class.is_empty() { "-" } else { row.class },
            row.damage,
            row.dps,
            row.share,
        );
    }
    let Some(party) = &shown.party else { return };
    for (tab, rows) in [("heal", &party.heal), ("tank", &party.tank)] {
        for row in rows {
            println!(
                "  {tab} #{} {}{} {} per second {} share {:.1}%",
                row.rank,
                row.id,
                if row.you { " (you)" } else { "" },
                row.amount,
                row.per_second.map_or_else(|| "-".into(), |v| format!("{v:.1}")),
                row.share,
            );
        }
    }
}

/// One change to the meter's rows, by position at the time it is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowStep {
    Remove(usize),
    /// Up only: `to` < `from`, so `to` is also Qt's destination row.
    Move { from: usize, to: usize },
    Insert(usize),
}

/// The steps that turn the rows `old` into `new`, ids in order: the rows
/// gone first, then from the top each place takes its row, moved up from
/// further down or new. `None` when an id repeats on either side.
fn row_steps(old: &[i32], new: &[i32]) -> Option<Vec<RowStep>> {
    let unique = |ids: &[i32]| {
        let mut seen = std::collections::HashSet::new();
        ids.iter().all(|id| seen.insert(*id))
    };
    if !unique(old) || !unique(new) {
        return None;
    }
    let mut rows: Vec<i32> = old.to_vec();
    let mut steps = Vec::new();
    for at in (0..rows.len()).rev() {
        if !new.contains(&rows[at]) {
            rows.remove(at);
            steps.push(RowStep::Remove(at));
        }
    }
    for (to, id) in new.iter().enumerate() {
        if rows.get(to) == Some(id) {
            continue;
        }
        match rows.iter().position(|r| r == id) {
            Some(from) => {
                let row = rows.remove(from);
                rows.insert(to, row);
                steps.push(RowStep::Move { from, to });
            }
            None => {
                rows.insert(to, *id);
                steps.push(RowStep::Insert(to));
            }
        }
    }
    Some(steps)
}

#[cfg(test)]
mod tests {
    use super::{row_steps, RowStep};

    #[test]
    fn lowest_first_orders_the_rows() {
        // The ⋯ menu's Lowest first once flipped a mark that nothing read.
        use std::sync::atomic::Ordering;
        let values = crate::settings_store::Values { player_limit: 3, ..Default::default() };
        super::LOWEST_FIRST.store(true, Ordering::SeqCst);
        let up = super::view_options(&values);
        super::LOWEST_FIRST.store(false, Ordering::SeqCst);
        assert!(up.ascending);
        assert_eq!(up.player_limit, 3, "the settings stay");
        assert!(!super::view_options(&values).ascending);
    }

    /// Applies `steps` to `old` as the model does.
    fn apply(old: &[i32], new: &[i32], steps: &[RowStep]) -> Vec<i32> {
        let mut rows = old.to_vec();
        for step in steps {
            match *step {
                RowStep::Remove(at) => {
                    rows.remove(at);
                }
                RowStep::Move { from, to } => {
                    assert!(to < from, "moves go up");
                    let row = rows.remove(from);
                    rows.insert(to, row);
                }
                RowStep::Insert(at) => rows.insert(at, new[at]),
            }
        }
        rows
    }

    #[test]
    fn rows_keep_their_place_and_change_only_where_they_must() {
        // Same rows: nothing to do.
        assert_eq!(row_steps(&[1, 2, 3], &[1, 2, 3]), Some(vec![]));
        // A new player below: one insert at the end, the rows above stay.
        assert_eq!(row_steps(&[1, 2], &[1, 2, 3]), Some(vec![RowStep::Insert(2)]));
        // Two players trade places: one move.
        assert_eq!(row_steps(&[1, 2, 3], &[2, 1, 3]), Some(vec![RowStep::Move { from: 1, to: 0 }]));
        // One left: one removal.
        assert_eq!(row_steps(&[1, 2, 3], &[1, 3]), Some(vec![RowStep::Remove(1)]));
        // Repeated ids cannot be followed.
        assert_eq!(row_steps(&[1, 1], &[1]), None);
        // Any change, as a TARGET switch in a crowd makes: the steps land on `new`.
        let cases: [(&[i32], &[i32]); 5] = [
            (&[5, 9, 2, 7, 3], &[9, 4, 5, 3, 8, 2]),
            (&[], &[1, 2, 3]),
            (&[1, 2, 3], &[]),
            (&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], &[10, 3, 11]),
            (&[4, 3, 2, 1], &[1, 2, 3, 4]),
        ];
        for (old, new) in cases {
            let steps = row_steps(old, new).unwrap();
            assert_eq!(apply(old, new, &steps), new, "{old:?} -> {new:?}");
        }
    }

    use super::*;

    #[test]
    fn the_tabs_come_with_their_columns_texts() {
        let row = PartyRow {
            id: 11,
            name: "a".into(),
            identifying: false,
            unattributed: false,
            job: daevalog::entity::job_class::JobClass::Cleric.class_name().into(),
            you: true,
            number: 0,
            rank: 1,
            amount: 46_830,
            per_second: Some(650.0),
            share: 100.0,
            fill: 1.0,
            taken: 1_200,
            shield_block: 0,
            parry: 2,
            records: 4,
            attacks: 9,
        };
        let party = Party { heal: vec![row.clone()], tank: vec![row.clone()], healers: vec![row], heal_total: None, tank_total: Some(1_250.0) };
        let powers = HashMap::from([("a".to_string(), 44_315)]);
        let v: serde_json::Value = serde_json::from_str(&party_json(&party, &["hps", "taken"], &["parry"], true, &powers)).unwrap();
        assert_eq!(v["heal"][0]["texts"], serde_json::json!(["650", "1.2K"]));
        assert_eq!(v["tank"][0]["texts"], serde_json::json!(["2"]));
        assert_eq!(v["healers"][0]["texts"], serde_json::json!(["46.8K", "650", "100%"]));
        assert_eq!((v["heal"][0]["job"].as_str(), v["heal"][0]["entityId"].as_i64()), (Some("cleric"), Some(11)));
        assert_eq!((v["healTotal"].as_str(), v["tankTotal"].as_str()), (Some(""), Some("1.2K")));
        // Combat power by name, on every tab, as on the DPS rows.
        for tab in ["heal", "tank", "healers"] {
            assert_eq!(v[tab][0]["combatPower"], "44K", "{tab}");
        }
        let none: serde_json::Value = serde_json::from_str(&party_json(&party, &[], &[], true, &HashMap::new())).unwrap();
        assert_eq!(none["heal"][0]["combatPower"], "", "outside the party");
    }

    #[test]
    fn the_ping_band_changes_after_80_and_after_150_ms() {
        assert_eq!(ping_level(None), 0);
        assert_eq!(ping_level(Some(-1)), 0);
        assert_eq!(ping_level(Some(0)), 1);
        assert_eq!(ping_level(Some(80)), 1);
        assert_eq!(ping_level(Some(81)), 2);
        assert_eq!(ping_level(Some(150)), 2);
        assert_eq!(ping_level(Some(151)), 3);
        assert_eq!(ping_level(Some(9999)), 3);
    }
}
