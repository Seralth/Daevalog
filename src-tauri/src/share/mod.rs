//! Preparing a fight for sharing — and, for now, only ever writing it to disk.
//!
//! Nothing here opens a socket. The dry run exists so that "we do not upload
//! your character names" is a claim you can check rather than one you have to
//! believe: it writes the exact two files an upload would send, into a folder it
//! then opens for you, and `a2t-inspect` reads them back.
//!
//! The artifacts are deliberately separate:
//!
//! - `<fight>.a2es` — the Evidence Slice, the packets themselves. See
//!   `capture::evidence_slice`.
//! - `<fight>.a2es.gz` — the same thing, gzipped: byte for byte what an upload
//!   would put on the wire. Both are written because the compressed one is what
//!   gets sent and the uncompressed one is what is easy to check, and making
//!   people choose between those would defeat the point. (`a2t-inspect` reads
//!   either.)
//! - `<fight>.upload.json` — the derived summary. **No field of it holds a
//!   character name**, not even your own; participants are identified by
//!   `sha256(dbid)`, and the server learns who that is only if you register the
//!   character yourself. There is a test asserting no name from the fight
//!   appears anywhere in the JSON.

use std::path::Path;

use crate::entity::fight_record::FightRecord;

mod envelope;
mod preview;
mod slices;
mod upload;

pub use envelope::{account_ref, build_envelope, Evidence, Participant, UploadEnvelope};
pub use preview::{find_captures, gzip, preview, read_capture, slice_for, PreviewResult};
pub use slices::{
    forget_slice, names_from, prune_slices, read_slice, save_slice, share_status, slice_stamp, slice_uploader,
    slices_dir, uploader_in, write_slice, ShareStatus, SliceMeta,
};
pub(crate) use upload::base64;
pub use upload::{upload, upload_detailed, UploadFailure, UploadResult};
use slices::{read_meta, slice_path, write_meta};

// ===== slices kept automatically, and uploading them =====

pub mod dev_logs;
pub mod ring;

/// The setting that turns automatic uploads on. Off unless the player turns
/// it on: an upload publishes a fight, and that is theirs to decide.
pub const AUTO_UPLOAD_KEY: &str = "dpsMeter.autoUpload";

/// Should the auto-save upload this fight now?
///
/// Only a finished fight, once, with its packets behind it. A boss still being
/// fought is re-saved every 30 seconds, and uploading those partial records
/// would publish a fight that has not happened yet. `finished` is the
/// meter's word that the fight cannot go on (`DpsCalculator::fight_finished`):
/// ten seconds of quiet was not enough, a boss phase can pause longer.
pub fn wants_auto_upload(app_data_dir: &Path, record: &FightRecord, finished: bool) -> bool {
    let meta = read_meta(app_data_dir, &record.id);
    !record.is_train
        && finished
        && slice_path(app_data_dir, &record.id).exists()
        && meta.url.is_none()
        // A fight that already failed once is the retry schedule's.
        && meta.auto_attempts == 0
}

/// How long to wait before each retry of a failed automatic upload, in
/// minutes; one more failure after the last and it stops.
const AUTO_RETRY_MINUTES: [i64; 6] = [1, 2, 5, 15, 30, 60];

/// How long to wait between tries while the keyring is locked.
const KEYRING_RETRY_MINUTES: i64 = 5;

/// An automatic upload of `id` failed. Schedule the next try, or stop: when
/// the failure is one waiting cannot fix (not signed in, a refused fight),
/// or the retries are used up. A locked keyring uses up no retries.
pub fn note_auto_upload_failure(app_data_dir: &Path, id: &str, failure: &UploadFailure, now_ms: i64) {
    let mut meta = read_meta(app_data_dir, id);
    if failure.keyring_locked {
        meta.auto_attempts = meta.auto_attempts.max(1);
        meta.retry_at_ms = now_ms + KEYRING_RETRY_MINUTES * 60_000;
        write_meta(app_data_dir, id, &meta);
        return;
    }
    meta.auto_attempts += 1;
    match AUTO_RETRY_MINUTES.get(meta.auto_attempts as usize - 1) {
        Some(minutes) if failure.retryable => meta.retry_at_ms = now_ms + minutes * 60_000,
        _ => meta.gave_up = true,
    }
    write_meta(app_data_dir, id, &meta);
}

/// Fights whose automatic upload failed and is due to be tried again.
pub fn auto_upload_retries_due(app_data_dir: &Path, now_ms: i64) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(slices_dir(app_data_dir)) else { return Vec::new() };
    rd.filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_string_lossy().strip_suffix(".json").map(str::to_string))
        .filter(|id| {
            let meta = read_meta(app_data_dir, id);
            meta.url.is_none()
                && meta.auto_attempts > 0
                && !meta.gave_up
                && meta.retry_at_ms <= now_ms
                && slice_path(app_data_dir, id).exists()
        })
        .collect()
}

#[cfg(test)]
mod upload_tests {
    use super::*;

    fn fight(id: &str, start: i64, duration: i64, is_train: bool) -> FightRecord {
        let mut r: FightRecord = serde_json::from_value(serde_json::json!({
            "id": id, "bossName": "B", "targetId": 1, "startTimeMs": start,
            "durationMs": duration, "totalDamage": 1, "jobs": [],
            "details": {"targetId": 1, "maxHp": 0, "totalTargetDamage": 1, "battleTime": duration,
                        "startTime": 0, "skills": [], "pingHistory": [], "healSkills": []},
            "actors": []
        }))
        .unwrap();
        r.is_train = is_train;
        r
    }

    #[test]
    fn a_failed_auto_upload_is_retried_on_a_schedule_and_then_left() {
        let dir = std::env::temp_dir().join(format!("a2t-retry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(slices_dir(&dir)).unwrap();
        std::fs::write(slice_path(&dir, "f1"), b"slice").unwrap();
        write_meta(&dir, "f1", &SliceMeta::default());
        let t = 1_000_000;
        assert!(auto_upload_retries_due(&dir, t).is_empty(), "never failed: not a retry");
        note_auto_upload_failure(&dir, "f1", &UploadFailure::retry("offline"), t);
        assert!(auto_upload_retries_due(&dir, t + 59_000).is_empty(), "first retry after a minute");
        assert_eq!(auto_upload_retries_due(&dir, t + 60_000), vec!["f1".to_string()]);
        for n in 2..=AUTO_RETRY_MINUTES.len() {
            note_auto_upload_failure(&dir, "f1", &UploadFailure::retry("offline"), t);
            assert!(!read_meta(&dir, "f1").gave_up, "attempt {n}");
        }
        note_auto_upload_failure(&dir, "f1", &UploadFailure::retry("offline"), t);
        assert!(read_meta(&dir, "f1").gave_up, "out of retries");
        assert!(auto_upload_retries_due(&dir, i64::MAX).is_empty());

        std::fs::write(slice_path(&dir, "f2"), b"slice").unwrap();
        write_meta(&dir, "f2", &SliceMeta::default());
        note_auto_upload_failure(&dir, "f2", &UploadFailure::fatal("refused"), t);
        assert!(read_meta(&dir, "f2").gave_up, "a refused fight is not retried");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_locked_keyring_never_ends_the_auto_upload() {
        let dir = std::env::temp_dir().join(format!("a2t-locked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(slices_dir(&dir)).unwrap();
        std::fs::write(slice_path(&dir, "f1"), b"slice").unwrap();
        write_meta(&dir, "f1", &SliceMeta::default());
        let t = 1_000_000;
        for _ in 0..50 {
            note_auto_upload_failure(&dir, "f1", &UploadFailure::locked("locked"), t);
        }
        let meta = read_meta(&dir, "f1");
        assert!(!meta.gave_up);
        assert_eq!(meta.auto_attempts, 1, "a locked keyring uses up no retries");
        assert_eq!(auto_upload_retries_due(&dir, t + KEYRING_RETRY_MINUTES * 60_000), vec!["f1".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_upload_waits_for_the_end_and_fires_once() {
        let dir = std::env::temp_dir().join(format!("a2t-auto-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(slices_dir(&dir)).unwrap();
        let boss = fight("auto_9_1000", 1_000, 60_000, false);

        assert!(!wants_auto_upload(&dir, &boss, true), "no slice, nothing to send");
        std::fs::write(slice_path(&dir, &boss.id), b"x").unwrap();
        assert!(!wants_auto_upload(&dir, &boss, false), "still being fought");
        assert!(wants_auto_upload(&dir, &boss, true));
        assert!(!wants_auto_upload(&dir, &fight("auto_9_1000", 1_000, 60_000, true), true),
                "a training dummy is never a log");
        write_meta(&dir, &boss.id, &SliceMeta { uploader_actor_id: None,
                   url: Some("https://a2tools.app/logs/x".into()), visibility: None, ..Default::default() });
        assert!(!wants_auto_upload(&dir, &boss, true), "already uploaded");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
