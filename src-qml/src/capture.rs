// SPDX-License-Identifier: AGPL-3.0-only

//! `Capture`, the QML singleton for the live capture: whether packets can be
//! captured at all (the meter's notice), the pause, and what Settings >
//! Connection shows and changes: the character name, the network devices and
//! the one picked (saved, as 1.x's "dpsMeter.manualDevice"), and the device
//! and port the meter found. The pick is a preference: while it is missing
//! or carries none of the game, the meter follows the game elsewhere and
//! Settings says so. The rules are the
//! backend's, called as 1.x calls them. A replay has no capture: there the
//! pause and the notice only change the look, and nothing is found.

use core::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::OnceLock;

use cxx_qt::{CxxQtThread, CxxQtType, Threading};
use cxx_qt_lib::QString;
use daevalog::combat::data_storage::DataStorage;
use daevalog::config::setting_changes::{apply_character_name, CHARACTER_NAME_KEY};

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
        #[qproperty(bool, notice)]
        #[qproperty(bool, suspended, READ, WRITE = change_suspended, NOTIFY)]
        #[qproperty(QString, name, READ, NOTIFY)]
        #[qproperty(QString, devices, READ, NOTIFY)]
        #[qproperty(QString, device, READ, WRITE = change_device, NOTIFY)]
        #[qproperty(bool, device_missing, READ, NOTIFY)]
        #[qproperty(QString, following, READ, NOTIFY)]
        #[qproperty(QString, locked_device, READ, NOTIFY)]
        #[qproperty(i32, port, READ, NOTIFY)]
        #[qproperty(bool, game_running, READ, NOTIFY)]
        type Capture = super::CaptureRust;

        fn change_suspended(self: Pin<&mut Capture>, value: bool);
        fn change_device(self: Pin<&mut Capture>, value: QString);

        /// The player's own word on their character's name, typed in
        /// Settings: saved, and taken by the meter even after the game has
        /// named a character.
        #[qinvokable]
        fn set_typed_name(self: Pin<&mut Capture>, name: QString);

        /// Reads the device and port found, and the name, again.
        #[qinvokable]
        fn refresh(self: Pin<&mut Capture>);

        /// Lists the network devices again, on a thread of its own.
        #[qinvokable]
        fn load_devices(self: Pin<&mut Capture>);

        /// Forgets the device and port found, and the ping, to look again.
        #[qinvokable]
        fn reset_detection(self: Pin<&mut Capture>);
    }

    impl cxx_qt::Threading for Capture {}
    impl cxx_qt::Initialize for Capture {}
}

pub struct CaptureRust {
    /// Capture cannot start; the meter shows a notice until it is dismissed.
    notice: bool,
    suspended: bool,
    /// The name the meter goes by: the setting, which takes over the game's.
    name: QString,
    /// The network devices, as JSON: ["eth0", ...].
    devices: QString,
    /// The device picked, saved; empty detects it.
    device: QString,
    /// The picked device is not in `devices`.
    device_missing: bool,
    /// The device the meter follows the game on instead of the picked one;
    /// empty when it is on the pick, or nothing is picked or found.
    following: QString,
    /// The device and port the meter found; empty and 0 before.
    locked_device: QString,
    port: i32,
    game_running: bool,
}

impl Default for CaptureRust {
    fn default() -> Self {
        Self {
            notice: false,
            suspended: PAUSED.load(Ordering::SeqCst),
            name: QString::from(&saved_name()),
            devices: QString::from("[]"),
            device: QString::from(&saved_device()),
            device_missing: false,
            following: QString::default(),
            locked_device: QString::default(),
            port: 0,
            game_running: false,
        }
    }
}

/// The object, for the capture's own threads.
static QT: OnceLock<CxxQtThread<qobject::Capture>> = OnceLock::new();

/// What the capture last said before the object existed: 0 nothing yet, 1
/// it captures, 2 it cannot.
static AVAILABLE: AtomicU8 = AtomicU8::new(0);

/// The pause: the live loop holds the screen while it is on (`live.rs`);
/// capture and counting go on.
static PAUSED: AtomicBool = AtomicBool::new(false);

impl cxx_qt::Initialize for qobject::Capture {
    fn initialize(mut self: Pin<&mut Self>) {
        let _ = QT.set(self.qt_thread());
        if AVAILABLE.load(Ordering::SeqCst) == 2 {
            self.as_mut().set_notice(true);
        }
    }
}

/// Whether the meter is paused now.
pub fn paused() -> bool {
    PAUSED.load(Ordering::SeqCst)
}

/// The capture can capture, or cannot: the meter's notice comes and goes
/// with it, as 1.x's.
pub fn show_available(available: bool) {
    AVAILABLE.store(if available { 1 } else { 2 }, Ordering::SeqCst);
    if let Some(qt) = QT.get() {
        let _ = qt.queue(move |capture| capture.set_notice(!available));
    }
}

/// The name setting, as the player last saved it or the game last gave it.
pub fn saved_name() -> String {
    crate::settings_store::store().raw(CHARACTER_NAME_KEY).map(|n| n.trim().to_string()).unwrap_or_default()
}

/// The saved capture device: 1.x's key, so a pick made there is kept.
pub const DEVICE_KEY: &str = "dpsMeter.manualDevice";

/// The capture device the player picked; empty is Auto-detect.
pub fn saved_device() -> String {
    crate::settings_store::store().raw(DEVICE_KEY).map(|d| d.trim().to_string()).unwrap_or_default()
}

/// Whether the picked device `pick` is missing from `devices`, and the
/// device the game is followed on instead (`locked`), else empty. An empty
/// list is not known yet, so nothing is missing.
pub fn pick_state(pick: &str, devices: &[String], locked: &str) -> (bool, String) {
    let pick = pick.trim();
    if pick.is_empty() {
        return (false, String::new());
    }
    let same = |d: &str| d.trim().eq_ignore_ascii_case(pick);
    let missing = !devices.is_empty() && !devices.iter().any(|d| same(d));
    let following = if locked.trim().is_empty() || same(locked) { String::new() } else { locked.trim().to_string() };
    (missing, following)
}

/// The name the game gave the local player, once it has: what the name
/// setting takes over, as 1.x's meter does. `None` before the game's self
/// record, and for an unnamed tutorial character, who is not worth
/// remembering over the last real name.
pub fn name_from_game(storage: &DataStorage) -> Option<String> {
    if !storage.local_identity_from_self_record() {
        return None;
    }
    storage.local_character_name().map(|n| n.trim().to_string()).filter(|n| !n.is_empty())
}

macro_rules! show {
    ($this:ident, $field:ident, $signal:ident, $value:expr) => {{
        let value = $value;
        if $this.$field != value {
            $this.as_mut().rust_mut().$field = value;
            $this.as_mut().$signal();
        }
    }};
}

impl qobject::Capture {
    fn change_suspended(mut self: Pin<&mut Self>, value: bool) {
        PAUSED.store(value, Ordering::SeqCst);
        show!(self, suspended, suspended_changed, value);
    }

    fn change_device(mut self: Pin<&mut Self>, value: QString) {
        let device = value.to_string().trim().to_string();
        if device != saved_device() {
            crate::settings_store::store().set_raw(DEVICE_KEY, &device);
        }
        if let Some(feed) = crate::live::feed() {
            feed.ports.set_preferred_device(Some(device.clone()).filter(|d| !d.is_empty()));
        }
        show!(self, device, device_changed, QString::from(&device));
        self.show_pick_state();
    }

    fn show_pick_state(mut self: Pin<&mut Self>) {
        let devices: Vec<String> = serde_json::from_str(&self.devices.to_string()).unwrap_or_default();
        let (missing, following) = pick_state(&self.device.to_string(), &devices, &self.locked_device.to_string());
        show!(self, device_missing, device_missing_changed, missing);
        show!(self, following, following_changed, QString::from(&following));
    }

    fn set_typed_name(mut self: Pin<&mut Self>, name: QString) {
        let name = name.to_string().trim().to_string();
        // Against what was last saved, not the name shown: typing the name
        // the game gave saves it as the player's own.
        if name == saved_name() {
            return;
        }
        crate::settings_store::store().set_raw(CHARACTER_NAME_KEY, &name);
        if let Some(feed) = crate::live::feed() {
            apply_character_name(&feed.storage, &name, true);
        }
        show!(self, name, name_changed, QString::from(&name));
    }

    fn refresh(mut self: Pin<&mut Self>) {
        show!(self, name, name_changed, QString::from(&saved_name()));
        // After Reset all settings: back to Auto-detect.
        let device = saved_device();
        if device != self.device.to_string() {
            self.as_mut().change_device(QString::from(&device));
        }
        self.as_mut().show_pick_state();
        let Some(feed) = crate::live::feed() else { return };
        let port = feed.ports.current_port().map_or(0, i32::from);
        let device = feed.ports.current_device().unwrap_or_default();
        show!(self, port, port_changed, port);
        show!(self, locked_device, locked_device_changed, QString::from(&device));
        show!(self, game_running, game_running_changed, daevalog::platform::window_detector::find_aion2_window());
        self.show_pick_state();
    }

    fn load_devices(self: Pin<&mut Self>) {
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            // The capture helper lists them; none while it is not running.
            let devices = daevalog::capture::live::list_device_labels().unwrap_or_default();
            let json = serde_json::to_string(&devices).unwrap_or_else(|_| "[]".into());
            let _ = qt.queue(move |mut capture| {
                show!(capture, devices, devices_changed, QString::from(&json));
                capture.show_pick_state();
            });
        });
    }

    fn reset_detection(mut self: Pin<&mut Self>) {
        if let Some(feed) = crate::live::feed() {
            feed.ports.reset();
            feed.ping.reset();
        }
        self.as_mut().refresh();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_games_name_counts_once_the_game_has_said_it() {
        let storage = DataStorage::new();
        storage.set_local_character_name(Some("Typed".into()));
        assert_eq!(name_from_game(&storage), None, "a typed name is not the game's");
        storage.set_local_identity_from_game(7577, Some(" Corvane ".into()));
        assert_eq!(name_from_game(&storage).as_deref(), Some("Corvane"));
        // An unnamed tutorial character.
        storage.set_local_identity_from_game(7578, None);
        assert_eq!(name_from_game(&storage), None);
    }

    #[test]
    fn a_picked_device_is_saved_and_comes_back() {
        let dir = std::env::temp_dir().join(format!("capture-device-{}", std::process::id()));
        let store = crate::settings_store::Store::open(&dir, None);
        assert_eq!(store.raw(DEVICE_KEY), None, "Auto-detect until a pick");
        store.set_raw(DEVICE_KEY, "vpn0");
        store.flush().unwrap();
        let again = crate::settings_store::Store::open(&dir, None);
        assert_eq!(again.raw(DEVICE_KEY).as_deref(), Some("vpn0"), "after a restart");
        // Auto-detect saves as empty.
        again.set_raw(DEVICE_KEY, "");
        again.flush().unwrap();
        assert_eq!(crate::settings_store::Store::open(&dir, None).raw(DEVICE_KEY).as_deref(), Some(""));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_or_idle_pick_shows_what_the_meter_uses() {
        let devices = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        let listed = devices(&["lo", "eth0", "vpn0"]);
        // Auto-detect: nothing to say.
        assert_eq!(pick_state("", &listed, "eth0"), (false, String::new()));
        // On the pick, any case.
        assert_eq!(pick_state("vpn0", &listed, "VPN0"), (false, String::new()));
        assert_eq!(pick_state("vpn0", &listed, ""), (false, String::new()), "nothing found yet");
        // The VPN is off: missing, and the game is followed on eth0.
        assert_eq!(pick_state("vpn0", &devices(&["lo", "eth0"]), "eth0"), (true, "eth0".into()));
        // There, but the game is elsewhere.
        assert_eq!(pick_state("vpn0", &listed, "eth0"), (false, "eth0".into()));
        // A list not read yet is not a missing device.
        assert_eq!(pick_state("vpn0", &[], ""), (false, String::new()));
    }
}
