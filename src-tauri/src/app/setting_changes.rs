//! What a changed setting does besides being stored.

use crate::combat::data_storage::DataStorage;

pub(super) const ENCOUNTER_TIMEOUT_KEY: &str = "dpsMeter.encounterTimeoutSec";

/// The encounter timeout setting, in whole seconds; anything else keeps the
/// default.
pub(super) fn apply_encounter_timeout(storage: &DataStorage, value: Option<&str>) {
    let secs = value.and_then(|v| v.trim().parse::<i64>().ok());
    let ms = secs.map_or(crate::combat::data_storage::DEFAULT_ENCOUNTER_TIMEOUT_MS, |s| s * 1000);
    storage.set_encounter_timeout_ms(ms);
}
