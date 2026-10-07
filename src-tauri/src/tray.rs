//! The tray icon: show or hide the meter, lock or unlock it, open Settings or
//! History, quit.
//!
//! Two settings go with it, both off unless turned on: start with the meter
//! hidden in the tray, and keep the meter's windows out of the taskbar.

use std::collections::HashMap;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::app::AppState;

pub const START_IN_TRAY_KEY: &str = "dpsMeter.startInTray";
pub const HIDE_FROM_TASKBAR_KEY: &str = "dpsMeter.hideFromTaskbar";

/// A tray setting is on only when it was turned on.
pub fn is_on(value: Option<&str>) -> bool {
    value == Some("true")
}

fn setting(app: &AppHandle, key: &str) -> bool {
    app.try_state::<AppState>()
        .is_some_and(|state| is_on(state.settings.get(key).as_deref()))
}

/// Whether new meter windows should stay out of the taskbar.
pub fn taskbar_hidden(app: &AppHandle) -> bool {
    setting(app, HIDE_FROM_TASKBAR_KEY)
}

pub fn start_in_tray(app: &AppHandle) -> bool {
    setting(app, START_IN_TRAY_KEY)
}

/// Apply "keep out of the taskbar" to every open window.
pub fn apply_taskbar(app: &AppHandle) {
    let hidden = taskbar_hidden(app);
    for window in app.webview_windows().values() {
        let _ = window.set_skip_taskbar(hidden);
    }
}

/// Where the meter was when the tray hid it. GTK forgets a window's place
/// when it is hidden, and X11 window managers (GNOME's XWayland, Xfce,
/// Cinnamon) then put it somewhere else. A layer overlay keeps its margins.
static HIDDEN_AT: parking_lot::Mutex<Option<tauri::PhysicalPosition<i32>>> = parking_lot::Mutex::new(None);

/// Show the meter if it is hidden, hide it if it is shown.
pub fn toggle_meter(app: &AppHandle) {
    let Some(main) = app.get_webview_window("main") else { return };
    if main.is_visible().unwrap_or(false) {
        let normal = !crate::platform::window::is_layer(&main);
        // Not a minimized place (Windows reports -32000,-32000).
        *HIDDEN_AT.lock() = main.outer_position().ok().filter(|p| normal && p.x > -10000 && p.y > -10000);
        let _ = main.hide();
    } else {
        // Before the show for the window manager's placement, and after it
        // for a window manager that placed it anyway.
        let place = HIDDEN_AT.lock().take();
        if let Some(pos) = place {
            let _ = main.set_position(pos);
        }
        let _ = main.show();
        if let Some(pos) = place {
            let _ = main.set_position(pos);
        }
        let _ = main.unminimize();
        let _ = main.set_focus();
    }
}

/// Add the tray icon. Returns false when the desktop has nowhere to show one,
/// in which case nothing may be hidden into it.
pub fn create(app: &AppHandle) -> bool {
    if !crate::platform::window::tray_available() {
        tracing::warn!("No tray icon: no tray library is installed (libayatana-appindicator)");
        return false;
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(app))) {
        Ok(Ok(())) => true,
        Ok(Err(e)) => {
            tracing::warn!("No tray icon: {e}");
            false
        }
        Err(_) => {
            tracing::warn!("No tray icon: the tray library failed to load");
            false
        }
    }
}

/// The menu's words in the meter's language, English where a text is missing.
fn menu_texts(app: &AppHandle) -> HashMap<String, String> {
    let Some(state) = app.try_state::<AppState>() else { return HashMap::new() };
    let lang = state.settings.get("dpsMeter.language").unwrap_or_else(|| "en".into());
    let read = |l: &str| -> Option<serde_json::Value> {
        let path = state.i18n_data_dir.as_ref()?.join("ui").join(format!("{l}.json"));
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    };
    let section = |doc: Option<serde_json::Value>| -> HashMap<String, String> {
        doc.and_then(|d| d.get("tray").and_then(|v| v.as_object()).cloned())
            .map(|o| o.into_iter().filter_map(|(k, v)| Some((k, v.as_str()?.to_string()))).collect())
            .unwrap_or_default()
    };
    let mut texts = section(read("en"));
    texts.extend(section(read(&lang)));
    texts
}

fn build(app: &AppHandle) -> tauri::Result<()> {
    let texts = menu_texts(app);
    let text = |key: &str, fallback: &str| texts.get(key).cloned().unwrap_or_else(|| fallback.to_string());
    // On Linux the tray sends no click events, so this menu is the only way
    // to the meter there: showing it comes first.
    let toggle = MenuItem::with_id(app, "toggle", text("toggle", "Show or hide the meter"), true, None::<&str>)?;
    // Where the desktop has no lock hotkey (Sway), this is the other way to
    // the click-through lock.
    let lock = MenuItem::with_id(app, "lock", text("lock", "Lock or unlock the meter"), true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", text("settings", "Settings"), true, None::<&str>)?;
    let history = MenuItem::with_id(app, "history", text("history", "History"), true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", text("quit", "Quit"), true, None::<&str>)?;
    let menu = if crate::app::overlay_lock_available() {
        Menu::with_items(app, &[&toggle, &lock, &settings, &history, &separator, &quit])?
    } else {
        Menu::with_items(app, &[&toggle, &settings, &history, &separator, &quit])?
    };

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Daevalog DPS Meter")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle" => toggle_meter(app),
            "lock" => crate::app::toggle_overlay_lock(app),
            "settings" => crate::app::open_settings_from_tray(app),
            "history" => crate::app::open_history_from_tray(app),
            "quit" => crate::app::quit_from_tray(app),
            _ => {}
        })
        // A left click toggles the meter where the OS reports clicks (not on
        // Linux).
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_meter(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_settings_are_off_unless_turned_on() {
        assert!(!is_on(None));
        assert!(!is_on(Some("")));
        assert!(!is_on(Some("false")));
        assert!(!is_on(Some("1")));
        assert!(is_on(Some("true")));
    }
}
