//! Deaths: how often you and your party fell to 0 HP in a fight.

use serde::{Deserialize, Serialize};

/// One player's deaths in a fight, in Details and saved fights.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeathEntry {
    pub actor_id: i32,
    pub deaths: u32,
}

/// A player's line in the meter's deaths list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeathRow {
    pub actor_id: i32,
    pub nickname: String,
    #[serde(default)]
    pub job: String,
    /// The player's meter number; 0 without a damage row.
    #[serde(default)]
    pub number: u32,
    /// `None` while no HP was seen, so the count is unknown.
    #[serde(default)]
    pub deaths: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_count_is_written_as_null() {
        let row = DeathRow { actor_id: 7, nickname: "n".into(), job: String::new(), number: 0, deaths: None };
        let json = serde_json::to_string(&row).unwrap();
        assert_eq!(json, r#"{"actorId":7,"nickname":"n","job":"","number":0,"deaths":null}"#);
        assert_eq!(serde_json::from_str::<DeathRow>(r#"{"actorId":7,"nickname":"n"}"#).unwrap(), row);
        assert_eq!(serde_json::from_str::<DeathRow>(&json).unwrap(), row);
        let counted: DeathRow = serde_json::from_str(r#"{"actorId":7,"nickname":"n","deaths":2}"#).unwrap();
        assert_eq!(counted.deaths, Some(2));
    }
}
