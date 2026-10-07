//! Deaths: how often you and your party fell to 0 HP in a fight.

use serde::{Deserialize, Serialize};

/// One player's deaths in a fight, in Details and saved fights.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeathEntry {
    pub actor_id: i32,
    pub deaths: u32,
}

/// You or a party member on the meter, beside the damage rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeathRow {
    /// The row id, as in `DpsData::map` when the player has a damage row.
    pub actor_id: i32,
    pub nickname: String,
    #[serde(default)]
    pub job: String,
    /// See `PersonalData::number`; 0 for a player with no damage row.
    #[serde(default)]
    pub number: u32,
    /// `None` when the game sent no HP for the player during the fights:
    /// the count is not known.
    pub deaths: Option<u32>,
}
