// SPDX-License-Identifier: AGPL-3.0-only

//! The settings the Qt meter uses, typed, with their defaults and checks,
//! kept in `settings.json` in the meter's own data folder. The file has the
//! 1.x meter's format and keys (string values, "dpsMeter.language"), so a
//! key means the same in both. The Qt meter keeps its own folder because the
//! two meters can run side by side and some 1.x keys change meaning in 2.0;
//! on its first start it takes over the 1.x meter's settings once.
//!
//! `Settings` (settings.rs) shows these to QML; the meter's threads read
//! them here.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{OnceLock, RwLock},
};

use daevalog::combat::data_storage::DataStorage;
use daevalog::combat::dps_calculator::DpsCalculator;
use daevalog::combat::meter_view::ViewOptions;
use daevalog::config::setting_changes::{ENCOUNTER_TIMEOUT_KEY, apply_encounter_timeout};
use daevalog::config::settings::Settings as File;
use daevalog::platform::hotkeys::parse_hotkey_label;

use crate::i18n::LANGUAGES;

/// The Qt meter's data folder, under the system's data folder
/// (~/.local/share on Linux): its app id (build.rs).
pub const FOLDER: &str = env!("DAEVALOG_APP_ID");
/// The data folder of the 2.0 test builds and packages (daevalog-qml),
/// moved to `FOLDER` on the first start.
pub const BETA_FOLDER: &str = "daevalog-qml";
/// The 1.x meter's data folder (its app id).
pub const LEGACY_FOLDER: &str = "com.daevalog.dps-meter";
const FILE_NAME: &str = "settings.json";

/// The backend's mode ids, in the footer's order.
pub const MODES: [&str; 5] = ["bossTargets", "lastHitByMe", "allTargets", "trainTargets", "encounter"];

/// The meter's row limits to pick from: a party (5), a field event (8), a
/// raid (10), and 12 for room (from the game's group sizes). The meter grows a row at a time up to the limit; a crowd past it
/// shows the top rows. 1.x offered 4 to 12 and started at 6.
pub const PLAYER_LIMITS: [usize; 4] = [5, 8, 10, 12];
pub const DEFAULT_PLAYER_LIMIT: usize = 10;

/// A saved or given row limit as one of `PLAYER_LIMITS`: the next one up, so
/// no one loses rows they had, and 12 at most.
pub fn player_limit(n: usize) -> usize {
    PLAYER_LIMITS.into_iter().find(|&l| l >= n).unwrap_or(PLAYER_LIMITS[PLAYER_LIMITS.len() - 1])
}

pub fn folder() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join(FOLDER))
}

/// The 1.x meter's folder to take settings and fights from. The developer
/// build takes none: they belong to the release.
pub fn legacy_folder() -> Option<PathBuf> {
    if cfg!(feature = "dev") {
        return None;
    }
    Some(dirs::data_dir()?.join(LEGACY_FOLDER))
}

/// Readies the data folder on the first start, before anything reads it:
/// moves the 2.0 test builds' folder there, or else copies the 1.x meter's
/// saved fights (`Store::open` takes over its settings). The 1.x folder
/// stays as it is, for daevalog-dps-meter-legacy. Returns what it did, for
/// the log.
pub fn prepare_folder() -> Option<String> {
    let base = dirs::data_dir()?;
    prepare(&base.join(FOLDER), &base.join(BETA_FOLDER), legacy_folder().as_deref())
}

fn prepare(folder: &Path, beta: &Path, legacy: Option<&Path>) -> Option<String> {
    if folder.exists() {
        return None;
    }
    if beta.is_dir() {
        return Some(match std::fs::rename(beta, folder) {
            Ok(()) => format!("moved {} to {}", beta.display(), folder.display()),
            Err(error) => format!("could not move {} to {}: {error}", beta.display(), folder.display()),
        });
    }
    let history = legacy?.join("history");
    if !history.is_dir() {
        return None;
    }
    // Copied beside the folder and then renamed, so a copy cut short is
    // tried again on the next start.
    let part = folder.with_file_name(format!("{FOLDER}.part"));
    let _ = std::fs::remove_dir_all(&part);
    let copied = copy_files(&history, &part.join("history")).and_then(|n| std::fs::rename(&part, folder).map(|()| n));
    Some(match copied {
        Ok(n) => format!("copied {n} saved fights from the 1.x meter ({})", history.display()),
        Err(error) => {
            let _ = std::fs::remove_dir_all(&part);
            format!("could not copy the 1.x meter's saved fights from {}: {error}", history.display())
        }
    })
}

/// Copies the files in `from` (not its folders) to a new folder `to`. The
/// copies are private, whatever modes the 1.x files had.
fn copy_files(from: &Path, to: &Path) -> std::io::Result<usize> {
    use daevalog::platform::files::{create_private_dir, private_options};

    create_private_dir(to)?;
    let mut n = 0;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            let mut copy = private_options().write(true).create_new(true).open(to.join(entry.file_name()))?;
            std::io::copy(&mut std::fs::File::open(entry.path())?, &mut copy)?;
            n += 1;
        }
    }
    Ok(n)
}

/// The settings the Qt meter uses now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Language,
    TimeFormat,
    DefaultMode,
    PlayerLimit,
    PinYou,
    ShowYou,
    HideOtherNames,
    Locked,
    LayerShell,
    WindowOpacity,
    ReloadHotkey,
    ToggleWindowHotkey,
    LockHotkey,
    Accent,
    SlimMode,
    MeterSize,
    SplitView,
    BarStrength,
    TargetNameSize,
    BoldNames,
    BoldDps,
    AutoHide,
    StartInTray,
    HideFromTaskbar,
    ShowPing,
    ShowTotal,
    FitRows,
    RoundDps,
    ShowSuspendButton,
    ShowLockButton,
    MeterColumns,
    EncounterTimeout,
    DebugLogging,
    PacketLogging,
    TrainingMode,
    AllTargetsWindow,
    DetailsFontSize,
    DetailsIconSize,
    HiddenColumns,
    SeenColumns,
    ScreenshotWithMeter,
    ScreenshotToFolder,
    ScreenshotFolder,
    ColorTheme,
    DetailsMonitor,
    LayerPosition,
    HealthText,
    LowShare,
    GroupDamage,
    SpawnNotices,
    ShardWarning,
    ServerTimeZone,
    TrayEdge,
}

impl Key {
    pub const ALL: [Key; 53] = [
        Key::Language,
        Key::TimeFormat,
        Key::DefaultMode,
        Key::PlayerLimit,
        Key::PinYou,
        Key::ShowYou,
        Key::HideOtherNames,
        Key::Locked,
        Key::LayerShell,
        Key::WindowOpacity,
        Key::ReloadHotkey,
        Key::ToggleWindowHotkey,
        Key::LockHotkey,
        Key::Accent,
        Key::SlimMode,
        Key::MeterSize,
        Key::SplitView,
        Key::BarStrength,
        Key::TargetNameSize,
        Key::BoldNames,
        Key::BoldDps,
        Key::AutoHide,
        Key::StartInTray,
        Key::HideFromTaskbar,
        Key::ShowPing,
        Key::ShowTotal,
        Key::FitRows,
        Key::RoundDps,
        Key::ShowSuspendButton,
        Key::ShowLockButton,
        Key::MeterColumns,
        Key::EncounterTimeout,
        Key::DebugLogging,
        Key::PacketLogging,
        Key::TrainingMode,
        Key::AllTargetsWindow,
        Key::DetailsFontSize,
        Key::DetailsIconSize,
        Key::HiddenColumns,
        Key::SeenColumns,
        Key::ScreenshotWithMeter,
        Key::ScreenshotToFolder,
        Key::ScreenshotFolder,
        Key::ColorTheme,
        Key::DetailsMonitor,
        Key::LayerPosition,
        Key::HealthText,
        Key::LowShare,
        Key::GroupDamage,
        Key::SpawnNotices,
        Key::ShardWarning,
        Key::ServerTimeZone,
        Key::TrayEdge,
    ];

    /// A switch: "true" or "false". `Values::flag_mut` is the one list.
    fn is_switch(self) -> bool {
        Values::default().flag_mut(self).is_some()
    }

    /// The key in settings.json; the 1.x meter's, except those of settings
    /// new in 2.0: "YOU", the accent, the split view, the bar strength and
    /// the meter's columns.
    pub fn name(self) -> &'static str {
        match self {
            Key::Language => "dpsMeter.language",
            Key::TimeFormat => "dpsMeter.timeFormat",
            Key::DefaultMode => "dpsMeter.defaultMeterMode",
            Key::PlayerLimit => "dpsMeter.playerLimit",
            Key::PinYou => "dpsMeter.pinMeToTop",
            Key::ShowYou => "dpsMeter.showYou",
            Key::HideOtherNames => "dpsMeter.hideOtherNames",
            Key::Locked => "dpsMeter.overlayLocked",
            Key::LayerShell => "dpsMeter.waylandLayer",
            Key::WindowOpacity => "dpsMeter.windowOpacity",
            Key::ReloadHotkey => "dpsMeter.hotkey",
            Key::ToggleWindowHotkey => "dpsMeter.toggleWindowHotkey",
            Key::LockHotkey => "dpsMeter.lockHotkey",
            Key::Accent => "dpsMeter.accent",
            Key::SlimMode => "dpsMeter.slimMode",
            // New in 2.0: 1.x's meter had one size.
            Key::MeterSize => "dpsMeter.meterSize",
            Key::SplitView => "dpsMeter.splitView",
            Key::BarStrength => "dpsMeter.barStrength",
            Key::TargetNameSize => "dpsMeter.bossNameSize",
            Key::BoldNames => "dpsMeter.mainPlayerNamesBold",
            Key::BoldDps => "dpsMeter.mainPlayerDpsBold",
            Key::AutoHide => "dpsMeter.autoHideMeter",
            Key::StartInTray => "dpsMeter.startInTray",
            Key::HideFromTaskbar => "dpsMeter.hideFromTaskbar",
            Key::ShowPing => "dpsMeter.showPing",
            Key::ShowTotal => "dpsMeter.showTotalDps",
            Key::FitRows => "dpsMeter.fitRows",
            Key::RoundDps => "dpsMeter.roundDps",
            Key::ShowSuspendButton => "dpsMeter.showSuspendBtn",
            Key::ShowLockButton => "dpsMeter.showLockBtn",
            Key::MeterColumns => "dpsMeter.meterColumns",
            Key::EncounterTimeout => ENCOUNTER_TIMEOUT_KEY,
            Key::DebugLogging => "dpsMeter.debugLoggingEnabled",
            Key::PacketLogging => "dpsMeter.saveRawPackets",
            Key::TrainingMode => "dpsMeter.trainSelectionMode",
            Key::AllTargetsWindow => "dpsMeter.allTargetsWindowMs",
            Key::DetailsFontSize => "dpsMeter.detailsFontSize",
            Key::DetailsIconSize => "dpsMeter.detailsIconSize",
            Key::HiddenColumns => "dpsMeter.detailsHiddenColumns",
            Key::SeenColumns => "dpsMeter.detailsSeenColumns",
            Key::ScreenshotWithMeter => "dpsMeter.detailsIncludeMeterScreenshot",
            Key::ScreenshotToFolder => "dpsMeter.detailsSaveScreenshotToFolder",
            Key::ScreenshotFolder => "dpsMeter.detailsScreenshotFolder",
            // New in 2.0: 1.x's ten themes were dpsMeter.theme.
            Key::ColorTheme => "dpsMeter.colorTheme",
            Key::DetailsMonitor => "dpsMeter.detailsMonitor",
            // New in 2.0: 1.x's window.* keys were Tauri's window places.
            Key::LayerPosition => "dpsMeter.layerPosition",
            // New in 2.0: 1.x's two looks came with its Beta and Classic
            // layouts.
            Key::HealthText => "dpsMeter.healthText",
            // New in 2.0: 1.x always recoloured the bar.
            Key::LowShare => "dpsMeter.lowShare",
            // New in 2.0: 1.x's place for it came with its layouts.
            Key::GroupDamage => "dpsMeter.groupDamage",
            // New in 2.0: the spawn notices came with the notice tray.
            Key::SpawnNotices => "dpsMeter.spawnNotices",
            // New in 2.0, as the spawn notices.
            Key::ShardWarning => "dpsMeter.shardWarning",
            Key::ServerTimeZone => "dpsMeter.serverTimeZone",
            Key::TrayEdge => "dpsMeter.trayEdge",
        }
    }
}

/// The accent presets.
pub const ACCENTS: [&str; 4] = ["sky", "rose", "lemon", "silver"];
/// The health bar's text: both in the middle (1.x's Classic layout), the
/// default, so the health left is read without the eye moving; or the
/// percent at the left end and the health at the right (1.x's Beta layout).
pub const HEALTH_TEXTS: [&str; 2] = ["centred", "split"];
/// How the meter marks a player under 5% (amber) or 3% (red) of the damage
/// shown: not at all, on the share's number, or on the whole row. Row is the
/// default: 1.x always recoloured the bar, with no setting.
pub const LOW_SHARES: [&str; 3] = ["off", "number", "row"];
/// Where the group's damage shows beside its DPS: in the footer (1.x's Beta
/// layout, the default) or on a line of its own under the rows (1.x's
/// Classic layout).
pub const GROUP_DAMAGES: [&str; 2] = ["footer", "line"];
/// Which edge of the meter the notice tray sits on: Auto (the default) puts
/// it on top, or at the bottom when the open list has no room above.
pub const TRAY_EDGES: [&str; 3] = ["auto", "top", "bottom"];
/// The meter window's sizes, in percent (MeterSizes.qml): 100 is the
/// default, the size the meter was drawn in.
pub const METER_SIZES: [u8; 4] = [100, 115, 130, 150];
/// The colour themes (Theme.qml): Daevalog first, the default.
pub const COLOR_THEMES: [&str; 4] = ["daevalog", "classic", "nord", "system"];
/// ALL mode's window choices, in ms; 0 is everything since the zone change.
pub const ALL_TARGETS_WINDOWS: [u32; 6] = [0, 30_000, 60_000, 120_000, 180_000, 300_000];
/// TRAIN mode's Details: every dummy, or the one the meter shows.
pub const TRAINING_MODES: [&str; 2] = ["all", "highestDamage"];

/// Each meter tab's columns, in the order a row shows them (1.x's ENC list
/// for DPS, `meterColumns.js`). Keys as 1.x saved them: `encDps` is DPS over
/// the whole fight, `dps` Active DPS.
pub const DPS_COLUMNS: [&str; 10] = ["total", "encDps", "dps", "pct", "crit", "last10", "last30", "last60", "maxHit", "hits"];
pub const HEAL_COLUMNS: [&str; 4] = ["healed", "hps", "pct", "taken"];
pub const TANK_COLUMNS: [&str; 5] = ["taken", "dtps", "pct", "block", "parry"];
/// Columns test builds had and 2.0 dropped: a saved choice just loses them.
const DROPPED_COLUMNS: [&str; 1] = ["eff"];

/// The number columns of the meter's tabs, each in its list's order.
#[derive(Debug, Clone, PartialEq)]
pub struct MeterColumns {
    pub dps: Vec<&'static str>,
    pub heal: Vec<&'static str>,
    pub tank: Vec<&'static str>,
}

impl Default for MeterColumns {
    fn default() -> Self {
        Self {
            dps: vec!["total", "encDps", "pct"],
            heal: vec!["healed", "hps", "taken"],
            tank: vec!["taken", "dtps", "pct"],
        }
    }
}

impl MeterColumns {
    /// From its saved form, {"dps": [...], "heal": [...], "tank": [...]}; a
    /// tab left out keeps its default. Any number of a tab's columns, as 1.x's
    /// ENC columns (up to all ten); `None` for an unknown column; a dropped
    /// column is left out.
    fn parse(raw: &str) -> Option<Self> {
        let saved: HashMap<String, Vec<String>> = serde_json::from_str(raw).ok()?;
        let mut columns = Self::default();
        for (tab, mut keys) in saved {
            keys.retain(|k| !DROPPED_COLUMNS.contains(&k.as_str()));
            let (list, known): (&mut Vec<&'static str>, &[&'static str]) = match tab.as_str() {
                "dps" => (&mut columns.dps, &DPS_COLUMNS),
                "heal" => (&mut columns.heal, &HEAL_COLUMNS),
                "tank" => (&mut columns.tank, &TANK_COLUMNS),
                _ => return None,
            };
            if keys.iter().any(|k| !known.contains(&k.as_str())) {
                return None;
            }
            *list = known.iter().copied().filter(|k| keys.iter().any(|w| w == k)).collect();
        }
        Some(columns)
    }

    fn saved(&self) -> String {
        serde_json::json!({ "dps": self.dps, "heal": self.heal, "tank": self.tank }).to_string()
    }
}

/// The Details window's skill columns, in its order (DetailsWindow.qml).
pub const SKILL_COLUMNS: [&str; 21] = [
    "hit", "dmg", "dmgpct", "mhit", "mdmg", "crit", "parry", "perfect", "double", "back", "frontal", "block",
    "perfectblock", "ironwall", "regeneration", "miss", "resist", "regen", "mindmg", "avgdmg", "maxdmg",
];
/// Shown until the player saves a choice (the 2.0 screens' default).
const DEFAULT_SKILL_COLUMNS: [&str; 5] = ["hit", "dmg", "dmgpct", "crit", "avgdmg"];
/// Columns 1.x added after its first release: hidden until the player has
/// switched them once (detailsSettings.js).
const LATER_SKILL_COLUMNS: [&str; 8] = ["regen", "parry", "block", "perfectblock", "ironwall", "regeneration", "miss", "resist"];

/// 1.x settings that 2.0 keeps with the same meaning but does not use yet,
/// so they are taken over now and wait in the file. By the section they
/// will have.
pub const KEPT_FOR_LATER: [&str; 5] = [
    // Connection: the Capture object reads the device (`DEVICE_KEY`);
    // History reads the name already
    "dpsMeter.userName",
    "dpsMeter.manualDevice",
    // Account
    "dpsMeter.autoUpload",
    // History, which reads and writes them as they are (`Store::raw`)
    "historyViewMode",
    "historyShowTraining",
];

// Not taken over, because 2.0 drops them or gives them another meaning:
// - dpsMeter.meterFillOpacity: a gradient at 10-100% (every 1.x install was
//   reset to 80); 2.0 has a flat fill at 10-50%, default 40.
// - dpsMeter.theme: ten themes; 2.0 has one look with accent colours.
// - dpsMeter.betaUi: the classic or beta layout; 2.0 has one.
// - dpsMeter.displayMode: the DPS / DMG / BOTH toggle, dropped.
// - dpsMeter.encColumns: up to ten ENC columns; 2.0 has a column picker per
//   tab, any number of the tab's columns.
// - dpsMeter.bossLogsEnabled: did nothing in 1.x.
// - dpsMeter.detailsBackgroundOpacity: Details' background; 2.0's is solid.
// - dpsMeter.migration.opacityReset1: a 1.x migration mark.
// - window.*: where 1.x's windows were.
// - backend.screenshotFolder: the folder the backend picked; the player's
//   own choice (dpsMeter.detailsScreenshotFolder) is kept.
// Only in the 1.x page's own storage, never in settings.json, so out of
// reach: the last mode used (1.x starts in the default mode anyway) and
// "only show me".

/// The values, each checked; anything missing or not valid is the default.
#[derive(Debug, Clone, PartialEq)]
pub struct Values {
    /// One of `LANGUAGES`; `None` follows the system's language.
    pub language: Option<String>,
    /// Display Time: times of day on a 24-hour clock ("24h") or a 12-hour
    /// one ("12h"); `None` follows the system's clock (`uses_24h`).
    pub clock_24h: Option<bool>,
    /// The mode the meter starts in, as the backend's id.
    pub default_mode: String,
    /// Rows shown; your own row comes on top when it is not among them.
    pub player_limit: usize,
    /// Your row first.
    pub pin_you: bool,
    /// "YOU" on your row instead of your name.
    pub show_you: bool,
    /// Other players as their class and a number.
    pub hide_other_names: bool,
    /// Clicks go through the meter; kept across restarts, as in 1.x.
    pub locked: bool,
    /// The meter as a Wayland layer-shell surface. Off here; while the
    /// player has not set it, `Store::default_layer_shell` turns it on on
    /// Hyprland, Sway and KDE (`display::layer_shell_by_default`).
    pub layer_shell: bool,
    /// The panel's background, in percent.
    pub window_opacity: u8,
    /// Global hotkeys, as labels ("Ctrl+Alt+R").
    pub reload_hotkey: String,
    pub toggle_window_hotkey: String,
    pub lock_hotkey: String,
    /// One of `ACCENTS`.
    pub accent: String,
    /// 22 px rows, no column heads, no boss strip.
    pub slim_mode: bool,
    /// The meter window's text, rows and bands, one of `METER_SIZES`.
    pub meter_size: u8,
    /// The healers under the DPS tab.
    pub split_view: bool,
    /// The bars' fill, 10-50%.
    pub bar_strength: u8,
    /// The target's name in the header, 8-20 px, rounded to whole pixels.
    pub target_name_size: f64,
    /// Names DemiBold and numbers Medium; normal weight when off.
    pub bold_names: bool,
    pub bold_dps: bool,
    /// Hide the meter while the game is not in front.
    pub auto_hide: bool,
    /// Start with the meter hidden in the tray, where there is one.
    pub start_in_tray: bool,
    /// Keep the app's windows out of the taskbar.
    pub hide_from_taskbar: bool,
    pub show_ping: bool,
    /// Countdowns in the notice tray before each Abyss Boss and Spacetime Rift.
    pub spawn_notices: bool,
    /// A notice in the tray when your hits stop using Power Shards.
    pub shard_warning: bool,
    /// The game server's time zone, an IANA name; empty: this computer's.
    pub server_time_zone: String,
    /// The group total in the footer.
    pub show_total: bool,
    /// The meter's height follows its rows; dragging an edge sets only the
    /// width. Off: the player sets the height.
    pub fit_rows: bool,
    /// DPS in thousands ("19.5K"); off, the whole number ("19,512").
    pub round_dps: bool,
    /// Suspend and lock as header buttons instead of in the ⋯ menu.
    pub show_suspend_button: bool,
    pub show_lock_button: bool,
    pub meter_columns: MeterColumns,
    /// Seconds without combat that end an encounter, 5-300.
    pub encounter_timeout: u16,
    /// debug.log and the packet log, in the data folder.
    pub debug_logging: bool,
    pub packet_logging: bool,
    /// One of `TRAINING_MODES`.
    pub training_mode: String,
    /// One of `ALL_TARGETS_WINDOWS`.
    pub all_targets_window_ms: u32,
    /// The Details window's text, 11-20 px, and skill icons, 20-56 px.
    pub details_font_size: u8,
    pub details_icon_size: u8,
    /// The skill columns hidden, as 1.x saves them; `None` until a choice
    /// is saved (`skill_columns`).
    pub hidden_columns: Option<Vec<String>>,
    /// The skill columns the player has switched.
    pub seen_columns: Vec<String>,
    /// Details' Screenshot: the meter beside Details, and a PNG file too.
    pub screenshot_with_meter: bool,
    pub screenshot_to_folder: bool,
    /// Where the PNG goes, an absolute path; empty is the default folder
    /// (`screenshot_folder`).
    pub screenshot_folder: String,
    /// One of `COLOR_THEMES`.
    pub color_theme: String,
    /// The screen (Qt's name, like "DP-2") Details opens on at the start and
    /// stays on; empty is off. X11 only: Wayland lets no window pick its
    /// screen, so the setting is hidden there.
    pub details_monitor: String,
    /// Where the layer-shell meter was last dropped: its margins from the
    /// screen's top-left corner, in pixels. `None` is the starting place.
    pub layer_position: Option<(i32, i32)>,
    /// One of `HEALTH_TEXTS`.
    pub health_text: String,
    /// One of `LOW_SHARES`.
    pub low_share: String,
    /// One of `GROUP_DAMAGES`.
    pub group_damage: String,
    /// One of `TRAY_EDGES`.
    pub tray_edge: String,
}

impl Default for Values {
    fn default() -> Self {
        Self {
            language: None,
            clock_24h: None,
            default_mode: MODES[0].to_string(),
            player_limit: DEFAULT_PLAYER_LIMIT,
            pin_you: false,
            show_you: false,
            hide_other_names: false,
            locked: false,
            layer_shell: false,
            // The 2.0 default: white text on every class's bar (40% fill)
            // keeps 3.7:1 or better over snow at 72%; at 60% Assassin and
            // Cleric drop to about 3:1. A value saved in
            // 1.x stays, except 1.x's default (see `LEGACY_DEFAULT_OPACITY`).
            window_opacity: 72,
            // What the 1.x backend registered when none was saved.
            reload_hotkey: "Ctrl+Alt+R".into(),
            toggle_window_hotkey: "Ctrl+Alt+Up".into(),
            lock_hotkey: "Ctrl+Alt+L".into(),
            accent: ACCENTS[0].into(),
            slim_mode: false,
            meter_size: METER_SIZES[0],
            split_view: false,
            bar_strength: 40,
            // The approved meter's (1.x: 15). Each other default is 1.x's
            // unless a comment says so; the accent, the split view and the
            // bar strength are new in 2.0.
            target_name_size: 13.0,
            bold_names: true,
            bold_dps: true,
            auto_hide: true,
            start_in_tray: false,
            hide_from_taskbar: false,
            show_ping: true,
            spawn_notices: true,
            shard_warning: true,
            server_time_zone: String::new(),
            show_total: true,
            fit_rows: true,
            round_dps: true,
            show_suspend_button: false,
            show_lock_button: false,
            meter_columns: MeterColumns::default(),
            encounter_timeout: 15,
            debug_logging: false,
            packet_logging: false,
            training_mode: TRAINING_MODES[0].into(),
            all_targets_window_ms: 0,
            // Details is a normal window in 2.0, on the type scale: 1.x had
            // 16 px and 36 px.
            details_font_size: 13,
            details_icon_size: 20,
            hidden_columns: None,
            seen_columns: Vec::new(),
            screenshot_with_meter: false,
            screenshot_to_folder: false,
            screenshot_folder: String::new(),
            details_monitor: String::new(),
            layer_position: None,
            color_theme: COLOR_THEMES[0].into(),
            health_text: HEALTH_TEXTS[0].into(),
            low_share: "row".into(),
            group_damage: GROUP_DAMAGES[0].into(),
            tray_edge: TRAY_EDGES[0].into(),
        }
    }
}

fn parse_bool(raw: &str) -> Option<bool> {
    match raw {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// An IANA time zone name as written ("America/Los_Angeles"); whether the
/// zone exists is Qt's to say (`world_spawns::valid_zone`).
fn zone_name(raw: &str) -> bool {
    raw.len() <= 64 && raw.contains('/') && raw.bytes().all(|b| b.is_ascii_alphanumeric() || b"/_+-".contains(&b))
}

fn number(raw: &str) -> Option<f64> {
    raw.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// A number as JavaScript writes it: "13", "14.5", "0.8".
fn number_text(v: f64) -> String {
    format!("{v}")
}

/// A list of column names as 1.x saves it, a JSON array of strings.
fn column_list(raw: &str) -> Option<Vec<String>> {
    let list: Vec<String> = serde_json::from_str(raw).ok()?;
    Some(list.into_iter().filter(|c| !c.trim().is_empty()).collect())
}

impl Values {
    /// The values saved under each key's name.
    pub fn from_saved(saved: impl Fn(&str) -> Option<String>) -> Self {
        let mut values = Self::default();
        for key in Key::ALL {
            if let Some(raw) = saved(key.name()) {
                values.take(key, &raw);
            }
        }
        values
    }

    /// Sets `key` from its saved form; false, and nothing changed, when
    /// `raw` is not a valid value for it.
    pub fn take(&mut self, key: Key, raw: &str) -> bool {
        let raw = raw.trim();
        match key {
            Key::Language if raw.is_empty() => self.language = None,
            Key::Language if LANGUAGES.contains(&raw) => self.language = Some(raw.to_string()),
            Key::TimeFormat => match raw {
                "" => self.clock_24h = None,
                "12h" => self.clock_24h = Some(false),
                "24h" => self.clock_24h = Some(true),
                _ => return false,
            },
            Key::DefaultMode if MODES.contains(&raw) => self.default_mode = raw.to_string(),
            Key::PlayerLimit => match raw.parse::<usize>() {
                Ok(n) if n >= 1 => self.player_limit = player_limit(n),
                _ => return false,
            },
            key if key.is_switch() => {
                let Some(on) = parse_bool(raw) else { return false };
                *self.flag_mut(key).expect("a switch") = on;
            }
            // 1.x rounds and keeps it within 0-100.
            Key::WindowOpacity => match raw.parse::<f64>() {
                Ok(v) if v.is_finite() => self.window_opacity = v.round().clamp(0.0, 100.0) as u8,
                _ => return false,
            },
            Key::ReloadHotkey | Key::ToggleWindowHotkey | Key::LockHotkey if parse_hotkey_label(raw).is_some() => {
                *self.hotkey_mut(key) = raw.to_string();
            }
            Key::Accent if ACCENTS.contains(&raw) => self.accent = raw.to_string(),
            Key::MeterSize => match raw.parse::<u8>() {
                Ok(size) if METER_SIZES.contains(&size) => self.meter_size = size,
                _ => return false,
            },
            // A number out of range goes to the nearest end, as 1.x's
            // controls do; anything else is refused.
            Key::BarStrength => match number(raw) {
                Some(v) => self.bar_strength = v.round().clamp(10.0, 50.0) as u8,
                None => return false,
            },
            Key::TargetNameSize => match number(raw) {
                // Whole pixels: Qt draws font sizes in whole pixels (1.x had
                // half pixels; a saved 14.5 loads as 15).
                Some(v) => self.target_name_size = v.round().clamp(8.0, 20.0),
                None => return false,
            },
            Key::MeterColumns => match MeterColumns::parse(raw) {
                Some(columns) => self.meter_columns = columns,
                None => return false,
            },
            Key::EncounterTimeout => match number(raw) {
                Some(v) => self.encounter_timeout = v.round().clamp(5.0, 300.0) as u16,
                None => return false,
            },
            Key::TrainingMode if TRAINING_MODES.contains(&raw) => self.training_mode = raw.to_string(),
            Key::AllTargetsWindow => match raw.parse::<u32>() {
                Ok(ms) if ALL_TARGETS_WINDOWS.contains(&ms) => self.all_targets_window_ms = ms,
                _ => return false,
            },
            Key::DetailsFontSize => match number(raw) {
                Some(v) => self.details_font_size = v.round().clamp(11.0, 20.0) as u8,
                None => return false,
            },
            Key::DetailsIconSize => match number(raw) {
                Some(v) => self.details_icon_size = v.round().clamp(20.0, 56.0) as u8,
                None => return false,
            },
            Key::HiddenColumns => match column_list(raw) {
                Some(list) => self.hidden_columns = Some(list),
                None => return false,
            },
            Key::SeenColumns => match column_list(raw) {
                Some(list) => self.seen_columns = list,
                None => return false,
            },
            Key::ScreenshotFolder if raw.is_empty() || Path::new(raw).is_absolute() => self.screenshot_folder = raw.to_string(),
            Key::ColorTheme if COLOR_THEMES.contains(&raw) => self.color_theme = raw.to_string(),
            Key::HealthText if HEALTH_TEXTS.contains(&raw) => self.health_text = raw.to_string(),
            Key::LowShare if LOW_SHARES.contains(&raw) => self.low_share = raw.to_string(),
            Key::GroupDamage if GROUP_DAMAGES.contains(&raw) => self.group_damage = raw.to_string(),
            Key::TrayEdge if TRAY_EDGES.contains(&raw) => self.tray_edge = raw.to_string(),
            // 1.x saved "off" or a monitor number in Tauri's order, which
            // Qt's screen list need not share: a number is refused (off).
            Key::DetailsMonitor if raw.is_empty() || raw == "off" => self.details_monitor.clear(),
            Key::DetailsMonitor if !raw.bytes().all(|b| b.is_ascii_digit()) => self.details_monitor = raw.to_string(),
            Key::ServerTimeZone if raw.is_empty() || zone_name(raw) => self.server_time_zone = raw.to_string(),
            Key::LayerPosition if raw.is_empty() => self.layer_position = None,
            Key::LayerPosition => match raw.split_once(',').map(|(x, y)| (x.trim().parse::<i32>(), y.trim().parse::<i32>())) {
                Some((Ok(x), Ok(y))) if x >= 0 && y >= 0 => self.layer_position = Some((x, y)),
                _ => return false,
            },
            _ => return false,
        }
        true
    }

    /// How `key` is saved; `None` removes it from the file.
    pub fn saved(&self, key: Key) -> Option<String> {
        match key {
            Key::Language => self.language.clone(),
            Key::TimeFormat => self.clock_24h.map(|h24| if h24 { "24h" } else { "12h" }.to_string()),
            Key::DefaultMode => Some(self.default_mode.clone()),
            Key::PlayerLimit => Some(self.player_limit.to_string()),
            key if key.is_switch() => {
                let mut values = self.clone();
                values.flag_mut(key).map(|on| on.to_string())
            }
            Key::WindowOpacity => Some(self.window_opacity.to_string()),
            Key::ReloadHotkey | Key::ToggleWindowHotkey | Key::LockHotkey => {
                let mut values = self.clone();
                Some(values.hotkey_mut(key).clone())
            }
            Key::Accent => Some(self.accent.clone()),
            Key::MeterSize => Some(self.meter_size.to_string()),
            Key::BarStrength => Some(self.bar_strength.to_string()),
            Key::TargetNameSize => Some(number_text(self.target_name_size)),
            Key::MeterColumns => Some(self.meter_columns.saved()),
            Key::EncounterTimeout => Some(self.encounter_timeout.to_string()),
            Key::TrainingMode => Some(self.training_mode.clone()),
            Key::AllTargetsWindow => Some(self.all_targets_window_ms.to_string()),
            Key::DetailsFontSize => Some(self.details_font_size.to_string()),
            Key::DetailsIconSize => Some(self.details_icon_size.to_string()),
            Key::HiddenColumns => self.hidden_columns.as_ref().map(|list| serde_json::json!(list).to_string()),
            Key::SeenColumns => Some(serde_json::json!(self.seen_columns).to_string()),
            Key::ScreenshotFolder => (!self.screenshot_folder.is_empty()).then(|| self.screenshot_folder.clone()),
            Key::ColorTheme => Some(self.color_theme.clone()),
            Key::HealthText => Some(self.health_text.clone()),
            Key::LowShare => Some(self.low_share.clone()),
            Key::GroupDamage => Some(self.group_damage.clone()),
            Key::TrayEdge => Some(self.tray_edge.clone()),
            Key::DetailsMonitor => (!self.details_monitor.is_empty()).then(|| self.details_monitor.clone()),
            Key::LayerPosition => self.layer_position.map(|(x, y)| format!("{x},{y}")),
            Key::ServerTimeZone => (!self.server_time_zone.is_empty()).then(|| self.server_time_zone.clone()),
            _ => unreachable!("{key:?} has a case above"),
        }
    }

    /// The switch `key` sets; None when it is no switch.
    fn flag_mut(&mut self, key: Key) -> Option<&mut bool> {
        Some(match key {
            Key::PinYou => &mut self.pin_you,
            Key::ShowYou => &mut self.show_you,
            Key::HideOtherNames => &mut self.hide_other_names,
            Key::Locked => &mut self.locked,
            Key::LayerShell => &mut self.layer_shell,
            Key::SlimMode => &mut self.slim_mode,
            Key::SplitView => &mut self.split_view,
            Key::BoldNames => &mut self.bold_names,
            Key::BoldDps => &mut self.bold_dps,
            Key::AutoHide => &mut self.auto_hide,
            Key::StartInTray => &mut self.start_in_tray,
            Key::HideFromTaskbar => &mut self.hide_from_taskbar,
            Key::ShowPing => &mut self.show_ping,
            Key::ShowTotal => &mut self.show_total,
            Key::FitRows => &mut self.fit_rows,
            Key::RoundDps => &mut self.round_dps,
            Key::ShowSuspendButton => &mut self.show_suspend_button,
            Key::ShowLockButton => &mut self.show_lock_button,
            Key::DebugLogging => &mut self.debug_logging,
            Key::PacketLogging => &mut self.packet_logging,
            Key::ScreenshotWithMeter => &mut self.screenshot_with_meter,
            Key::ScreenshotToFolder => &mut self.screenshot_to_folder,
            Key::SpawnNotices => &mut self.spawn_notices,
            Key::ShardWarning => &mut self.shard_warning,
            _ => return None,
        })
    }

    fn hotkey_mut(&mut self, key: Key) -> &mut String {
        match key {
            Key::ReloadHotkey => &mut self.reload_hotkey,
            Key::ToggleWindowHotkey => &mut self.toggle_window_hotkey,
            Key::LockHotkey => &mut self.lock_hotkey,
            _ => unreachable!("{key:?} is not a hotkey"),
        }
    }

    /// Whether times of day are on a 24-hour clock: the player's choice,
    /// else the system's (`system`), else 12-hour, as in 1.x.
    pub fn uses_24h(&self, system: Option<bool>) -> bool {
        self.clock_24h.or(system).unwrap_or(false)
    }

    /// Where Details' screenshots go: the folder chosen, else
    /// `Pictures/Daevalog DPS Meter`, as 1.x; `None` with no Pictures folder.
    pub fn screenshot_folder(&self) -> Option<PathBuf> {
        if !self.screenshot_folder.is_empty() {
            return Some(PathBuf::from(&self.screenshot_folder));
        }
        Some(dirs::picture_dir()?.join("Daevalog DPS Meter"))
    }

    /// Which rows the meter shows, and in which order.
    pub fn view(&self) -> ViewOptions {
        ViewOptions { player_limit: self.player_limit, pin_you: self.pin_you, ..ViewOptions::default() }
    }

    /// The Details skill columns shown, in the window's order. Until a
    /// choice is saved, the 2.0 default; after, 1.x's rule: what was not
    /// hidden, less the later columns the player has not switched yet.
    pub fn skill_columns(&self) -> Vec<&'static str> {
        let Some(hidden) = &self.hidden_columns else {
            return SKILL_COLUMNS.into_iter().filter(|c| DEFAULT_SKILL_COLUMNS.contains(c)).collect();
        };
        let seen = |c: &str| self.seen_columns.iter().any(|s| s == c);
        SKILL_COLUMNS
            .into_iter()
            .filter(|&c| !hidden.iter().any(|h| h == c) && (!LATER_SKILL_COLUMNS.contains(&c) || seen(c)))
            .collect()
    }
}

/// Settings from the command line, for this run only: never saved.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Overrides {
    pub language: Option<String>,
    pub default_mode: Option<String>,
    pub player_limit: Option<usize>,
    pub pin_you: bool,
    pub show_you: bool,
    pub hide_other_names: bool,
    pub locked: bool,
    pub layer_shell: bool,
    /// --open meter/slim and meter/split.
    pub slim_mode: bool,
    pub split_view: bool,
    /// --open meter/health-split: the health bar's other text.
    pub health_split: bool,
    /// --open meter/low-share-number and meter/damage-line: those choices.
    pub low_share_number: bool,
    pub damage_line: bool,
    /// --theme: one of `COLOR_THEMES`.
    pub color_theme: Option<String>,
    /// --columns: the DPS tab's columns, any of `DPS_COLUMNS`.
    pub dps_columns: Option<Vec<&'static str>>,
    /// --icon-size and --font-size: Details' skill icons and text, in px.
    pub details_icon_size: Option<u8>,
    pub details_font_size: Option<u8>,
    /// --meter-size: one of `METER_SIZES`.
    pub meter_size: Option<u8>,
}

impl Overrides {
    /// Puts the command line's settings on top of `values`.
    pub fn apply_to(&self, values: &mut Values) {
        self.apply(values);
    }

    fn apply(&self, values: &mut Values) {
        if let Some(language) = &self.language {
            values.language = Some(language.clone());
        }
        if let Some(mode) = &self.default_mode {
            values.default_mode = mode.clone();
        }
        if let Some(n) = self.player_limit {
            values.player_limit = player_limit(n);
        }
        values.pin_you |= self.pin_you;
        values.show_you |= self.show_you;
        values.hide_other_names |= self.hide_other_names;
        values.locked |= self.locked;
        values.layer_shell |= self.layer_shell;
        values.slim_mode |= self.slim_mode;
        if self.health_split {
            values.health_text = "split".into();
        }
        if self.low_share_number {
            values.low_share = "number".into();
        }
        if self.damage_line {
            values.group_damage = "line".into();
        }
        values.split_view |= self.split_view;
        if let Some(theme) = &self.color_theme {
            values.color_theme = theme.clone();
        }
        if let Some(columns) = &self.dps_columns {
            values.meter_columns.dps = DPS_COLUMNS.iter().copied().filter(|k| columns.contains(k)).collect();
        }
        if let Some(px) = self.details_icon_size {
            values.details_icon_size = px.clamp(20, 56);
        }
        if let Some(px) = self.details_font_size {
            values.details_font_size = px.clamp(11, 20);
        }
        if let Some(size) = self.meter_size.filter(|size| METER_SIZES.contains(size)) {
            values.meter_size = size;
        }
    }
}

/// The settings of this run: the file, and the values with the command
/// line's on top.
pub struct Store {
    /// `None`: nothing is read or written.
    file: Option<File>,
    /// The data folder, where the logs go too; `None` with no file.
    folder: Option<PathBuf>,
    values: RwLock<Values>,
    /// This desktop's layer-shell default and the command line's settings,
    /// put on top again by `reset`.
    layer_default: RwLock<Option<bool>>,
    overrides: RwLock<Overrides>,
}

impl Store {
    /// The defaults, saved nowhere.
    pub fn in_memory() -> Self {
        Self {
            file: None,
            folder: None,
            values: RwLock::new(Values::default()),
            layer_default: RwLock::new(None),
            overrides: RwLock::new(Overrides::default()),
        }
    }

    /// The settings in `folder`. When it has none yet, the 1.x meter's
    /// settings in `legacy` are taken over first; after that the folder has
    /// its own file, so they are taken over once.
    pub fn open(folder: &Path, legacy: Option<&Path>) -> Self {
        let first = !folder.join(FILE_NAME).exists();
        // Writes the file at once when there is none.
        let file = File::new(folder.to_path_buf());
        if first {
            if let Some(legacy) = legacy {
                let taken = take_over(&file, &legacy.join(FILE_NAME));
                if taken > 0 {
                    eprintln!("daevalog-dps-meter: took over {taken} settings from the 1.x meter");
                }
            }
        }
        let values = Values::from_saved(|key| file.get(key));
        Self {
            file: Some(file),
            folder: Some(folder.to_path_buf()),
            values: RwLock::new(values),
            layer_default: RwLock::new(None),
            overrides: RwLock::new(Overrides::default()),
        }
    }

    pub fn values(&self) -> Values {
        self.values.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Changes one setting and saves it. Returns the new values, or `None`
    /// when `raw` is not a valid value for `key`.
    pub fn set(&self, key: Key, raw: &str) -> Option<Values> {
        let mut values = self.values.write().unwrap_or_else(|e| e.into_inner());
        let mut next = values.clone();
        if !next.take(key, raw) {
            return None;
        }
        if let Some(file) = &self.file {
            match next.saved(key) {
                Some(value) => {
                    file.set(key.name(), &value);
                }
                None => file.remove(key.name()),
            }
        }
        *values = next.clone();
        Some(next)
    }

    /// Shows the Details skill columns in `shown` and hides the rest, saved
    /// as 1.x saves them: the hidden ones, and each column switched marked
    /// as seen. `None` for a column the window does not have.
    pub fn set_skill_columns(&self, shown: &[String]) -> Option<Values> {
        if shown.iter().any(|c| !SKILL_COLUMNS.contains(&c.as_str())) {
            return None;
        }
        let mut values = self.values.write().unwrap_or_else(|e| e.into_inner());
        let mut next = values.clone();
        let before = values.skill_columns();
        let is_shown = |c: &str| shown.iter().any(|s| s == c);
        for column in SKILL_COLUMNS {
            if before.contains(&column) != is_shown(column) && !next.seen_columns.iter().any(|s| s == column) {
                next.seen_columns.push(column.to_string());
            }
        }
        next.hidden_columns = Some(SKILL_COLUMNS.into_iter().filter(|&c| !is_shown(c)).map(String::from).collect());
        if let Some(file) = &self.file {
            for key in [Key::HiddenColumns, Key::SeenColumns] {
                if let Some(value) = next.saved(key) {
                    file.set(key.name(), &value);
                }
            }
        }
        *values = next.clone();
        Some(next)
    }

    /// Starts or stops debug.log and the packet log, in the data folder, as
    /// the settings say; with no folder (a replay) nothing is written.
    pub fn apply_logging(&self) {
        let Some(folder) = &self.folder else { return };
        // On the first start the settings file, and so the folder, is still
        // waiting for its writer; the logs need the folder now.
        let _ = daevalog::platform::files::create_private_dir(folder);
        let values = self.values();
        daevalog::logging::logger::set_debug_enabled(values.debug_logging, folder);
        daevalog::logging::logger::set_packet_log_enabled(values.packet_logging, folder);
    }

    /// The layer-shell switch for this desktop, used while the player has
    /// not set it.
    pub fn default_layer_shell(&self, on: bool) {
        *self.layer_default.write().unwrap_or_else(|e| e.into_inner()) = Some(on);
        let unset = self.file.as_ref().is_none_or(|file| file.get(Key::LayerShell.name()).is_none());
        if unset {
            self.values.write().unwrap_or_else(|e| e.into_inner()).layer_shell = on;
        }
    }

    /// The command line's settings, for this run.
    pub fn apply(&self, overrides: &Overrides) {
        overrides.apply(&mut self.values.write().unwrap_or_else(|e| e.into_inner()));
        *self.overrides.write().unwrap_or_else(|e| e.into_inner()) = overrides.clone();
    }

    /// Every setting back to its default, as 1.x's Reset all settings: the
    /// file emptied, every key in it, 1.x's too. The file stays, so the 1.x
    /// settings are not taken over again. This desktop's layer default and
    /// the command line's settings go on top again. Returns the values.
    pub fn reset(&self) -> Values {
        if let Some(file) = &self.file {
            file.clear();
        }
        let mut next = Values::default();
        if let Some(on) = *self.layer_default.read().unwrap_or_else(|e| e.into_inner()) {
            next.layer_shell = on;
        }
        self.overrides.read().unwrap_or_else(|e| e.into_inner()).apply(&mut next);
        *self.values.write().unwrap_or_else(|e| e.into_inner()) = next.clone();
        next
    }

    /// A setting as saved, for those kept as 1.x saved them.
    pub fn raw(&self, name: &str) -> Option<String> {
        self.file.as_ref()?.get(name)
    }

    /// Saves a setting kept as 1.x saves it; nothing without a file.
    pub fn set_raw(&self, name: &str, value: &str) {
        if let Some(file) = &self.file {
            file.set(name, value);
        }
    }

    /// Waits for the last change to reach the disk; for quitting.
    pub fn flush(&self) -> Result<(), String> {
        self.file.as_ref().map_or(Ok(()), File::flush)
    }
}

/// The settings the backend's meter takes, as last handed over.
#[derive(Debug, Default)]
pub struct MeterApplied {
    encounter_timeout: Option<u16>,
    all_targets_window_ms: Option<u32>,
}

impl MeterApplied {
    /// Hands the settings that changed since the last call to the backend,
    /// through the code 1.x uses: the encounter timeout to storage, ALL
    /// mode's window to the meter. Returns the keys handed over.
    pub fn apply(&mut self, values: &Values, storage: &DataStorage, meter: &mut DpsCalculator) -> Vec<Key> {
        let mut applied = Vec::new();
        if self.encounter_timeout != Some(values.encounter_timeout) {
            apply_encounter_timeout(storage, values.saved(Key::EncounterTimeout).as_deref());
            self.encounter_timeout = Some(values.encounter_timeout);
            applied.push(Key::EncounterTimeout);
        }
        if self.all_targets_window_ms != Some(values.all_targets_window_ms) {
            meter.set_all_targets_window_ms(i64::from(values.all_targets_window_ms));
            self.all_targets_window_ms = Some(values.all_targets_window_ms);
            applied.push(Key::AllTargetsWindow);
        }
        applied
    }
}

/// The 1.x meter's window opacity when none was saved. A saved 40 counts as
/// never changed, so it is not taken over and the 2.0 default applies.
const LEGACY_DEFAULT_OPACITY: u8 = 40;

/// Copies the 1.x settings that keep their meaning from `legacy` into
/// `file`: the ones used now if valid, the later ones as they are. Returns
/// how many.
fn take_over(file: &File, legacy: &Path) -> usize {
    let Some(saved) = std::fs::read_to_string(legacy)
        .ok()
        .and_then(|text| serde_json::from_str::<HashMap<String, String>>(&text).ok())
    else {
        return 0;
    };
    let mut taken = 0;
    for key in Key::ALL {
        let Some(raw) = saved.get(key.name()) else { continue };
        let mut values = Values::default();
        if values.take(key, raw) {
            if key == Key::WindowOpacity && values.window_opacity == LEGACY_DEFAULT_OPACITY {
                continue;
            }
            if let Some(value) = values.saved(key) {
                file.set(key.name(), &value);
                taken += 1;
            }
        }
    }
    for name in KEPT_FOR_LATER {
        if let Some(value) = saved.get(name) {
            file.set(name, value);
            taken += 1;
        }
    }
    taken
}

static STORE: OnceLock<Store> = OnceLock::new();

/// Makes `store` this run's settings; once, before Qt starts.
pub fn init(store: Store) {
    if STORE.set(store).is_err() {
        panic!("the settings are set once");
    }
}

/// This run's settings; the defaults if `init` was not called.
pub fn store() -> &'static Store {
    STORE.get_or_init(Store::in_memory)
}

/// The system's clock, read once: 24-hour or not, `None` when the system
/// does not say (the time locale, `platform::process::clock_24h`).
pub fn system_clock_24h() -> Option<bool> {
    static SYSTEM: OnceLock<Option<bool>> = OnceLock::new();
    *SYSTEM.get_or_init(daevalog::platform::process::clock_24h)
}

/// Whether this run writes times of day on a 24-hour clock (`uses_24h`).
pub fn clock_24h() -> bool {
    store().values().uses_24h(system_clock_24h())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    use super::*;

    /// A whole path to a screenshot folder on this platform.
    #[cfg(windows)]
    const SHOTS: &str = r"C:\Users\p\Shots";
    #[cfg(not(windows))]
    const SHOTS: &str = "/home/p/Shots";

    /// A temporary folder, removed when dropped.
    struct Folder(PathBuf);

    impl Folder {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "daevalog-qml-settings-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write(&self, pairs: &[(&str, &str)]) {
            let map: HashMap<&str, &str> = pairs.iter().copied().collect();
            std::fs::write(self.0.join(FILE_NAME), serde_json::to_string(&map).unwrap()).unwrap();
        }

        fn read(&self) -> HashMap<String, String> {
            serde_json::from_slice(&std::fs::read(self.0.join(FILE_NAME)).unwrap()).unwrap()
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn the_first_start_moves_the_test_builds_folder() {
        let base = Folder::new();
        let (folder, beta, legacy) = (base.0.join("new"), base.0.join("beta"), base.0.join("legacy"));
        for dir in [&beta, &legacy] {
            std::fs::create_dir_all(dir.join("history")).unwrap();
            std::fs::write(dir.join(FILE_NAME), "{}").unwrap();
            std::fs::write(dir.join("history/a.json"), "{}").unwrap();
        }
        assert!(prepare(&folder, &beta, Some(&legacy)).unwrap().starts_with("moved"));
        assert!(folder.join(FILE_NAME).is_file() && folder.join("history/a.json").is_file());
        assert!(!beta.exists());
        // Once there, the folder is left alone.
        std::fs::create_dir_all(&beta).unwrap();
        assert_eq!(prepare(&folder, &beta, Some(&legacy)), None);
        assert!(beta.exists() && legacy.join("history/a.json").is_file());
    }

    #[test]
    fn the_first_start_copies_the_1x_fights() {
        let base = Folder::new();
        let (folder, beta, legacy) = (base.0.join("new"), base.0.join("beta"), base.0.join("legacy"));
        std::fs::create_dir_all(legacy.join("history/sub")).unwrap();
        std::fs::write(legacy.join("history/a.json"), "{\"id\":\"a\"}").unwrap();
        std::fs::write(legacy.join("history/b.json"), "{}").unwrap();
        #[cfg(unix)]
        set_mode(&legacy.join("history/a.json"), 0o644);
        assert!(prepare(&folder, &beta, Some(&legacy)).unwrap().starts_with("copied 2 "));
        assert_eq!(std::fs::read_to_string(folder.join("history/a.json")).unwrap(), "{\"id\":\"a\"}");
        // The copies are the player's alone, whatever the 1.x modes were.
        #[cfg(unix)]
        for name in ["a.json", "b.json"] {
            assert_eq!(mode(&folder.join("history").join(name)), 0o600, "{name}");
        }
        assert!(folder.join("history/b.json").is_file() && !folder.join("history/sub").exists());
        // The 1.x folder keeps its fights.
        assert!(legacy.join("history/a.json").is_file());
        assert!(!base.0.join(format!("{FOLDER}.part")).exists());
        assert_eq!(prepare(&folder, &beta, Some(&legacy)), None);
    }

    #[test]
    fn a_first_start_with_nothing_to_take_over_makes_no_folder() {
        let base = Folder::new();
        let folder = base.0.join("new");
        assert_eq!(prepare(&folder, &base.0.join("beta"), Some(&base.0.join("legacy"))), None);
        assert_eq!(prepare(&folder, &base.0.join("beta"), None), None);
        assert!(!folder.exists());
    }

    #[test]
    fn nothing_saved_gives_the_defaults_and_an_empty_file() {
        let folder = Folder::new();
        let store = Store::open(&folder.0, None);
        assert_eq!(store.values(), Values::default());
        assert_eq!(store.values().view(), ViewOptions { player_limit: DEFAULT_PLAYER_LIMIT, ..ViewOptions::default() });
        store.flush().unwrap();
        assert!(folder.read().is_empty());
    }

    #[test]
    fn the_desktops_layer_default_holds_until_the_player_sets_it() {
        let store = Store::in_memory();
        store.default_layer_shell(true);
        assert!(store.values().layer_shell);

        let folder = Folder::new();
        let store = Store::open(&folder.0, None);
        store.default_layer_shell(true);
        assert!(store.values().layer_shell);
        assert!(store.set(Key::LayerShell, "false").is_some());
        store.flush().unwrap();
        let store = Store::open(&folder.0, None);
        store.default_layer_shell(true);
        assert!(!store.values().layer_shell, "the player's choice stays");
        // Nothing is written for the default.
        let folder = Folder::new();
        Store::open(&folder.0, None).default_layer_shell(true);
        assert!(!folder.read().contains_key(Key::LayerShell.name()));
    }

    #[test]
    fn the_saved_layer_switch_decides_the_start_and_the_flag_wins() {
        // As main() sets up a run: the desktop's default, then the command line.
        let start = |folder: &Folder, desktop_default: bool, flag: bool| {
            let store = Store::open(&folder.0, None);
            store.default_layer_shell(desktop_default);
            store.apply(&Overrides { layer_shell: flag, ..Overrides::default() });
            store.values().layer_shell
        };
        let folder = Folder::new();
        assert!(!start(&folder, false, false), "KDE: off until the player turns it on");
        assert!(start(&folder, false, true), "--layer-shell");
        let store = Store::open(&folder.0, None);
        assert!(store.set(Key::LayerShell, "true").is_some());
        store.flush().unwrap();
        drop(store);
        assert!(start(&folder, false, false), "turned on in Settings: on from the next start");
        let store = Store::open(&folder.0, None);
        assert!(store.set(Key::LayerShell, "false").is_some());
        store.flush().unwrap();
        drop(store);
        assert!(!start(&folder, true, false), "turned off on Hyprland: off from the next start");
        assert!(start(&folder, true, true), "--layer-shell wins over the saved switch");
        let saved = folder.read().get(Key::LayerShell.name()).cloned();
        assert_eq!(saved.as_deref(), Some("false"), "the flag is not saved");
    }

    #[test]
    fn the_server_time_zone_takes_a_zone_name_or_nothing() {
        let mut v = Values::default();
        assert_eq!(v.server_time_zone, "", "this computer's zone until the player picks one");
        assert!(v.take(Key::ServerTimeZone, "America/Los_Angeles"));
        assert_eq!(v.saved(Key::ServerTimeZone).as_deref(), Some("America/Los_Angeles"));
        for junk in ["Pacific", "../../etc/passwd", "America/Los Angeles", "Europe/Berlin;rm"] {
            assert!(!v.take(Key::ServerTimeZone, junk), "{junk}");
        }
        assert!(v.take(Key::ServerTimeZone, ""));
        assert_eq!(v.saved(Key::ServerTimeZone), None);
        assert!(v.spawn_notices, "on by default");
        assert!(v.shard_warning, "on by default");
    }

    #[test]
    fn every_setting_round_trips_through_the_file() {
        let folder = Folder::new();
        let changes = [
            (Key::Language, "ko"),
            (Key::TimeFormat, "24h"),
            (Key::DefaultMode, "trainTargets"),
            (Key::PlayerLimit, "10"),
            (Key::PinYou, "true"),
            (Key::ShowYou, "true"),
            (Key::HideOtherNames, "true"),
            (Key::Locked, "true"),
            (Key::LayerShell, "true"),
            (Key::WindowOpacity, "85"),
            (Key::ReloadHotkey, "Ctrl+Shift+F5"),
            (Key::ToggleWindowHotkey, "Alt+Down"),
            (Key::LockHotkey, "Ctrl+K"),
            (Key::Accent, "lemon"),
            (Key::SlimMode, "true"),
            (Key::MeterSize, "130"),
            (Key::SplitView, "true"),
            (Key::BarStrength, "25"),
            (Key::TargetNameSize, "15"),
            (Key::BoldNames, "false"),
            (Key::BoldDps, "false"),
            (Key::AutoHide, "false"),
            (Key::StartInTray, "true"),
            (Key::HideFromTaskbar, "true"),
            (Key::ShowPing, "false"),
            (Key::ShowTotal, "false"),
            (Key::FitRows, "false"),
            (Key::RoundDps, "false"),
            (Key::ShowSuspendButton, "true"),
            (Key::ShowLockButton, "true"),
            (Key::MeterColumns, r#"{"dps":["hits","encDps","crit"],"heal":["hps"],"tank":[]}"#),
            (Key::EncounterTimeout, "45"),
            (Key::DebugLogging, "true"),
            (Key::PacketLogging, "true"),
            (Key::TrainingMode, "highestDamage"),
            (Key::AllTargetsWindow, "120000"),
            (Key::DetailsFontSize, "16"),
            (Key::DetailsIconSize, "36"),
            (Key::HiddenColumns, r#"["mhit","regen"]"#),
            (Key::SeenColumns, r#"["regen","miss"]"#),
            (Key::ScreenshotWithMeter, "true"),
            (Key::ScreenshotToFolder, "true"),
            (Key::ScreenshotFolder, SHOTS),
            (Key::ColorTheme, "nord"),
            (Key::DetailsMonitor, "DP-2"),
            (Key::LayerPosition, "1200,640"),
            (Key::HealthText, "split"),
            (Key::LowShare, "number"),
            (Key::GroupDamage, "line"),
            (Key::SpawnNotices, "false"),
            (Key::ShardWarning, "false"),
            (Key::ServerTimeZone, "America/New_York"),
            (Key::TrayEdge, "bottom"),
        ];
        assert_eq!(changes.len(), Key::ALL.len(), "every setting");
        let set = {
            let store = Store::open(&folder.0, None);
            for (key, raw) in changes {
                assert!(store.set(key, raw).is_some(), "{key:?} {raw}");
            }
            store.flush().unwrap();
            store.values()
        };
        let expected = Values {
            language: Some("ko".into()),
            clock_24h: Some(true),
            default_mode: "trainTargets".into(),
            player_limit: 10,
            pin_you: true,
            show_you: true,
            hide_other_names: true,
            locked: true,
            layer_shell: true,
            window_opacity: 85,
            reload_hotkey: "Ctrl+Shift+F5".into(),
            toggle_window_hotkey: "Alt+Down".into(),
            lock_hotkey: "Ctrl+K".into(),
            accent: "lemon".into(),
            slim_mode: true,
            meter_size: 130,
            split_view: true,
            bar_strength: 25,
            target_name_size: 15.0,
            bold_names: false,
            bold_dps: false,
            auto_hide: false,
            start_in_tray: true,
            hide_from_taskbar: true,
            show_ping: false,
            spawn_notices: false,
            shard_warning: false,
            server_time_zone: "America/New_York".into(),
            show_total: false,
            fit_rows: false,
            round_dps: false,
            show_suspend_button: true,
            show_lock_button: true,
            // In each list's order.
            meter_columns: MeterColumns { dps: vec!["encDps", "crit", "hits"], heal: vec!["hps"], tank: vec![] },
            encounter_timeout: 45,
            debug_logging: true,
            packet_logging: true,
            training_mode: "highestDamage".into(),
            all_targets_window_ms: 120_000,
            details_font_size: 16,
            details_icon_size: 36,
            hidden_columns: Some(vec!["mhit".into(), "regen".into()]),
            seen_columns: vec!["regen".into(), "miss".into()],
            screenshot_with_meter: true,
            screenshot_to_folder: true,
            screenshot_folder: SHOTS.into(),
            color_theme: "nord".into(),
            details_monitor: "DP-2".into(),
            layer_position: Some((1200, 640)),
            health_text: "split".into(),
            low_share: "number".into(),
            group_damage: "line".into(),
            tray_edge: "bottom".into(),
        };
        assert_eq!(set, expected);
        let file = folder.read();
        assert_eq!(file.get("dpsMeter.pinMeToTop").map(String::as_str), Some("true"));
        // The 1.x keys and forms.
        assert_eq!(file.get("dpsMeter.bossNameSize").map(String::as_str), Some("15"));
        assert_eq!(file.get("dpsMeter.meterSize").map(String::as_str), Some("130"));
        assert_eq!(file.get("dpsMeter.encounterTimeoutSec").map(String::as_str), Some("45"));
        assert_eq!(file.get("dpsMeter.showTotalDps").map(String::as_str), Some("false"));
        assert_eq!(file.get("dpsMeter.detailsHiddenColumns").map(String::as_str), Some(r#"["mhit","regen"]"#));
        assert_eq!(
            file.get("dpsMeter.meterColumns").map(String::as_str),
            Some(r#"{"dps":["encDps","crit","hits"],"heal":["hps"],"tank":[]}"#)
        );
        assert_eq!(Store::open(&folder.0, None).values(), expected);

        // Back to the system's language and clock: the keys go.
        let store = Store::open(&folder.0, None);
        assert_eq!(store.set(Key::Language, "").unwrap().language, None);
        assert_eq!(store.set(Key::TimeFormat, "").unwrap().clock_24h, None);
        store.flush().unwrap();
        assert!(!folder.read().contains_key("dpsMeter.language"));
        assert!(!folder.read().contains_key("dpsMeter.timeFormat"));
    }

    #[test]
    fn display_time_is_the_setting_else_the_systems_clock_else_12_hour() {
        let mut values = Values::default();
        assert!(!values.uses_24h(None), "nothing says: 12-hour, as 1.x");
        assert!(values.uses_24h(Some(true)), "the system's clock");
        assert!(!values.uses_24h(Some(false)));
        assert!(values.take(Key::TimeFormat, "12h"));
        assert!(!values.uses_24h(Some(true)), "the setting beats the system");
        assert_eq!(values.saved(Key::TimeFormat).as_deref(), Some("12h"));
        assert!(values.take(Key::TimeFormat, "24h"));
        assert!(values.uses_24h(Some(false)) && values.uses_24h(None));
        assert_eq!(values.saved(Key::TimeFormat).as_deref(), Some("24h"));
        assert!(values.take(Key::TimeFormat, ""));
        assert_eq!((values.clock_24h, values.saved(Key::TimeFormat)), (None, None));
    }

    #[test]
    fn a_tab_takes_any_number_of_its_columns() {
        // Every DPS column, as 1.x's ENC columns allowed, saved backwards: they
        // come back in the list's order and are saved that way.
        let backwards: Vec<&str> = DPS_COLUMNS.iter().rev().copied().collect();
        let raw = serde_json::json!({ "dps": backwards, "tank": TANK_COLUMNS }).to_string();
        let columns = MeterColumns::parse(&raw).expect("every column of a tab is taken");
        assert_eq!(columns.dps, DPS_COLUMNS.to_vec());
        assert_eq!(columns.tank, TANK_COLUMNS.to_vec());
        assert_eq!(columns.heal, MeterColumns::default().heal, "a tab left out keeps its default");
        assert_eq!(MeterColumns::parse(&columns.saved()), Some(columns));
    }

    #[test]
    fn values_that_are_not_valid_are_refused() {
        let store = Store::in_memory();
        for (key, raw) in [
            (Key::Language, "xx"),
            (Key::TimeFormat, "24"),
            (Key::TimeFormat, "system"),
            (Key::DefaultMode, "dps"),
            (Key::PlayerLimit, "0"),
            (Key::PlayerLimit, "-3"),
            (Key::PinYou, "yes"),
            (Key::WindowOpacity, "dark"),
            (Key::ReloadHotkey, "R"),
            (Key::LockHotkey, "Ctrl+Alt+"),
            (Key::Accent, "teal"),
            (Key::HealthText, "middle"),
            (Key::LowShare, "bar"),
            (Key::GroupDamage, "header"),
            (Key::TrayEdge, "left"),
            (Key::SlimMode, "1"),
            // Only the four choices: no size between them, none past them.
            (Key::MeterSize, "120"),
            (Key::MeterSize, "200"),
            (Key::MeterSize, "1.5"),
            (Key::MeterSize, "150%"),
            (Key::MeterSize, ""),
            (Key::ShowPing, "on"),
            (Key::BarStrength, "strong"),
            (Key::TargetNameSize, "big"),
            (Key::TargetNameSize, "NaN"),
            (Key::EncounterTimeout, "soon"),
            (Key::EncounterTimeout, ""),
            (Key::AllTargetsWindow, "45000"),
            (Key::AllTargetsWindow, "-1"),
            (Key::TrainingMode, "every"),
            (Key::DetailsFontSize, "inf"),
            (Key::DetailsIconSize, "large"),
            // A column of another tab, an unknown tab, not JSON.
            (Key::MeterColumns, r#"{"heal":["dtps"]}"#),
            (Key::MeterColumns, r#"{"party":["total"]}"#),
            (Key::MeterColumns, "total,encDps"),
            (Key::HiddenColumns, "crit"),
            (Key::SeenColumns, r#"{"crit":true}"#),
        ] {
            assert_eq!(store.set(key, raw), None, "{key:?} {raw}");
        }
        assert_eq!(store.values(), Values::default());
        assert_eq!(store.set_skill_columns(&["dmg".into(), "dps".into()]), None, "not a Details column");
        assert_eq!(store.values(), Values::default());
        // Opacity is kept within 0-100 and rounded, as 1.x does.
        assert_eq!(store.set(Key::WindowOpacity, "140").unwrap().window_opacity, 100);
        assert_eq!(store.set(Key::WindowOpacity, "33.6").unwrap().window_opacity, 34);
    }

    #[test]
    fn the_defaults_are_1xs_unless_2_0_decided_otherwise() {
        let v = Values::default();
        // 1.x's.
        assert!(v.bold_names && v.bold_dps && v.show_ping && v.show_total && v.round_dps && v.auto_hide);
        assert!(!v.slim_mode && !v.start_in_tray && !v.hide_from_taskbar);
        assert!(!v.show_suspend_button && !v.show_lock_button && !v.debug_logging && !v.packet_logging);
        assert_eq!(v.encounter_timeout, 15);
        assert_eq!(v.all_targets_window_ms, 0);
        assert_eq!(v.training_mode, "all");
        // 2.0's: the accent, the bars, the target
        // name, Details as a normal window on the type scale.
        assert_eq!((v.accent.as_str(), v.bar_strength, v.split_view), ("sky", 40, false));
        // Centred, not 1.x's default Beta layout: the health left is read
        // without the eye moving.
        assert_eq!(v.health_text, "centred");
        // 1.x always recoloured the bar: Row.
        assert_eq!(v.low_share, "row");
        // 1.x's Beta layout, its default: the footer.
        assert_eq!(v.group_damage, "footer");
        // Auto: on top, or at the bottom when the open list has no room above.
        assert_eq!(v.tray_edge, "auto");
        assert_eq!(v.target_name_size, 13.0);
        // The meter at the size it was drawn in.
        assert_eq!(v.meter_size, 100);
        assert_eq!(v.saved(Key::MeterSize).as_deref(), Some("100"));
        assert_eq!((v.details_font_size, v.details_icon_size), (13, 20));
        assert_eq!(v.skill_columns(), ["hit", "dmg", "dmgpct", "crit", "avgdmg"]);
        assert_eq!(v.meter_columns.dps, ["total", "encDps", "pct"]);
        assert_eq!(v.meter_columns.heal, ["healed", "hps", "taken"]);
        assert_eq!(v.meter_columns.tank, ["taken", "dtps", "pct"]);
    }

    #[test]
    fn numbers_out_of_range_go_to_the_nearest_end() {
        let store = Store::in_memory();
        let check = |key: Key, raw: &str, saved: &str| {
            let values = store.set(key, raw).unwrap_or_else(|| panic!("{key:?} {raw} refused"));
            assert_eq!(values.saved(key).as_deref(), Some(saved), "{key:?} {raw}");
        };
        check(Key::EncounterTimeout, "4", "5");
        check(Key::EncounterTimeout, "301", "300");
        check(Key::EncounterTimeout, "29.6", "30");
        check(Key::BarStrength, "5", "10");
        check(Key::BarStrength, "60", "50");
        check(Key::TargetNameSize, "7", "8");
        check(Key::TargetNameSize, "14.3", "14");
        check(Key::TargetNameSize, "14.5", "15");
        check(Key::TargetNameSize, "21", "20");
        check(Key::DetailsFontSize, "10", "11");
        check(Key::DetailsFontSize, "25", "20");
        check(Key::DetailsIconSize, "10", "20");
        check(Key::DetailsIconSize, "60", "56");
    }

    #[test]
    fn meter_columns_keep_the_lists_order_and_a_tab_left_out_its_default() {
        let mut values = Values::default();
        assert!(values.take(Key::MeterColumns, r#"{"dps":["pct","last60","total"]}"#));
        assert_eq!(values.meter_columns.dps, ["total", "pct", "last60"]);
        assert_eq!(values.meter_columns.heal, MeterColumns::default().heal);
        assert_eq!(
            values.saved(Key::MeterColumns).as_deref(),
            Some(r#"{"dps":["total","pct","last60"],"heal":["healed","hps","taken"],"tank":["taken","dtps","pct"]}"#)
        );
        // No column at all is the player's choice too.
        assert!(values.take(Key::MeterColumns, r#"{"tank":[]}"#));
        assert!(values.meter_columns.tank.is_empty());
        // Eff was dropped (no overheal for most players): the rest stays.
        assert!(values.take(Key::MeterColumns, r#"{"heal":["healed","eff","pct","taken"],"tank":["dtps"]}"#));
        assert_eq!((values.meter_columns.heal.as_slice(), values.meter_columns.tank.as_slice()), (&["healed", "pct", "taken"][..], &["dtps"][..]));
    }

    #[test]
    fn skill_columns_follow_1xs_hidden_and_seen_lists() {
        let mut values = Values::default();
        // A 1.x choice: everything but the hidden ones and the later
        // columns not switched yet. 1.x never hides FRNT.
        assert!(values.take(Key::HiddenColumns, r#"["mhit","mdmg"]"#));
        assert_eq!(
            values.skill_columns(),
            ["hit", "dmg", "dmgpct", "crit", "perfect", "double", "back", "frontal", "mindmg", "avgdmg", "maxdmg"]
        );
        assert!(values.take(Key::SeenColumns, r#"["miss","mhit"]"#));
        assert!(values.skill_columns().contains(&"miss"), "a later column switched on in 1.x");
        assert!(!values.skill_columns().contains(&"mhit"), "seen and hidden stays hidden");
    }

    #[test]
    fn switching_a_skill_column_saves_it_as_1x_does() {
        let folder = Folder::new();
        let store = Store::open(&folder.0, None);
        let mut shown: Vec<String> = Values::default().skill_columns().iter().map(|c| c.to_string()).collect();
        shown.retain(|c| c != "crit");
        shown.push("parry".into());
        let values = store.set_skill_columns(&shown).unwrap();
        assert_eq!(values.skill_columns(), ["hit", "dmg", "dmgpct", "parry", "avgdmg"]);
        store.flush().unwrap();
        let file = folder.read();
        let hidden: Vec<String> = serde_json::from_str(&file["dpsMeter.detailsHiddenColumns"]).unwrap();
        assert_eq!(hidden.len(), SKILL_COLUMNS.len() - 5);
        assert!(hidden.contains(&"crit".to_string()) && !hidden.contains(&"parry".to_string()));
        assert_eq!(file["dpsMeter.detailsSeenColumns"], r#"["crit","parry"]"#, "the two switched");
        assert_eq!(Store::open(&folder.0, None).values().skill_columns(), values.skill_columns());
    }

    #[test]
    fn the_meter_takes_a_changed_setting_at_the_next_reading() {
        use daevalog::capture::ping_tracker::PingTracker;
        use daevalog::i18n::lookup::{NpcLookup, SkillLookup};
        use std::sync::Arc;

        let storage = Arc::new(DataStorage::new());
        let mut meter = DpsCalculator::new(
            storage.clone(),
            Arc::new(SkillLookup::new()),
            Arc::new(NpcLookup::new()),
            Arc::new(PingTracker::new()),
        );
        let store = Store::in_memory();
        let mut applied = MeterApplied::default();
        assert_eq!(
            applied.apply(&store.values(), &storage, &mut meter),
            [Key::EncounterTimeout, Key::AllTargetsWindow],
            "both at the start"
        );
        assert!(applied.apply(&store.values(), &storage, &mut meter).is_empty(), "nothing changed");
        store.set(Key::EncounterTimeout, "60").unwrap();
        assert_eq!(applied.apply(&store.values(), &storage, &mut meter), [Key::EncounterTimeout]);
        store.set(Key::AllTargetsWindow, "300000").unwrap();
        store.set(Key::RoundDps, "false").unwrap();
        assert_eq!(applied.apply(&store.values(), &storage, &mut meter), [Key::AllTargetsWindow]);
    }

    #[test]
    fn the_logs_start_and_stop_with_their_settings() {
        let folder = Folder::new();
        let store = Store::open(&folder.0, None);
        let logs = || -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(&folder.0)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n != FILE_NAME)
                .collect();
            names.sort();
            names
        };
        #[cfg(unix)]
        set_mode(&folder.0, 0o755);
        store.apply_logging();
        assert!(logs().is_empty(), "off by default");
        // A folder made open before is tightened.
        #[cfg(unix)]
        assert_eq!(mode(&folder.0), 0o700);
        store.set(Key::DebugLogging, "true").unwrap();
        store.set(Key::PacketLogging, "true").unwrap();
        store.apply_logging();
        let open = logs();
        assert!(open.iter().any(|n| n == "debug.log"), "{open:?}");
        assert!(open.iter().any(|n| n.starts_with("packets_")), "{open:?}");
        store.set(Key::DebugLogging, "false").unwrap();
        store.set(Key::PacketLogging, "false").unwrap();
        store.apply_logging();
        // Nothing is written in memory, as in a replay.
        let memory = Store::in_memory();
        memory.set(Key::DebugLogging, "true").unwrap();
        memory.apply_logging();

        // A first start: no data folder yet, the 1.x settings say both logs
        // on. The logs start at once, before the settings file is written.
        let data = folder.0.join("first-start");
        let legacy = folder.0.join("legacy");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join(FILE_NAME),
            r#"{"dpsMeter.saveRawPackets":"true","dpsMeter.debugLoggingEnabled":"true"}"#,
        )
        .unwrap();
        let first = Store::open(&data, Some(&legacy));
        first.apply_logging();
        #[cfg(unix)]
        assert_eq!(mode(&data), 0o700);
        let names: Vec<String> =
            std::fs::read_dir(&data).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(names.iter().any(|n| n == "debug.log"), "{names:?}");
        assert!(names.iter().any(|n| n.starts_with("packets_")), "{names:?}");
        first.set(Key::DebugLogging, "false").unwrap();
        first.set(Key::PacketLogging, "false").unwrap();
        first.apply_logging();
    }

    #[test]
    fn a_bad_saved_value_falls_back_to_the_default() {
        let folder = Folder::new();
        folder.write(&[
            ("dpsMeter.playerLimit", "lots"),
            ("dpsMeter.defaultMeterMode", "dps"),
            ("dpsMeter.hideOtherNames", "true"),
        ]);
        let values = Store::open(&folder.0, None).values();
        assert_eq!(values.player_limit, DEFAULT_PLAYER_LIMIT);
        assert_eq!(values.default_mode, "bossTargets");
        assert!(values.hide_other_names);
    }

    #[test]
    fn a_saved_target_time_range_is_ignored() {
        // The setting did nothing in 1.x or 2.0 and is gone.
        let folder = Folder::new();
        folder.write(&[("dpsMeter.targetSelectionWindowMs", "20000"), ("dpsMeter.playerLimit", "8")]);
        let values = Store::open(&folder.0, None).values();
        assert_eq!(values, Values { player_limit: 8, ..Values::default() });
    }

    #[test]
    fn a_saved_panel_background_is_ignored() {
        // Details' background opacity is gone from 2.0.
        let folder = Folder::new();
        folder.write(&[("dpsMeter.detailsBackgroundOpacity", "0.35"), ("dpsMeter.playerLimit", "8")]);
        let values = Store::open(&folder.0, None).values();
        assert_eq!(values, Values { player_limit: 8, ..Values::default() });
    }

    /// A 1.x settings.json, made up: every value a player could have set.
    fn legacy_sample() -> Folder {
        let legacy = Folder::new();
        legacy.write(&[
            ("dpsMeter.language", "de"),
            ("dpsMeter.timeFormat", "24h"),
            ("dpsMeter.defaultMeterMode", "lastHitByMe"),
            ("dpsMeter.playerLimit", "8"),
            ("dpsMeter.pinMeToTop", "true"),
            ("dpsMeter.hideOtherNames", "true"),
            ("dpsMeter.overlayLocked", "true"),
            ("dpsMeter.waylandLayer", "false"),
            ("dpsMeter.windowOpacity", "40"),
            ("dpsMeter.hotkey", "Ctrl+Shift+F5"),
            ("dpsMeter.toggleWindowHotkey", "not a key"),
            ("dpsMeter.lockHotkey", "Alt+K"),
            ("dpsMeter.showPing", "false"),
            ("dpsMeter.roundDps", "false"),
            ("dpsMeter.slimMode", "true"),
            ("dpsMeter.mainPlayerNamesBold", "true"),
            ("dpsMeter.bossNameSize", "14.5"),
            ("dpsMeter.encounterTimeoutSec", "30"),
            ("dpsMeter.trainSelectionMode", "highestDamage"),
            ("dpsMeter.allTargetsWindowMs", "60000"),
            ("dpsMeter.detailsHiddenColumns", "[\"regen\",\"parry\"]"),
            ("dpsMeter.detailsSeenColumns", "[\"regen\"]"),
            ("dpsMeter.showLockBtn", "yes"),
            ("dpsMeter.detailsFontSize", "huge"),
            ("dpsMeter.detailsSaveScreenshotToFolder", "true"),
            ("dpsMeter.detailsScreenshotFolder", SHOTS),
            // Kept for later screens.
            ("dpsMeter.userName", "Tester"),
            ("dpsMeter.autoUpload", "true"),
            // Not taken over.
            ("dpsMeter.targetSelectionWindowMs", "5000"),
            ("dpsMeter.detailsBackgroundOpacity", "0.8"),
            ("dpsMeter.meterFillOpacity", "80"),
            ("dpsMeter.theme", "frost"),
            ("dpsMeter.betaUi", "true"),
            ("dpsMeter.encColumns", "encDps,dps,pct,total"),
            ("dpsMeter.migration.opacityReset1", "done"),
            ("window.x", "3017"),
            ("window.details.w", "1180"),
            ("backend.screenshotFolder", "/tmp/shots"),
            ("someday.unknownKey", "1"),
        ]);
        legacy
    }

    #[test]
    fn the_first_start_takes_over_the_1x_settings() {
        let legacy = legacy_sample();
        let folder = Folder::new();
        let store = Store::open(&folder.0, Some(&legacy.0));
        let values = store.values();
        assert_eq!(
            values,
            Values {
                language: Some("de".into()),
                clock_24h: Some(true),
                default_mode: "lastHitByMe".into(),
                player_limit: 8,
                pin_you: true,
                hide_other_names: true,
                locked: true,
                layer_shell: false,
                // 1.x's default, so the 2.0 one.
                window_opacity: 72,
                reload_hotkey: "Ctrl+Shift+F5".into(),
                lock_hotkey: "Alt+K".into(),
                show_ping: false,
                round_dps: false,
                slim_mode: true,
                bold_names: true,
                target_name_size: 15.0,
                encounter_timeout: 30,
                training_mode: "highestDamage".into(),
                all_targets_window_ms: 60_000,
                hidden_columns: Some(vec!["regen".into(), "parry".into()]),
                seen_columns: vec!["regen".into()],
                screenshot_to_folder: true,
                screenshot_folder: SHOTS.into(),
                ..Values::default()
            }
        );
        store.flush().unwrap();
        let file = folder.read();
        for key in [
            "dpsMeter.userName",
            "dpsMeter.autoUpload",
            "dpsMeter.showPing",
            "dpsMeter.encounterTimeoutSec",
            "dpsMeter.detailsHiddenColumns",
            "dpsMeter.detailsSeenColumns",
        ] {
            assert_eq!(file.get(key), legacy.read().get(key), "{key}");
        }
        // 1.x's half pixels come over as whole pixels.
        assert_eq!(file.get("dpsMeter.bossNameSize").map(String::as_str), Some("15"));
        for key in file.keys() {
            assert!(
                Key::ALL.iter().any(|k| k.name() == key) || KEPT_FOR_LATER.contains(&key.as_str()),
                "{key} should not be taken over"
            );
        }
        assert!(!file.contains_key("dpsMeter.toggleWindowHotkey"), "a bad hotkey is not taken over");
        assert!(!file.contains_key("dpsMeter.showLockBtn"), "nor a bad switch");
        assert!(!file.contains_key("dpsMeter.detailsFontSize"), "nor a bad number");
        assert!(!file.contains_key("dpsMeter.windowOpacity"), "1.x's default opacity is not taken over");
        assert!(!file.contains_key("dpsMeter.targetSelectionWindowMs"), "a setting 2.0 dropped");
        assert!(!file.contains_key("dpsMeter.detailsBackgroundOpacity"), "nor Details' background, dropped too");
        assert_eq!(file.len(), 24);
    }

    #[test]
    fn a_1x_opacity_other_than_its_default_is_taken_over() {
        let default = Values::default().window_opacity;
        for (saved, wanted) in [("40", default), ("40.2", default), ("39.6", default), ("41", 41), ("85", 85), ("0", 0), ("100", 100)] {
            let legacy = Folder::new();
            legacy.write(&[("dpsMeter.windowOpacity", saved)]);
            let folder = Folder::new();
            let store = Store::open(&folder.0, Some(&legacy.0));
            assert_eq!(store.values().window_opacity, wanted, "1.x saved {saved}");
            store.flush().unwrap();
            let file = folder.read();
            let kept = (wanted != default).then(|| wanted.to_string());
            assert_eq!(file.get("dpsMeter.windowOpacity"), kept.as_ref(), "1.x saved {saved}");
        }
    }

    #[test]
    fn every_colour_theme_is_taken_and_no_other() {
        let mut values = Values::default();
        assert_eq!(values.color_theme, "daevalog");
        for theme in COLOR_THEMES {
            assert!(values.take(Key::ColorTheme, theme), "{theme}");
            assert_eq!(values.color_theme, theme);
        }
        assert!(!values.take(Key::ColorTheme, "dark"));
        assert_eq!(values.color_theme, "system");
    }

    #[test]
    fn a_row_limit_snaps_up_to_a_choice() {
        let mut values = Values::default();
        assert_eq!(values.player_limit, 10);
        for (raw, limit) in [("1", 5), ("4", 5), ("5", 5), ("6", 8), ("7", 8), ("8", 8), ("9", 10), ("10", 10), ("12", 12), ("30", 12), ("500", 12)] {
            assert!(values.take(Key::PlayerLimit, raw), "{raw}");
            assert_eq!(values.player_limit, limit, "{raw}");
        }
        let mut given = Values::default();
        Overrides { player_limit: Some(30), ..Overrides::default() }.apply(&mut given);
        assert_eq!(given.player_limit, 12);
    }

    #[test]
    fn the_1x_settings_are_taken_over_once() {
        let legacy = legacy_sample();
        let folder = Folder::new();
        let store = Store::open(&folder.0, Some(&legacy.0));
        store.set(Key::PlayerLimit, "5").unwrap();
        store.flush().unwrap();
        drop(store);
        // Changed in 1.x afterwards: the Qt meter keeps its own.
        legacy.write(&[("dpsMeter.playerLimit", "12"), ("dpsMeter.language", "ko")]);
        let values = Store::open(&folder.0, Some(&legacy.0)).values();
        assert_eq!((values.player_limit, values.language.as_deref()), (5, Some("de")));
    }

    #[test]
    fn reset_brings_back_the_defaults_and_takes_nothing_over_again() {
        let legacy = legacy_sample();
        let folder = Folder::new();
        let store = Store::open(&folder.0, Some(&legacy.0));
        store.default_layer_shell(true);
        store.apply(&Overrides { show_you: true, ..Overrides::default() });
        store.set(Key::PlayerLimit, "4").unwrap();
        store.set_raw("dpsMeter.userName", "Someone");
        assert_ne!(store.values().language, None, "1.x's language was taken over");

        let values = store.reset();
        let mut expected = Values { layer_shell: true, ..Values::default() };
        expected.show_you = true;
        assert_eq!(values, expected, "the defaults, the desktop's layer and the command line on top");
        assert_eq!(store.values(), expected);
        assert_eq!(store.raw("dpsMeter.userName"), None, "1.x's keys go too");
        store.flush().unwrap();
        drop(store);
        assert!(folder.read().is_empty());
        assert_eq!(Store::open(&folder.0, Some(&legacy.0)).values(), Values::default(), "not taken over again");
    }

    #[test]
    fn screenshots_go_to_the_chosen_folder_else_the_default() {
        let mut values = Values::default();
        assert_eq!(values.screenshot_folder(), dirs::picture_dir().map(|p| p.join("Daevalog DPS Meter")));
        assert!(values.take(Key::ScreenshotFolder, SHOTS));
        assert_eq!(values.screenshot_folder(), Some(PathBuf::from(SHOTS)));
        assert!(!values.take(Key::ScreenshotFolder, "Shots"), "only a whole path");
        assert!(values.take(Key::ScreenshotFolder, ""));
        assert_eq!(values.saved(Key::ScreenshotFolder), None, "the default: no key");
    }

    #[test]
    fn details_opens_on_a_screen_by_name_and_a_1x_number_is_off() {
        let mut values = Values::default();
        assert!(values.take(Key::DetailsMonitor, "DP-2"));
        assert_eq!(values.saved(Key::DetailsMonitor), Some("DP-2".into()));
        assert!(!values.take(Key::DetailsMonitor, "1"), "1.x's monitor number");
        assert_eq!(values.details_monitor, "DP-2", "a refused value changes nothing");
        assert!(values.take(Key::DetailsMonitor, "off"));
        assert!(values.take(Key::LayerPosition, "1200, 640"));
        assert_eq!(values.saved(Key::LayerPosition), Some("1200,640".into()));
        assert!(!values.take(Key::LayerPosition, "-5,10"), "never off the screen's top-left");
        assert!(!values.take(Key::LayerPosition, "left"));
        assert_eq!(values.saved(Key::DetailsMonitor), None, "off: no key");
    }

    #[test]
    fn without_a_1x_meter_the_first_start_has_the_defaults() {
        let missing = Folder::new();
        let folder = Folder::new();
        let store = Store::open(&folder.0, Some(&missing.0.join("gone")));
        assert_eq!(store.values(), Values::default());
        store.flush().unwrap();
        assert!(folder.0.join(FILE_NAME).exists(), "the file marks the first start as done");
    }

    #[test]
    fn command_line_settings_are_not_saved() {
        let folder = Folder::new();
        let store = Store::open(&folder.0, None);
        store.apply(&Overrides {
            language: Some("ja".into()),
            default_mode: Some("encounter".into()),
            player_limit: Some(3),
            pin_you: true,
            show_you: true,
            hide_other_names: true,
            locked: true,
            layer_shell: true,
            slim_mode: true,
            split_view: true,
            health_split: true,
            low_share_number: true,
            damage_line: true,
            color_theme: Some("nord".into()),
            dps_columns: Some(vec!["hits", "total"]),
            details_icon_size: Some(60),
            details_font_size: Some(5),
            meter_size: Some(150),
        });
        let values = store.values();
        assert_eq!(values.language.as_deref(), Some("ja"));
        assert_eq!(values.meter_size, 150);
        // Sizes past the sliders' ends stop at them.
        assert_eq!((values.details_icon_size, values.details_font_size), (56, 11));
        assert_eq!(values.color_theme, "nord");
        assert_eq!(values.meter_columns.dps, vec!["total", "hits"]);
        // A given limit snaps up to a choice too.
        assert_eq!(values.view(), ViewOptions { player_limit: 5, pin_you: true, ..ViewOptions::default() });
        assert!(values.show_you && values.hide_other_names && values.locked && values.layer_shell);
        assert!(values.slim_mode && values.split_view);
        assert_eq!((values.health_text.as_str(), values.low_share.as_str(), values.group_damage.as_str()), ("split", "number", "line"));
        // A change the player makes is saved, and only that one.
        store.set(Key::Locked, "false").unwrap();
        store.flush().unwrap();
        drop(store);
        assert_eq!(folder.read(), HashMap::from([("dpsMeter.overlayLocked".to_string(), "false".to_string())]));
        assert_eq!(Store::open(&folder.0, None).values(), Values::default());
    }
}
