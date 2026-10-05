//! Screenshots of the meter and the tool windows.

use tauri::Manager;

use crate::platform;

/// What a screenshot achieved: on the clipboard, and the file it was saved to.
#[derive(serde::Serialize)]
pub(super) struct ScreenshotResult {
    clipboard: bool,
    file: Option<String>,
}

/// Capture part of the calling window: `x`/`y`/`width`/`height` are the page's
/// CSS pixels and `scale` its `devicePixelRatio`. Measured against the window
/// that asked, so the Details window captures itself rather than whatever sits
/// at the same offset from the meter. `include_meter` adds the whole meter
/// window, for a tool window that cannot measure the meter itself. With
/// `save_file`, also writes a PNG to `folder` (default: Pictures\Daevalog DPS
/// Meter) named `filename`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(super) async fn capture_screenshot(
    app: tauri::AppHandle,
    webview_window: tauri::WebviewWindow,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    scale: Option<f64>,
    include_meter: Option<bool>,
    save_file: Option<bool>,
    folder: Option<String>,
    filename: Option<String>,
) -> ScreenshotResult {
    let scale = scale.unwrap_or_else(|| webview_window.scale_factor().unwrap_or(1.0));
    let meter = include_meter
        .unwrap_or(false)
        .then(|| app.get_webview_window("main"))
        .flatten()
        .filter(|main| main.label() != webview_window.label());
    let path = save_file.unwrap_or(false).then(|| {
        let dir = folder
            .filter(|f| !f.trim().is_empty())
            .map(std::path::PathBuf::from)
            .or_else(platform::screenshot::default_folder)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let name = filename
            .filter(|n| !n.trim().is_empty() && !n.contains(['/', '\\']))
            .unwrap_or_else(|| format!("AION2_DPS_{}.png", chrono::Local::now().format("%Y%m%d_%H%M%S")));
        dir.join(name)
    });
    tauri::async_runtime::spawn_blocking(move || {
        let (clipboard, file_ok) = platform::screenshot::capture(
            &webview_window, x, y, width, height, scale, meter.as_ref(), path.as_deref(),
        );
        if path.is_some() && !file_ok {
            tracing::warn!("Screenshot not saved to {:?}", path);
        }
        ScreenshotResult {
            clipboard,
            file: path.filter(|_| file_ok).map(|p| p.display().to_string()),
        }
    })
    .await
    .unwrap_or(ScreenshotResult { clipboard: false, file: None })
}

/// Where screenshots go when no folder has been chosen.
#[tauri::command]
pub(super) fn default_screenshot_folder() -> String {
    platform::screenshot::default_folder()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

/// Let the player pick the screenshot folder. `None` if they cancel.
#[tauri::command]
pub(super) async fn choose_screenshot_folder(
    webview_window: tauri::WebviewWindow,
    current: Option<String>,
) -> Option<String> {
    platform::screenshot::pick_folder(&webview_window, current.as_deref())
}
