//! Packet capture: its status, device, suspend, the game window and debug status.

use crate::platform;

use crate::app::{capture_control, AppState};

#[tauri::command]
pub(crate) fn get_capture_status(state: tauri::State<'_, AppState>) -> serde_json::Value {
    capture_control::get_capture_status(&state)
}

#[tauri::command]
pub(crate) fn is_admin() -> bool {
    platform::admin::is_admin()
}

#[tauri::command]
pub(crate) fn reset_auto_detection(state: tauri::State<'_, AppState>) {
    capture_control::reset_auto_detection(&state);
}

#[tauri::command]
pub(crate) fn get_available_devices() -> Vec<String> {
    // Load the OS's pcap library and enumerate devices
    match crate::capture::pcap_capturer::list_device_labels() {
        Ok(labels) => labels,
        Err(_) => Vec::new(),
    }
}

#[tauri::command]
pub(crate) fn set_manual_device(state: tauri::State<'_, AppState>, device: String) {
    let dev = if device.trim().is_empty() { None } else { Some(device) };
    state.port_detector.set_preferred_device(dev);
}

#[tauri::command]
pub(crate) fn suspend_capture(state: tauri::State<'_, AppState>, suspended: bool) {
    // The header's suspend button. It was wired to empty stubs since the move
    // to Tauri, so it changed its icon and the status line but counting went
    // on (a player found it in 2.0.37, issue #6).
    state.capture_suspended.store(suspended, std::sync::atomic::Ordering::SeqCst);
    tracing::info!("Capture {}", if suspended { "suspended" } else { "resumed" });
}

#[tauri::command]
pub(crate) fn is_capture_suspended(state: tauri::State<'_, AppState>) -> bool {
    state.capture_suspended.load(std::sync::atomic::Ordering::SeqCst)
}

#[tauri::command]
pub(crate) fn get_aion2_window_title() -> Option<String> {
    platform::window_detector::find_aion2_window_title()
}

#[tauri::command]
pub(crate) fn test_auto_hide() -> serde_json::Value {
    let aion_fg = platform::window_detector::is_aion2_foreground();
    let aion_title = platform::window_detector::find_aion2_window_title();
    serde_json::json!({
        "aion2_foreground": aion_fg,
        "aion2_title": aion_title,
    })
}

#[tauri::command]
pub(crate) fn debug_status(state: tauri::State<'_, AppState>) -> serde_json::Value {
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
