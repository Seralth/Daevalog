//! The tray icon: show or hide the meter, open Settings or History, quit.
//!
//! Two settings go with it, both off unless turned on: start with the meter
//! hidden in the tray, and keep the meter's windows out of the taskbar.

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

/// Show the meter if it is hidden, hide it if it is shown.
pub fn toggle_meter(app: &AppHandle) {
    let Some(main) = app.get_webview_window("main") else { return };
    if main.is_visible().unwrap_or(false) {
        let _ = main.hide();
    } else {
        let _ = main.show();
        let _ = main.unminimize();
        let _ = main.set_focus();
    }
}

/// Add the tray icon. Returns false when the desktop has nowhere to show one,
/// in which case nothing may be hidden into it.
pub fn create(app: &AppHandle) -> bool {
    match build(app) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("No tray icon: {e}");
            false
        }
    }
}

fn build(app: &AppHandle) -> tauri::Result<()> {
    let toggle = MenuItem::with_id(app, "toggle", "Show or hide the meter", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let history = MenuItem::with_id(app, "history", "History", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&toggle, &settings, &history, &separator, &quit])?;

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Daevalog DPS Meter")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle" => toggle_meter(app),
            "settings" => crate::app::open_settings_from_tray(app),
            "history" => crate::app::open_history_from_tray(app),
            "quit" => crate::app::quit_from_tray(app),
            _ => {}
        })
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
