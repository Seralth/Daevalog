//! What the Details page shows of a fight, and what a saved fight keeps of it.
//! Every key is lower camel case; a field with `default` may be missing in
//! older files.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::deaths::DeathEntry;
use super::taken::TakenSkillEntry;

/// A player or summon owner as Details lists it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActorBreakdown {
    pub actor_id: i32,
    /// Masked by `fight_record::mask_name` in a saved fight, except
    /// for the local player.
    pub nickname: String,
    #[serde(default)]
    pub job: String,
    /// Class prefix id; 0 when unknown.
    #[serde(default)]
    pub job_id: i32,
    #[serde(default)]
    pub party_heal: i64,
    #[serde(default)]
    pub regen: i64,
    #[serde(default)]
    pub damage_received: i64,
    #[serde(default)]
    pub hits_received: i32,
    /// Party roster id, 0 if the roster never named this actor. Kept on this
    /// machine; an upload sends neither the id nor a hash of it.
    #[serde(default)]
    pub dbid: u64,
    #[serde(default)]
    pub server_id: u16,
    #[serde(default)]
    pub level: i32,
    #[serde(default)]
    pub gear_score: i32,
    #[serde(default)]
    pub combat_power: i64,
}

/// One target in the Details target list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetBreakdown {
    pub target_id: i32,
    #[serde(default)]
    pub target_name: String,
    #[serde(default)]
    pub mob_code: i32,
    #[serde(default)]
    pub max_hp: i32,
    pub battle_time: i64,
    pub last_damage_time: i64,
    pub total_damage: i64,
    /// Damage on this target per actor id.
    pub actor_damage: HashMap<i32, i64>,
}

/// Everything the Details page needs to open: the targets, the actors and
/// the fight-wide lists.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailBundle {
    pub current_target_id: i32,
    pub targets: Vec<TargetBreakdown>,
    pub actors: Vec<ActorBreakdown>,
    /// The meter number of every other player, by actor id.
    #[serde(default)]
    pub numbers: HashMap<i32, u32>,
    #[serde(default)]
    pub taken_skills: Vec<TakenSkillEntry>,
    #[serde(default)]
    pub heal_skills: Vec<SkillLine>,
    /// `None` when deaths were not counted.
    #[serde(default)]
    pub deaths: Option<Vec<DeathEntry>>,
    #[serde(default)]
    pub battle_time: i64,
}

/// One heal tick: when it landed, whom it healed and how much.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HealAt {
    pub at: i64,
    pub target: i32,
    pub amount: i64,
}

/// One actor's use of one skill: totals and how the hits landed. In the heal
/// list `dmg` is the amount healed, `time` the tick count and `is_dot` marks
/// a heal over time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillLine {
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
    /// Older files call this flag `smite`.
    #[serde(default, alias = "smite")]
    pub regeneration: i32,
    /// Older files call this flag `powershard`.
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
    /// Live only: neither written nor read back.
    #[serde(skip)]
    pub heal_ticks: Vec<HealAt>,
}

/// A ping sample: when, and the round trip in milliseconds.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencySample {
    pub ts_ms: i64,
    pub ping_ms: i32,
}

/// Details for one target; also the `details` of a saved fight.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetBreakdownReply {
    pub target_id: i32,
    #[serde(default)]
    pub max_hp: i32,
    pub total_target_damage: i64,
    pub battle_time: i64,
    #[serde(default)]
    pub start_time: i64,
    pub skills: Vec<SkillLine>,
    #[serde(default)]
    pub ping_history: Vec<LatencySample>,
    #[serde(default)]
    pub heal_skills: Vec<SkillLine>,
    #[serde(default)]
    pub taken_skills: Vec<TakenSkillEntry>,
    /// `None` in fights saved before deaths were counted. Written as `null`.
    #[serde(default)]
    pub deaths: Option<Vec<DeathEntry>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_skill_row_saved_before_the_flag_rename_reads_under_the_new_names() {
        let old = r#"{"actorId":1,"code":2,"name":"x","time":3,"dmg":4,"multiHitCount":0,"multiHitDamage":0,
            "crit":0,"parry":1,"back":0,"perfect":0,"double":0,"smite":5,"powershard":6,"regen":0}"#;
        let row: SkillLine = serde_json::from_str(old).unwrap();
        assert_eq!((row.parry, row.regeneration, row.perfect_block, row.shield_block, row.miss), (1, 5, 6, 0, 0));
        let json = serde_json::to_string(&row).unwrap();
        assert!(json.contains(r#""regeneration":5"#) && json.contains(r#""perfectBlock":6"#) && !json.contains("smite"));
    }

    const BARE_SKILL: &str = r#"{"actorId":1,"code":2,"name":"x","time":3,"dmg":4,"multiHitCount":0,
        "multiHitDamage":0,"crit":0,"parry":0,"back":0,"perfect":0,"double":0,"regen":0}"#;

    /// Compact JSON of a value; these tests only build values that serialize.
    fn flat<T: Serialize>(v: &T) -> String {
        serde_json::to_string(v).expect("serializable")
    }

    fn load<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
        serde_json::from_str(text).ok()
    }

    /// `text` with `key_value` put in as its first key.
    fn with_key(text: &str, key_value: &str) -> String {
        format!("{{{key_value},{}", &text.trim_start()[1..])
    }

    fn bare_skill() -> SkillLine {
        load(BARE_SKILL).expect("bare skill")
    }

    const BARE_TARGET: &str = r#"{"skills":[],"battleTime":2,"totalTargetDamage":1,"targetId":9}"#;

    fn target(deaths: Option<Vec<DeathEntry>>) -> TargetBreakdownReply {
        TargetBreakdownReply { deaths, ..load(BARE_TARGET).expect("bare target") }
    }

    #[test]
    fn heal_ticks_stay_in_memory() {
        let mut row = bare_skill();
        row.heal_ticks = vec![HealAt { at: 1, target: 2, amount: 3 }; 3];
        assert!(!flat(&row).contains("healTicks"));
        let back: SkillLine = load(&with_key(BARE_SKILL, r#""healTicks":[1]"#)).expect("ticks skipped");
        assert_eq!(back.heal_ticks.len(), 0);
    }

    #[test]
    fn a_bare_skill_row_fills_the_optional_fields() {
        let row = bare_skill();
        assert_eq!((row.multi_hit_hits, row.min_dmg, row.max_dmg, row.frontal, row.iron_wall), (0, 0, 0, 0, 0));
        assert_eq!((row.resist, row.job.as_str(), row.is_dot), (0, "", false));
        assert!(row.hit_timestamps.is_empty() && row.specs.is_empty());
        let no_regen = BARE_SKILL.replace(r#","regen":0"#, "");
        assert!(load::<SkillLine>(&no_regen).is_none());
    }

    #[test]
    fn missing_deaths_and_null_deaths_both_read_as_none() {
        let written = flat(&target(None));
        assert!(written.contains(r#""deaths":null"#), "{written}");
        let reread: TargetBreakdownReply = load(&written).expect("null deaths");
        assert!(reread.deaths.is_none());
        let bare: TargetBreakdownReply = load(BARE_TARGET).expect("no deaths key");
        assert!(bare.deaths.is_none());
        assert_eq!((bare.max_hp, bare.start_time), (0, 0));
        assert!(bare.ping_history.is_empty() && bare.heal_skills.is_empty() && bare.taken_skills.is_empty());
        let empty: TargetBreakdownReply = load(&with_key(BARE_TARGET, r#""deaths":[]"#)).expect("empty deaths");
        assert_eq!(empty.deaths.map(|d| d.len()), Some(0));
    }

    #[test]
    fn a_target_without_skills_does_not_load() {
        let no_skills = BARE_TARGET.replace(r#""skills":[],"#, "");
        assert!(load::<TargetBreakdownReply>(&no_skills).is_none());
    }

    #[test]
    fn deaths_survive_a_round_trip_and_extra_keys_are_skipped() {
        let fell = vec![DeathEntry { actor_id: 4, deaths: 2 }];
        let text = with_key(&flat(&target(Some(fell.clone()))), r#""foo":1"#);
        assert_eq!(load::<TargetBreakdownReply>(&text).expect("extra key").deaths, Some(fell));
    }

    #[test]
    fn a_bare_actor_and_a_ping_take_defaults() {
        let who: ActorBreakdown = load(r#"{"nickname":"n","actorId":3}"#).expect("bare actor");
        assert_eq!((who.job.as_str(), who.job_id, who.party_heal, who.regen), ("", 0, 0, 0));
        assert_eq!((who.damage_received, who.hits_received, who.dbid, who.server_id), (0, 0, 0, 0));
        assert_eq!((who.level, who.gear_score, who.combat_power), (0, 0, 0));
        assert!(load::<ActorBreakdown>(r#"{"actorId":3}"#).is_none());
        let ping: LatencySample = load(r#"{"tsMs":10,"pingMs":42}"#).expect("ping");
        assert_eq!(flat(&ping), r#"{"tsMs":10,"pingMs":42}"#);
        assert!(load::<LatencySample>(r#"{"tsMs":10}"#).is_none());
    }

    #[test]
    fn integers_out_of_range_fail() {
        let actor = |extra: &str| load::<ActorBreakdown>(&format!(r#"{{"actorId":1,"nickname":"n",{extra}}}"#));
        assert_eq!(actor(r#""dbid":5"#).map(|a| a.dbid), Some(5));
        for bad in [r#""dbid":-1"#, r#""serverId":70000"#, r#""level":1.5"#] {
            assert!(actor(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn actor_maps_use_string_keys() {
        let mut t: TargetBreakdown =
            load(r#"{"targetId":1,"battleTime":0,"lastDamageTime":0,"totalDamage":100,"actorDamage":{}}"#)
                .expect("target summary");
        t.actor_damage.insert(5, 100);
        let written = flat(&t);
        assert!(written.contains(r#""actorDamage":{"5":100}"#), "{written}");
        let reread: TargetBreakdown = load(&written).expect("summary back");
        assert_eq!(reread.actor_damage[&5], 100);

        let mut ctx: DetailBundle = load(r#"{"currentTargetId":1,"targets":[],"actors":[]}"#).expect("context");
        assert!(ctx.numbers.is_empty() && ctx.deaths.is_none() && ctx.battle_time == 0);
        ctx.numbers.insert(7, 2);
        ctx.targets.push(t);
        let written = flat(&ctx);
        assert!(written.contains(r#""numbers":{"7":2}"#) && written.contains(r#""deaths":null"#), "{written}");
    }
}
