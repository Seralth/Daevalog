//! Capture status and control, for the capture commands.

use crate::platform;

use super::AppState;

pub(crate) fn get_capture_status(state: &AppState) -> serde_json::Value {
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

pub(crate) fn reset_auto_detection(state: &AppState) {
    state.port_detector.reset();
    state.ping_tracker.reset();
}

pub(crate) async fn get_available_devices() -> Vec<String> {
    // Load the OS's pcap library and enumerate devices. That can block in the
    // library, so not on the main thread.
    tauri::async_runtime::spawn_blocking(|| {
        crate::capture::live::list_device_labels().unwrap_or_default()
    }).await.unwrap_or_default()
}

pub(crate) fn set_manual_device(state: &AppState, device: String) {
    let dev = if device.trim().is_empty() { None } else { Some(device) };
    state.port_detector.set_preferred_device(dev);
}

pub(crate) fn suspend_capture(state: &AppState, suspended: bool) {
    // The header's suspend button. It was wired to empty stubs since the move
    // to Tauri, so it changed its icon and the status line but counting went
    // on (a player found it in 2.0.37, issue #6).
    state.capture_suspended.store(suspended, std::sync::atomic::Ordering::SeqCst);
    tracing::info!("Capture {}", if suspended { "suspended" } else { "resumed" });
}

pub(crate) fn test_auto_hide() -> serde_json::Value {
    let aion_fg = platform::window_detector::is_aion2_foreground();
    let aion_title = platform::window_detector::find_aion2_window_title();
    serde_json::json!({
        "aion2_foreground": aion_fg,
        "aion2_title": aion_title,
    })
}

pub(crate) fn debug_status(state: &AppState) -> serde_json::Value {
    let port = state.port_detector.current_port();
    let device = state.port_detector.current_device();
    let ping = state.ping_tracker.current_ping_ms();
    let dmg_gen = state.data_storage.damage_generation();
    let window = platform::window_detector::find_aion2_window_title();
    let admin = crate::capture::live::can_capture();
    serde_json::json!({
        "port": port,
        "device": device,
        "ping": ping,
        "damageGeneration": dmg_gen,
        "aion2Window": window,
        "isAdmin": admin,
    })
}
