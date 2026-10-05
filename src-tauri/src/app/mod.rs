//! The desktop application: Tauri commands, capture wiring, window management.
//!
//! Split out of `lib.rs` so the crate can be built *without* it. The parser has
//! to compile to `wasm32-unknown-unknown` — that is how the server re-derives an
//! uploaded encounter with the same code the client ran — and nothing in this
//! file can: `tauri`, `libloading`, `reqwest` and the Windows API all fail on
//! that target. Everything here is behind the `desktop` feature; the parser core
//! is not, and `cargo check --no-default-features --target wasm32-unknown-unknown`
//! is what keeps it that way.

// Brought into scope as modules because this file was split out of lib.rs,
// where these were siblings at the crate root.
use crate::{entity, i18n, logging, platform, share};

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tauri::{Emitter, Manager};
use tokio::sync::mpsc;

use crate::capture::captured_payload::CapturedPayload;
use crate::capture::combat_port_detector::CombatPortDetector;
use crate::capture::pcap_capturer::PcapCapturer;
use crate::combat::capture_dispatcher::CaptureDispatcher;
use crate::combat::data_storage::DataStorage;
use crate::combat::dps_calculator::{DpsCalculator, PARTY_ROW_ID_BASE};
use crate::combat::ping_tracker::PingTracker;
use crate::config::settings::Settings;
use crate::entity::dps_data::DpsData;
use crate::entity::fight_record::{FightRecord, FightSummary};
use crate::entity::details_context::{DetailsContext, TargetDetailsResponse};
use crate::history::fight_history::FightHistoryManager;
use crate::i18n::lookup::{NpcLookup, SkillLookup};

mod auto_upload;
mod commands;
mod drag_resize;
mod local_player;
mod overlay_lock;
mod replay;
mod screenshots;
mod supporter_roster;
mod tool_windows;
mod tray_actions;
mod updater;

use auto_upload::auto_upload;
use commands::system::open_url;
use drag_resize::WAYLAND_LAYER_KEY;
use local_player::{bind_local_actor, bind_local_name, is_placeholder_id};
pub(crate) use overlay_lock::{overlay_lock_available, toggle_overlay_lock};
use overlay_lock::OverlayLock;
use supporter_roster::{
    fetch_supporter_roster, load_supporter_override, ROSTER_POLL_OVERRIDE, ROSTER_POLL_PUBLISHED,
};
pub(crate) use tray_actions::{open_history_from_tray, open_settings_from_tray, quit_from_tray};
use tray_actions::save_fights_before_exit;
use tool_windows::{details_monitor_setting, open_details_on_monitor, rect_covers_a_monitor, DETAILS_USER_PLACED_KEY};

/// Shared application state.
pub struct AppState {
    pub data_storage: Arc<DataStorage>,
    pub dps_calculator: Mutex<DpsCalculator>,
    pub ping_tracker: Arc<PingTracker>,
    pub port_detector: Arc<CombatPortDetector>,
    pub fight_history: FightHistoryManager,
    pub settings: Settings,
    pub skill_lookup: Arc<SkillLookup>,
    pub npc_lookup: Arc<NpcLookup>,
    pub app_data_dir: std::path::PathBuf,
    pub i18n_data_dir: Option<std::path::PathBuf>,
    /// One client, reused. The update check and the MSI download each used a
    /// one-shot `reqwest::get`, which builds a fresh client and TLS stack per
    /// call; anything periodic wants a pool rather than a handshake every time.
    pub http: reqwest::Client,
    /// The header's suspend button: while set, the capture dispatcher drops
    /// every packet. Shared with it (`CaptureDispatcher::use_suspend_flag`).
    pub capture_suspended: Arc<std::sync::atomic::AtomicBool>,
    /// The overlay's click-through lock. See `apply_overlay_lock`.
    pub overlay_lock: Arc<OverlayLock>,
    /// What the last account check found: `None` until one has run, then
    /// `Some(None)` signed out or `Some(Some(_))` signed in. Settings shows it
    /// at once instead of "checking" for as long as the server takes.
    pub account_seen: Mutex<Option<Option<crate::account::AccountSummary>>>,
}

// ===== TAURI COMMANDS =====

/// Who, if anyone, this meter is signed in as.
///
/// Returns `None` when there is no token, or when the server says the one we
/// have is no longer valid — a token revoked from the website should stop
/// looking connected here at the next check, not at the next reinstall.
#[tauri::command]
async fn account_status(
    state: tauri::State<'_, AppState>,
) -> Result<Option<crate::account::AccountSummary>, String> {
    let who = match crate::account::whoami(&state.http, &state.app_data_dir).await {
        crate::account::AccountState::SignedIn(summary) => Some(summary),
        crate::account::AccountState::SignedOut => None,
        crate::account::AccountState::Unavailable(why) => return Err(why),
    };
    *state.account_seen.lock() = Some(who.clone());
    Ok(who)
}

/// Whether this build can show a Discord activity (it has a Discord
/// application configured). The Settings toggle is hidden when it cannot.
#[tauri::command]
fn discord_activity_available() -> bool {
    crate::presence::available()
}

/// What the last `account_status` found, without asking the server again.
/// `None` when nothing has been checked yet this session.
#[tauri::command]
fn account_status_cached(
    state: tauri::State<'_, AppState>,
) -> Option<Option<crate::account::AccountSummary>> {
    state.account_seen.lock().clone()
}

/// Begin signing in, and return the code to show the player.
///
/// The meter has no browser and cannot hold a client secret, so this is the
/// Device Authorization Grant: the server hands back a short code, the player
/// approves it on a2tools.app, and a background task polls until they do. The
/// polling credential never reaches the webview — see `DeviceGrant`.
#[tauri::command]
async fn account_begin_link(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<crate::account::LinkPrompt, String> {
    let grant = crate::account::start(&state.http, &crate::account::device_label()).await?;
    let prompt = crate::account::LinkPrompt::from(&grant);

    // Open the browser straight onto the filled-in code. If it fails the player
    // still has the code and the URL in front of them.
    if crate::account::is_site_url(&grant.verification_uri_complete) {
        open_url(grant.verification_uri_complete.clone());
    } else {
        tracing::warn!("Not opening the sign-in page: the server sent a link outside a2tools.app");
    }

    let http = state.http.clone();
    let app_data_dir = state.app_data_dir.clone();
    tauri::async_runtime::spawn(async move {
        let outcome = crate::account::poll_until_decided(&http, &grant, &app_data_dir).await;
        let payload = match &outcome {
            Ok(()) => serde_json::json!({ "connected": true }),
            Err(why) => serde_json::json!({ "connected": false, "error": why }),
        };
        if let Err(e) = app.emit("account-changed", payload) {
            tracing::warn!("could not announce the account change: {e}");
        }
    });

    Ok(prompt)
}

/// Forget the token on this machine.
///
/// Local only, deliberately. Revoking it everywhere is a decision to make on the
/// website, where every device that holds a token is listed — a meter that could
/// silently revoke its own credential server-side would make "sign out on this
/// PC" and "this PC was stolen" the same button.
#[tauri::command]
fn account_sign_out(state: tauri::State<'_, AppState>) {
    crate::account::secret::clear(&state.app_data_dir);
    *state.account_seen.lock() = Some(None);
    tracing::info!("Account signed out on this machine");
}

#[tauri::command]
fn get_settings(state: tauri::State<'_, AppState>) -> std::collections::HashMap<String, String> {
    state.settings.get_all()
}

/// Store a setting and tell every window about it.
///
/// Settings are edited in their own window, so without this broadcast the meter
/// keeps rendering with whatever it read at startup — toggling something like
/// "Round DPS" would appear to do nothing until the app restarted. Only real
/// changes are emitted (see `Settings::set`), so the originating window's echo
/// stops here rather than bouncing between windows.
#[tauri::command]
fn update_settings(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    key: String,
    value: String,
) {
    if key == ENCOUNTER_TIMEOUT_KEY {
        apply_encounter_timeout(&state.data_storage, Some(&value));
    }
    if state.settings.set(&key, &value) {
        if key == crate::tray::HIDE_FROM_TASKBAR_KEY {
            crate::tray::apply_taskbar(&app);
        }
        let _ = app.emit("setting-changed", serde_json::json!({ "key": key, "value": value }));
    }
}

const ENCOUNTER_TIMEOUT_KEY: &str = "dpsMeter.encounterTimeoutSec";

/// The encounter timeout setting, in whole seconds; anything else keeps the
/// default.
fn apply_encounter_timeout(storage: &DataStorage, value: Option<&str>) {
    let secs = value.and_then(|v| v.trim().parse::<i64>().ok());
    let ms = secs.map_or(crate::combat::data_storage::DEFAULT_ENCOUNTER_TIMEOUT_MS, |s| s * 1000);
    storage.set_encounter_timeout_ms(ms);
}

#[tauri::command]
fn clear_settings(state: tauri::State<'_, AppState>) {
    state.settings.clear();
}

#[tauri::command]
fn get_capture_status(state: tauri::State<'_, AppState>) -> serde_json::Value {
    let port = state.port_detector.current_port();
    let device = state.port_detector.current_device();
    let local_id = state.data_storage.local_player_id();
    let char_name = state.data_storage.local_character_name();
    serde_json::json!({
        "locked": port.is_some(),
        "port": port,
        "device": device.clone().unwrap_or_default(),
        "ip": device.unwrap_or_else(|| "127.0.0.1".to_string()),
        "localPlayerId": local_id,
        "characterName": char_name,
        // When true, characterName is the game's (null = an unnamed tutorial
        // character) and the UI should adopt it rather than push its own.
        "characterNameFromGame": state.data_storage.local_identity_from_self_record(),
    })
}

#[tauri::command]
fn set_character_name(state: tauri::State<'_, AppState>, name: String, manual: Option<bool>) {
    // The game has said who is playing; a name from the window title or the
    // last session is at best the same and at worst another character. A name
    // the player typed (`manual`) is taken anyway: it is their call, and the
    // game's next self record replaces it if it was wrong.
    let ds = &state.data_storage;
    if ds.local_identity_from_self_record() && !manual.unwrap_or(false) {
        return;
    }
    let trimmed = name.trim().to_string();
    ds.set_local_character_name(Some(name));
    // If an actor ID was already bound, put the name on it now so the meter
    // updates. Not permanent: the name follows the local id, not this one.
    if !trimmed.is_empty() {
        if let Some(id) = ds.local_player_id().filter(|&id| !is_placeholder_id(id)) {
            ds.set_local_nickname(id as i32, &trimmed);
        }
    }
}

/// `manual`: typed in Settings, so it wins over the game's identity.
/// `view`: the window that sent it, for the log.
#[tauri::command]
fn bind_local_actor_id(
    state: tauri::State<'_, AppState>,
    actor_id: i64,
    manual: Option<bool>,
    view: Option<String>,
) {
    bind_local_actor(&state.data_storage, actor_id, manual.unwrap_or(false), &view.unwrap_or_default());
}

#[tauri::command]
fn bind_local_nickname(
    state: tauri::State<'_, AppState>,
    actor_id: i64,
    nickname: String,
    manual: Option<bool>,
    view: Option<String>,
) {
    bind_local_name(&state.data_storage, actor_id, &nickname, manual.unwrap_or(false), &view.unwrap_or_default());
}

#[tauri::command]
fn is_admin() -> bool {
    platform::admin::is_admin()
}

#[tauri::command]
fn set_language(state: tauri::State<'_, AppState>, language: String) {
    tracing::info!("Language change requested: {}", language);
    if let Some(ref data_dir) = state.i18n_data_dir {
        i18n::lookup::load_language(&state.skill_lookup, &state.npc_lookup, data_dir, &language);
    } else {
        tracing::warn!("No i18n data dir available for language reload");
    }
    state.settings.set("dpsMeter.language", &language);
}

#[tauri::command]
fn set_debug_logging(state: tauri::State<'_, AppState>, enabled: bool) {
    logging::logger::set_debug_enabled(enabled, &state.app_data_dir);
    state.settings.set("dpsMeter.debugLoggingEnabled", if enabled { "true" } else { "false" });
}

#[tauri::command]
fn set_packet_logging(state: tauri::State<'_, AppState>, enabled: bool) {
    logging::logger::set_packet_log_enabled(enabled, &state.app_data_dir);
    state.settings.set("dpsMeter.saveRawPackets", if enabled { "true" } else { "false" });
}

#[tauri::command]
fn reset_auto_detection(state: tauri::State<'_, AppState>) {
    state.port_detector.reset();
    state.ping_tracker.reset();
}

#[tauri::command]
fn get_available_devices() -> Vec<String> {
    // Load the OS's pcap library and enumerate devices
    match crate::capture::pcap_capturer::list_device_labels() {
        Ok(labels) => labels,
        Err(_) => Vec::new(),
    }
}

#[tauri::command]
fn set_manual_device(state: tauri::State<'_, AppState>, device: String) {
    let dev = if device.trim().is_empty() { None } else { Some(device) };
    state.port_detector.set_preferred_device(dev);
}

#[tauri::command]
fn suspend_capture(state: tauri::State<'_, AppState>, suspended: bool) {
    // The header's suspend button. It was wired to empty stubs since the move
    // to Tauri, so it changed its icon and the status line but counting went
    // on (a player found it in 2.0.37, issue #6).
    state.capture_suspended.store(suspended, std::sync::atomic::Ordering::SeqCst);
    tracing::info!("Capture {}", if suspended { "suspended" } else { "resumed" });
}

#[tauri::command]
fn is_capture_suspended(state: tauri::State<'_, AppState>) -> bool {
    state.capture_suspended.load(std::sync::atomic::Ordering::SeqCst)
}




#[tauri::command]
fn get_aion2_window_title() -> Option<String> {
    platform::window_detector::find_aion2_window_title()
}

#[tauri::command]
fn test_auto_hide() -> serde_json::Value {
    let aion_fg = platform::window_detector::is_aion2_foreground();
    let aion_title = platform::window_detector::find_aion2_window_title();
    serde_json::json!({
        "aion2_foreground": aion_fg,
        "aion2_title": aion_title,
    })
}

#[tauri::command]
fn debug_status(state: tauri::State<'_, AppState>) -> serde_json::Value {
    let port = state.port_detector.current_port();
    let device = state.port_detector.current_device();
    let ping = state.ping_tracker.current_ping_ms();
    let dmg_gen = state.data_storage.damage_generation();
    let window = platform::window_detector::find_aion2_window_title();
    let admin = platform::admin::is_admin();
    serde_json::json!({
        "port": port,
        "device": device,
        "ping": ping,
        "damageGeneration": dmg_gen,
        "aion2Window": window,
        "isAdmin": admin,
    })
}

// ===== APP SETUP =====

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Started as the Linux hotkey helper: that is all this process does.
    if platform::hotkeys::run_helper_if_asked() {
        return;
    }
    // Before anything starts a thread: it may set environment variables.
    let process_note = platform::process::prepare();
    logging::logger::init_logging();
    if let Some(note) = process_note {
        tracing::info!("{note}");
    }
    // Before Tauri or WebKit opens anything in the data folders.
    crate::migrate::from_a2tools();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            // Resolve data directory
            let app_data_dir = app.path().app_data_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let _ = std::fs::create_dir_all(&app_data_dir);

            // Load resources — try multiple paths (dev vs production)
            let skill_lookup = SkillLookup::new();
            let npc_lookup = NpcLookup::new();
            let mut dot_ids: HashSet<i32> = HashSet::new();

            let resource_dir = app.path().resource_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let candidate_dirs = [
                resource_dir.join("data"),                        // production: resources/data
                resource_dir.join("_up_").join("src").join("data"), // production: resources/_up_/src/data (from ../src/data)
                resource_dir.join("..").join("src").join("data"), // dev: src-tauri/../src/data
                std::path::PathBuf::from("src/data"),             // dev: cwd fallback
                std::path::PathBuf::from("../src/data"),          // dev: from src-tauri/
            ];

            // Find the data directory
            let mut found_data_dir: Option<std::path::PathBuf> = None;
            for data_dir in &candidate_dirs {
                if data_dir.exists() && data_dir.join("i18n").join("skills").exists() {
                    found_data_dir = Some(data_dir.clone());
                    break;
                }
            }

            if let Some(ref data_dir) = found_data_dir {
                // Load DOT skill IDs (language-independent)
                if let Ok(text) = std::fs::read_to_string(data_dir.join("dot_skill_ids.json")) {
                    if let Ok(ids) = serde_json::from_str::<Vec<i32>>(&text) {
                        for id in ids { dot_ids.insert(id); }
                        tracing::info!("Loaded {} DOT skill IDs", dot_ids.len());
                    }
                }

                // Load skill/NPC data in the user's language
                let language = Settings::new(app_data_dir.clone())
                    .get("dpsMeter.language")
                    .unwrap_or_else(|| "en".to_string());
                i18n::lookup::load_language(&skill_lookup, &npc_lookup, data_dir, &language);
            } else {
                tracing::warn!("Failed to find data directory!");
            }

            let skill_lookup = Arc::new(skill_lookup);
            let npc_lookup = Arc::new(npc_lookup);

            let data_storage = Arc::new(DataStorage::new());
            let ping_tracker = Arc::new(PingTracker::with_perf_clock(platform::clock::perf_clock()));
            let port_detector = Arc::new(CombatPortDetector::new());

            let dps_calculator = DpsCalculator::new(
                data_storage.clone(),
                skill_lookup.clone(),
                npc_lookup.clone(),
                ping_tracker.clone(),
            );

            let settings = Settings::new(app_data_dir.clone());
            apply_encounter_timeout(&data_storage, settings.get(ENCOUNTER_TIMEOUT_KEY).as_deref());

            // Load logging settings from saved state
            if settings.get("dpsMeter.debugLoggingEnabled").as_deref() == Some("true") {
                logging::logger::set_debug_enabled(true, &app_data_dir);
            }
            if settings.get("dpsMeter.saveRawPackets").as_deref() == Some("true") {
                logging::logger::set_packet_log_enabled(true, &app_data_dir);
            }

            let state = AppState {
                data_storage: data_storage.clone(),
                dps_calculator: Mutex::new(dps_calculator),
                ping_tracker: ping_tracker.clone(),
                port_detector: port_detector.clone(),
                fight_history: FightHistoryManager::new(app_data_dir.clone()),
                settings,
                skill_lookup: skill_lookup.clone(),
                npc_lookup: npc_lookup.clone(),
                app_data_dir: app_data_dir.clone(),
                i18n_data_dir: found_data_dir.clone(),
                http: reqwest::Client::builder()
                    .user_agent(concat!("A2Tools-DPS-Meter/", env!("CARGO_PKG_VERSION")))
                    .connect_timeout(Duration::from_secs(10))
                    .timeout(Duration::from_secs(30))
                    .build()
                    .unwrap_or_default(),
                capture_suspended: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                overlay_lock: Arc::new(OverlayLock::default()),
                account_seen: Mutex::new(None),
            };
            let capture_suspended = state.capture_suspended.clone();

            app.manage(state);
            crate::presence::spawn(app.handle().clone());

            // Hidden into the tray only when there is a tray to bring it back
            // from: a desktop without one would leave no way to the meter.
            let has_tray = crate::tray::create(app.handle());
            crate::tray::apply_taskbar(app.handle());
            let start_hidden = has_tray && crate::tray::start_in_tray(app.handle());
            if start_hidden {
                if let Some(main) = app.get_webview_window("main") {
                    let _ = main.hide();
                }
            }

            // Reopen the Details window if it was left enabled. Done here rather
            // than from JS because the backend already has settings loaded — the
            // frontend reads them asynchronously and would race the first paint.
            {
                let saved = app.state::<AppState>().settings.get("dpsMeter.detailsMonitor");
                if let Some(value) = saved {
                    let value = value.trim().to_string();
                    if !value.is_empty() && value != "off" {
                        if let Ok(index) = value.parse::<usize>() {
                            let handle = app.handle().clone();
                            // Deferred: available_monitors is unreliable until the
                            // main window exists and the event loop has run once.
                            tauri::async_runtime::spawn(async move {
                                tokio::time::sleep(Duration::from_millis(600)).await;
                                if let Err(e) = open_details_on_monitor(&handle, index) {
                                    tracing::warn!("details window reopen failed: {}", e);
                                }
                            });
                        }
                    }
                }
            }

            // Restore saved window position and ensure always-on-top
            if let Some(window) = app.get_webview_window("main") {
                let state_ref = app.state::<AppState>();
                let saved = match (state_ref.settings.get("window.x"), state_ref.settings.get("window.y")) {
                    (Some(x), Some(y)) => x.parse::<i32>().ok().zip(y.parse::<i32>().ok())
                        // Don't restore minimized positions (Windows uses -32000,-32000)
                        .filter(|&(x, y)| x > -10000 && y > -10000),
                    _ => None,
                };
                // On Linux the overlay starts hidden (tauri.linux.conf.json):
                // a layer surface can only be made before it is first shown.
                let scale = window.scale_factor().unwrap_or(1.0);
                let logical = saved.map_or((0, 0), |(x, y)| ((x as f64 / scale) as i32, (y as f64 / scale) as i32));
                let layer = platform::window::init_overlay_layer(
                    &window,
                    state_ref.settings.get(WAYLAND_LAYER_KEY).as_deref() == Some("true"),
                    logical,
                );
                if let (false, Some((x, y))) = (layer, saved) {
                    let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }));
                }
                let _ = window.set_always_on_top(true);
                if let Ok(size) = window.inner_size() {
                    platform::window::set_size(&window, tauri::Size::Physical(size));
                }
                if !start_hidden {
                    let _ = window.show();
                }
            }

            // Check if Npcap is available before starting capture
            let npcap_available = platform::pcap::library_available();
            if !npcap_available {
                tracing::error!("Npcap is not installed — packet capture disabled");
                // Notify frontend to show install prompt
                let handle_npcap = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    // Small delay so frontend has time to initialize
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    let _ = handle_npcap.emit("npcap-missing", ());
                });
            }

            // Start capture pipeline
            let (tx, rx) = mpsc::channel::<CapturedPayload>(4096);

            let capturer = PcapCapturer::new(tx);
            if npcap_available {
                capturer.start();
            }

            let mut dispatcher = CaptureDispatcher::new(
                data_storage.clone(),
                skill_lookup.clone(),
                npc_lookup.clone(),
                port_detector.clone(),
                ping_tracker.clone(),
            );
            dispatcher.set_dot_skill_ids(dot_ids);
            dispatcher.use_suspend_flag(capture_suspended);

            // Run dispatcher in background
            tauri::async_runtime::spawn(async move {
                dispatcher.run(rx).await;
            });

            // Register global hotkeys from saved settings (or defaults)
            let hotkey_handle = app.handle().clone();
            let hotkey_manager = platform::hotkeys::HotkeyManager::new();

            let reload_label = app.state::<AppState>().settings
                .get("dpsMeter.hotkey").unwrap_or_default();
            let toggle_label = app.state::<AppState>().settings
                .get("dpsMeter.toggleWindowHotkey").unwrap_or_default();
            let lock_label = app.state::<AppState>().settings
                .get("dpsMeter.lockHotkey").unwrap_or_default();

            let (reload_mods, reload_vk) = platform::hotkeys::parse_hotkey_label(&reload_label)
                .unwrap_or((0x0002 | 0x0001, 0x52)); // Default: Ctrl+Alt+R
            let (toggle_mods, toggle_vk) = platform::hotkeys::parse_hotkey_label(&toggle_label)
                .unwrap_or((0x0002 | 0x0001, 0x26)); // Default: Ctrl+Alt+Up
            let (lock_mods, lock_vk) = platform::hotkeys::parse_hotkey_label(&lock_label)
                .unwrap_or((0x0002 | 0x0001, 0x4C)); // Default: Ctrl+Alt+L

            hotkey_manager.start(
                reload_mods, reload_vk,
                toggle_mods, toggle_vk,
                lock_mods, lock_vk,
                {
                    let h = hotkey_handle.clone();
                    move || {
                        tracing::info!("Hotkey: reload triggered");
                        if let Some(state) = h.try_state::<AppState>() {
                            state.dps_calculator.lock().restart_target_selection(true);
                            state.data_storage.reset_nicknames();
                        }
                        // Notify frontend to clear UI
                        let _ = h.emit("combat-reset", ());
                        let _ = h.emit("dps-update", &entity::dps_data::DpsData::new());
                    }
                },
                {
                    let h = hotkey_handle.clone();
                    move || {
                        // Toggle window visibility
                        if let Some(window) = h.get_webview_window("main") {
                            if window.is_visible().unwrap_or(false) {
                                let _ = window.hide();
                            } else {
                                let _ = window.show();
                                let _ = window.set_always_on_top(true);
                                let _ = window.set_focus();
                            }
                        }
                    }
                },
                {
                    let h = hotkey_handle;
                    move || toggle_overlay_lock(&h)
                },
            );

            // Periodic DPS update emission (every 500ms)
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_millis(500));
                let mut tick_count: u64 = 0;
                let mut hide_delay: u64 = 0; // ticks to wait before hiding
                loop {
                    interval.tick().await;
                    tick_count += 1;

                    if let Some(state) = handle.try_state::<AppState>() {
                        let t0 = std::time::Instant::now();
                        let lock_guard = state.dps_calculator.lock();
                        let lock_ms = t0.elapsed().as_millis();
                        let dps = {
                            let mut calc = lock_guard;
                            calc.get_dps()
                        };
                        let calc_ms = t0.elapsed().as_millis();
                        let _ = handle.emit("dps-update", &dps);
                        let total_ms = t0.elapsed().as_millis();
                        if total_ms > 200 {
                            tracing::warn!("Slow: lock={}ms calc={}ms emit={}ms total={}ms gen={}",
                                lock_ms, calc_ms - lock_ms, total_ms - calc_ms, total_ms,
                                state.data_storage.damage_generation());
                        }

                        if let Some(ping) = state.ping_tracker.current_ping_ms() {
                            let _ = handle.emit("ping-update", ping);
                        }

                        // --- Auto-hide when AION2 loses focus (every tick) ---
                        let auto_hide = tick_count > 20
                            && state.settings.get("dpsMeter.autoHideMeter")
                                .unwrap_or_default() == "true";
                        if auto_hide {
                            if let Some(window) = handle.get_webview_window("main") {
                                let aion_fg = platform::window_detector::is_aion2_foreground();
                                let is_self_fg = window.is_focused().unwrap_or(false);
                                let is_visible = window.is_visible().unwrap_or(true);
                                let is_minimized = window.is_minimized().unwrap_or(false);
                                if tick_count % 4 == 0 {
                                    tracing::trace!("auto-hide: aion_fg={} self_fg={} visible={} minimized={} hide_delay={}",
                                        aion_fg, is_self_fg, is_visible, is_minimized, hide_delay);
                                }
                                if aion_fg || is_self_fg {
                                    hide_delay = 0;
                                    if !is_visible || is_minimized {
                                        platform::window::show_on_top_without_focus(&window);
                                        // Notify frontend to recalculate window size
                                        // (content may have changed while minimized)
                                        let _ = window.emit("force-resize", ());
                                    }
                                } else if is_visible && !is_minimized {
                                    // Wait 3 ticks (1.5s) before hiding to avoid
                                    // flickering during alt-tab transitions
                                    hide_delay += 1;
                                    if hide_delay >= 3 {
                                        platform::window::minimize_off_top(&window);
                                    }
                                }
                            }
                        }

                        // --- Save window position every ~5 seconds (every 10 ticks) ---
                        if tick_count % 10 == 0 {
                            if let Some(window) = handle.get_webview_window("main") {
                                let scale = window.scale_factor().unwrap_or(1.0);
                                let layer_pos = platform::window::overlay_layer_position(&window).map(|(x, y)| {
                                    tauri::PhysicalPosition { x: (x as f64 * scale) as i32, y: (y as f64 * scale) as i32 }
                                });
                                if let Some(pos) = layer_pos.or_else(|| window.outer_position().ok()) {
                                    // Don't save minimized/hidden positions
                                    if pos.x > -10000 && pos.y > -10000 {
                                        state.settings.set("window.x", &pos.x.to_string());
                                        state.settings.set("window.y", &pos.y.to_string());
                                    }
                                }
                            }
                            // These float independently of the overlay, so each
                            // remembers where it was left. Per-fight windows
                            // (details-*) are deliberately absent: they are
                            // opened to be read and closed, and keying geometry
                            // by fight id would grow settings without bound.
                            for label in ["details", "settings", "history"] {
                                // While Details is pinned to a monitor its rect
                                // comes from the setting, not from the user.
                                // Saving it poisons the windowed geometry: turn
                                // the setting off and Details would reopen
                                // full-size on that same screen.
                                if label == "details"
                                    && details_monitor_setting(&handle).is_some()
                                {
                                    continue;
                                }
                                if let Some(w) = handle.get_webview_window(label) {
                                    if !w.is_visible().unwrap_or(false) {
                                        continue;
                                    }
                                    if let Ok(pos) = w.outer_position() {
                                        if pos.x > -10000 && pos.y > -10000 {
                                            state.settings.set(&format!("window.{}.x", label), &pos.x.to_string());
                                            state.settings.set(&format!("window.{}.y", label), &pos.y.to_string());
                                        }
                                    }
                                    // Inner, not outer: restore applies it with
                                    // set_size, which sets the inner size. Saving
                                    // the outer size grew every tool window by its
                                    // border (16x9 px here) on each reopen.
                                    if let Ok(size) = w.inner_size() {
                                        if size.width > 100 && size.height > 100 {
                                            state.settings.set(&format!("window.{}.w", label), &size.width.to_string());
                                            state.settings.set(&format!("window.{}.h", label), &size.height.to_string());
                                        }
                                    }
                                    // Details is unpinned here, but the window
                                    // may still be sitting on the fill rect from
                                    // before the setting was switched off. A rect
                                    // that swallows a whole screen is not a
                                    // placement anyone chose by dragging, so it
                                    // never earns the marker.
                                    if label == "details" {
                                        let filling = match (w.outer_position(), w.outer_size()) {
                                            (Ok(pos), Ok(size)) => rect_covers_a_monitor(&handle, pos, size),
                                            _ => true,
                                        };
                                        if !filling {
                                            state.settings.set(DETAILS_USER_PLACED_KEY, "true");
                                        }
                                    }
                                }
                            }
                        }

                    }
                }
            });

            // Separate task for boss fight auto-save (every 30s, on blocking thread)
            let handle_save = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    if let Some(state) = handle_save.try_state::<AppState>() {
                        if state.data_storage.damage_generation() > 0 {
                            // Run on blocking thread to avoid starving the async runtime
                            // snapshot_boss_fights acquires the dps_calculator lock
                            // Run synchronously but only if lock is available
                            if let Some(mut calc) = state.dps_calculator.try_lock() {
                                let records = calc.snapshot_boss_fights();
                                let finished: HashSet<String> = records.iter()
                                    .filter(|r| calc.fight_finished(r))
                                    .map(|r| r.id.clone())
                                    .collect();
                                drop(calc);
                                for record in &records {
                                    let _ = state.fight_history.save_fight(record);
                                    // The packets behind it, so it can be
                                    // uploaded and verified later, and checked
                                    // against the game's own record. Training
                                    // dummies keep one for that check; they are
                                    // never uploaded.
                                    if let Err(e) = share::save_slice(
                                        &state.app_data_dir, record, &state.data_storage) {
                                        tracing::debug!("No slice for {}: {e}", record.id);
                                    }
                                }
                                if !records.is_empty() {
                                    share::prune_slices(&state.app_data_dir);
                                }
                                if state.settings.get(share::AUTO_UPLOAD_KEY).as_deref() == Some("true") {
                                    for record in records.into_iter()
                                        .filter(|r| share::wants_auto_upload(&state.app_data_dir, r, finished.contains(&r.id)))
                                    {
                                        auto_upload(handle_save.clone(), record);
                                    }
                                }
                            }
                        }
                        // Automatic uploads that failed and are due again,
                        // fighting or not: a meter left open after the
                        // connection came back catches up on its own.
                        if state.settings.get(share::AUTO_UPLOAD_KEY).as_deref() == Some("true") {
                            let now = crate::clock::now_ms();
                            for id in share::auto_upload_retries_due(&state.app_data_dir, now) {
                                if let Ok(record) = state.fight_history.load_fight(&id) {
                                    auto_upload(handle_save.clone(), record);
                                }
                            }
                        }
                    }
                }
            });

            // Supporter roster: fetched, never queried. See `crate::supporters`
            // — asking the server "is this player a supporter?" would hand it a
            // list of who you play with, every fight, for a cosmetic.
            let handle_roster = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut had_override = false;
                loop {
                    let mut wait = ROSTER_POLL_PUBLISHED;
                    if let Some(state) = handle_roster.try_state::<AppState>() {
                        // The override wins when present, and is re-read every
                        // pass so editing it takes effect without a restart.
                        match load_supporter_override(&state.app_data_dir) {
                            Some(roster) => {
                                had_override = true;
                                wait = ROSTER_POLL_OVERRIDE;
                                state.data_storage.set_supporters(roster);
                            }
                            None => {
                                let just_lost_override = had_override;
                                if had_override {
                                    tracing::info!("Supporter roster override removed");
                                    had_override = false;
                                }
                                match fetch_supporter_roster(&state.http).await {
                                    Some(roster) => {
                                        tracing::info!(
                                            "Supporter roster: {} entries",
                                            roster.len()
                                        );
                                        state.data_storage.set_supporters(roster);
                                    }
                                    None => {
                                        tracing::debug!("Supporter roster unavailable");
                                        // Keep looking for the override often, so
                                        // dropping the file in works on a machine
                                        // that has never reached the CDN.
                                        wait = ROSTER_POLL_OVERRIDE;
                                        // Only wipe the roster if the override we
                                        // were using has just gone away. Clearing
                                        // on any failed fetch would mean one CDN
                                        // hiccup removes every supporter's gold
                                        // until the next successful poll.
                                        if just_lost_override {
                                            state
                                                .data_storage
                                                .set_supporters(Default::default());
                                        }
                                    }
                                }
                            }
                        }
                    }
                    tokio::time::sleep(wait).await;
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::meter::get_app_version,
            commands::meter::get_dps_snapshot,
            commands::meter::get_skill_details,
            commands::meter::get_displayed_skill_details,
            commands::meter::get_details_context,
            commands::history::get_fight_history,
            commands::history::save_fight,
            commands::history::load_fight,
            commands::history::delete_fight,
            commands::history::export_fight_json,
            commands::share::preview_share,
            commands::share::upload_fight,
            commands::share::share_status,
            commands::share::game_record_status,
            commands::share::game_record_details,
            account_status,
            account_status_cached,
            discord_activity_available,
            account_begin_link,
            account_sign_out,
            get_settings,
            update_settings,
            commands::meter::get_ping,
            get_capture_status,
            commands::meter::set_target_mode,
            commands::meter::set_all_targets_window_ms,
            set_character_name,
            bind_local_actor_id,
            bind_local_nickname,
            clear_settings,
            commands::meter::reset_combat,
            is_admin,
            set_language,
            set_debug_logging,
            set_packet_logging,
            commands::share::send_logs_to_dev,
            get_aion2_window_title,
            debug_status,
            commands::system::quit_app,
            commands::system::open_url,
            commands::system::read_cached_icon,
            commands::system::write_cached_icon,
            commands::system::log_from_ui,
            suspend_capture,
            overlay_lock::overlay_lock_supported,
            overlay_lock::set_overlay_locked,
            overlay_lock::is_overlay_locked,
            overlay_lock::set_lock_button_rect,
            is_capture_suspended,
            drag_resize::resize_window,
            tool_windows::list_monitors,
            tool_windows::open_details_window,
            tool_windows::close_details_window,
            tool_windows::request_details_view,
            tool_windows::take_pending_details_request,
            tool_windows::close_tool_window,
            tool_windows::open_settings_window,
            tool_windows::close_settings_window,
            tool_windows::tool_window_ready,
            tool_windows::details_window_ready,
            screenshots::capture_screenshot,
            screenshots::default_screenshot_folder,
            screenshots::choose_screenshot_folder,
            drag_resize::start_drag,
            drag_resize::move_overlay,
            drag_resize::end_overlay_drag,
            drag_resize::wayland_layer_state,
            drag_resize::start_tool_drag,
            drag_resize::begin_tool_resize,
            drag_resize::compositor_resize_supported,
            drag_resize::begin_window_resize,
            drag_resize::finish_window_resize,
            reset_auto_detection,
            get_available_devices,
            set_manual_device,
            replay::replay_file,
            test_auto_hide,
            commands::system::fetch_url,
            updater::show_update_window,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app, event| {
            // However the meter closes, the fight in progress is kept.
            if let tauri::RunEvent::Exit = event {
                save_fights_before_exit(app);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_csp_allows_every_inline_handler() {
        use sha2::{Digest, Sha256};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let read = |p: std::path::PathBuf| std::fs::read_to_string(p).unwrap();
        let conf: serde_json::Value =
            serde_json::from_str(&read(root.join("src-tauri/tauri.conf.json"))).unwrap();
        let mut pages = vec![read(root.join("index.html"))];
        // Every script, subfolders included.
        let mut dirs = vec![root.join("public/src/js")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() { dirs.push(path) } else { pages.push(read(path)) }
            }
        }
        // `onload="..."` and the like, in the page and in HTML the scripts build.
        let mut handlers = Vec::new();
        for page in &pages {
            let mut rest = page.as_str();
            while let Some(at) = rest.find(" on") {
                rest = &rest[at + 3..];
                let name = rest.bytes().take_while(|b| b.is_ascii_lowercase()).count();
                if name > 0 && rest[name..].starts_with("=\"") {
                    let body = &rest[name + 2..];
                    handlers.push(body[..body.find('"').unwrap()].to_string());
                }
            }
        }
        assert!(handlers.len() >= 5);
        for key in ["csp", "devCsp"] {
            let script_src = conf["app"]["security"][key]["script-src"].as_str().unwrap();
            for handler in &handlers {
                let hash = format!("'sha256-{}'", crate::share::base64(&Sha256::digest(handler.as_bytes())));
                assert!(script_src.contains(&hash), "{key} script-src has no {hash} for {handler}");
            }
        }
    }
}
