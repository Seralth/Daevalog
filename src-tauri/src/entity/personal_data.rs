//! One row of the meter: a player (or a summon's owner) and their numbers.

use serde::{Deserialize, Serialize};

/// Serialized with the camelCase names the 1.x front end reads.
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonalData {
    /// The class name (`JobClass::class_name`), or empty while unknown.
    pub job: String,
    pub dps: f64,
    pub amount: f64,
    pub damage_contribution: f64,
    pub nickname: String,
    pub combat_power: i64,
    #[serde(default)]
    pub active_dps: f64,
    #[serde(default)]
    pub last10_dps: f64,
    #[serde(default)]
    pub last30_dps: f64,
    #[serde(default)]
    pub last60_dps: f64,
    #[serde(default)]
    pub hits: i64,
    #[serde(default)]
    pub crit_hits: i64,
    #[serde(default)]
    pub max_hit: i64,
    #[serde(default)]
    pub number: u32,
}

impl PersonalData {
    /// A row with no class yet and every number at 0.
    pub fn new(nickname: String) -> Self {
        Self::with_job(nickname, String::new())
    }

    pub fn with_job(nickname: String, job: String) -> Self {
        PersonalData { job, nickname, ..Default::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::PersonalData;

    fn all_zero(row: &PersonalData) -> bool {
        [row.dps, row.amount, row.damage_contribution, row.active_dps, row.last10_dps, row.last30_dps, row.last60_dps]
            .iter()
            .all(|&v| v == 0.0)
            && [row.combat_power, row.hits, row.crit_hits, row.max_hit].iter().all(|&v| v == 0)
            && row.number == 0
    }

    #[test]
    fn a_fresh_row_has_a_name_and_nothing_else() {
        let row = PersonalData::new("p1".into());
        assert_eq!(row.nickname, "p1");
        assert_eq!(row.job, "");
        assert!(all_zero(&row));
    }

    #[test]
    fn a_row_can_start_with_its_class() {
        let row = PersonalData::with_job("p2".into(), "검성".into());
        assert_eq!(row.nickname, "p2");
        assert_eq!(row.job, "검성");
        assert!(all_zero(&row));
    }

    #[test]
    fn a_row_is_written_with_the_front_end_names_in_order() {
        let mut row = PersonalData::with_job("p3".into(), "궁성".into());
        row.number = 4;
        let text = serde_json::to_string(&row).unwrap();
        assert_eq!(
            text,
            r#"{"job":"궁성","dps":0.0,"amount":0.0,"damageContribution":0.0,"nickname":"p3","combatPower":0,"activeDps":0.0,"last10Dps":0.0,"last30Dps":0.0,"last60Dps":0.0,"hits":0,"critHits":0,"maxHit":0,"number":4}"#
        );
    }

    #[test]
    fn reading_a_row_back_needs_only_the_first_six() {
        let row: PersonalData = serde_json::from_str(
            r#"{"job":"","dps":1.5,"amount":3.0,"damageContribution":50.0,"nickname":"p4","combatPower":9}"#,
        )
        .unwrap();
        assert_eq!((row.dps, row.amount, row.combat_power), (1.5, 3.0, 9));
        assert_eq!((row.active_dps, row.hits, row.max_hit, row.number), (0.0, 0, 0, 0));
        let short = serde_json::from_str::<PersonalData>(r#"{"job":"","dps":1.5,"amount":3.0,"damageContribution":50.0,"nickname":"p4"}"#);
        assert!(short.is_err());
    }
}
