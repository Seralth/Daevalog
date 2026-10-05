//! Packet capture: its status, device, suspend, the game window, debug status and replays.

use crate::platform;

use crate::app::{capture_control, replay, AppState};

#[tauri::command]
pub(crate) fn get_capture_status(state: tauri::State<'_, AppState>) -> serde_json::Value {
    capture_control::get_capture_status(&state)
}

#[tauri::command]
pub(crate) fn is_admin() -> bool {
    crate::capture::live::can_capture()
}

#[tauri::command]
pub(crate) fn reset_auto_detection(state: tauri::State<'_, AppState>) {
    capture_control::reset_auto_detection(&state);
}

#[tauri::command]
pub(crate) async fn get_available_devices() -> Vec<String> {
    capture_control::get_available_devices().await
}

#[tauri::command]
pub(crate) fn set_manual_device(state: tauri::State<'_, AppState>, device: String) {
    capture_control::set_manual_device(&state, device);
}

#[tauri::command]
pub(crate) fn suspend_capture(state: tauri::State<'_, AppState>, suspended: bool) {
    capture_control::suspend_capture(&state, suspended);
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
pub(crate) fn debug_status(state: tauri::State<'_, AppState>) -> serde_json::Value {
    capture_control::debug_status(&state)
}

#[tauri::command]
pub(crate) async fn replay_file(app: tauri::AppHandle, file_path: String) -> Result<String, String> {
    replay::replay_file(app, file_path).await
}
