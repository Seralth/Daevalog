use super::deaths::DeathRow;
use super::personal_data::PersonalData;
use super::taken::TakenRow;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One reading of the meter: every row, and the target the rows are about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DpsData {
    /// Rows by row id (the player, or a summon's owner).
    pub map: HashMap<i32, PersonalData>,
    pub target_name: String,
    pub target_mode: String,
    pub target_id: i32,
    /// The target's NPC code; 0 when there is none.
    #[serde(default)]
    pub target_mob_code: i32,
    pub battle_time: i64,
    pub local_player_id: Option<i64>,
    pub target_max_hp: i64,
    pub target_total_damage: i64,
    /// -1 while the game has sent no HP for the target.
    pub target_current_hp: i64,
    pub dungeon_id: i32,
    #[serde(default)]
    pub taken: Vec<TakenRow>,
    #[serde(default)]
    pub deaths: Vec<DeathRow>,
    /// The fight time at which the target enrages, from the deadline the
    /// server sent at the pull; 0 when none came.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub target_enrage_ms: i64,
    /// The server took the target out of combat, or into its next pull: the
    /// fight on the rows is over.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub target_out_of_combat: bool,
}

fn is_zero(v: &i64) -> bool {
    *v == 0
}

impl Default for DpsData {
    fn default() -> Self {
        Self::new()
    }
}

impl DpsData {
    /// A reading with no rows and no target, in the default boss mode.
    pub fn new() -> Self {
        DpsData {
            map: HashMap::new(),
            target_name: String::new(),
            target_mode: "bossTargets".to_string(),
            target_id: 0,
            target_mob_code: 0,
            battle_time: 0,
            local_player_id: None,
            target_max_hp: 0,
            target_total_damage: 0,
            target_current_hp: -1,
            dungeon_id: 0,
            taken: Vec::new(),
            deaths: Vec::new(),
            target_enrage_ms: 0,
            target_out_of_combat: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DpsData;

    #[test]
    fn an_empty_reading_waits_for_a_target() {
        let r = DpsData::new();
        assert!(r.map.is_empty());
        assert_eq!((r.target_name.as_str(), r.target_mode.as_str()), ("", "bossTargets"));
        assert_eq!((r.target_id, r.target_mob_code, r.dungeon_id), (0, 0, 0));
        assert_eq!((r.battle_time, r.target_max_hp, r.target_total_damage), (0, 0, 0));
        assert_eq!(r.target_current_hp, -1);
        assert_eq!(r.local_player_id, None);
        assert!(r.taken.is_empty() && r.deaths.is_empty());
    }

    #[test]
    fn a_reading_is_written_with_its_fields_in_order() {
        let mut r = DpsData::new();
        r.map.insert(7, super::PersonalData::new("p1".into()));
        let v = serde_json::to_value(&r).unwrap();
        let text = serde_json::to_string(&r).unwrap();
        let keys = [
            "map", "targetName", "targetMode", "targetId", "targetMobCode", "battleTime", "localPlayerId",
            "targetMaxHp", "targetTotalDamage", "targetCurrentHp", "dungeonId", "taken", "deaths",
        ];
        assert_eq!(v.as_object().unwrap().len(), keys.len());
        let at: Vec<usize> = keys.iter().map(|k| text.find(&format!("\"{k}\":")).unwrap()).collect();
        assert!(at.windows(2).all(|w| w[0] < w[1]), "{text}");
        assert!(v["map"]["7"].is_object());
        assert!(v["localPlayerId"].is_null());
    }

    #[test]
    fn the_servers_word_on_the_fight_is_written_only_when_it_has_one() {
        let mut r = DpsData::new();
        let text = serde_json::to_string(&r).unwrap();
        assert!(!text.contains("targetEnrageMs") && !text.contains("targetOutOfCombat"));
        (r.target_enrage_ms, r.target_out_of_combat) = (298_884, true);
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!((v["targetEnrageMs"].as_i64(), v["targetOutOfCombat"].as_bool()), (Some(298_884), Some(true)));
    }

    #[test]
    fn reading_back_fills_only_the_optional_fields() {
        let full = r#"{"map":{},"targetName":"","targetMode":"bossTargets","targetId":3,"battleTime":0,
            "targetMaxHp":0,"targetTotalDamage":0,"targetCurrentHp":-1,"dungeonId":0}"#;
        let r: DpsData = serde_json::from_str(full).unwrap();
        assert_eq!((r.target_id, r.target_mob_code, r.local_player_id), (3, 0, None));
        assert!(r.taken.is_empty() && r.deaths.is_empty());
        let no_dungeon = full.replace(r#","dungeonId":0"#, "");
        assert!(serde_json::from_str::<DpsData>(&no_dungeon).is_err());
    }
}
