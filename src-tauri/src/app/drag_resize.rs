//! Dragging and resizing the overlay and the tool windows.

use std::time::Duration;

use tauri::Manager;

use crate::platform;

use super::AppState;

#[tauri::command]
pub(super) fn resize_window(app: tauri::AppHandle, width: f64, height: f64, scale: Option<f64>) {
    // Only the overlay auto-sizes itself. The details window is sized to a
    // whole monitor by open_details_window and must never be resized from JS.
    let Some(window) = app.get_webview_window("main") else { return };
    // `width`/`height` are the page's CSS pixels and `scale` its
    // devicePixelRatio. WebView2 draws a CSS pixel at the display scale TIMES
    // Windows' Accessibility "Text size", while a logical size here covers the
    // display scale only, so with Text size above 100% the meter outgrew its
    // window and was cut off. The page's own ratio covers both.
    let size = match scale.filter(|s| s.is_finite() && *s > 0.0) {
        Some(scale) => tauri::Size::Physical(tauri::PhysicalSize {
            width: (width * scale).ceil() as u32,
            height: (height * scale).ceil() as u32,
        }),
        None => tauri::Size::Logical(tauri::LogicalSize { width, height }),
    };
    platform::window::set_size(&window, size);
}

/// Returns the overlay's place in logical pixels when it is a Wayland layer
/// surface: it has no compositor move, so the page drags it itself with
/// `move_overlay`. The third value says whether pointer events in the drag
/// measure from that first place (Sway) rather than the current one.
#[tauri::command]
pub(super) fn start_drag(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Option<(i32, i32, bool)> {
    // A locked overlay stays where it is.
    if state.overlay_lock.locked.load(std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    let window = app.get_webview_window("main")?;
    if platform::window::is_layer(&window) {
        let from_start = platform::window::begin_layer_drag(&window);
        let (x, y) = platform::window::overlay_layer_position(&window)?;
        return Some((x, y, from_start));
    }
    platform::window::start_drag(&window);
    None
}

/// Put the layer-surface overlay at `x`, `y` (logical pixels) during a drag.
/// Returns where it went.
#[tauri::command]
pub(super) fn move_overlay(app: tauri::AppHandle, state: tauri::State<'_, AppState>, x: i32, y: i32) -> Option<(i32, i32)> {
    if state.overlay_lock.locked.load(std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    platform::window::place_overlay_layer(&app.get_webview_window("main")?, x, y)
}

/// The page saw the end of a layer-overlay drag.
#[tauri::command]
pub(super) fn end_overlay_drag(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        platform::window::end_layer_drag(&window);
    }
}

/// Whether the Wayland layer setting can work here, whether the overlay is a
/// layer surface now, and whether the setting is on until the player sets it.
#[tauri::command]
pub(super) fn wayland_layer_state(app: tauri::AppHandle) -> serde_json::Value {
    let active = app.get_webview_window("main").is_some_and(|w| platform::window::is_layer(&w));
    serde_json::json!({
        "supported": platform::window::layer_supported(),
        "active": active,
        "byDefault": platform::process::overlay_layer_by_default(),
    })
}

/// Drag a tool window (Details, History, Settings) by its header. Their CSS
/// marks the header `-webkit-app-region: drag`, which WebView2 honours and
/// WebKitGTK does not, so on Linux the page asks for the drag instead.
#[tauri::command]
pub(super) fn start_tool_drag(window: tauri::WebviewWindow) {
    if window.label() == "main" {
        return;
    }
    platform::window::start_drag(&window);
}

/// Whether this backend supports compositor-driven window resizing.
#[tauri::command]
pub(super) fn compositor_resize_supported(window: tauri::WebviewWindow) -> bool {
    platform::window::compositor_resize_supported(&window)
}

/// Unpin the window before a compositor resize gesture.
#[tauri::command]
pub(super) async fn begin_window_resize(
    window: tauri::WebviewWindow,
    min_width: f64,
    min_height: f64,
    scale: f64,
) -> Result<bool, String> {
    if !platform::window::compositor_resize_supported(&window) {
        return Err("Compositor resize is unavailable".into());
    }
    if ![min_width, min_height, scale].iter().all(|v| v.is_finite() && *v > 0.0) {
        return Err("Invalid resize dimensions".into());
    }
    let display_scale = window.scale_factor().map_err(|e| e.to_string())?;
    platform::window::prepare_resize(&window, tauri::LogicalSize::new(
        min_width * scale / display_scale,
        min_height * scale / display_scale,
    )).await?;
    Ok(platform::window::resize_pointer_down(&window) == Some(true))
}

#[tauri::command]
pub(super) fn finish_window_resize(window: tauri::WebviewWindow, cancel: bool) -> Result<bool, String> {
    if platform::window::compositor_resize_supported(&window) {
        if !cancel && platform::window::resize_pointer_down(&window) != Some(false) {
            return Ok(false);
        }
        let size = window.inner_size().map_err(|e| e.to_string())?;
        platform::window::set_size(&window, tauri::Size::Physical(size));
    }
    Ok(true)
}

/// Compatibility path for backends without compositor resize support.
#[tauri::command]
pub(super) fn begin_tool_resize(window: tauri::WebviewWindow, min_width: f64, min_height: f64) {
    if window.label() == "main" {
        return;
    }
    platform::window::release_size(&window, tauri::LogicalSize::new(min_width, min_height));
    std::thread::spawn(move || {
        // Wait for release; use stable size only if the X11 pointer query fails.
        std::thread::sleep(Duration::from_millis(150));
        let mut last = window.inner_size().ok();
        let mut still = 0;
        for _ in 0..1200 {
            std::thread::sleep(Duration::from_millis(50));
            match platform::window::primary_button_down() {
                Some(true) => continue,
                Some(false) => break,
                None => {
                    let now = window.inner_size().ok();
                    still = if now == last { still + 1 } else { 0 };
                    last = now;
                    if still >= 10 {
                        break;
                    }
                }
            }
        }
        if let Ok(size) = window.inner_size() {
            platform::window::set_size(&window, tauri::Size::Physical(size));
        }
    });
}
