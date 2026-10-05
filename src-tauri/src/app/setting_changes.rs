//! What a changed setting does besides being stored.

use crate::combat::data_storage::DataStorage;
use crate::i18n;

use super::AppState;

pub(super) const ENCOUNTER_TIMEOUT_KEY: &str = "dpsMeter.encounterTimeoutSec";

/// The encounter timeout setting, in whole seconds; anything else keeps the
/// default.
pub(super) fn apply_encounter_timeout(storage: &DataStorage, value: Option<&str>) {
    let secs = value.and_then(|v| v.trim().parse::<i64>().ok());
    let ms = secs.map_or(crate::combat::data_storage::DEFAULT_ENCOUNTER_TIMEOUT_MS, |s| s * 1000);
    storage.set_encounter_timeout_ms(ms);
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
