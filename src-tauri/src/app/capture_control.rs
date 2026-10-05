//! Capture status and control, for the capture commands.

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
