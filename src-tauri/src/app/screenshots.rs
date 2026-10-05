//! Screenshots of the meter and the tool windows.

use std::path::{Path, PathBuf};

use tauri::Manager;

use crate::platform;

use super::AppState;

/// What a screenshot achieved: on the clipboard, and the file it was saved to.
#[derive(serde::Serialize)]
pub(super) struct ScreenshotResult {
    clipboard: bool,
    file: Option<String>,
}

/// The folder chosen with the meter's own picker, kept where the page
/// cannot write it.
pub(super) const CHOSEN_FOLDER_KEY: &str = "backend.screenshotFolder";

/// Where a screenshot is saved: the folder the player chose with the meter's
/// own picker when the page asks for that one, else the default folder. A
/// page cannot name any other folder. The name is the page's, made a plain
/// `.png` file name.
fn screenshot_path(asked: Option<&str>, chosen: Option<&Path>, default: Option<PathBuf>, name: Option<&str>, stamp: &str) -> Option<PathBuf> {
    let asked = asked.map(str::trim).filter(|f| !f.is_empty()).map(Path::new);
    let dir = match (asked, chosen) {
        (Some(asked), Some(chosen)) if asked == chosen => chosen.to_path_buf(),
        (Some(asked), _) if Some(asked) != default.as_deref() => {
            tracing::warn!("Screenshot: {} was not chosen in the meter; saving to the default folder", asked.display());
            default?
        }
        _ => default?,
    };
    Some(dir.join(png_name(name, stamp)))
}

/// A file name of letters, digits, spaces and `-_.()`, ending in `.png`.
fn png_name(name: Option<&str>, stamp: &str) -> String {
    let name = name.unwrap_or("").trim();
    let cut = name.len().saturating_sub(4);
    let stem = match name.get(cut..) {
        Some(end) if end.eq_ignore_ascii_case(".png") => &name[..cut],
        _ => name,
    };
    let clean: String = stem
        .chars()
        .map(|c| if c.is_alphanumeric() || " -_.()".contains(c) { c } else { '_' })
        .take(120)
        .collect();
    let clean = clean.trim_matches(|c: char| c == '.' || c == ' ');
    if clean.is_empty() {
        format!("AION2_DPS_{stamp}.png")
    } else {
        format!("{clean}.png")
    }
}

/// Capture part of the calling window: `x`/`y`/`width`/`height` are the page's
/// CSS pixels and `scale` its `devicePixelRatio`. Measured against the window
/// that asked, so the Details window captures itself rather than whatever sits
/// at the same offset from the meter. `include_meter` adds the whole meter
/// window, for a tool window that cannot measure the meter itself. With
/// `save_file`, also writes a PNG named after `filename` to `folder` when that
/// is the folder chosen in the meter (else to Pictures\Daevalog DPS Meter),
/// never over an existing file.
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
        let chosen = app.state::<AppState>().screenshot_folder.lock().clone();
        let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
        screenshot_path(folder.as_deref(), chosen.as_deref(), platform::screenshot::default_folder(), filename.as_deref(), &stamp)
    }).flatten();
    tauri::async_runtime::spawn_blocking(move || {
        let (clipboard, file) = platform::screenshot::capture(
            &webview_window, x, y, width, height, scale, meter.as_ref(), path.as_deref(),
        );
        if path.is_some() && file.is_none() {
            tracing::warn!("Screenshot not saved to {:?}", path);
        }
        ScreenshotResult {
            clipboard,
            file: file.map(|p| p.display().to_string()),
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

/// Let the player pick the screenshot folder. `None` if they cancel. The
/// choice is kept here: screenshots go only to this folder or the default.
#[tauri::command]
pub(super) async fn choose_screenshot_folder(
    app: tauri::AppHandle,
    webview_window: tauri::WebviewWindow,
    current: Option<String>,
) -> Option<String> {
    let chosen = platform::screenshot::pick_folder(&webview_window, current.as_deref())?;
    let state = app.state::<AppState>();
    *state.screenshot_folder.lock() = Some(PathBuf::from(&chosen));
    state.settings.set(CHOSEN_FOLDER_KEY, &chosen);
    Some(chosen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshots_go_only_to_the_chosen_or_the_default_folder() {
        let chosen = Path::new("/home/p/Shots");
        let default = || Some(PathBuf::from("/home/p/Pictures/Daevalog DPS Meter"));
        let path = |asked, chosen, name| screenshot_path(asked, chosen, default(), name, "20261005_120000");
        assert_eq!(path(Some("/home/p/Shots"), Some(chosen), Some("a.png")), Some(PathBuf::from("/home/p/Shots/a.png")));
        // A folder the picker did not return goes to the default.
        assert_eq!(path(Some("/home/p/.config/autostart"), Some(chosen), Some("a.png")), Some(default().unwrap().join("a.png")));
        assert_eq!(path(Some("/home/p/Shots"), None, Some("a.png")), Some(default().unwrap().join("a.png")));
        assert_eq!(path(None, Some(chosen), None), Some(default().unwrap().join("AION2_DPS_20261005_120000.png")));
        assert_eq!(screenshot_path(Some("/tmp"), None, None, Some("a"), "x"), None, "no default folder: no file");
    }

    #[test]
    fn the_file_name_is_a_plain_png_name() {
        let name = |n| png_name(Some(n), "S");
        assert_eq!(name("AION2_DPS_Boss (2).png"), "AION2_DPS_Boss (2).png");
        assert_eq!(name("../../.bashrc"), "_.._.bashrc.png");
        assert_eq!(name("a/b\\c:d"), "a_b_c_d.png");
        assert_eq!(name("shot.PNG"), "shot.png");
        assert_eq!(name("x.desktop"), "x.desktop.png");
        assert_eq!(name("..."), "AION2_DPS_S.png");
        assert_eq!(name("보스"), "보스.png");
        assert_eq!(png_name(None, "S"), "AION2_DPS_S.png");
    }
}
