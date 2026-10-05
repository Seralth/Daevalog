//! Which row a skill's damage goes to.
//!
//! The game's own Damage Analyzer reports some skills under another id: the
//! ranks of a spirit's basic attack under the first, a follow-up under the
//! skill it follows (Skill table, `DamageAnalyzerSkillIdOverride`). The meter
//! uses the same rows, so its per-skill numbers line up with the game's.

use std::collections::HashMap;

use crate::i18n::lookup::SkillLookup;

static GROUPS: std::sync::LazyLock<HashMap<i32, i32>> = std::sync::LazyLock::new(|| {
    #[derive(serde::Deserialize)]
    struct Table {
        groups: HashMap<String, i32>,
    }
    serde_json::from_str::<Table>(include_str!("../../../src/data/skill_groups.json"))
        .map(|t| t.groups.into_iter().filter_map(|(k, v)| Some((k.parse().ok()?, v))).collect())
        .unwrap_or_default()
});

/// The id a skill's damage is reported under. A skill the game table groups
/// takes the game's id; any other is folded into its base skill (the code
/// rounded down to 10000) when both carry the same name.
pub fn row_skill(raw: i32, skills: &SkillLookup) -> i32 {
    if let Some(&group) = GROUPS.get(&raw) {
        return group;
    }
    if (30_000_000..=30_999_999).contains(&raw) {
        return raw;
    }
    let base = raw - (raw % 10000);
    let base_name = skills.get_skill_name(base);
    if base_name.is_empty() {
        return raw;
    }
    let raw_name = skills.get_skill_name(raw);
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
}
