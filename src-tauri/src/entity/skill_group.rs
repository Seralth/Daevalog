//! Which row a skill's damage goes to.
//!
//! The game's own Damage Analyzer reports some skills under another id: the
//! ranks of a spirit's basic attack under the first, a follow-up under the
//! skill it follows (Skill table, `DamageAnalyzerSkillIdOverride`). The meter
//! uses the same rows, so its per-skill numbers line up with the game's.

use std::collections::HashMap;

use crate::i18n::lookup::SkillLookup;

/// Theostone effect ids (item id * 10 + 1): known skills, named or not.
pub const THEOSTONE_EFFECTS: std::ops::RangeInclusive<i32> = 30_000_000..=30_999_999;

static GROUPS: std::sync::LazyLock<HashMap<i32, i32>> = std::sync::LazyLock::new(|| {
    #[derive(serde::Deserialize)]
    struct Table {
        groups: HashMap<String, i32>,
    }
    serde_json::from_str::<Table>(include_str!("../../../src/data/skill_groups.json"))
        .map(|t| t.groups.into_iter().filter_map(|(k, v)| Some((k.parse().ok()?, v))).collect())
        .unwrap_or_default()
});

static RESOURCE_RESTORES: std::sync::LazyLock<std::collections::HashSet<i32>> = std::sync::LazyLock::new(|| {
    #[derive(serde::Deserialize)]
    struct Table {
        skills: Vec<i32>,
    }
    serde_json::from_str::<Table>(include_str!("../../../src/data/resource_restore_skills.json"))
        .map(|t| t.skills.into_iter().collect())
        .unwrap_or_default()
});

static ATTACKS: std::sync::LazyLock<std::collections::HashSet<i32>> = std::sync::LazyLock::new(|| {
    #[derive(serde::Deserialize)]
    struct Table {
        skills: Vec<i32>,
    }
    serde_json::from_str::<Table>(include_str!("../../../src/data/attack_skills.json"))
        .map(|t| t.skills.into_iter().collect())
        .unwrap_or_default()
});

/// Whether a raw skill id restores only MP (or another resource), never HP.
/// Its records carry the amount like a heal does.
pub fn restores_resource(raw: i32) -> bool {
    RESOURCE_RESTORES.contains(&raw)
}

/// Whether a raw skill id attacks: the game's Skill table files it under
/// Attack. Its records are never healing. Raw ids are Skill table ids, but
/// for Theostone procs (30M, item ids), which are not counted as attacks.
pub fn is_attack(raw: i32) -> bool {
    ATTACKS.contains(&raw)
}

/// Skill codes only NPCs use. No player record seen carries one.
pub const NPC_SKILLS: std::ops::RangeInclusive<i32> = 1_000_000..=9_999_999;

/// The id a skill's damage is reported under. A skill the game table groups
/// takes the game's id; any other is folded into its base skill (the code
/// rounded down to 10000) when both carry the same name.
pub fn row_skill(raw: i32, skills: &SkillLookup) -> i32 {
    if let Some(&group) = GROUPS.get(&raw) {
        return group;
    }
    // Theostone procs are item ids; their low digits are not a skill rank.
    if THEOSTONE_EFFECTS.contains(&raw) {
        return raw;
    }
    let base = raw - raw % 10_000;
    let base_name = skills.skill_title(base);
    if base_name.is_empty() {
        return raw;
    }
    let raw_name = skills.skill_title(raw);
    if raw_name.is_empty() || raw_name == base_name { base } else { raw }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_take_the_game_damage_analyzer_rows() {
        let skills = SkillLookup::new();
        skills.load_from_json(r#"{"11050000":"Crushing Wave","11050017":"Crushing Wave","11060000":"Frenzied Wave","12340000":"Strike","12340011":"Strike"}"#);
        // Fire Spirit basic attack ranks, and its summon variant.
        assert_eq!(row_skill(100012, &skills), 100011);
        assert_eq!(row_skill(16990002, &skills), 100011);
        // A follow-up the game files under the skill it follows.
        assert_eq!(row_skill(11050017, &skills), 11060000);
        // Not in the game table: the same-name base rule.
        assert_eq!(row_skill(12340011, &skills), 12340000);
        assert_eq!(row_skill(30_000_123, &skills), 30_000_123);
    }

    #[test]
    fn only_theostone_ids_skip_the_base_rule() {
        let skills = SkillLookup::new();
        skills.load_from_json(r#"{"30000000":"T","30000123":"T","31000000":"U","31000005":"U","29990000":"V","29999999":"V","12340000":"S","12340011":"S"}"#);
        assert_eq!(row_skill(30_000_123, &skills), 30_000_123);
        assert_eq!(row_skill(31_000_005, &skills), 31_000_000);
        assert_eq!(row_skill(29_999_999, &skills), 29_990_000);
        assert_eq!(row_skill(12_340_011, &skills), 12_340_000);
        let negative = SkillLookup::new();
        negative.load_from_json(r#"{"-10000":"N","-12345":"N"}"#);
        assert_eq!(row_skill(-12_345, &negative), -10_000);
    }
}
