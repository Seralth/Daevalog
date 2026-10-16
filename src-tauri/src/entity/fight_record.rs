//! A saved fight, as History keeps it on disk, and the short form History
//! lists. The file is one `FightRecord` as pretty JSON: field order and keys
//! are the file format.

use serde::{Deserialize, Serialize};

use crate::combat::timeline_tracks::SavedRecords;
use super::detail_bundle::{ActorBreakdown, TargetBreakdownReply};

/// One saved fight. Its id is also its file name.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FightRecord {
    pub id: String,
    /// The display name when the fight was saved.
    pub boss_name: String,
    pub target_id: i32,
    pub start_time_ms: i64,
    pub duration_ms: i64,
    pub total_damage: i64,
    /// Class names, such as "Cleric".
    pub jobs: Vec<String>,
    #[serde(default)]
    pub job_ids: Vec<i32>,
    pub details: TargetBreakdownReply,
    pub actors: Vec<ActorBreakdown>,
    #[serde(default)]
    pub is_train: bool,
    #[serde(default)]
    pub app_version: String,
    #[serde(default)]
    pub mob_code: i32,
    /// Instance id; 0 in the open world.
    #[serde(default)]
    pub dungeon_id: i32,
    /// 0 when unknown.
    #[serde(default)]
    pub server_id: u16,
    /// Left out of the file when there is nothing in it.
    #[serde(default, skip_serializing_if = "SavedRecords::is_empty")]
    pub timeline: SavedRecords,
}

/// One row of History's list. Built from a saved file, never written.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FightSummary {
    pub id: String,
    pub boss_name: String,
    pub target_id: i32,
    pub start_time_ms: i64,
    pub duration_ms: i64,
    pub total_damage: i64,
    pub jobs: Vec<String>,
    #[serde(default)]
    pub job_ids: Vec<i32>,
    #[serde(default)]
    pub is_train: bool,
    #[serde(default)]
    pub is_live: bool,
    #[serde(default)]
    pub app_version: String,
    #[serde(default)]
    pub mob_code: i32,
    #[serde(default)]
    pub dungeon_id: i32,
    #[serde(default)]
    pub member_jobs: Vec<String>,
}

/// True for a nickname that is only an entity id, maybe masked: ASCII
/// digits, `*` and `#`. An empty name is not one.
fn named_by_bare_id(nick: &str) -> bool {
    // Any byte of a non-ASCII character fails the match too.
    let id_byte = |b: u8| matches!(b, b'0'..=b'9' | b'*' | b'#');
    !nick.is_empty() && nick.bytes().all(id_byte)
}

impl FightRecord {
    /// The class of each party member, sorted, for History's icons. When the
    /// party roster named anyone, only roster members count; otherwise every
    /// classed actor with a real name does.
    pub fn party_jobs(&self) -> Vec<String> {
        let by_roster = !self.actors.iter().all(|a| a.dbid == 0);
        let mut classes = Vec::new();
        for actor in &self.actors {
            let member = if by_roster { actor.dbid != 0 } else { !named_by_bare_id(&actor.nickname) };
            if member && !actor.job.is_empty() {
                classes.push(actor.job.clone());
            }
        }
        classes.sort_unstable();
        classes
    }
}

/// Masks a nickname for a saved fight: the start and the last character
/// stay, at most four `*` go between. Counts characters, not bytes.
pub fn mask_name(name: &str) -> String {
    let len = name.chars().count();
    if len < 2 {
        return name.to_owned();
    }
    let lead = if len > 3 { 2 } else { 1 };
    let stars = if len > 3 { (len - 3).min(4) } else { 1 };
    let mut masked: String = name.chars().take(lead).collect();
    masked.extend(std::iter::repeat('*').take(stars));
    if len > 2 {
        masked.extend(name.chars().last());
    }
    masked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(nickname: &str, job: &str, dbid: u64) -> ActorBreakdown {
        let v = serde_json::json!({ "dbid": dbid, "job": job, "nickname": nickname, "actorId": 1 });
        serde_json::from_value(v).unwrap()
    }

    const MINIMAL: &str = r#"{"id":"auto_1_2","bossName":"b","targetId":1,"startTimeMs":2,"durationMs":3,
        "totalDamage":4,"jobs":["Cleric"],"actors":[],
        "details":{"targetId":1,"totalTargetDamage":4,"battleTime":3,"skills":[]}}"#;

    fn minimal() -> FightRecord {
        parse(MINIMAL).expect("minimal record")
    }

    fn parse(text: &str) -> Option<FightRecord> {
        serde_json::from_str(text).ok()
    }

    /// The record as History writes it to disk.
    fn as_file(r: &FightRecord) -> String {
        serde_json::to_string_pretty(r).expect("record serializes")
    }

    /// `text` with `key_value` put in as its first key.
    fn with_key(text: &str, key_value: &str) -> String {
        format!("{{{key_value},{}", &text.trim_start()[1..])
    }

    fn party(actors: Vec<ActorBreakdown>) -> FightRecord {
        FightRecord { actors, ..minimal() }
    }

    #[test]
    fn missing_optional_keys_take_defaults() {
        let r = minimal();
        assert!(r.job_ids.is_empty() && !r.is_train && r.app_version.is_empty());
        assert_eq!((r.mob_code, r.dungeon_id, r.server_id), (0, 0, 0));
        assert_eq!(r.timeline, SavedRecords::default());
        assert!(r.details.deaths.is_none());
    }

    #[test]
    fn a_record_without_jobs_does_not_load() {
        let no_jobs = MINIMAL.replace(r#""jobs":["Cleric"],"#, "");
        assert!(parse(&no_jobs).is_none());
    }

    #[test]
    fn a_saved_record_also_reads_as_a_summary() {
        let file = with_key(&as_file(&minimal()), r#""foo":1"#);
        let row: FightSummary = serde_json::from_str(&file).expect("summary of a saved file");
        assert_eq!((row.id.as_str(), row.start_time_ms, row.jobs.len()), ("auto_1_2", 2, 1));
        assert!(!row.is_live && row.member_jobs.is_empty());
    }

    #[test]
    fn a_summary_needs_only_the_list_fields() {
        let bare = r#"{"id":"a","bossName":"b","targetId":1,"startTimeMs":2,"durationMs":3,"totalDamage":4,"jobs":[]}"#;
        let row: FightSummary = serde_json::from_str(bare).expect("bare summary");
        assert!(row.job_ids.is_empty() && !row.is_train && row.app_version.is_empty());
        assert_eq!((row.mob_code, row.dungeon_id), (0, 0));
        let sent = serde_json::to_string(&row).expect("summary serializes");
        assert!(sent.contains(r#""isLive":false"#) && sent.contains(r#""memberJobs":[]"#), "{sent}");
        assert!(serde_json::from_str::<FightSummary>(&bare.replace(r#","jobs":[]"#, "")).is_err());
    }

    #[test]
    fn server_id_must_fit_in_u16() {
        assert!(parse(&with_key(MINIMAL, r#""serverId":70000"#)).is_none());
        let top = parse(&with_key(MINIMAL, r#""serverId":65535"#)).map(|r| r.server_id);
        assert_eq!(top, Some(u16::MAX));
    }

    #[test]
    fn timeline_is_written_only_when_it_holds_something() {
        let mut fight = minimal();
        fight.timeline.you = 5;
        assert!(!as_file(&fight).contains("timeline"));
        fight.timeline.abnormals.push([1, 2, 3, 4, 5, 6]);
        let file = as_file(&fight);
        assert!(file.contains(r#""timeline": {"#), "{file}");
        assert_eq!(parse(&file).map(|r| r.timeline), Some(fight.timeline));
    }

    #[test]
    fn a_save_reads_back_to_the_same_bytes() {
        let mut fight = minimal();
        fight.app_version = "2.0.0".into();
        fight.actors = vec![person("Ab*c", "Cleric", 7)];
        fight.details.deaths = Some(vec![]);
        let first = as_file(&fight);
        assert_eq!(parse(&first).map(|r| as_file(&r)), Some(first));
    }

    #[test]
    fn an_old_style_file_loads() {
        let old = r#"{"id":"auto_1_2","bossName":"b","targetId":1,"startTimeMs":2,"durationMs":3,
            "totalDamage":4,"jobs":["Cleric"],"actors":[{"actorId":1,"nickname":"A*"}],
            "details":{"targetId":1,"totalTargetDamage":4,"battleTime":3,"skills":[
              {"actorId":1,"code":2,"name":"x","time":3,"dmg":4,"multiHitCount":0,"multiHitDamage":0,
               "crit":0,"parry":0,"back":0,"perfect":0,"double":0,"smite":2,"powershard":3,"regen":0}]}}"#;
        let fight = parse(old).expect("1.x file");
        let skill = &fight.details.skills[0];
        assert_eq!((skill.regeneration, skill.perfect_block), (2, 3));
        assert!(fight.details.deaths.is_none() && fight.timeline.is_empty());
        assert_eq!((fight.actors[0].dbid, fight.actors[0].job.as_str()), (0, ""));
    }

    #[test]
    fn roster_members_decide_member_jobs_when_any_exist() {
        let r = party(vec![
            person("a", "Cleric", 5),
            person("b", "Cleric", 6),
            person("c", "Sorcerer", 0),
            person("d", "", 7),
        ]);
        assert_eq!(r.party_jobs(), ["Cleric", "Cleric"]);
    }

    #[test]
    fn without_a_roster_bare_ids_are_left_out() {
        let r = party(vec![
            person("Ab****x", "Gladiator", 0),
            person("12****6", "Cleric", 0),
            person("", "Ranger", 0),
            person("#12", "Chanter", 0),
            person("가*", "Templar", 0),
        ]);
        assert_eq!(r.party_jobs(), ["Gladiator", "Ranger", "Templar"]);
    }

    #[test]
    fn member_jobs_are_sorted_with_repeats() {
        let r = party(["Sorcerer", "Cleric", "Gladiator", "Cleric"].iter().map(|j| person("Xy", j, 0)).collect());
        assert_eq!(r.party_jobs(), ["Cleric", "Cleric", "Gladiator", "Sorcerer"]);
        assert!(party(vec![]).party_jobs().is_empty());
    }

    #[test]
    fn masking_by_length() {
        // Prefixes of one alphabet, by length; index 0 is the empty name.
        let want = ["", "A", "A*", "A*C", "AB*D", "AB**E", "AB***F", "AB****G", "AB****H", "AB****I", "AB****J"];
        let letters = "ABCDEFGHIJ";
        for (len, masked) in want.iter().enumerate() {
            assert_eq!(mask_name(&letters[..len]), *masked, "length {len}");
        }
        assert_eq!(mask_name("123456"), "12***6");
    }

    #[test]
    fn masking_counts_characters_not_bytes() {
        assert_eq!(mask_name("가나"), "가*");
        assert_eq!(mask_name("가나다"), "가*다");
        assert_eq!(mask_name("가나다라마"), "가나**마");
    }
}
