//! Uploading finished fights in the background.

use std::collections::HashSet;

use parking_lot::Mutex;
use tauri::{Emitter, Manager};

use crate::entity::fight_record::FightRecord;
use crate::share;

use super::AppState;

/// Upload a finished fight in the background, and tell every window.
///
/// Failure is quiet on purpose: not being signed in, or being offline, is not
/// something to interrupt a fight about. The fight keeps its slice, and the
/// upload button in History still works. A failure that waiting could fix
/// (offline, a server error, a rate limit) is tried again later, on the
/// schedule in `share::note_auto_upload_failure`.
pub(super) fn auto_upload(app: tauri::AppHandle, record: FightRecord) {
    let Some(in_flight) = InFlight::start(&record.id) else { return };
    tauri::async_runtime::spawn(async move {
        let _in_flight = in_flight;
        let Some(state) = app.try_state::<AppState>() else { return };
        match share::upload_detailed(&state.http, &state.app_data_dir, &state.settings, &record).await {
            Ok(result) => {
                tracing::info!("Auto-uploaded {} -> {}", record.id, result.url);
                let _ = app.emit("fight-uploaded", serde_json::json!({
                    "fightId": record.id, "url": result.url, "visibility": result.visibility,
                }));
            }
            Err(failure) => {
                share::note_auto_upload_failure(
                    &state.app_data_dir, &record.id, &failure, crate::clock::now_ms());
                tracing::info!(
                    "Auto-upload of {} failed{}: {}",
                    record.id,
                    if failure.retryable { ", will retry" } else { "" },
                    failure.message
                );
            }
        }
    });
}

/// Fights with an automatic upload running. One upload of a fight at a time:
/// a retry must not start while the first try is still waiting on the network.
static IN_FLIGHT: std::sync::LazyLock<Mutex<HashSet<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashSet::new()));

/// A fight's place in `IN_FLIGHT`, given up on drop: every way out of the
/// upload task clears it, a panic included.
struct InFlight(String);

impl InFlight {
    fn start(id: &str) -> Option<Self> {
        IN_FLIGHT.lock().insert(id.to_string()).then(|| Self(id.to_string()))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        IN_FLIGHT.lock().remove(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panicking_upload_still_frees_its_fight() {
        let held = InFlight::start("in-flight-test").unwrap();
        assert!(InFlight::start("in-flight-test").is_none(), "one upload of a fight at a time");
        drop(held);
        let outcome = std::panic::catch_unwind(|| {
            let _held = InFlight::start("in-flight-test").unwrap();
            panic!("upload task panicked");
        });
        assert!(outcome.is_err());
        assert!(InFlight::start("in-flight-test").is_some());
    }
}
