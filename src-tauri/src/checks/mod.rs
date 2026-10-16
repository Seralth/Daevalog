mod details_report;
#[cfg(feature = "backend")]
mod format_report;
#[cfg(feature = "backend")]
mod history_report;
mod ping_report;
mod replay_character;
mod replay_deaths;
pub(crate) mod replay_report;
mod replay_players;
mod replay_taken;
pub(crate) mod replay_timeline;
mod view_report;
#[cfg(feature = "backend")]
mod record_check;

use std::collections::HashSet;
use std::sync::Arc;

use crate::i18n::lookup::{NpcLookup, SkillLookup};

/// The game data folder of this checkout.
pub(super) fn data_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data")
}

/// The skill and NPC tables in English, and the skills whose damage comes
/// over time (none when that list is missing or not a JSON array of ids).
pub(super) fn lookups() -> (Arc<SkillLookup>, Arc<NpcLookup>, HashSet<i32>) {
    let skills = Arc::new(SkillLookup::new());
    let npcs = Arc::new(NpcLookup::new());
    crate::i18n::lookup::load_language(&skills, &npcs, &data_dir(), "en");
    let dot_ids: HashSet<i32> = std::fs::read_to_string(data_dir().join("dot_skill_ids.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<i32>>(&t).ok())
        .unwrap_or_default()
        .into_iter()
        .collect();
    (skills, npcs, dot_ids)
}
