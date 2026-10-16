//! Kept slices: each saved fight's gzipped slice and upload state, in `slices/`.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};

use super::ring;
use crate::capture::fight_tape::{self, SliceRecord, FightTape, NameMap};
use crate::combat::data_storage::DataStorage;
use crate::entity::fight_record::FightRecord;
use crate::platform::files::{create_private_dir, write_private};

const GZ_END: &str = ".a2es.gz";
const META_END: &str = ".json";

/// What is known about a kept slice besides its bytes: who recorded it and
/// how its uploads went. Saved as `<id>.json`, a format players already have
/// on disk, so fields keep their names and order; a missing one reads as its
/// default.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SliceSidecar {
    pub uploader_actor_id: Option<i32>,
    pub url: Option<String>,
    pub visibility: Option<String>,
    pub auto_attempts: u32,
    pub retry_at_ms: i64,
    pub gave_up: bool,
}

/// Whether a fight has a kept slice, and where it was uploaded.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SliceState {
    pub has_slice: bool,
    pub url: Option<String>,
}

/// `<app data>/slices`. Beside `history/`, never in it: that folder is read
/// as fight records.
pub fn slice_folder(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("slices")
}

/// `<app data>/slices/<id><ending>`.
fn file_for(app_data_dir: &Path, id: &str, ending: &str) -> PathBuf {
    let mut path = slice_folder(app_data_dir);
    path.push(id.to_owned() + ending);
    path
}

pub(super) fn slice_bin_path(app_data_dir: &Path, id: &str) -> PathBuf {
    file_for(app_data_dir, id, GZ_END)
}

/// The names of the files in `slices/`; none when there is no such folder.
fn listed(app_data_dir: &Path) -> Vec<String> {
    match std::fs::read_dir(slice_folder(app_data_dir)) {
        Ok(listing) => listing.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect(),
        Err(_) => Vec::new(),
    }
}

/// The ids of the fights with a meta file in `slices/`.
pub(super) fn meta_ids(app_data_dir: &Path) -> Vec<String> {
    listed(app_data_dir).into_iter().filter_map(|name| name.strip_suffix(META_END).map(String::from)).collect()
}

/// The fight id a file in `slices/` belongs to.
fn owner_id(file_name: &str) -> &str {
    file_name.strip_suffix(GZ_END).or_else(|| file_name.strip_suffix(META_END)).unwrap_or(file_name)
}

/// A fight's meta, or the default when there is none or it does not parse.
pub(super) fn load_sidecar(app_data_dir: &Path, id: &str) -> SliceSidecar {
    std::fs::read(file_for(app_data_dir, id, META_END))
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

/// Best effort: a meta that cannot be written only loses upload state.
pub(super) fn save_sidecar(app_data_dir: &Path, id: &str, meta: &SliceSidecar) {
    let Ok(json) = serde_json::to_string(meta) else {
        return;
    };
    let _ = write_private(file_for(app_data_dir, id, META_END), json);
}

/// Gzip at the best level with the default header. Kept slices are hashed
/// byte for byte, so neither may change.
pub fn gzip(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut packer = GzEncoder::new(Vec::new(), Compression::best());
    packer.write_all(data).and_then(|()| packer.finish()).map_err(|why| why.to_string())
}

/// Whether the packets reach into the window: the first by its end, the last
/// from its start.
pub(super) fn covers(packets: &[SliceRecord], start_ms: i64, end_ms: i64) -> bool {
    match (packets.first(), packets.last()) {
        (Some(first), Some(last)) => first.captured_at_ms <= end_ms && last.captured_at_ms >= start_ms,
        _ => false,
    }
}

/// Keeps the slice's names but sets each roster id to 0: the id is the
/// player's account.
pub(super) fn strip_party_ids(slice: &mut FightTape) {
    slice.blind_map.values_mut().for_each(|id| *id = 0);
}

/// Every name the meter has resolved, so the slice can blind them: party
/// members with their roster id, then anyone else it has named, with 0.
pub fn names_from(storage: &DataStorage) -> NameMap {
    let mut known: NameMap = storage.roster_entries().into_iter().map(|(name, m)| (name, m.dbid)).collect();
    storage.all_names().into_values().for_each(|n| {
        known.entry(n).or_insert(0);
    });
    known
}

/// Cuts the fight's slice from the capture ring and keeps it. An error means
/// the ring does not reach the fight or the slice could not be built.
pub fn store_slice(app_data_dir: &Path, record: &FightRecord, storage: &DataStorage) -> Result<usize, String> {
    let held = ring::snapshot();
    let start = record.start_time_ms;
    let end = start + record.duration_ms;
    if covers(&held, start, end) {
        let slice = fight_tape::build(&held, start, end, &names_from(storage)).map_err(|e| e.to_string())?;
        write_slice(app_data_dir, &record.id, slice, uploader_in(record, storage))
    } else {
        Err(format!("no captured traffic covers fight {}", record.id))
    }
}

/// Writes `<id>.a2es.gz` (roster ids left out) and records the uploader in
/// the meta, keeping the rest of it. Gives the gzipped size.
pub fn write_slice(
    app_data_dir: &Path,
    id: &str,
    mut slice: FightTape,
    uploader_actor_id: Option<i32>,
) -> Result<usize, String> {
    strip_party_ids(&mut slice);
    let packed = gzip(&fight_tape::encode(&slice))?;
    let folder = slice_folder(app_data_dir);
    create_private_dir(&folder).map_err(|e| format!("could not create {}: {e}", folder.display()))?;
    let target = slice_bin_path(app_data_dir, id);
    write_private(&target, &packed).map_err(|e| format!("could not write {}: {e}", target.display()))?;
    let mut meta = load_sidecar(app_data_dir, id);
    meta.uploader_actor_id = uploader_actor_id;
    save_sidecar(app_data_dir, id, &meta);
    Ok(packed.len())
}

/// Drops a deleted fight's slice and meta.
pub fn delete_slice_files(app_data_dir: &Path, id: &str) {
    for ending in [GZ_END, META_END] {
        let _ = std::fs::remove_file(file_for(app_data_dir, id, ending));
    }
}

/// Deletes every file in `slices/` whose fight is no longer in the history.
pub fn trim_old_slices(app_data_dir: &Path) {
    let history = app_data_dir.join("history");
    let folder = slice_folder(app_data_dir);
    listed(app_data_dir)
        .into_iter()
        .filter(|name| !history.join(owner_id(name).to_owned() + META_END).exists())
        .for_each(|name| {
            let _ = std::fs::remove_file(folder.join(name));
        });
}

/// Each fight with a file in `slices/`, by id.
pub fn share_status(app_data_dir: &Path) -> HashMap<String, SliceState> {
    fn entry<'m>(states: &'m mut HashMap<String, SliceState>, id: &str) -> &'m mut SliceState {
        states.entry(id.to_owned()).or_insert_with(|| SliceState { has_slice: false, url: None })
    }
    let mut states = HashMap::new();
    for name in listed(app_data_dir) {
        if let Some(id) = name.strip_suffix(GZ_END) {
            entry(&mut states, id).has_slice = true;
        } else if let Some(id) = name.strip_suffix(META_END) {
            entry(&mut states, id).url = load_sidecar(app_data_dir, id).url;
        }
    }
    states
}

/// A fight's kept slice, unzipped, and the player's actor id in it.
pub fn read_slice(app_data_dir: &Path, id: &str) -> Option<(Vec<u8>, Option<i32>)> {
    use std::io::Read;
    let compressed = std::fs::read(slice_bin_path(app_data_dir, id)).ok()?;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(&compressed[..]).read_to_end(&mut out).ok()?;
    Some((out, load_sidecar(app_data_dir, id).uploader_actor_id))
}

/// The player's actor id in a fight's kept slice.
pub fn slice_uploader(app_data_dir: &Path, id: &str) -> Option<i32> {
    load_sidecar(app_data_dir, id).uploader_actor_id
}

/// The size and time of a fight's kept slice, which change while it is written.
pub fn slice_stamp(app_data_dir: &Path, id: &str) -> Option<(u64, std::time::SystemTime)> {
    let meta = std::fs::metadata(slice_bin_path(app_data_dir, id)).ok()?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::built::{self, Scratch};
    use crate::combat::data_storage::RosterEntry;

    fn a_slice() -> FightTape {
        let hit = built::tick(9001, 0, 9002, 1, 1, 50, &[]);
        let packets = [SliceRecord { captured_at_ms: 1_000, stream: "Client:5000:13328".into(), bytes: hit }];
        let mut slice = fight_tape::build(&packets, 1_000, 2_000, &NameMap::new()).expect("a slice");
        slice.blind_map.insert("Qx".into(), 42);
        slice
    }

    #[test]
    fn files_sit_in_the_slices_folder_beside_the_history() {
        let dir = Path::new("/data");
        assert_eq!(slice_folder(dir), Path::new("/data/slices"));
        assert_eq!(slice_bin_path(dir, "auto_40001_1767380447200"), Path::new("/data/slices/auto_40001_1767380447200.a2es.gz"));
        assert_eq!(file_for(dir, "f", META_END), Path::new("/data/slices/f.json"));
    }

    #[test]
    fn the_meta_file_matches_what_players_have_on_disk() {
        let tmp = Scratch::new("slices.meta-bytes");
        std::fs::create_dir_all(slice_folder(&tmp.0)).unwrap();
        save_sidecar(&tmp.0, "f", &SliceSidecar { uploader_actor_id: Some(11297), ..Default::default() });
        assert_eq!(
            std::fs::read_to_string(tmp.0.join("slices/f.json")).unwrap(),
            r#"{"uploaderActorId":11297,"url":null,"visibility":null,"autoAttempts":0,"retryAtMs":0,"gaveUp":false}"#
        );
    }

    #[test]
    fn a_partial_or_broken_meta_reads_with_defaults() {
        let tmp = Scratch::new("slices.meta-read");
        assert_eq!(load_sidecar(&tmp.0, "none").auto_attempts, 0);
        tmp.put("slices/part.json", r#"{"url":"https://a2tools.app/logs/abc","extra":1}"#);
        let meta = load_sidecar(&tmp.0, "part");
        assert_eq!(meta.url.as_deref(), Some("https://a2tools.app/logs/abc"));
        assert_eq!(meta.uploader_actor_id, None);
        assert_eq!(meta.auto_attempts, 0);
        assert!(!meta.gave_up);
        tmp.put("slices/bad.json", "not json");
        let meta = load_sidecar(&tmp.0, "bad");
        assert!(meta.url.is_none() && meta.uploader_actor_id.is_none() && meta.retry_at_ms == 0);
    }

    #[test]
    fn writing_a_meta_does_not_make_the_folder() {
        let tmp = Scratch::new("slices.meta-nofolder");
        save_sidecar(&tmp.0, "f", &SliceSidecar::default());
        assert!(!tmp.has("slices"));
    }

    #[test]
    fn gzip_has_the_kept_header_and_unpacks_to_its_input() {
        use std::io::Read;
        for data in [&b""[..], b"abc"] {
            let packed = gzip(data).unwrap();
            assert_eq!(packed[..10], [0x1F, 0x8B, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xFF]);
            let mut back = Vec::new();
            flate2::read::GzDecoder::new(&packed[..]).read_to_end(&mut back).unwrap();
            assert_eq!(back, data);
        }
    }

    #[test]
    fn covering_looks_at_the_first_and_last_packets() {
        let at = |stamps: &[i64]| -> Vec<SliceRecord> {
            stamps.iter().map(|&t| SliceRecord { captured_at_ms: t, stream: "s".into(), bytes: vec![1] }).collect()
        };
        let pair = at(&[100, 200]);
        assert!(covers(&pair, 150, 160));
        assert!(covers(&pair, 200, 300));
        assert!(covers(&pair, 0, 100));
        assert!(!covers(&pair, 201, 300));
        assert!(!covers(&pair, 0, 99));
        assert!(!covers(&at(&[300, 100]), 150, 250));
        assert!(!covers(&[], i64::MIN, i64::MAX));
    }

    #[test]
    fn roster_ids_go_to_zero_and_names_stay() {
        let mut slice = a_slice();
        slice.blind_map = HashMap::from([("Abcd".to_string(), 77), ("Efgh".to_string(), 0)]);
        strip_party_ids(&mut slice);
        assert_eq!(slice.blind_map, HashMap::from([("Abcd".to_string(), 0), ("Efgh".to_string(), 0)]));
    }

    #[test]
    fn names_come_from_the_party_then_the_nicknames() {
        let storage = DataStorage::new();
        storage.replace_roster(vec![("Aa".to_string(), RosterEntry { dbid: 7, ..Default::default() })], false);
        storage.set_local_nickname(5, "Aa");
        storage.set_local_nickname(6, "Bb");
        let names = names_from(&storage);
        assert_eq!(names.get("Aa"), Some(&7));
        assert_eq!(names.get("Bb"), Some(&0));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn a_ring_that_misses_the_fight_saves_nothing() {
        let _turn = ring::test_turn();
        let tmp = Scratch::new("slices.nocover");
        let storage = DataStorage::new();
        let fight = built::fight("f", 1_000_000, 10_000);
        ring::clear();
        assert!(store_slice(&tmp.0, &fight, &storage).is_err(), "empty ring");
        ring::record_at(999_999, "s".into(), &[1]);
        assert!(store_slice(&tmp.0, &fight, &storage).is_err(), "ends before the start");
        ring::clear();
        ring::record_at(1_010_001, "s".into(), &[1]);
        assert!(store_slice(&tmp.0, &fight, &storage).is_err(), "starts after the end");
        ring::clear();
        assert!(!tmp.has("slices"));
    }

    #[test]
    fn a_covered_fight_is_saved_from_the_ring() {
        let _turn = ring::test_turn();
        let tmp = Scratch::new("slices.save");
        let storage = DataStorage::new();
        storage.set_local_player_id(Some(4321));
        let start = 1_767_380_434_407;
        ring::clear();
        for (n, at) in [start - 5_000, start + 1_000, start + 20_000].into_iter().enumerate() {
            ring::record_at(at, "Client:5000:13328".into(), &built::tick(9001, 0, 4321, n as u32, 1, 50, &[]));
        }
        let size = store_slice(&tmp.0, &built::fight("auto_1_2", start, 15_000), &storage).unwrap();
        ring::clear();
        assert_eq!(std::fs::metadata(slice_bin_path(&tmp.0, "auto_1_2")).unwrap().len() as usize, size);
        assert!(read_slice(&tmp.0, "auto_1_2").is_some());
        assert_eq!(load_sidecar(&tmp.0, "auto_1_2").uploader_actor_id, Some(4321));
    }

    #[test]
    fn writing_a_slice_keeps_the_upload_state_and_drops_roster_ids() {
        use std::io::Read;
        let tmp = Scratch::new("slices.write");
        tmp.put("slices/f.json", r#"{"url":"u","autoAttempts":2}"#);
        let size = write_slice(&tmp.0, "f", a_slice(), Some(5)).unwrap();
        let meta = load_sidecar(&tmp.0, "f");
        assert_eq!((meta.uploader_actor_id, meta.url.as_deref(), meta.auto_attempts), (Some(5), Some("u"), 2));

        let packed = std::fs::read(slice_bin_path(&tmp.0, "f")).unwrap();
        assert_eq!(packed.len(), size);
        let mut plain = Vec::new();
        flate2::read::GzDecoder::new(&packed[..]).read_to_end(&mut plain).unwrap();
        let mut expected = a_slice();
        strip_party_ids(&mut expected);
        assert_eq!(plain, fight_tape::encode(&expected));
        let (_, map) = fight_tape::decode(&plain).unwrap();
        assert!(map.values().all(|&id| id == 0));

        write_slice(&tmp.0, "f", a_slice(), None).unwrap();
        let meta = std::fs::read_to_string(tmp.0.join("slices/f.json")).unwrap();
        assert!(meta.starts_with(r#"{"uploaderActorId":null,"url":"u""#), "{meta}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_slices_folder_and_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = Scratch::new("slices.modes");
        write_slice(&tmp.0, "f", a_slice(), Some(1)).unwrap();
        let mode = |rel: &str| std::fs::metadata(tmp.0.join(rel)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode("slices"), 0o700);
        assert_eq!(mode("slices/f.a2es.gz"), 0o600);
        assert_eq!(mode("slices/f.json"), 0o600);
    }

    #[test]
    fn forgetting_a_fight_removes_both_files() {
        let tmp = Scratch::new("slices.forget");
        for rel in ["slices/f.a2es.gz", "slices/f.json", "slices/g.a2es.gz"] {
            tmp.put(rel, "x");
        }
        delete_slice_files(&tmp.0, "f");
        delete_slice_files(&tmp.0, "f");
        delete_slice_files(&tmp.0, "never");
        assert!(!tmp.has("slices/f.a2es.gz") && !tmp.has("slices/f.json"));
        assert!(tmp.has("slices/g.a2es.gz"));
    }

    #[test]
    fn pruning_follows_the_history() {
        let tmp = Scratch::new("slices.prune");
        trim_old_slices(&tmp.0);
        for name in ["kept.a2es.gz", "kept.json", "orphan.a2es.gz", "orphan.json", "stray.tmp", "loose.tmp"] {
            tmp.put(&format!("slices/{name}"), "x");
        }
        tmp.put("history/kept.json", "{}");
        tmp.put("history/stray.tmp.json", "{}");
        trim_old_slices(&tmp.0);
        let mut left = listed(&tmp.0);
        left.sort();
        assert_eq!(left, ["kept.a2es.gz", "kept.json", "stray.tmp"]);
    }

    #[test]
    fn status_pairs_each_slice_with_its_meta() {
        let tmp = Scratch::new("slices.status");
        assert!(share_status(&tmp.0).is_empty());
        let url = "https://a2tools.app/logs/u";
        let with_url = format!(r#"{{"url":"{url}"}}"#);
        tmp.put("slices/both.a2es.gz", "x");
        tmp.put("slices/both.json", &with_url);
        tmp.put("slices/gz.a2es.gz", "x");
        tmp.put("slices/note.json", &with_url);
        tmp.put("slices/bare.json", "{}");
        tmp.put("slices/broken.json", "not json");
        tmp.put("slices/other.txt", "x");
        let states = share_status(&tmp.0);
        let get = |id: &str| states.get(id).map(|s| (s.has_slice, s.url.clone()));
        assert_eq!(get("both"), Some((true, Some(url.to_string()))));
        assert_eq!(get("gz"), Some((true, None)));
        assert_eq!(get("note"), Some((false, Some(url.to_string()))));
        assert_eq!(get("bare"), Some((false, None)));
        assert_eq!(get("broken"), Some((false, None)));
        assert_eq!(states.len(), 5);
        assert_eq!(serde_json::to_string(&states["gz"]).unwrap(), r#"{"hasSlice":true,"url":null}"#);
    }

    #[test]
    fn an_uploaded_slice_shows_its_link_until_its_fight_is_gone() {
        let tmp = Scratch::new("slices.upload-link");
        tmp.put("slices/f1.a2es.gz", "slice one");
        tmp.put("slices/f2.a2es.gz", "slice two");
        let link = "https://a2tools.app/logs/abc";
        save_sidecar(&tmp.0, "f1", &SliceSidecar { uploader_actor_id: Some(5), url: Some(link.into()), ..Default::default() });
        let first = share_status(&tmp.0);
        assert!(first["f1"].has_slice && first["f2"].has_slice);
        assert_eq!(first["f1"].url.as_deref(), Some(link));
        assert_eq!(first["f2"].url, None);

        tmp.put("history/f2.json", "{}");
        trim_old_slices(&tmp.0);
        let after = share_status(&tmp.0);
        assert!(!after.contains_key("f1"));
        assert!(after.contains_key("f2"));
    }
}
