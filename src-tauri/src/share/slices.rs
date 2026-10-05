//! Kept slices: each saved fight's gzipped slice and upload state, in `slices/`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::preview::{covers, gzip, without_roster_ids};
use super::ring;
use crate::capture::evidence_slice::{self, NameMap};
use crate::combat::data_storage::DataStorage;
use crate::entity::fight_record::FightRecord;

/// Where a fight's slice and its upload state live. Beside `history/`, not in
/// it: that directory is scanned as fight records.
pub fn slices_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("slices")
}

pub(super) fn slice_path(app_data_dir: &Path, id: &str) -> PathBuf {
    slices_dir(app_data_dir).join(format!("{id}.a2es.gz"))
}

fn meta_path(app_data_dir: &Path, id: &str) -> PathBuf {
    slices_dir(app_data_dir).join(format!("{id}.json"))
}

/// What the meter remembers about one fight's slice.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SliceMeta {
    /// The local player's entity id in this fight: the one name an upload
    /// shows in full. `None` when the meter never identified the player.
    #[serde(default)]
    pub uploader_actor_id: Option<i32>,
    /// Set once uploaded.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub visibility: Option<String>,
    /// Automatic uploads tried and failed so far. See `note_auto_upload_failure`.
    #[serde(default)]
    pub auto_attempts: u32,
    /// When to try the next automatic upload (ms since the epoch).
    #[serde(default)]
    pub retry_at_ms: i64,
    /// Automatic uploads have stopped for this fight: the failure was one a
    /// retry cannot fix, or the retries ran out. The History button still works.
    #[serde(default)]
    pub gave_up: bool,
}

pub(super) fn read_meta(app_data_dir: &Path, id: &str) -> SliceMeta {
    std::fs::read_to_string(meta_path(app_data_dir, id))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub(super) fn write_meta(app_data_dir: &Path, id: &str, meta: &SliceMeta) {
    if let Ok(json) = serde_json::to_string(meta) {
        let _ = std::fs::write(meta_path(app_data_dir, id), json);
    }
}

/// Every name the meter has resolved, which is what the blinder must remove.
pub fn names_from(storage: &DataStorage) -> NameMap {
    let mut names = NameMap::new();
    for (name, member) in storage.get_party_members() {
        names.insert(name.clone(), member.dbid);
    }
    for name in storage.get_nicknames().values() {
        names.entry(name.clone()).or_insert(0);
    }
    names
}

/// Cut and keep the slice for a fight the meter just saved, from memory.
///
/// Called by the auto-save each time it writes the record, so the slice grows
/// with the fight and the last write is the whole of it. Returns the gzipped
/// size. Failing is normal and quiet: a fight that began before the meter
/// started has no packets behind it.
pub fn save_slice(
    app_data_dir: &Path,
    record: &FightRecord,
    storage: &DataStorage,
) -> Result<usize, String> {
    let packets = ring::snapshot();
    let start = record.start_time_ms;
    if !covers(&packets, start, start + record.duration_ms) {
        return Err("no packets in memory for this fight".into());
    }
    let slice = evidence_slice::build(&packets, start, start + record.duration_ms, &names_from(storage))
        .map_err(|e| e.to_string())?;
    write_slice(app_data_dir, &record.id, slice, uploader_in(record, storage))
}

/// Keep a fight's slice on disk, with the player's actor id in it.
pub fn write_slice(
    app_data_dir: &Path,
    id: &str,
    mut slice: evidence_slice::EvidenceSlice,
    uploader_actor_id: Option<i32>,
) -> Result<usize, String> {
    without_roster_ids(&mut slice);
    let compressed = gzip(&evidence_slice::encode(&slice))?;

    let dir = slices_dir(app_data_dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(slice_path(app_data_dir, id), &compressed).map_err(|e| e.to_string())?;
    let mut meta = read_meta(app_data_dir, id);
    meta.uploader_actor_id = uploader_actor_id;
    write_meta(app_data_dir, id, &meta);
    Ok(compressed.len())
}

/// A fight's kept slice, unzipped, and the player's actor id in it.
pub fn read_slice(app_data_dir: &Path, id: &str) -> Option<(Vec<u8>, Option<i32>)> {
    use std::io::Read;
    let compressed = std::fs::read(slice_path(app_data_dir, id)).ok()?;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(&compressed[..]).read_to_end(&mut out).ok()?;
    Some((out, read_meta(app_data_dir, id).uploader_actor_id))
}

/// The player's actor id in a fight's kept slice.
pub fn slice_uploader(app_data_dir: &Path, id: &str) -> Option<i32> {
    read_meta(app_data_dir, id).uploader_actor_id
}

/// The size and time of a fight's kept slice, which change while it is written.
pub fn slice_stamp(app_data_dir: &Path, id: &str) -> Option<(u64, std::time::SystemTime)> {
    let meta = std::fs::metadata(slice_path(app_data_dir, id)).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

/// Your actor id in the fight. The record names you in full, so look for your
/// name there: after a zone load your current id is a stranger's in the fight.
pub fn uploader_in(record: &FightRecord, storage: &DataStorage) -> Option<i32> {
    storage
        .local_character_name()
        .and_then(|name| record.actors.iter().find(|a| a.nickname == name))
        .map(|a| a.actor_id)
        .or_else(|| storage.local_player_id().map(|v| v as i32))
}

/// Remove a fight's slice along with the fight.
pub fn forget_slice(app_data_dir: &Path, id: &str) {
    let _ = std::fs::remove_file(slice_path(app_data_dir, id));
    let _ = std::fs::remove_file(meta_path(app_data_dir, id));
}

/// Drop slices whose fight is gone (history is pruned to a fixed count).
pub fn prune_slices(app_data_dir: &Path) {
    let Ok(rd) = std::fs::read_dir(slices_dir(app_data_dir)) else { return };
    for entry in rd.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        let id = name.trim_end_matches(".a2es.gz").trim_end_matches(".json");
        if !app_data_dir.join("history").join(format!("{id}.json")).exists() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Which saved fights can be uploaded, and which already were.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareStatus {
    pub has_slice: bool,
    pub url: Option<String>,
}

pub fn share_status(app_data_dir: &Path) -> HashMap<String, ShareStatus> {
    let mut out: HashMap<String, ShareStatus> = HashMap::new();
    let Ok(rd) = std::fs::read_dir(slices_dir(app_data_dir)) else { return out };
    for entry in rd.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(id) = name.strip_suffix(".a2es.gz") {
            out.entry(id.to_string())
                .or_insert(ShareStatus { has_slice: false, url: None })
                .has_slice = true;
        } else if let Some(id) = name.strip_suffix(".json") {
            let url = read_meta(app_data_dir, id).url;
            out.entry(id.to_string())
                .or_insert(ShareStatus { has_slice: false, url: None })
                .url = url;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_status_pairs_a_slice_with_its_upload() {
        let dir = std::env::temp_dir().join(format!("a2t-share-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(slices_dir(&dir)).unwrap();
        std::fs::write(slice_path(&dir, "auto_1_2"), b"x").unwrap();
        write_meta(&dir, "auto_1_2", &SliceMeta {
            uploader_actor_id: Some(5),
            url: Some("https://a2tools.app/logs/abc".into()),
            visibility: None,
            ..Default::default()
        });
        std::fs::write(slice_path(&dir, "auto_3_4"), b"x").unwrap();
        let status = share_status(&dir);
        assert!(status["auto_1_2"].has_slice);
        assert_eq!(status["auto_1_2"].url.as_deref(), Some("https://a2tools.app/logs/abc"));
        assert!(status["auto_3_4"].has_slice && status["auto_3_4"].url.is_none());

        // A slice whose fight is gone is removed; one whose fight exists stays.
        std::fs::create_dir_all(dir.join("history")).unwrap();
        std::fs::write(dir.join("history").join("auto_3_4.json"), b"{}").unwrap();
        prune_slices(&dir);
        let status = share_status(&dir);
        assert!(!status.contains_key("auto_1_2"));
        assert!(status.contains_key("auto_3_4"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
