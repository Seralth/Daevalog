// SPDX-License-Identifier: AGPL-3.0-only

//! `Settings`, the QML singleton for the meter's settings (settings_store.rs):
//! one property per setting, with a change signal. A change from QML is
//! checked and saved; a value that is not valid is refused, and the
//! property keeps its value. Every window binds to the same object, so a
//! change shows in each one at once.

use core::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;

use crate::settings_store::{self, Key, Values};

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
        #[qproperty(QString, language, READ, WRITE = change_language, NOTIFY)]
        #[qproperty(QString, time_format, READ, WRITE = change_time_format, NOTIFY)]
        #[qproperty(bool, hour24, READ, NOTIFY)]
        #[qproperty(QString, default_mode, READ, WRITE = change_default_mode, NOTIFY)]
        #[qproperty(i32, player_limit, READ, WRITE = change_player_limit, NOTIFY)]
        #[qproperty(bool, pin_you, READ, WRITE = change_pin_you, NOTIFY)]
        #[qproperty(bool, show_you, READ, WRITE = change_show_you, NOTIFY)]
        #[qproperty(bool, hide_other_names, READ, WRITE = change_hide_other_names, NOTIFY)]
        #[qproperty(bool, locked, READ, WRITE = change_locked, NOTIFY)]
        #[qproperty(bool, layer_shell, READ, WRITE = change_layer_shell, NOTIFY)]
        #[qproperty(i32, window_opacity, READ, WRITE = change_window_opacity, NOTIFY)]
        #[qproperty(QString, reload_hotkey, READ, WRITE = change_reload_hotkey, NOTIFY)]
        #[qproperty(QString, toggle_window_hotkey, READ, WRITE = change_toggle_window_hotkey, NOTIFY)]
        #[qproperty(QString, lock_hotkey, READ, WRITE = change_lock_hotkey, NOTIFY)]
        #[qproperty(QString, accent, READ, WRITE = change_accent, NOTIFY)]
        #[qproperty(QString, health_text, READ, WRITE = change_health_text, NOTIFY)]
        #[qproperty(QString, low_share, READ, WRITE = change_low_share, NOTIFY)]
        #[qproperty(QString, group_damage, READ, WRITE = change_group_damage, NOTIFY)]
        #[qproperty(QString, tray_edge, READ, WRITE = change_tray_edge, NOTIFY)]
        #[qproperty(bool, slim_mode, READ, WRITE = change_slim_mode, NOTIFY)]
        #[qproperty(i32, meter_size, READ, WRITE = change_meter_size, NOTIFY)]
        #[qproperty(bool, split_view, READ, WRITE = change_split_view, NOTIFY)]
        #[qproperty(i32, bar_strength, READ, WRITE = change_bar_strength, NOTIFY)]
        #[qproperty(f64, target_name_size, READ, WRITE = change_target_name_size, NOTIFY)]
        #[qproperty(bool, bold_names, READ, WRITE = change_bold_names, NOTIFY)]
        #[qproperty(bool, bold_dps, READ, WRITE = change_bold_dps, NOTIFY)]
        #[qproperty(bool, auto_hide, READ, WRITE = change_auto_hide, NOTIFY)]
        #[qproperty(bool, start_in_tray, READ, WRITE = change_start_in_tray, NOTIFY)]
        #[qproperty(bool, hide_from_taskbar, READ, WRITE = change_hide_from_taskbar, NOTIFY)]
        #[qproperty(bool, show_ping, READ, WRITE = change_show_ping, NOTIFY)]
        #[qproperty(bool, spawn_notices, READ, WRITE = change_spawn_notices, NOTIFY)]
        #[qproperty(bool, shard_warning, READ, WRITE = change_shard_warning, NOTIFY)]
        #[qproperty(QString, server_time_zone, READ, WRITE = change_server_time_zone, NOTIFY)]
        #[qproperty(bool, show_total, READ, WRITE = change_show_total, NOTIFY)]
        #[qproperty(bool, fit_rows, READ, WRITE = change_fit_rows, NOTIFY)]
        #[qproperty(bool, round_dps, READ, WRITE = change_round_dps, NOTIFY)]
        #[qproperty(bool, show_suspend_button, READ, WRITE = change_show_suspend_button, NOTIFY)]
        #[qproperty(bool, show_lock_button, READ, WRITE = change_show_lock_button, NOTIFY)]
        #[qproperty(QString, meter_columns, READ, WRITE = change_meter_columns, NOTIFY)]
        #[qproperty(i32, encounter_timeout, READ, WRITE = change_encounter_timeout, NOTIFY)]
        #[qproperty(bool, debug_logging, READ, WRITE = change_debug_logging, NOTIFY)]
        #[qproperty(bool, packet_logging, READ, WRITE = change_packet_logging, NOTIFY)]
        #[qproperty(QString, training_mode, READ, WRITE = change_training_mode, NOTIFY)]
        #[qproperty(i32, all_targets_window, READ, WRITE = change_all_targets_window, NOTIFY)]
        #[qproperty(i32, details_font_size, READ, WRITE = change_details_font_size, NOTIFY)]
        #[qproperty(i32, details_icon_size, READ, WRITE = change_details_icon_size, NOTIFY)]
        #[qproperty(QString, skill_columns, READ, WRITE = change_skill_columns, NOTIFY)]
        #[qproperty(bool, screenshot_with_meter, READ, WRITE = change_screenshot_with_meter, NOTIFY)]
        #[qproperty(bool, screenshot_to_folder, READ, WRITE = change_screenshot_to_folder, NOTIFY)]
        #[qproperty(QString, screenshot_folder, READ, WRITE = change_screenshot_folder, NOTIFY)]
        #[qproperty(QString, screenshot_place, READ, NOTIFY)]
        #[qproperty(QString, color_theme, READ, WRITE = change_color_theme, NOTIFY)]
        #[qproperty(QString, details_monitor, READ, WRITE = change_details_monitor, NOTIFY)]
        type Settings = super::SettingsRust;

        fn change_language(self: Pin<&mut Settings>, value: QString);
        fn change_time_format(self: Pin<&mut Settings>, value: QString);
        fn change_default_mode(self: Pin<&mut Settings>, value: QString);
        fn change_player_limit(self: Pin<&mut Settings>, value: i32);
        fn change_pin_you(self: Pin<&mut Settings>, value: bool);
        fn change_show_you(self: Pin<&mut Settings>, value: bool);
        fn change_hide_other_names(self: Pin<&mut Settings>, value: bool);
        fn change_locked(self: Pin<&mut Settings>, value: bool);
        fn change_layer_shell(self: Pin<&mut Settings>, value: bool);
        fn change_window_opacity(self: Pin<&mut Settings>, value: i32);
        fn change_reload_hotkey(self: Pin<&mut Settings>, value: QString);
        fn change_toggle_window_hotkey(self: Pin<&mut Settings>, value: QString);
        fn change_lock_hotkey(self: Pin<&mut Settings>, value: QString);
        fn change_accent(self: Pin<&mut Settings>, value: QString);
        fn change_health_text(self: Pin<&mut Settings>, value: QString);
        fn change_low_share(self: Pin<&mut Settings>, value: QString);
        fn change_group_damage(self: Pin<&mut Settings>, value: QString);
        fn change_tray_edge(self: Pin<&mut Settings>, value: QString);
        fn change_slim_mode(self: Pin<&mut Settings>, value: bool);
        fn change_meter_size(self: Pin<&mut Settings>, value: i32);
        fn change_split_view(self: Pin<&mut Settings>, value: bool);
        fn change_bar_strength(self: Pin<&mut Settings>, value: i32);
        fn change_target_name_size(self: Pin<&mut Settings>, value: f64);
        fn change_bold_names(self: Pin<&mut Settings>, value: bool);
        fn change_bold_dps(self: Pin<&mut Settings>, value: bool);
        fn change_auto_hide(self: Pin<&mut Settings>, value: bool);
        fn change_start_in_tray(self: Pin<&mut Settings>, value: bool);
        fn change_hide_from_taskbar(self: Pin<&mut Settings>, value: bool);
        fn change_show_ping(self: Pin<&mut Settings>, value: bool);
        fn change_spawn_notices(self: Pin<&mut Settings>, value: bool);
        fn change_shard_warning(self: Pin<&mut Settings>, value: bool);
        fn change_server_time_zone(self: Pin<&mut Settings>, value: QString);

        /// Whether `zone` is a time zone name the clock data knows.
        #[qinvokable]
        fn zone_valid(self: &Settings, zone: &QString) -> bool;

        /// The server regions in `language`, as JSON [{"label", "value"}]
        /// with each region's time zone as its value.
        #[qinvokable]
        fn zone_regions(self: &Settings, language: &QString) -> QString;
        fn change_show_total(self: Pin<&mut Settings>, value: bool);
        fn change_fit_rows(self: Pin<&mut Settings>, value: bool);
        fn change_round_dps(self: Pin<&mut Settings>, value: bool);
        fn change_show_suspend_button(self: Pin<&mut Settings>, value: bool);
        fn change_show_lock_button(self: Pin<&mut Settings>, value: bool);
        fn change_meter_columns(self: Pin<&mut Settings>, value: QString);
        fn change_encounter_timeout(self: Pin<&mut Settings>, value: i32);
        fn change_debug_logging(self: Pin<&mut Settings>, value: bool);
        fn change_packet_logging(self: Pin<&mut Settings>, value: bool);
        fn change_training_mode(self: Pin<&mut Settings>, value: QString);
        fn change_all_targets_window(self: Pin<&mut Settings>, value: i32);
        fn change_details_font_size(self: Pin<&mut Settings>, value: i32);
        fn change_details_icon_size(self: Pin<&mut Settings>, value: i32);
        fn change_skill_columns(self: Pin<&mut Settings>, value: QString);
        fn change_screenshot_with_meter(self: Pin<&mut Settings>, value: bool);
        fn change_screenshot_to_folder(self: Pin<&mut Settings>, value: bool);
        fn change_screenshot_folder(self: Pin<&mut Settings>, value: QString);
        fn change_color_theme(self: Pin<&mut Settings>, value: QString);
        fn change_details_monitor(self: Pin<&mut Settings>, value: QString);

        /// Every setting back to its default (Advanced > Reset all
        /// settings), shown at once in every window.
        #[qinvokable]
        fn reset_all(self: Pin<&mut Settings>);
    }
}

/// The values as `settings_store::Values` has them, in Qt's types.
pub struct SettingsRust {
    /// One of the ten language codes; empty follows the system.
    language: QString,
    /// Display Time: "12h", "24h", or empty to follow the system's clock.
    time_format: QString,
    /// Times of day on a 24-hour clock: the setting, else the system's
    /// clock, else 12-hour.
    hour24: bool,
    /// The backend's id of the mode the meter starts in ("bossTargets").
    default_mode: QString,
    player_limit: i32,
    pin_you: bool,
    show_you: bool,
    hide_other_names: bool,
    locked: bool,
    layer_shell: bool,
    /// The panel's background, 0-100.
    window_opacity: i32,
    reload_hotkey: QString,
    toggle_window_hotkey: QString,
    lock_hotkey: QString,
    /// "sky", "rose", "lemon" or "silver".
    accent: QString,
    health_text: QString,
    low_share: QString,
    group_damage: QString,
    tray_edge: QString,
    slim_mode: bool,
    /// The meter window's size in percent: 100, 115, 130 or 150.
    meter_size: i32,
    split_view: bool,
    /// 10-50, in percent.
    bar_strength: i32,
    /// 8-20 px, in half pixels.
    target_name_size: f64,
    bold_names: bool,
    bold_dps: bool,
    auto_hide: bool,
    start_in_tray: bool,
    hide_from_taskbar: bool,
    show_ping: bool,
    spawn_notices: bool,
    shard_warning: bool,
    server_time_zone: QString,
    show_total: bool,
    fit_rows: bool,
    round_dps: bool,
    show_suspend_button: bool,
    show_lock_button: bool,
    /// Each tab's columns, as JSON: {"dps": ["total", ...], "heal": ..., "tank": ...}.
    meter_columns: QString,
    /// Seconds, 5-300.
    encounter_timeout: i32,
    debug_logging: bool,
    packet_logging: bool,
    /// In ms.
    /// "all" or "highestDamage".
    training_mode: QString,
    /// In ms; 0 is since the zone change.
    all_targets_window: i32,
    details_font_size: i32,
    details_icon_size: i32,
    /// The Details skill columns shown, as a JSON list, in the window's order.
    skill_columns: QString,
    screenshot_with_meter: bool,
    screenshot_to_folder: bool,
    /// The folder chosen; empty is the default one.
    screenshot_folder: QString,
    /// Where a screenshot's file goes: the chosen folder, else the default;
    /// empty with neither.
    screenshot_place: QString,
    /// "daevalog", "nord" or "system".
    color_theme: QString,
    /// The screen Details stays open on (X11 only); empty is off.
    details_monitor: QString,
}

impl SettingsRust {
    fn new(v: &Values) -> Self {
        Self {
            language: QString::from(v.language.as_deref().unwrap_or_default()),
            time_format: QString::from(v.saved(Key::TimeFormat).as_deref().unwrap_or_default()),
            hour24: v.uses_24h(settings_store::system_clock_24h()),
            default_mode: QString::from(&v.default_mode),
            player_limit: i32::try_from(v.player_limit).unwrap_or(i32::MAX),
            pin_you: v.pin_you,
            show_you: v.show_you,
            hide_other_names: v.hide_other_names,
            locked: v.locked,
            layer_shell: v.layer_shell,
            window_opacity: i32::from(v.window_opacity),
            reload_hotkey: QString::from(&v.reload_hotkey),
            toggle_window_hotkey: QString::from(&v.toggle_window_hotkey),
            lock_hotkey: QString::from(&v.lock_hotkey),
            accent: QString::from(&v.accent),
            health_text: QString::from(&v.health_text),
            low_share: QString::from(&v.low_share),
            group_damage: QString::from(&v.group_damage),
            tray_edge: QString::from(&v.tray_edge),
            slim_mode: v.slim_mode,
            meter_size: i32::from(v.meter_size),
            split_view: v.split_view,
            bar_strength: i32::from(v.bar_strength),
            target_name_size: v.target_name_size,
            bold_names: v.bold_names,
            bold_dps: v.bold_dps,
            auto_hide: v.auto_hide,
            start_in_tray: v.start_in_tray,
            hide_from_taskbar: v.hide_from_taskbar,
            show_ping: v.show_ping,
            spawn_notices: v.spawn_notices,
            shard_warning: v.shard_warning,
            server_time_zone: QString::from(&v.server_time_zone),
            show_total: v.show_total,
            fit_rows: v.fit_rows,
            round_dps: v.round_dps,
            show_suspend_button: v.show_suspend_button,
            show_lock_button: v.show_lock_button,
            meter_columns: QString::from(v.saved(Key::MeterColumns).as_deref().unwrap_or_default()),
            encounter_timeout: i32::from(v.encounter_timeout),
            debug_logging: v.debug_logging,
            packet_logging: v.packet_logging,
            training_mode: QString::from(&v.training_mode),
            all_targets_window: i32::try_from(v.all_targets_window_ms).unwrap_or(i32::MAX),
            details_font_size: i32::from(v.details_font_size),
            details_icon_size: i32::from(v.details_icon_size),
            skill_columns: QString::from(&serde_json::json!(v.skill_columns()).to_string()),
            screenshot_with_meter: v.screenshot_with_meter,
            screenshot_to_folder: v.screenshot_to_folder,
            screenshot_folder: QString::from(&v.screenshot_folder),
            screenshot_place: QString::from(&v.screenshot_folder().map(|p| p.display().to_string()).unwrap_or_default()),
            color_theme: QString::from(&v.color_theme),
            details_monitor: QString::from(&v.details_monitor),
        }
    }
}

impl Default for SettingsRust {
    fn default() -> Self {
        Self::new(&settings_store::store().values())
    }
}

/// Sets a property to `$value` and signals it, if it changed.
macro_rules! show {
    ($this:ident, $field:ident, $signal:ident, $value:expr) => {{
        let value = $value;
        if $this.$field != value {
            $this.as_mut().rust_mut().$field = value;
            $this.as_mut().$signal();
        }
    }};
}

impl qobject::Settings {
    fn change_language(self: Pin<&mut Self>, value: QString) {
        self.change(Key::Language, value.to_string());
    }

    fn change_time_format(self: Pin<&mut Self>, value: QString) {
        self.change(Key::TimeFormat, value.to_string());
    }

    fn change_default_mode(self: Pin<&mut Self>, value: QString) {
        self.change(Key::DefaultMode, value.to_string());
    }

    fn change_player_limit(self: Pin<&mut Self>, value: i32) {
        self.change(Key::PlayerLimit, value.to_string());
    }

    fn change_pin_you(self: Pin<&mut Self>, value: bool) {
        self.change(Key::PinYou, value.to_string());
    }

    fn change_show_you(self: Pin<&mut Self>, value: bool) {
        self.change(Key::ShowYou, value.to_string());
    }

    fn change_hide_other_names(self: Pin<&mut Self>, value: bool) {
        self.change(Key::HideOtherNames, value.to_string());
    }

    fn change_locked(self: Pin<&mut Self>, value: bool) {
        self.change(Key::Locked, value.to_string());
    }

    fn change_layer_shell(self: Pin<&mut Self>, value: bool) {
        self.change(Key::LayerShell, value.to_string());
    }

    fn change_window_opacity(self: Pin<&mut Self>, value: i32) {
        self.change(Key::WindowOpacity, value.to_string());
    }

    fn change_reload_hotkey(self: Pin<&mut Self>, value: QString) {
        self.change(Key::ReloadHotkey, value.to_string());
    }

    fn change_toggle_window_hotkey(self: Pin<&mut Self>, value: QString) {
        self.change(Key::ToggleWindowHotkey, value.to_string());
    }

    fn change_lock_hotkey(self: Pin<&mut Self>, value: QString) {
        self.change(Key::LockHotkey, value.to_string());
    }

    fn change_accent(self: Pin<&mut Self>, value: QString) {
        self.change(Key::Accent, value.to_string());
    }

    fn change_health_text(self: Pin<&mut Self>, value: QString) {
        self.change(Key::HealthText, value.to_string());
    }

    fn change_low_share(self: Pin<&mut Self>, value: QString) {
        self.change(Key::LowShare, value.to_string());
    }

    fn change_group_damage(self: Pin<&mut Self>, value: QString) {
        self.change(Key::GroupDamage, value.to_string());
    }

    fn change_tray_edge(self: Pin<&mut Self>, value: QString) {
        self.change(Key::TrayEdge, value.to_string());
    }

    fn change_screenshot_with_meter(self: Pin<&mut Self>, value: bool) {
        self.change(Key::ScreenshotWithMeter, value.to_string());
    }

    fn change_screenshot_to_folder(self: Pin<&mut Self>, value: bool) {
        self.change(Key::ScreenshotToFolder, value.to_string());
    }

    fn change_screenshot_folder(self: Pin<&mut Self>, value: QString) {
        self.change(Key::ScreenshotFolder, value.to_string());
    }

    fn change_color_theme(self: Pin<&mut Self>, value: QString) {
        self.change(Key::ColorTheme, value.to_string());
    }

    fn change_details_monitor(self: Pin<&mut Self>, value: QString) {
        self.change(Key::DetailsMonitor, value.to_string());
    }

    fn change_slim_mode(self: Pin<&mut Self>, value: bool) {
        self.change(Key::SlimMode, value.to_string());
    }

    fn change_meter_size(self: Pin<&mut Self>, value: i32) {
        self.change(Key::MeterSize, value.to_string());
    }

    fn change_split_view(self: Pin<&mut Self>, value: bool) {
        self.change(Key::SplitView, value.to_string());
    }

    fn change_bar_strength(self: Pin<&mut Self>, value: i32) {
        self.change(Key::BarStrength, value.to_string());
    }

    fn change_target_name_size(self: Pin<&mut Self>, value: f64) {
        self.change(Key::TargetNameSize, value.to_string());
    }

    fn change_bold_names(self: Pin<&mut Self>, value: bool) {
        self.change(Key::BoldNames, value.to_string());
    }

    fn change_bold_dps(self: Pin<&mut Self>, value: bool) {
        self.change(Key::BoldDps, value.to_string());
    }

    fn change_auto_hide(self: Pin<&mut Self>, value: bool) {
        self.change(Key::AutoHide, value.to_string());
    }

    fn change_start_in_tray(self: Pin<&mut Self>, value: bool) {
        self.change(Key::StartInTray, value.to_string());
    }

    fn change_hide_from_taskbar(self: Pin<&mut Self>, value: bool) {
        self.change(Key::HideFromTaskbar, value.to_string());
    }

    fn change_show_ping(self: Pin<&mut Self>, value: bool) {
        self.change(Key::ShowPing, value.to_string());
    }

    fn change_spawn_notices(self: Pin<&mut Self>, value: bool) {
        self.change(Key::SpawnNotices, value.to_string());
    }

    fn change_shard_warning(self: Pin<&mut Self>, value: bool) {
        self.change(Key::ShardWarning, value.to_string());
    }

    fn change_server_time_zone(self: Pin<&mut Self>, value: QString) {
        self.change(Key::ServerTimeZone, value.to_string());
    }

    fn zone_valid(&self, zone: &QString) -> bool {
        crate::world_spawns::valid_zone(&zone.to_string())
    }

    fn zone_regions(&self, language: &QString) -> QString {
        QString::from(&crate::world_spawns::regions_json(&language.to_string()))
    }

    fn change_fit_rows(self: Pin<&mut Self>, value: bool) {
        self.change(Key::FitRows, value.to_string());
    }

    fn change_show_total(self: Pin<&mut Self>, value: bool) {
        self.change(Key::ShowTotal, value.to_string());
    }

    fn change_round_dps(self: Pin<&mut Self>, value: bool) {
        self.change(Key::RoundDps, value.to_string());
    }

    fn change_show_suspend_button(self: Pin<&mut Self>, value: bool) {
        self.change(Key::ShowSuspendButton, value.to_string());
    }

    fn change_show_lock_button(self: Pin<&mut Self>, value: bool) {
        self.change(Key::ShowLockButton, value.to_string());
    }

    fn change_meter_columns(self: Pin<&mut Self>, value: QString) {
        self.change(Key::MeterColumns, value.to_string());
    }

    fn change_encounter_timeout(self: Pin<&mut Self>, value: i32) {
        self.change(Key::EncounterTimeout, value.to_string());
    }

    fn change_debug_logging(self: Pin<&mut Self>, value: bool) {
        self.change(Key::DebugLogging, value.to_string());
    }

    fn change_packet_logging(self: Pin<&mut Self>, value: bool) {
        self.change(Key::PacketLogging, value.to_string());
    }

    fn change_training_mode(self: Pin<&mut Self>, value: QString) {
        self.change(Key::TrainingMode, value.to_string());
    }

    fn change_all_targets_window(self: Pin<&mut Self>, value: i32) {
        self.change(Key::AllTargetsWindow, value.to_string());
    }

    fn change_details_font_size(self: Pin<&mut Self>, value: i32) {
        self.change(Key::DetailsFontSize, value.to_string());
    }

    fn change_details_icon_size(self: Pin<&mut Self>, value: i32) {
        self.change(Key::DetailsIconSize, value.to_string());
    }

    /// A JSON list of the columns to show.
    fn change_skill_columns(mut self: Pin<&mut Self>, value: QString) {
        let raw = value.to_string();
        let shown = serde_json::from_str::<Vec<String>>(&raw).ok();
        match shown.and_then(|shown| settings_store::store().set_skill_columns(&shown)) {
            Some(values) => self.show(&values),
            None => {
                eprintln!("daevalog-dps-meter: {raw:?} is not a valid list of Details columns");
                self.as_mut().skill_columns_changed();
            }
        }
    }

    fn reset_all(self: Pin<&mut Self>) {
        let store = settings_store::store();
        let values = store.reset();
        store.apply_logging();
        self.show(&values);
    }

    fn change(mut self: Pin<&mut Self>, key: Key, raw: String) {
        let store = settings_store::store();
        match store.set(key, &raw) {
            Some(values) => {
                if matches!(key, Key::DebugLogging | Key::PacketLogging) {
                    store.apply_logging();
                }
                self.show(&values);
            }
            None => {
                eprintln!("daevalog-dps-meter: {raw:?} is not a valid {}", key.name());
                // A control that moved on its own reads the kept value again.
                self.as_mut().signal(key);
            }
        }
    }

    fn signal(self: Pin<&mut Self>, key: Key) {
        match key {
            Key::Language => self.language_changed(),
            Key::TimeFormat => self.time_format_changed(),
            Key::DefaultMode => self.default_mode_changed(),
            Key::PlayerLimit => self.player_limit_changed(),
            Key::PinYou => self.pin_you_changed(),
            Key::ShowYou => self.show_you_changed(),
            Key::HideOtherNames => self.hide_other_names_changed(),
            Key::Locked => self.locked_changed(),
            Key::LayerShell => self.layer_shell_changed(),
            Key::WindowOpacity => self.window_opacity_changed(),
            Key::ReloadHotkey => self.reload_hotkey_changed(),
            Key::ToggleWindowHotkey => self.toggle_window_hotkey_changed(),
            Key::LockHotkey => self.lock_hotkey_changed(),
            Key::Accent => self.accent_changed(),
            Key::HealthText => self.health_text_changed(),
            Key::LowShare => self.low_share_changed(),
            Key::GroupDamage => self.group_damage_changed(),
            Key::TrayEdge => self.tray_edge_changed(),
            Key::SlimMode => self.slim_mode_changed(),
            Key::MeterSize => self.meter_size_changed(),
            Key::SplitView => self.split_view_changed(),
            Key::BarStrength => self.bar_strength_changed(),
            Key::TargetNameSize => self.target_name_size_changed(),
            Key::BoldNames => self.bold_names_changed(),
            Key::BoldDps => self.bold_dps_changed(),
            Key::AutoHide => self.auto_hide_changed(),
            Key::StartInTray => self.start_in_tray_changed(),
            Key::HideFromTaskbar => self.hide_from_taskbar_changed(),
            Key::ShowPing => self.show_ping_changed(),
            Key::SpawnNotices => self.spawn_notices_changed(),
            Key::ShardWarning => self.shard_warning_changed(),
            Key::ServerTimeZone => self.server_time_zone_changed(),
            Key::ShowTotal => self.show_total_changed(),
            Key::FitRows => self.fit_rows_changed(),
            Key::RoundDps => self.round_dps_changed(),
            Key::ShowSuspendButton => self.show_suspend_button_changed(),
            Key::ShowLockButton => self.show_lock_button_changed(),
            Key::MeterColumns => self.meter_columns_changed(),
            Key::EncounterTimeout => self.encounter_timeout_changed(),
            Key::DebugLogging => self.debug_logging_changed(),
            Key::PacketLogging => self.packet_logging_changed(),
            Key::TrainingMode => self.training_mode_changed(),
            Key::AllTargetsWindow => self.all_targets_window_changed(),
            Key::DetailsFontSize => self.details_font_size_changed(),
            Key::DetailsIconSize => self.details_icon_size_changed(),
            Key::HiddenColumns | Key::SeenColumns => self.skill_columns_changed(),
            Key::ScreenshotWithMeter => self.screenshot_with_meter_changed(),
            Key::ScreenshotToFolder => self.screenshot_to_folder_changed(),
            Key::ScreenshotFolder => self.screenshot_folder_changed(),
            Key::ColorTheme => self.color_theme_changed(),
            Key::DetailsMonitor => self.details_monitor_changed(),
            // Kept by the Overlay, not shown in Settings.
            Key::LayerPosition => {}
        }
    }

    fn show(mut self: Pin<&mut Self>, values: &Values) {
        let v = SettingsRust::new(values);
        show!(self, language, language_changed, v.language);
        show!(self, time_format, time_format_changed, v.time_format);
        show!(self, hour24, hour24_changed, v.hour24);
        show!(self, default_mode, default_mode_changed, v.default_mode);
        show!(self, player_limit, player_limit_changed, v.player_limit);
        show!(self, pin_you, pin_you_changed, v.pin_you);
        show!(self, show_you, show_you_changed, v.show_you);
        show!(self, hide_other_names, hide_other_names_changed, v.hide_other_names);
        show!(self, locked, locked_changed, v.locked);
        show!(self, layer_shell, layer_shell_changed, v.layer_shell);
        show!(self, window_opacity, window_opacity_changed, v.window_opacity);
        show!(self, reload_hotkey, reload_hotkey_changed, v.reload_hotkey);
        show!(self, toggle_window_hotkey, toggle_window_hotkey_changed, v.toggle_window_hotkey);
        show!(self, lock_hotkey, lock_hotkey_changed, v.lock_hotkey);
        show!(self, accent, accent_changed, v.accent);
        show!(self, health_text, health_text_changed, v.health_text);
        show!(self, low_share, low_share_changed, v.low_share);
        show!(self, group_damage, group_damage_changed, v.group_damage);
        show!(self, tray_edge, tray_edge_changed, v.tray_edge);
        show!(self, slim_mode, slim_mode_changed, v.slim_mode);
        show!(self, meter_size, meter_size_changed, v.meter_size);
        show!(self, split_view, split_view_changed, v.split_view);
        show!(self, bar_strength, bar_strength_changed, v.bar_strength);
        show!(self, target_name_size, target_name_size_changed, v.target_name_size);
        show!(self, bold_names, bold_names_changed, v.bold_names);
        show!(self, bold_dps, bold_dps_changed, v.bold_dps);
        show!(self, auto_hide, auto_hide_changed, v.auto_hide);
        show!(self, start_in_tray, start_in_tray_changed, v.start_in_tray);
        show!(self, hide_from_taskbar, hide_from_taskbar_changed, v.hide_from_taskbar);
        show!(self, show_ping, show_ping_changed, v.show_ping);
        show!(self, spawn_notices, spawn_notices_changed, v.spawn_notices);
        show!(self, shard_warning, shard_warning_changed, v.shard_warning);
        show!(self, server_time_zone, server_time_zone_changed, v.server_time_zone);
        show!(self, show_total, show_total_changed, v.show_total);
        show!(self, fit_rows, fit_rows_changed, v.fit_rows);
        show!(self, round_dps, round_dps_changed, v.round_dps);
        show!(self, show_suspend_button, show_suspend_button_changed, v.show_suspend_button);
        show!(self, show_lock_button, show_lock_button_changed, v.show_lock_button);
        show!(self, meter_columns, meter_columns_changed, v.meter_columns);
        show!(self, encounter_timeout, encounter_timeout_changed, v.encounter_timeout);
        show!(self, debug_logging, debug_logging_changed, v.debug_logging);
        show!(self, packet_logging, packet_logging_changed, v.packet_logging);
        show!(self, training_mode, training_mode_changed, v.training_mode);
        show!(self, all_targets_window, all_targets_window_changed, v.all_targets_window);
        show!(self, details_font_size, details_font_size_changed, v.details_font_size);
        show!(self, details_icon_size, details_icon_size_changed, v.details_icon_size);
        show!(self, skill_columns, skill_columns_changed, v.skill_columns);
        show!(self, screenshot_with_meter, screenshot_with_meter_changed, v.screenshot_with_meter);
        show!(self, screenshot_to_folder, screenshot_to_folder_changed, v.screenshot_to_folder);
        show!(self, screenshot_folder, screenshot_folder_changed, v.screenshot_folder);
        show!(self, screenshot_place, screenshot_place_changed, v.screenshot_place);
        show!(self, color_theme, color_theme_changed, v.color_theme);
        show!(self, details_monitor, details_monitor_changed, v.details_monitor);
    }
}
