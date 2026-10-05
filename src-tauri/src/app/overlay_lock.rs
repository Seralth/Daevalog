//! The overlay's click-through lock, and the commands that drive it.

use std::time::Duration;

use parking_lot::Mutex;
use tauri::{Emitter, Manager};

use crate::platform;

use super::AppState;

/// The overlay's click-through lock: while locked, clicks go through the meter
/// to the game, and it cannot be dragged. Only its own lock button stays
/// clickable, so the same button unlocks it; a hotkey does too.
///
/// Where the platform supports it (Linux, X11 and Wayland), the window's
/// input region shrinks to the lock button: clicks anywhere else go through.
/// Elsewhere the button is kept clickable by watching the pointer: while it
/// is over the button the window takes the mouse again. That needs the
/// pointer's position outside the window (`platform::window::cursor_position`).
#[derive(Default)]
pub struct OverlayLock {
    pub(super) locked: std::sync::atomic::AtomicBool,
    /// The lock button in the main window's page: CSS-pixel x, y, width,
    /// height, and the page's scale. Sent by the page when it lays out.
    button: Mutex<Option<(f64, f64, f64, f64, f64)>>,
    /// Whether the pointer watch is running.
    watching: std::sync::atomic::AtomicBool,
    /// What the window was last told (`true` = ignore the mouse). Every change
    /// goes through `sync_click_through` under this lock, so a late pointer
    /// check can never leave an unlocked window click-through.
    applied: Mutex<Option<bool>>,
}

/// Tell the main window whether to ignore the mouse: when locked, unless the
/// pointer is over the lock button.
fn sync_click_through(app: &tauri::AppHandle, lock: &OverlayLock, over_button: bool) {
    use std::sync::atomic::Ordering;
    let mut applied = lock.applied.lock();
    let ignore = lock.locked.load(Ordering::SeqCst) && !over_button;
    if *applied == Some(ignore) {
        return;
    }
    if let Some(main) = app.get_webview_window("main") {
        if main.set_ignore_cursor_events(ignore).is_ok() {
            *applied = Some(ignore);
        }
    }
}

fn pointer_over_lock_button(app: &tauri::AppHandle, lock: &OverlayLock) -> bool {
    let Some((px, py)) = platform::window::cursor_position() else { return false };
    let Some((x, y, w, h, scale)) = *lock.button.lock() else { return false };
    let Some(origin) = app.get_webview_window("main").and_then(|m| m.inner_position().ok()) else {
        return false;
    };
    let (left, top) = (origin.x as f64 + x * scale, origin.y as f64 + y * scale);
    let (px, py) = (px as f64, py as f64);
    px >= left && px < left + w * scale && py >= top && py < top + h * scale
}

/// Whether the click-through lock can work here.
pub(crate) fn overlay_lock_available() -> bool {
    platform::window::input_region_supported() || platform::window::cursor_position().is_some()
}

/// The part of the window that takes the mouse: all of it unlocked, the lock
/// button locked. With no button placed yet, all of it, so a lock can never
/// leave the overlay with no way back.
fn lock_input_rect(locked: bool, button: Option<(f64, f64, f64, f64, f64)>) -> Option<(f64, f64, f64, f64, f64)> {
    if locked { button } else { None }
}

fn apply_lock_region(app: &tauri::AppHandle, lock: &OverlayLock) {
    use std::sync::atomic::Ordering;
    let Some(main) = app.get_webview_window("main") else { return };
    let locked = lock.locked.load(Ordering::SeqCst);
    let button = *lock.button.lock();
    if locked && button.is_none() {
        tracing::warn!("Overlay locked before its lock button was placed; it keeps the mouse");
    }
    platform::window::set_input_region(&main, lock_input_rect(locked, button));
}

/// Lock or unlock the overlay. Without an input region, locking starts the
/// pointer watch, which ends by itself once unlocked.
fn apply_overlay_lock(app: &tauri::AppHandle, locked: bool) {
    use std::sync::atomic::Ordering;
    let Some(state) = app.try_state::<AppState>() else { return };
    let lock = state.overlay_lock.clone();
    lock.locked.store(locked, Ordering::SeqCst);
    if platform::window::input_region_supported() {
        apply_lock_region(app, &lock);
        tracing::info!("Overlay {}", if locked { "locked (click-through)" } else { "unlocked" });
        return;
    }
    sync_click_through(app, &lock, false);
    tracing::info!("Overlay {}", if locked { "locked (click-through)" } else { "unlocked" });
    if locked && !lock.watching.swap(true, Ordering::SeqCst) {
        let app = app.clone();
        std::thread::spawn(move || {
            while lock.locked.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(50));
                let over = pointer_over_lock_button(&app, &lock);
                sync_click_through(&app, &lock, over);
            }
            sync_click_through(&app, &lock, false);
            lock.watching.store(false, Ordering::SeqCst);
        });
    }
}

/// Lock the overlay if it is unlocked and the other way round, and tell the
/// page so its button and saved setting follow. For the hotkey and the tray.
pub fn toggle_overlay_lock(app: &tauri::AppHandle) {
    let locked = app
        .try_state::<AppState>()
        .is_some_and(|s| s.overlay_lock.locked.load(std::sync::atomic::Ordering::SeqCst));
    if !overlay_lock_available() && !locked {
        return; // the lock is not offered here
    }
    apply_overlay_lock(app, !locked);
    let _ = app.emit("overlay-lock-changed", !locked);
}

/// Whether the click-through lock can work here (see `OverlayLock`).
#[tauri::command]
pub(super) fn overlay_lock_supported() -> bool {
    overlay_lock_available()
}

#[tauri::command]
pub(super) fn set_overlay_locked(app: tauri::AppHandle, locked: bool) {
    apply_overlay_lock(&app, locked);
}

/// Where the lock button is in the main window's page, so it stays clickable
/// while the rest of the window lets clicks through. A locked overlay follows
/// the button when the layout moves it.
#[tauri::command]
pub(super) fn set_lock_button_rect(app: tauri::AppHandle, state: tauri::State<'_, AppState>, x: f64, y: f64, width: f64, height: f64, scale: f64) {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    *state.overlay_lock.button.lock() = Some((x, y, width, height, scale));
    if platform::window::input_region_supported()
        && state.overlay_lock.locked.load(std::sync::atomic::Ordering::SeqCst)
    {
        apply_lock_region(&app, &state.overlay_lock);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_locked_overlay_takes_the_mouse_only_on_its_button() {
        let button = Some((300.0, 4.0, 24.0, 24.0, 1.0));
        assert_eq!(lock_input_rect(true, button), button);
        assert_eq!(lock_input_rect(false, button), None, "unlocked: the whole window");
        assert_eq!(lock_input_rect(true, None), None, "no button placed: never a dead overlay");
    }
}
