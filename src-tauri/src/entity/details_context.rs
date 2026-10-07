use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailsActorSummary {
    pub actor_id: i32,
    pub nickname: String,
    #[serde(default)]
    pub job: String,
    /// Job class prefix ID (e.g. 11=Gladiator) for language-independent storage
    #[serde(default)]
    pub job_id: i32,
    #[serde(default)]
    pub party_heal: i64,
    #[serde(default)]
    pub regen: i64,
    /// Damage taken, and the attacks on the actor (`TakenStats::total`).
    #[serde(default)]
    pub damage_received: i64,
    #[serde(default)]
    pub hits_received: i32,
    /// Roster id for this actor, or 0 when the party roster never named them
    /// (solo play, or a player who joined before the roster packet arrived).
    ///
    /// Local-only. It is deliberately *not* what goes on the wire when a fight
    /// is shared: uploads carry `sha256(dbid)` so the server can tell two rows
    /// apart without learning who they are. See `docs/PRIVACY.md`.
    #[serde(default)]
    pub dbid: u64,
    /// World/server id, the top 16 bits of `dbid`. 0 when unknown.
    #[serde(default)]
    pub server_id: u16,
    /// Character level, gear score and combat power from the party roster, as
    /// the game reported them for this fight. 0 when the roster never named
    /// the actor: only your own party's members are on it.
    ///
    /// Saved with the fight because they change: a gear score from today says
    /// nothing about a fight from last month, and the class statistics compare
    /// players at like strength.
    #[serde(default)]
    pub level: i32,
    #[serde(default)]
    pub gear_score: i32,
    #[serde(default)]
    pub combat_power: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailsTargetSummary {
    pub target_id: i32,
    #[serde(default)]
    pub target_name: String,
    /// The npc's code, which names it in every language; 0 when not known.
    #[serde(default)]
    pub mob_code: i32,
    #[serde(default)]
    pub max_hp: i32,
    pub battle_time: i64,
    pub last_damage_time: i64,
    pub total_damage: i64,
    pub actor_damage: std::collections::HashMap<i32, i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailsContext {
    pub current_target_id: i32,
    pub targets: Vec<DetailsTargetSummary>,
    pub actors: Vec<DetailsActorSummary>,
    /// Each other player's number, as on the meter. See `PersonalData::number`.
    #[serde(default)]
    pub numbers: std::collections::HashMap<i32, u32>,
    /// The damage taken that `actors` add up, per player and skill. Details
    /// lists it for several targets at once: each target's own list would
    /// count a hit taken during two fights twice.
    #[serde(default)]
    pub taken_skills: Vec<super::taken::TakenSkillEntry>,
    /// The healing done over the listed targets' fights, per healer and skill,
    /// each tick once (`dmg` = heal amount, `time` = tick count, `is_dot` =
    /// HoT). Details shows it on every target: each target's own list holds
    /// the ticks of that target's span, so targets fought at once overlap.
    #[serde(default)]
    pub heal_skills: Vec<DetailSkillEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailSkillEntry {
    pub actor_id: i32,
    pub code: i32,
    pub name: String,
    pub time: i32,
    pub dmg: i64,
    pub multi_hit_count: i32,
    pub multi_hit_damage: i64,
    #[serde(default)]
    pub multi_hit_hits: i32,
    #[serde(default)]
    pub min_dmg: i64,
    #[serde(default)]
    pub max_dmg: i64,
    pub crit: i32,
    #[serde(default)]
    pub shield_block: i32,
    pub parry: i32,
    pub back: i32,
    #[serde(default)]
    pub frontal: i32,
    pub perfect: i32,
    pub double: i32,
    #[serde(default)]
    pub iron_wall: i32,
    // Saved fights before the flags were renamed have the old names.
    #[serde(default, alias = "smite")]
    pub regeneration: i32,
    #[serde(default, alias = "powershard")]
    pub perfect_block: i32,
    #[serde(default)]
    pub miss: i32,
    #[serde(default)]
    pub resist: i32,
    pub regen: i64,
    #[serde(default)]
    pub job: String,
    #[serde(default)]
    pub is_dot: bool,
    #[serde(default)]
    pub hit_timestamps: Vec<i64>,
    #[serde(default)]
    pub specs: Vec<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PingPoint {
    pub ts_ms: i64,
    pub ping_ms: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetDetailsResponse {
    pub target_id: i32,
    #[serde(default)]
    pub max_hp: i32,
    pub total_target_damage: i64,
    pub battle_time: i64,
    #[serde(default)]
    pub start_time: i64,
    pub skills: Vec<DetailSkillEntry>,
    #[serde(default)]
    pub ping_history: Vec<PingPoint>,
    /// Per-actor / per-skill HEALING done in this fight. Reuses DetailSkillEntry:
    /// `dmg` = heal amount, `time` = tick count, `is_dot` = HoT. Empty for old files.
    #[serde(default)]
    pub heal_skills: Vec<DetailSkillEntry>,
    /// Damage taken in this fight, per player and skill. Empty for old files.
    #[serde(default)]
    pub taken_skills: Vec<super::taken::TakenSkillEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_skill_row_saved_before_the_flag_rename_reads_under_the_new_names() {
        let old = r#"{"actorId":1,"code":2,"name":"x","time":3,"dmg":4,"multiHitCount":0,"multiHitDamage":0,
            "crit":0,"parry":1,"back":0,"perfect":0,"double":0,"smite":5,"powershard":6,"regen":0}"#;
        let row: DetailSkillEntry = serde_json::from_str(old).unwrap();
        assert_eq!((row.parry, row.regeneration, row.perfect_block, row.shield_block, row.miss), (1, 5, 6, 0, 0));
        let json = serde_json::to_string(&row).unwrap();
        assert!(json.contains(r#""regeneration":5"#) && json.contains(r#""perfectBlock":6"#) && !json.contains("smite"));
    }
}
