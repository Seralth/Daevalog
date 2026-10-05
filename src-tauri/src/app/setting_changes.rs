//! What a changed setting does besides being stored.

use crate::combat::data_storage::DataStorage;
use crate::{i18n, logging};

use super::AppState;

pub(super) const ENCOUNTER_TIMEOUT_KEY: &str = "dpsMeter.encounterTimeoutSec";

/// The encounter timeout setting, in whole seconds; anything else keeps the
/// default.
pub(super) fn apply_encounter_timeout(storage: &DataStorage, value: Option<&str>) {
    storage.set_encounter_timeout_ms(encounter_timeout_ms(value));
}

fn encounter_timeout_ms(value: Option<&str>) -> i64 {
    value
        .and_then(|v| v.trim().parse::<i64>().ok())
        .and_then(|s| s.checked_mul(1000))
        .unwrap_or(crate::combat::data_storage::DEFAULT_ENCOUNTER_TIMEOUT_MS)
}

pub(crate) fn set_language(state: &AppState, language: String) {
    tracing::info!("Language change requested: {}", language);
    if let Some(ref data_dir) = state.i18n_data_dir {
        i18n::lookup::load_language(&state.skill_lookup, &state.npc_lookup, data_dir, &language);
    } else {
        tracing::warn!("No i18n data dir available for language reload");
    }
    state.settings.set("dpsMeter.language", &language);
}

pub(crate) fn set_debug_logging(state: &AppState, enabled: bool) {
    logging::logger::set_debug_enabled(enabled, &state.app_data_dir);
    state.settings.set("dpsMeter.debugLoggingEnabled", if enabled { "true" } else { "false" });
}

pub(crate) fn set_packet_logging(state: &AppState, enabled: bool) {
    logging::logger::set_packet_log_enabled(enabled, &state.app_data_dir);
    state.settings.set("dpsMeter.saveRawPackets", if enabled { "true" } else { "false" });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_encounter_timeout_too_big_to_be_milliseconds_keeps_the_default() {
        let default = crate::combat::data_storage::DEFAULT_ENCOUNTER_TIMEOUT_MS;
        assert_eq!(encounter_timeout_ms(Some("9223372036854775807")), default);
        assert_eq!(encounter_timeout_ms(Some("x")), default);
        assert_eq!(encounter_timeout_ms(Some(" 60 ")), 60_000);
    }
}
