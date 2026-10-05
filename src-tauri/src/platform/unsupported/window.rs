//! Tauri's own cross-platform equivalents, which is the best available here.

/// Not available: on Wayland an app cannot read the pointer outside its own
/// windows, so the click-through lock (which needs it) is not offered.
pub fn cursor_position() -> Option<(i32, i32)> {
    None
}

pub fn start_drag(window: &tauri::WebviewWindow) {
    let _ = window.start_dragging();
}

pub fn show_on_top_without_focus(window: &tauri::WebviewWindow) {
    let _ = window.show();
    let _ = window.set_always_on_top(true);
}

pub fn minimize_off_top(window: &tauri::WebviewWindow) {
    let _ = window.set_always_on_top(false);
    let _ = window.minimize();
}

pub fn set_size(window: &tauri::WebviewWindow, size: tauri::Size) {
    let _ = window.set_size(size);
}

pub fn release_size(_window: &tauri::WebviewWindow, _min: tauri::LogicalSize<f64>) {}

pub fn primary_button_down() -> Option<bool> {
    None
}

pub fn compositor_resize_supported(_window: &tauri::WebviewWindow) -> bool {
    false
}

pub fn resize_pointer_down(_window: &tauri::WebviewWindow) -> Option<bool> {
    None
}

pub async fn prepare_resize(_window: &tauri::WebviewWindow, _min: tauri::LogicalSize<f64>) -> Result<(), String> {
    Err("Compositor resize is unavailable".into())
}

/// Whether a tray icon can be built here. Nothing to check on this OS.
pub fn tray_available() -> bool {
    true
}

/// The click-through lock does not use an input region here.
pub fn input_region_supported() -> bool {
    false
}

pub fn set_input_region(_window: &tauri::WebviewWindow, _rect: Option<(f64, f64, f64, f64, f64)>) -> bool {
    false
}

/// No Wayland layer surfaces on this platform: the overlay is a normal window.
pub fn init_overlay_layer(_window: &tauri::WebviewWindow, _enabled: bool, _pos: (i32, i32)) -> bool {
    false
}

pub fn is_layer(_window: &tauri::WebviewWindow) -> bool {
    false
}

pub fn place_overlay_layer(_window: &tauri::WebviewWindow, _x: i32, _y: i32) -> Option<(i32, i32)> {
    None
}

pub fn begin_layer_drag(_window: &tauri::WebviewWindow) -> bool {
    false
}

pub fn end_layer_drag(_window: &tauri::WebviewWindow) {}

pub fn layer_supported() -> bool {
    false
}

pub fn overlay_layer_position(_window: &tauri::WebviewWindow) -> Option<(i32, i32)> {
    None
}
