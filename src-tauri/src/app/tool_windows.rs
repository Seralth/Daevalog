//! The tool windows: Details, fight, History and Settings, and where they open.

use std::time::Duration;

use tauri::{Emitter, Manager};

use crate::platform;

use super::AppState;

/// Monitor the Details window was last placed on. Recorded here rather than
/// passed back from JS so the confirmation cannot disagree with the placement.
static DETAILS_MONITOR: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(usize::MAX);

/// Requests waiting for the window that will serve them, keyed by window label.
/// A webview that has just been built has no event listener attached yet, so a
/// push would be dropped; each window pulls its own entry on startup instead
/// (see `take_pending_details_request`). Keyed rather than a single slot
/// because several fight windows can be opening at once, and one clobbering
/// another's request would leave a blank window.
static PENDING_DETAILS_REQUEST: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Stamped onto every request so the Details window can ignore one it has
/// already applied — the pull and the push can both carry the same request.
static DETAILS_REQUEST_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Displays as reported by the OS, for the "Show Details on Monitor" picker.
/// Positions and sizes are physical pixels, which is what set_position and
/// set_size want for exact monitor placement.
#[tauri::command]
pub(super) fn list_monitors(app: tauri::AppHandle) -> Vec<serde_json::Value> {
    let primary = app.primary_monitor().ok().flatten();
    let primary_name = primary.as_ref().and_then(|m| m.name().cloned());
    let primary_rect = primary.as_ref().map(|m| {
        let p = *m.position();
        let s = *m.size();
        (p.x, p.y, s.width as i32, s.height as i32)
    });

    let monitors = match app.available_monitors() {
        Ok(m) => m,
        Err(_) => return Vec::new(),
    };

    let mut entries: Vec<(usize, serde_json::Value)> = monitors
        .into_iter()
        .enumerate()
        .map(|(index, m)| {
            let pos = m.position();
            let size = m.size();
            let name = m.name().cloned().unwrap_or_else(|| format!("Display {}", index + 1));
            let is_primary = primary_name.as_ref() == Some(&name);
            // Where this screen sits relative to the primary, so the picker can
            // say "right" / "above" instead of only a resolution — a resolution
            // alone does not tell you which physical monitor you just chose.
            let side = match primary_rect {
                _ if is_primary => "",
                Some((px, py, pw, ph)) => {
                    let (x, y, w, h) = (pos.x, pos.y, size.width as i32, size.height as i32);
                    if x >= px + pw { "right" }
                    else if x + w <= px { "left" }
                    else if y >= py + ph { "below" }
                    else if y + h <= py { "above" }
                    else { "" }
                }
                None => "",
            };
            (
                index,
                serde_json::json!({
                    // Index into available_monitors — this is what gets saved and
                    // passed back to open_details_window, so it must stay stable
                    // regardless of the display order below.
                    "index": index,
                    "name": name,
                    "x": pos.x,
                    "y": pos.y,
                    "width": size.width,
                    "height": size.height,
                    "scaleFactor": m.scale_factor(),
                    "isPrimary": is_primary,
                    "side": side,
                }),
            )
        })
        .collect();

    // Present the primary first so the picker's "1" is the screen the game is
    // on and "2" is the other one. The OS order is not dependable: on a
    // two-screen setup here it reported the secondary display first, which made
    // "Monitor 2" select the primary.
    entries.sort_by_key(|(index, v)| {
        let primary = v.get("isPrimary").and_then(|p| p.as_bool()).unwrap_or(false);
        (!primary, *index)
    });
    entries.into_iter().map(|(_, v)| v).collect()
}

/// Open (or move) the always-on Details window, filling the chosen monitor.
/// Frameless to match the overlay; the in-page header carries the close button.
///
/// `async` is load-bearing — see the note on `open_settings_window`.
#[tauri::command]
pub(super) async fn open_details_window(app: tauri::AppHandle, monitor_index: usize) -> Result<(), String> {
    open_details_on_monitor_inner(&app, monitor_index, true)
}

pub(super) fn open_details_on_monitor(app: &tauri::AppHandle, monitor_index: usize) -> Result<(), String> {
    open_details_on_monitor_inner(app, monitor_index, false)
}

/// `force_place` = the user just picked this monitor, so ignore any remembered
/// position and fill that screen.
fn open_details_on_monitor_inner(
    app: &tauri::AppHandle,
    monitor_index: usize,
    force_place: bool,
) -> Result<(), String> {
    let monitors = app.available_monitors().map_err(|e| e.to_string())?;
    if monitors.is_empty() {
        return Err("no monitors reported".into());
    }
    let monitor = monitors
        .get(monitor_index)
        .ok_or_else(|| format!("monitor {} is not connected", monitor_index))?;
    DETAILS_MONITOR.store(monitor_index, std::sync::atomic::Ordering::Relaxed);

    // available_monitors reports PHYSICAL pixels, but WebviewWindowBuilder's
    // position()/inner_size() take LOGICAL pixels. Convert, or on a scaled
    // display the window lands in the wrong place at the wrong size.
    let scale = monitor.scale_factor();
    let pos = *monitor.position();
    let size = *monitor.size();
    let lx = pos.x as f64 / scale;
    let ly = pos.y as f64 / scale;
    let lw = size.width as f64 / scale;
    let lh = size.height as f64 / scale;

    tracing::info!(
        "details window -> monitor {} '{}' physical {}x{} at {},{} (scale {}) => logical {}x{} at {},{}",
        monitor_index,
        monitor.name().cloned().unwrap_or_default(),
        size.width, size.height, pos.x, pos.y, scale, lw, lh, lx, ly
    );

    if let Some(existing) = app.get_webview_window("details") {
        // Already open — move it only when the user explicitly picked a monitor.
        if force_place {
            let _ = existing.unmaximize();
            let _ = existing.set_position(tauri::Position::Physical(pos));
            platform::window::set_size(&existing, tauri::Size::Physical(size));
        }
        let _ = existing.show();
        let _ = existing.unminimize();
        let _ = existing.set_focus();
        announce_details_placement(app, monitor_index);
        return Ok(());
    }

    // Born at the target coordinates rather than created-then-moved. Moving a
    // hidden window and calling maximize() put it on whichever monitor Windows
    // still considered current, which is how Details kept opening on the same
    // screen as the overlay.
    let window = tauri::WebviewWindowBuilder::new(
        app,
        "details",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .initialization_script("window.__A2_VIEW__ = 'details';")
    .title("Daevalog DPS Meter — Details")
    .decorations(false)
    .transparent(false)
    // Intentional: the point of this window is to stay readable on a second
    // monitor without being buried by whatever else is on that screen.
    .always_on_top(true)
    .resizable(true)
    .skip_taskbar(crate::tray::taskbar_hidden(app))
    .position(lx, ly)
    .inner_size(lw, lh)
    // Visible from the start. Creating it hidden and having the page reveal
    // itself deadlocked: a hidden WebView2 window may never load its content,
    // so the reveal never ran. The background colour below covers the load so
    // there is no white flash.
    .background_color(tauri::window::Color(10, 14, 22, 255))
    .build()
    .map_err(|e| e.to_string())?;
    platform::window::set_size(&window, tauri::Size::Logical(tauri::LogicalSize { width: lw, height: lh }));

    // An explicit monitor pick always wins; otherwise fall back to wherever the
    // user last dragged the window.
    if force_place || !restore_window_geometry(app, &window, "details") {
        // Re-assert in physical units: the builder's logical values round on
        // fractional-scale displays.
        let _ = window.set_position(tauri::Position::Physical(pos));
        platform::window::set_size(&window, tauri::Size::Physical(size));
    }

    // Announced once the window reports ready (see details_window_ready); a
    // freshly built webview has no listener attached yet.
    // Safety net. Unconditional: is_visible() does not reliably reflect whether
    // the window was actually mapped, so guarding on it left the window created
    // but never revealed. show() on an already-visible window is a no-op, so the
    // worst case here is a redundant call.
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        if let Some(w) = handle.get_webview_window("details") {
            let _ = w.show();
            let _ = w.set_focus();
            tracing::info!("details window revealed (safety net)");
        }
    });
    Ok(())
}

/// The monitor the user has pinned Details to, or `None` when the setting is
/// off. Read from settings on every use rather than from `DETAILS_MONITOR`:
/// that atomic is session-sticky and never cleared, so once a monitor had been
/// picked, Details kept landing on that screen for the rest of the run even
/// after the user set the dropdown back to Off.
pub(super) fn details_monitor_setting(app: &tauri::AppHandle) -> Option<usize> {
    let state = app.try_state::<AppState>()?;
    let raw = state.settings.get("dpsMeter.detailsMonitor")?;
    let raw = raw.trim();
    if raw.is_empty() || raw == "off" {
        return None;
    }
    raw.parse::<usize>().ok()
}

/// Whether the Details window's saved rect is somewhere the *user* put it.
///
/// Geometry recorded while Details was pinned to a monitor is the setting's
/// placement, not a choice — and it used to be saved anyway, so turning the
/// setting off left Details reopening on that same screen. The saver now stamps
/// this marker only when it records an unpinned window, so geometry written
/// under the old behaviour has no marker and is ignored exactly once. After
/// that the window remembers wherever the user drags it, including onto a
/// second screen deliberately.
fn details_geometry_is_user_placed(app: &tauri::AppHandle) -> bool {
    app.try_state::<AppState>()
        .and_then(|state| state.settings.get(DETAILS_USER_PLACED_KEY))
        .map(|v| v.trim() == "true")
        .unwrap_or(false)
}

pub(super) const DETAILS_USER_PLACED_KEY: &str = "window.details.userPlaced";

/// Whether a window rect swallows a whole monitor — the shape of a
/// monitor-filling placement rather than somewhere the user dragged a window.
pub(super) fn rect_covers_a_monitor(
    app: &tauri::AppHandle,
    pos: tauri::PhysicalPosition<i32>,
    size: tauri::PhysicalSize<u32>,
) -> bool {
    app.available_monitors()
        .map(|monitors| {
            monitors.iter().any(|m| {
                let p = *m.position();
                let s = *m.size();
                pos.x <= p.x
                    && pos.y <= p.y
                    && pos.x + size.width as i32 >= p.x + s.width as i32
                    && pos.y + size.height as i32 >= p.y + s.height as i32
            })
        })
        .unwrap_or(false)
}

/// Centre a tool window on whichever screen the overlay is on — where the user
/// is actually playing — rather than on whatever Windows considers current.
fn center_on_overlay_monitor(app: &tauri::AppHandle, window: &tauri::WebviewWindow) {
    if let Some(main) = app.get_webview_window("main") {
        if let (Ok(Some(monitor)), Ok(size)) = (main.current_monitor(), window.outer_size()) {
            let p = *monitor.position();
            let s = *monitor.size();
            let x = p.x + ((s.width as i32 - size.width as i32) / 2).max(0);
            let y = p.y + ((s.height as i32 - size.height as i32) / 2).max(0);
            let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }));
            return;
        }
    }
    let _ = window.center();
}

/// Restore a tool window's remembered geometry. Returns true if anything was
/// applied, so callers know whether they still need to place it themselves.
fn restore_window_geometry(app: &tauri::AppHandle, window: &tauri::WebviewWindow, label: &str) -> bool {
    let Some(state) = app.try_state::<AppState>() else { return false };
    let get = |k: &str| state.settings.get(&format!("window.{}.{}", label, k))
        .and_then(|v| v.trim().parse::<i32>().ok());
    let (Some(x), Some(y)) = (get("x"), get("y")) else { return false };
    if x <= -10000 || y <= -10000 {
        return false;
    }
    // Only restore onto a screen that still exists — an unplugged monitor would
    // otherwise strand the window off-desktop.
    let on_screen = app.available_monitors().map(|ms| {
        ms.iter().any(|m| {
            let p = *m.position();
            let s = *m.size();
            x >= p.x - 64 && x < p.x + s.width as i32 && y >= p.y - 64 && y < p.y + s.height as i32
        })
    }).unwrap_or(false);
    if !on_screen {
        tracing::info!("{} window: saved position {},{} is off-desktop; ignoring", label, x, y);
        return false;
    }
    let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }));
    // Settings is a fixed list of options: it reopens where it was left, at its
    // own size. A remembered size only ever made it sprawl (see the inner-size
    // note where geometry is saved), and it can still be resized while open.
    if label == "settings" {
        return true;
    }
    if let (Some(w), Some(h)) = (get("w"), get("h")) {
        if w > 200 && h > 150 {
            platform::window::set_size(window, tauri::Size::Physical(tauri::PhysicalSize {
                width: w as u32,
                height: h as u32,
            }));
        }
    }
    true
}

/// The Settings window. Free-floating like Details — it used to be a panel that
/// forced the overlay to resize itself to ~820px tall.
///
/// **This command must stay `async`.** Tauri runs synchronous commands on the
/// main thread, and on Windows `WebviewWindowBuilder::build()` deadlocks there:
/// WebView2 needs the main thread's message loop to deliver the
/// `CreateCoreWebView2Controller` callback, which cannot run while `build()` is
/// blocking that same loop. The window still appeared — sized, and painting its
/// background colour — but its WebView2 host stayed 0x0 and hidden, and the page
/// never left `about:blank`. That is the "blank tool window". Marshalling
/// through `run_on_main_thread` makes it worse, not better. An `async` command
/// runs off the main thread, so the loop stays free to complete the callback.
/// See <https://docs.rs/tauri/latest/tauri/webview/struct.WebviewWindowBuilder.html>.
#[tauri::command]
pub(super) async fn open_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window("settings") {
        let _ = existing.show();
        let _ = existing.unminimize();
        let _ = existing.set_always_on_top(true);
        let _ = existing.set_focus();
        // Already open (perhaps behind the game): check the account again.
        let _ = app.emit_to("settings", "settings-shown", ());
        return Ok(());
    }
    build_settings_window(&app)
}

/// Runs in the Settings window before the page. Besides naming the view, it
/// answers Quit, Close and Escape the moment they are on screen. They used to
/// be wired when the page's scripts had run (about 800 KB, lucide first), so
/// on every open both buttons sat dead for a while: the window is rebuilt each
/// time it opens (see `close_settings_window`). A listener on `document` is
/// there before the buttons are, and catches clicks on them as they appear.
const SETTINGS_WINDOW_SCRIPT: &str = r#"
window.__A2_VIEW__ = 'settings';
(function () {
  const call = (cmd) => window.__TAURI_INTERNALS__.invoke(cmd).catch(() => {});
  document.addEventListener('click', (event) => {
    const el = event.target instanceof Element ? event.target : null;
    if (el?.closest('.quitButton')) {
      event.stopPropagation();
      call('quit_app');
    } else if (el?.closest('.settingsClose')) {
      event.stopPropagation();
      call('close_settings_window');
    }
  }, true);
  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') call('close_settings_window');
  }, true);
})();
"#;

fn build_settings_window(app: &tauri::AppHandle) -> Result<(), String> {
    let window = tauri::WebviewWindowBuilder::new(
        app,
        "settings",
        tauri::WebviewUrl::App("index.html".into()),
    )
    // Injected before any page script. WebviewUrl::App is a path, so a ?query
    // gets percent-encoded — this is the one channel that is reliable.
    .initialization_script(SETTINGS_WINDOW_SCRIPT)
    .title("Daevalog DPS Meter — Settings")
    .decorations(false)
    .transparent(false)
    // Matches Details: the overlay itself is always-on-top, so a settings window
    // that could fall behind it would be unreachable while the game is focused.
    .always_on_top(true)
    .resizable(true)
    .skip_taskbar(crate::tray::taskbar_hidden(app))
    .inner_size(760.0, 820.0)
    .min_inner_size(520.0, 420.0)
    // Built visible, with the app's background colour to cover the load rather
    // than flashing white. Building it hidden is not an option: a hidden
    // WebView2 window may never load its content, so a page-driven reveal
    // deadlocks.
    .background_color(tauri::window::Color(10, 14, 22, 255))
    .build()
    .map_err(|e| e.to_string())?;
    platform::window::set_size(&window, tauri::Size::Logical(tauri::LogicalSize { width: 760.0, height: 820.0 }));

    if !restore_window_geometry(app, &window, "settings") {
        let _ = window.center();
    }
    Ok(())
}

#[tauri::command]
pub(super) fn close_settings_window(app: tauri::AppHandle) {
    // Closed, not hidden: a hidden WebView2 window came back blank when shown
    // again. Rebuilding it costs little now that Quit and Close are answered
    // before the page loads (SETTINGS_WINDOW_SCRIPT) and the account line
    // starts from the last check.
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.close();
    }
}

/// Shown when the frontend of a tool window has painted.
#[tauri::command]
pub(super) fn tool_window_ready(app: tauri::AppHandle, label: String) {
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Tell the Details window which screen it just landed on, so it can confirm
/// visually. A dropdown label alone does not prove the right monitor was picked.
fn announce_details_placement(app: &tauri::AppHandle, monitor_index: usize) {
    let monitors = match app.available_monitors() {
        Ok(m) => m,
        Err(_) => return,
    };
    let Some(monitor) = monitors.get(monitor_index) else { return };
    let primary = app.primary_monitor().ok().flatten();
    let primary_name = primary.as_ref().and_then(|m| m.name().cloned());
    let name = monitor.name().cloned().unwrap_or_default();
    let is_primary = primary_name.as_ref() == Some(&name);

    // Position in the primary-first ordering the picker shows.
    let mut ordered: Vec<(bool, usize)> = monitors
        .iter()
        .enumerate()
        .map(|(i, m)| (m.name().cloned() == primary_name, i))
        .collect();
    ordered.sort_by_key(|(is_p, i)| (!*is_p, *i));
    let position = ordered
        .iter()
        .position(|(_, i)| *i == monitor_index)
        .unwrap_or(monitor_index);

    let size = *monitor.size();
    let _ = app.emit_to(
        "details",
        "details-placed",
        serde_json::json!({
            "number": position + 1,
            "width": size.width,
            "height": size.height,
            "isPrimary": is_primary,
        }),
    );
}

/// Called by a Details-family window once its panel has painted. Reveals the
/// calling window rather than a fixed label, since there can now be several.
#[tauri::command]
pub(super) fn details_window_ready(app: tauri::AppHandle, window: tauri::Window) {
    let _ = window.show();
    tracing::info!("{} window revealed (frontend ready)", window.label());
    // Only the monitor-pinned singleton has a placement to confirm; a fight
    // window is placed by cascade and the History window by its own geometry.
    if window.label() == "details" {
        let index = DETAILS_MONITOR.load(std::sync::atomic::Ordering::Relaxed);
        if index != usize::MAX {
            announce_details_placement(&app, index);
        }
    }
}

#[tauri::command]
pub(super) fn close_details_window(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("details") {
        let _ = window.close();
    }
}

/// Window label for a saved fight. Each fight gets its own window so several can
/// be compared side by side, so the id has to survive as a label — sanitised,
/// because labels are also used to build the webview's internal identifiers.
fn fight_window_label(fight_id: &str) -> String {
    let safe: String = fight_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("details-{}", safe)
}

/// Ask a Details surface to show something. Which window serves the request
/// depends on what is being asked for:
///
/// - `history` → the one persistent History window. It is a browser you leave
///   open, so it never closes just because you opened something from it.
/// - `fight`   → a window of its own, `details-<id>`, so fights can be compared
///   side by side. Asking for a fight that is already open raises that window
///   rather than opening a second copy of it.
/// - anything else (a meter row) → the single live `details` window, re-targeted
///   in place. Live rows are clicked constantly during combat; spawning a window
///   per click would bury the game.
///
/// If the target window is up the request is pushed straight to it. If not, the
/// request is parked under that window's label and the window pulls it on
/// startup — a webview that was created to serve a request has no listener
/// attached at the moment the request is emitted.
///
/// `async` is load-bearing — it creates windows. See `open_settings_window`.
#[tauri::command]
pub(super) async fn request_details_view(
    app: tauri::AppHandle,
    payload: serde_json::Value,
) -> Result<(), String> {
    let seq = DETAILS_REQUEST_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let mut payload = payload;
    if !payload.is_object() {
        payload = serde_json::json!({});
    }
    let kind = payload
        .get("kind")
        .and_then(|k| k.as_str())
        .unwrap_or("row")
        .to_string();
    let fight_id = payload
        .get("fightId")
        .and_then(|f| f.as_str())
        .unwrap_or("")
        .to_string();
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("seq".into(), serde_json::json!(seq));
    }

    let label = match kind.as_str() {
        "history" => "history".to_string(),
        "fight" if !fight_id.is_empty() => fight_window_label(&fight_id),
        _ => "details".to_string(),
    };

    if let Some(window) = app.get_webview_window(&label) {
        // Live window: nothing to park, the listener is already attached.
        if let Ok(mut pending) = PENDING_DETAILS_REQUEST.lock() {
            pending.remove(&label);
        }
        // Raise it if it was put away, but do not steal focus from a window
        // that is already on screen — the click came from an overlay sitting
        // on top of a full-screen game, and pulling focus would tab out of it.
        let hidden = !window.is_visible().unwrap_or(true)
            || window.is_minimized().unwrap_or(false);
        if hidden {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        } else if kind == "fight" {
            // Re-asking for a fight that is already open means "show me that
            // one", so bring it forward even though it was never hidden.
            let _ = window.set_focus();
        }
        app.emit_to(&label, "details-request", payload)
            .map_err(|e| e.to_string())?;
        return Ok(());
    }

    if let Ok(mut pending) = PENDING_DETAILS_REQUEST.lock() {
        pending.insert(label.clone(), payload);
    }

    let result = match kind.as_str() {
        "history" => open_history_window_inner(&app),
        "fight" if !fight_id.is_empty() => open_fight_window(&app, &label),
        // The setting decides, every time. Reading DETAILS_MONITOR here is what
        // made "Show Details on monitor: Off" do nothing once a monitor had
        // been picked earlier in the session.
        _ => match details_monitor_setting(&app) {
            Some(index) => open_details_on_monitor(&app, index),
            None => {
                // Forget the earlier pick too, so the placement badge does not
                // announce a monitor this window is no longer tied to.
                DETAILS_MONITOR.store(usize::MAX, std::sync::atomic::Ordering::Relaxed);
                open_details_windowed(&app)
            }
        },
    };
    if result.is_err() {
        // Nothing will ever pull it, and a stale request must not resurface
        // against some later window that happens to take the same label.
        if let Ok(mut pending) = PENDING_DETAILS_REQUEST.lock() {
            pending.remove(&label);
        }
    }
    result
}

/// One window per saved fight, cascaded so a second fight does not land exactly
/// on top of the first. These are deliberately not remembered: they are opened
/// to be read and closed, and persisting geometry per fight id would accumulate
/// settings without bound.
fn open_fight_window(app: &tauri::AppHandle, label: &str) -> Result<(), String> {
    let step = app.webview_windows().keys().filter(|l| l.starts_with("details-")).count() as f64;
    let offset = (step % 6.0) * 34.0;

    let window = tauri::WebviewWindowBuilder::new(
        app,
        label,
        tauri::WebviewUrl::App("index.html".into()),
    )
    .initialization_script("window.__A2_VIEW__ = 'details';")
    .title("Daevalog DPS Meter — Fight")
    .decorations(false)
    .transparent(false)
    .always_on_top(true)
    .resizable(true)
    .skip_taskbar(crate::tray::taskbar_hidden(app))
    .inner_size(1180.0, 760.0)
    .min_inner_size(520.0, 360.0)
    .background_color(tauri::window::Color(10, 14, 22, 255))
    .build()
    .map_err(|e| e.to_string())?;
    platform::window::set_size(&window, tauri::Size::Logical(tauri::LogicalSize { width: 1180.0, height: 760.0 }));

    center_on_overlay_monitor(app, &window);
    if offset > 0.0 {
        if let Ok(pos) = window.outer_position() {
            let shift = offset as i32;
            let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition {
                x: pos.x + shift,
                y: pos.y + shift,
            }));
        }
    }
    Ok(())
}

/// The History window. Persistent by design — it is the browser you pick fights
/// from, and it stays put while those fights open in windows of their own.
fn open_history_window_inner(app: &tauri::AppHandle) -> Result<(), String> {
    let window = tauri::WebviewWindowBuilder::new(
        app,
        "history",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .initialization_script("window.__A2_VIEW__ = 'history';")
    .title("Daevalog DPS Meter — Battle History")
    .decorations(false)
    .transparent(false)
    .always_on_top(true)
    .resizable(true)
    .skip_taskbar(crate::tray::taskbar_hidden(app))
    .inner_size(1100.0, 720.0)
    .min_inner_size(480.0, 360.0)
    .background_color(tauri::window::Color(10, 14, 22, 255))
    .build()
    .map_err(|e| e.to_string())?;
    platform::window::set_size(&window, tauri::Size::Logical(tauri::LogicalSize { width: 1100.0, height: 720.0 }));

    if !restore_window_geometry(app, &window, "history") {
        center_on_overlay_monitor(app, &window);
    }
    Ok(())
}

/// Close whichever tool window asked. Fight windows are frameless and there can
/// be several, so each closes itself rather than the overlay guessing which.
#[tauri::command]
pub(super) fn close_tool_window(window: tauri::Window) {
    let _ = window.close();
}

/// Create the Details window without claiming a whole screen. Used when the
/// user has never picked a monitor in Settings: clicking a meter row should
/// give them a window they can move, not black out a display over the game.
/// A remembered position still wins — this is only the first-run geometry.
fn open_details_windowed(app: &tauri::AppHandle) -> Result<(), String> {
    let window = tauri::WebviewWindowBuilder::new(
        app,
        "details",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .initialization_script("window.__A2_VIEW__ = 'details';")
    .title("Daevalog DPS Meter — Details")
    .decorations(false)
    .transparent(false)
    .always_on_top(true)
    .resizable(true)
    .skip_taskbar(crate::tray::taskbar_hidden(app))
    .inner_size(1180.0, 760.0)
    .min_inner_size(520.0, 360.0)
    // Visible, with the app background painted behind the load — same reason as
    // the monitor-filling path: a hidden WebView2 window may never load at all.
    .background_color(tauri::window::Color(10, 14, 22, 255))
    .build()
    .map_err(|e| e.to_string())?;
    platform::window::set_size(&window, tauri::Size::Logical(tauri::LogicalSize { width: 1180.0, height: 760.0 }));

    // A remembered position still wins, but only one the user actually chose.
    if !details_geometry_is_user_placed(app)
        || !restore_window_geometry(app, &window, "details")
    {
        center_on_overlay_monitor(app, &window);
    }
    Ok(())
}

/// Pulled by a tool window once its listener is attached. The label comes from
/// the calling window rather than an argument, so a window can only ever claim
/// its own request. Clearing on read keeps a stale one from resurfacing.
#[tauri::command]
pub(super) fn take_pending_details_request(window: tauri::Window) -> Option<serde_json::Value> {
    PENDING_DETAILS_REQUEST
        .lock()
        .ok()
        .and_then(|mut pending| pending.remove(window.label()))
}
