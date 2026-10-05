//! The meter's background tasks: the 500 ms tick and the fight auto-save.

use std::collections::HashSet;
use std::time::Duration;

use tauri::{Emitter, Manager};

use crate::{platform, share};

use super::auto_upload::auto_upload;
use super::tool_windows::{details_monitor_setting, rect_covers_a_monitor, DETAILS_USER_PLACED_KEY};
use super::AppState;

pub(super) fn spawn_meter_tick(app: &tauri::AppHandle) {
    // Periodic DPS update emission (every 500ms)
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        let mut tick_count: u64 = 0;
        let mut hide_delay: u64 = 0; // ticks to wait before hiding
        loop {
            interval.tick().await;
            tick_count += 1;

            if let Some(state) = handle.try_state::<AppState>() {
                let t0 = std::time::Instant::now();
                let lock_guard = state.dps_calculator.lock();
                let lock_ms = t0.elapsed().as_millis();
                let dps = {
                    let mut calc = lock_guard;
                    calc.get_dps()
                };
                let calc_ms = t0.elapsed().as_millis();
                let _ = handle.emit("dps-update", &dps);
                let total_ms = t0.elapsed().as_millis();
                if total_ms > 200 {
                    tracing::warn!("Slow: lock={}ms calc={}ms emit={}ms total={}ms gen={}",
                        lock_ms, calc_ms - lock_ms, total_ms - calc_ms, total_ms,
                        state.data_storage.damage_generation());
                }

                if let Some(ping) = state.ping_tracker.current_ping_ms() {
                    let _ = handle.emit("ping-update", ping);
                }

                // --- Auto-hide when AION2 loses focus (every tick) ---
                let auto_hide = tick_count > 20
                    && state.settings.get("dpsMeter.autoHideMeter")
                        .unwrap_or_default() == "true";
                if auto_hide {
                    if let Some(window) = handle.get_webview_window("main") {
                        let aion_fg = platform::window_detector::is_aion2_foreground();
                        let is_self_fg = window.is_focused().unwrap_or(false);
                        let is_visible = window.is_visible().unwrap_or(true);
                        let is_minimized = window.is_minimized().unwrap_or(false);
                        if tick_count % 4 == 0 {
                            tracing::trace!("auto-hide: aion_fg={} self_fg={} visible={} minimized={} hide_delay={}",
                                aion_fg, is_self_fg, is_visible, is_minimized, hide_delay);
                        }
                        if aion_fg || is_self_fg {
                            hide_delay = 0;
                            if !is_visible || is_minimized {
                                platform::window::show_on_top_without_focus(&window);
                                // Notify frontend to recalculate window size
                                // (content may have changed while minimized)
                                let _ = window.emit("force-resize", ());
                            }
                        } else if is_visible && !is_minimized {
                            // Wait 3 ticks (1.5s) before hiding to avoid
                            // flickering during alt-tab transitions
                            hide_delay += 1;
                            if hide_delay >= 3 {
                                platform::window::minimize_off_top(&window);
                            }
                        }
                    }
                }

                // --- Save window position every ~5 seconds (every 10 ticks) ---
                if tick_count % 10 == 0 {
                    if let Some(window) = handle.get_webview_window("main") {
                        let scale = window.scale_factor().unwrap_or(1.0);
                        let layer_pos = platform::window::overlay_layer_position(&window).map(|(x, y)| {
                            tauri::PhysicalPosition { x: (x as f64 * scale) as i32, y: (y as f64 * scale) as i32 }
                        });
                        if let Some(pos) = layer_pos.or_else(|| window.outer_position().ok()) {
                            // Don't save minimized/hidden positions
                            if pos.x > -10000 && pos.y > -10000 {
                                state.settings.set("window.x", &pos.x.to_string());
                                state.settings.set("window.y", &pos.y.to_string());
                            }
                        }
                    }
                    // These float independently of the overlay, so each
                    // remembers where it was left. Per-fight windows
                    // (details-*) are deliberately absent: they are
                    // opened to be read and closed, and keying geometry
                    // by fight id would grow settings without bound.
                    for label in ["details", "settings", "history"] {
                        // While Details is pinned to a monitor its rect
                        // comes from the setting, not from the user.
                        // Saving it poisons the windowed geometry: turn
                        // the setting off and Details would reopen
                        // full-size on that same screen.
                        if label == "details"
                            && details_monitor_setting(&handle).is_some()
                        {
                            continue;
                        }
                        if let Some(w) = handle.get_webview_window(label) {
                            if !w.is_visible().unwrap_or(false) {
                                continue;
                            }
                            if let Ok(pos) = w.outer_position() {
                                if pos.x > -10000 && pos.y > -10000 {
                                    state.settings.set(&format!("window.{}.x", label), &pos.x.to_string());
                                    state.settings.set(&format!("window.{}.y", label), &pos.y.to_string());
                                }
                            }
                            // Inner, not outer: restore applies it with
                            // set_size, which sets the inner size. Saving
                            // the outer size grew every tool window by its
                            // border (16x9 px here) on each reopen.
                            if let Ok(size) = w.inner_size() {
                                if size.width > 100 && size.height > 100 {
                                    state.settings.set(&format!("window.{}.w", label), &size.width.to_string());
                                    state.settings.set(&format!("window.{}.h", label), &size.height.to_string());
                                }
                            }
                            // Details is unpinned here, but the window
                            // may still be sitting on the fill rect from
                            // before the setting was switched off. A rect
                            // that swallows a whole screen is not a
                            // placement anyone chose by dragging, so it
                            // never earns the marker.
                            if label == "details" {
                                let filling = match (w.outer_position(), w.outer_size()) {
                                    (Ok(pos), Ok(size)) => rect_covers_a_monitor(&handle, pos, size),
                                    _ => true,
                                };
                                if !filling {
                                    state.settings.set(DETAILS_USER_PLACED_KEY, "true");
                                }
                            }
                        }
                    }
                }

            }
        }
    });
}

pub(super) fn spawn_auto_save(app: &tauri::AppHandle) {
    // Separate task for boss fight auto-save (every 30s, on blocking thread)
    let handle_save = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            if let Some(state) = handle_save.try_state::<AppState>() {
                if state.data_storage.damage_generation() > 0 {
                    // Run on blocking thread to avoid starving the async runtime
                    // snapshot_boss_fights acquires the dps_calculator lock
                    // Run synchronously but only if lock is available
                    if let Some(mut calc) = state.dps_calculator.try_lock() {
                        let records = calc.snapshot_boss_fights();
                        let finished: HashSet<String> = records.iter()
                            .filter(|r| calc.fight_finished(r))
                            .map(|r| r.id.clone())
                            .collect();
                        drop(calc);
                        for record in &records {
                            let _ = state.fight_history.save_fight(record);
                            // The packets behind it, so it can be
                            // uploaded and verified later, and checked
                            // against the game's own record. Training
                            // dummies keep one for that check; they are
                            // never uploaded.
                            if let Err(e) = share::save_slice(
                                &state.app_data_dir, record, &state.data_storage) {
                                tracing::debug!("No slice for {}: {e}", record.id);
                            }
                        }
                        if !records.is_empty() {
                            share::prune_slices(&state.app_data_dir);
                        }
                        if state.settings.get(share::AUTO_UPLOAD_KEY).as_deref() == Some("true") {
                            for record in records.into_iter()
                                .filter(|r| share::wants_auto_upload(&state.app_data_dir, r, finished.contains(&r.id)))
                            {
                                auto_upload(handle_save.clone(), record);
                            }
                        }
                    }
                }
                // Automatic uploads that failed and are due again,
                // fighting or not: a meter left open after the
                // connection came back catches up on its own.
                if state.settings.get(share::AUTO_UPLOAD_KEY).as_deref() == Some("true") {
                    let now = crate::clock::now_ms();
                    for id in share::auto_upload_retries_due(&state.app_data_dir, now) {
                        if let Ok(record) = state.fight_history.load_fight(&id) {
                            auto_upload(handle_save.clone(), record);
                        }
                    }
                }
            }
        }
    });
}
