//! Starting the meter: state, windows, capture, hotkeys and the background tasks.

// Brought into scope as modules because this file was split out of lib.rs,
// where these were siblings at the crate root.
use crate::{entity, i18n, logging, platform};

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
use crate::combat::dps_calculator::DpsCalculator;
use crate::combat::ping_tracker::PingTracker;
use crate::config::settings::Settings;
use crate::history::fight_history::FightHistoryManager;
use crate::i18n::lookup::{NpcLookup, SkillLookup};

use super::overlay_lock::{toggle_overlay_lock, OverlayLock};
use super::setting_changes::{apply_encounter_timeout, ENCOUNTER_TIMEOUT_KEY};
use super::tool_windows::open_details_on_monitor;
use super::tray_actions::{flush_settings_before_exit, quit, save_fights_before_exit};
use super::{commands, drag_resize, overlay_lock, screenshots, supporter_roster, tasks, tool_windows, AppState};

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
        .on_window_event(|window, event| {
            // Closing the overlay quits, even with another window still open.
            if let (tauri::WindowEvent::CloseRequested { api, .. }, "main") = (event, window.label()) {
                api.prevent_close();
                quit(window.app_handle());
            }
        })
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

            // One instance: it owns the writer of settings.json.
            let settings = Settings::new(app_data_dir.clone());
            if let Some(ref data_dir) = found_data_dir {
                // Load DOT skill IDs (language-independent)
                if let Ok(text) = std::fs::read_to_string(data_dir.join("dot_skill_ids.json")) {
                    if let Ok(ids) = serde_json::from_str::<Vec<i32>>(&text) {
                        for id in ids { dot_ids.insert(id); }
                        tracing::info!("Loaded {} DOT skill IDs", dot_ids.len());
                    }
                }

                // Load skill/NPC data in the user's language
                let language = settings.get("dpsMeter.language")
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
                details: dps_calculator.details_source(),
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
                let layer = platform::window::init_overlay_layer(&window, platform::process::overlay_layer(), logical);
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

            register_hotkeys(app.handle());

            tasks::spawn_meter_tick(app.handle());

            tasks::spawn_auto_save(app.handle());

            supporter_roster::spawn_roster_poll(app.handle());

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
            commands::account::account_status,
            commands::account::account_status_cached,
            commands::settings::discord_activity_available,
            commands::account::account_begin_link,
            commands::account::account_sign_out,
            commands::settings::get_settings,
            commands::settings::update_settings,
            commands::meter::get_ping,
            commands::capture::get_capture_status,
            commands::meter::set_target_mode,
            commands::meter::set_all_targets_window_ms,
            commands::identity::set_character_name,
            commands::identity::bind_local_actor_id,
            commands::identity::bind_local_nickname,
            commands::settings::clear_settings,
            commands::meter::reset_combat,
            commands::capture::is_admin,
            commands::settings::set_language,
            commands::settings::set_debug_logging,
            commands::settings::set_packet_logging,
            commands::share::send_logs_to_dev,
            commands::capture::get_aion2_window_title,
            commands::capture::debug_status,
            commands::system::quit_app,
            commands::system::open_url,
            commands::system::read_cached_icon,
            commands::system::write_cached_icon,
            commands::system::log_from_ui,
            commands::capture::suspend_capture,
            overlay_lock::overlay_lock_supported,
            overlay_lock::set_overlay_locked,
            overlay_lock::is_overlay_locked,
            overlay_lock::set_lock_button_rect,
            commands::capture::is_capture_suspended,
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
            commands::capture::reset_auto_detection,
            commands::capture::get_available_devices,
            commands::capture::set_manual_device,
            commands::capture::replay_file,
            commands::capture::test_auto_hide,
            commands::system::fetch_url,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app, event| {
            // However the meter closes, the fight in progress is kept.
            if let tauri::RunEvent::Exit = event {
                save_fights_before_exit(app);
                flush_settings_before_exit(app);
            }
        });
}

fn register_hotkeys(app: &tauri::AppHandle) {
    // Register global hotkeys from saved settings (or defaults)
    let hotkey_handle = app.clone();
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
                    super::meter::reset_combat(&state);
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
}
