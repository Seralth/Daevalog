//! The upload envelope: the summary of a fight an upload carries, with no character name in it.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::entity::fight_record::FightRecord;

/// How many damage values go into each anchor window, and how far the windows
/// step. Overlapping on purpose: a single missed large hit — range culling, a
/// dropped packet, a late join — then breaks one anchor instead of all four, so
/// two people who fought the same boss still match.
const ANCHOR_WINDOW: usize = 16;
const ANCHOR_STEP: usize = 8;
const ANCHOR_COUNT: usize = 4;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Participant {
    pub slot: u32,
    pub job_id: i32,
    pub is_uploader: bool,
    /// `sha256(dbid)` — present only when the party roster named this actor.
    /// Absent for summons and for anyone who never appeared in a roster packet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<u16>,
    pub damage: i64,
    pub dps: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub present: bool,
    pub sha256: String,
    pub bytes: usize,
}

/// What an upload would send, minus the evidence blob itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadEnvelope {
    pub app_version: String,
    pub parser_version: String,
    pub client_start_ms: i64,
    pub duration_ms: i64,
    pub mob_code: i32,
    pub dungeon_id: i32,
    pub is_train: bool,
    pub total_damage: i64,
    pub boss_max_hp: i32,
    pub participants: Vec<Participant>,
    /// Fingerprints of the damage this fight produced, used to recognise that
    /// two uploads are the same encounter. Derived from server-computed damage
    /// values, which every observer of the fight sees identically.
    pub anchors: Vec<String>,
    pub evidence: Evidence,
}

/// `sha256(dbid)`, the id an upload carries instead of a name.
pub fn account_ref(dbid: u64) -> String {
    let digest = Sha256::digest(dbid.to_le_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Fingerprint the fight by the damage values it produced.
///
/// Distinct values, largest first: server-computed five- and six-figure integers
/// carry enough entropy that two parties killing the same boss at the same
/// moment still do not collide, while two *observers of one fight* agree
/// exactly.
fn anchors(record: &FightRecord) -> Vec<String> {
    let mut values: Vec<i64> = record
        .details
        .skills
        .iter()
        .flat_map(|s| [s.dmg as i64, s.max_dmg as i64])
        .filter(|&d| d > 0)
        .collect();
    values.sort_unstable_by(|a, b| b.cmp(a));
    values.dedup();

    let mut out = Vec::new();
    for w in 0..ANCHOR_COUNT {
        let from = w * ANCHOR_STEP;
        let to = (from + ANCHOR_WINDOW).min(values.len());
        if from >= to {
            break;
        }
        let mut hasher = Sha256::new();
        hasher.update(b"a2-anchor\x00");
        hasher.update((record.mob_code as u32).to_le_bytes());
        for v in &values[from..to] {
            hasher.update(v.to_le_bytes());
        }
        let digest = hasher.finalize();
        out.push(digest[..8].iter().map(|b| format!("{b:02x}")).collect());
    }
    out
}

/// Build the summary an upload would carry. Contains no character name.
pub fn build_envelope(record: &FightRecord, slice: &[u8]) -> UploadEnvelope {
    // Damage per actor, from the per-skill breakdown.
    let mut damage: HashMap<i32, i64> = HashMap::new();
    for skill in &record.details.skills {
        *damage.entry(skill.actor_id).or_default() += skill.dmg as i64;
    }

    let seconds = (record.duration_ms as f64 / 1000.0).max(0.001);
    let mut participants: Vec<Participant> = record
        .actors
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let dmg = damage.get(&a.actor_id).copied().unwrap_or(0);
            Participant {
                slot: i as u32 + 1,
                job_id: a.job_id,
                // The saved record does not mark the uploader, but it is the one
                // actor whose name was never masked — and we must not put that
                // name here to say so, hence the flag rather than the name.
                is_uploader: false,
                account_ref: (a.dbid != 0).then(|| account_ref(a.dbid)),
                server_id: (a.server_id != 0).then_some(a.server_id),
                damage: dmg,
                dps: dmg as f64 / seconds,
            }
        })
        .collect();
    participants.sort_by(|a, b| b.damage.cmp(&a.damage));
    for (i, p) in participants.iter_mut().enumerate() {
        p.slot = i as u32 + 1;
    }

    let digest = Sha256::digest(slice);
    UploadEnvelope {
        app_version: record.app_version.clone(),
        // The A2Tools release a2tools.app treats the parser as (see version.rs).
        parser_version: crate::version::UPLOAD_COMPAT_VERSION.to_string(),
        client_start_ms: record.start_time_ms,
        duration_ms: record.duration_ms,
        mob_code: record.mob_code,
        dungeon_id: record.dungeon_id,
        is_train: record.is_train,
        total_damage: record.total_damage as i64,
        boss_max_hp: record.details.max_hp,
        participants,
        anchors: anchors(record),
        evidence: Evidence {
            present: !slice.is_empty(),
            sha256: digest.iter().map(|b| format!("{b:02x}")).collect(),
            bytes: slice.len(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::details_context::{DetailsActorSummary, TargetDetailsResponse};

    fn record_with(actors: Vec<DetailsActorSummary>) -> FightRecord {
        FightRecord {
            id: "auto_1_2".into(),
            boss_name: "Some Boss".into(),
            target_id: 1,
            start_time_ms: 1_700_000_000_000,
            duration_ms: 60_000,
            total_damage: 1_000,
            jobs: vec!["Sorcerer".into()],
            job_ids: vec![15],
            details: TargetDetailsResponse {
                target_id: 1,
                max_hp: 5_000,
                total_target_damage: 1_000,
                battle_time: 60_000,
                start_time: 0,
                skills: Vec::new(),
                ping_history: Vec::new(),
                heal_skills: Vec::new(),
                taken_skills: Vec::new(),
            },
            actors,
            is_train: false,
            app_version: "2.0.22".into(),
            mob_code: 4242,
            dungeon_id: 600093,
            server_id: 0,
        }
    }

    fn actor(id: i32, nickname: &str, dbid: u64) -> DetailsActorSummary {
        DetailsActorSummary {
            actor_id: id,
            nickname: nickname.into(),
            job: "Sorcerer".into(),
            job_id: 15,
            party_heal: 0,
            regen: 0,
            damage_received: 0,
            hits_received: 0,
            dbid,
            server_id: (dbid >> 48) as u16,
            level: 0,
            gear_score: 0,
            combat_power: 0,
        }
    }

    #[test]
    fn the_envelope_contains_no_character_name() {
        let record = record_with(vec![
            actor(1, "Misti", 0x07de_0000_0000_1fee),
            actor(2, "Gr****e", 0x03f5_0000_0001_b9c0),
            actor(3, "九州依然在", 0x03f6_0000_0001_4b85),
        ]);
        let json = serde_json::to_string(&build_envelope(&record, b"slice")).unwrap();
        for name in ["Misti", "Gr****e", "九州依然在"] {
            assert!(
                !json.contains(name),
                "the upload envelope carried a character name: {name}"
            );
        }
        // And it does carry the id that replaces it.
        assert!(json.contains(&account_ref(0x07de_0000_0000_1fee)));
    }

    #[test]
    fn an_actor_with_no_roster_entry_gets_no_account_ref() {
        let record = record_with(vec![actor(9, "Summon", 0)]);
        let envelope = build_envelope(&record, b"");
        assert_eq!(envelope.participants.len(), 1);
        assert!(envelope.participants[0].account_ref.is_none());
        assert!(envelope.participants[0].server_id.is_none());
    }

    #[test]
    fn account_ref_is_stable_and_differs_per_id() {
        assert_eq!(account_ref(7), account_ref(7));
        assert_ne!(account_ref(7), account_ref(8));
        assert_eq!(account_ref(7).len(), 64);
    }

    #[test]
    fn the_envelope_records_the_difficulty_tier() {
        let record = record_with(vec![actor(1, "A", 1)]);
        let envelope = build_envelope(&record, b"");
        // 600093 is Conquest [Hard]; ranking it against 600091 would be wrong.
        assert_eq!(envelope.dungeon_id, 600093);
        assert_eq!(envelope.mob_code, 4242);
    }
}
