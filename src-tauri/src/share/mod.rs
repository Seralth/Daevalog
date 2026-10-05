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

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::entity::fight_record::FightRecord;

mod envelope;
mod preview;
mod slices;

pub use envelope::{account_ref, build_envelope, Evidence, Participant, UploadEnvelope};
pub use preview::{find_captures, gzip, preview, read_capture, slice_for, PreviewResult};
pub use slices::{
    forget_slice, names_from, prune_slices, read_slice, save_slice, share_status, slice_stamp, slice_uploader,
    slices_dir, uploader_in, write_slice, ShareStatus, SliceMeta,
};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadResult {
    pub url: String,
    #[serde(default)]
    pub visibility: String,
    #[serde(default)]
    pub duplicate: bool,
}

/// The meter's display language, as the settings file holds it (`ko`, `en`, …).
///
/// Sent with an upload because a server id cannot tell Korea from Taiwan:
/// both number their servers 1001–1058 and 2001–2058. The language and the
/// computer's time zone are what the site has to go on; a player on Korean
/// servers almost always has one or the other Korean.
fn ui_language(app_data_dir: &Path) -> String {
    std::fs::read_to_string(app_data_dir.join("settings.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<HashMap<String, String>>(&text).ok())
        .and_then(|values| values.get("dpsMeter.language").cloned())
        .unwrap_or_default()
}

/// Upload a saved fight as a log.
///
/// Sends the slice and the names to show, never a number: the service derives
/// the fight from the slice with this same parser. The names are the ones the
/// saved record already holds, which the meter masked for everyone but the
/// local player when it wrote them; the service masks them again regardless.
pub async fn upload(
    client: &reqwest::Client,
    app_data_dir: &Path,
    record: &FightRecord,
) -> Result<UploadResult, String> {
    upload_detailed(client, app_data_dir, record).await.map_err(|f| f.message)
}

/// Why an upload failed, and whether trying the same upload later could work.
#[derive(Debug, Clone)]
pub struct UploadFailure {
    pub message: String,
    /// Offline, a server error, or rate limited: worth another try later. Not
    /// signed in, or the service refused the fight: the same again would fail.
    pub retryable: bool,
    /// The keyring did not hand over the token. Retried for as long as it takes.
    pub keyring_locked: bool,
}

impl UploadFailure {
    fn retry(message: impl Into<String>) -> Self {
        Self { message: message.into(), retryable: true, keyring_locked: false }
    }
    fn fatal(message: impl Into<String>) -> Self {
        Self { message: message.into(), retryable: false, keyring_locked: false }
    }
    fn locked(message: impl Into<String>) -> Self {
        Self { message: message.into(), retryable: true, keyring_locked: true }
    }
}

/// `upload`, saying whether a failure is worth retrying.
pub async fn upload_detailed(
    client: &reqwest::Client,
    app_data_dir: &Path,
    record: &FightRecord,
) -> Result<UploadResult, UploadFailure> {
    let token = match crate::account::secret::load_stored(app_data_dir) {
        crate::account::secret::Stored::Token(token) => token,
        crate::account::secret::Stored::Locked => {
            return Err(UploadFailure::locked(
                "The desktop keyring is locked. Unlock the keyring to upload fights.",
            ))
        }
        crate::account::secret::Stored::Missing => {
            return Err(UploadFailure::fatal("Sign in under Settings → A2 Tools Account to upload fights."))
        }
    };

    let compressed = match std::fs::read(slice_path(app_data_dir, &record.id)) {
        Ok(bytes) => bytes,
        // Older fights, and fights recorded with packet logging on: cut it
        // from a capture if one covers the fight.
        Err(_) => {
            let captures = find_captures(app_data_dir);
            let (encoded, _, _) = slice_for(record, &captures).map_err(|_| {
                UploadFailure::fatal(
                    "This fight has no packets saved, so it cannot be verified or uploaded. \
                     Fights recorded from this version on can be.",
                )
            })?;
            gzip(&encoded).map_err(UploadFailure::fatal)?
        }
    };

    let meta = read_meta(app_data_dir, &record.id);
    let names: HashMap<String, String> = record
        .actors
        .iter()
        .map(|a| (a.actor_id.to_string(), a.nickname.clone()))
        .collect();
    let body = serde_json::json!({
        "slice": base64(&compressed),
        "names": names,
        "uploaderActorId": meta.uploader_actor_id,
        "fightStartMs": record.start_time_ms,
        "appVersion": crate::entity::fight_record::APP_VERSION,
        // Korea and Taiwan number their servers alike (10xx/20xx), so the
        // slice cannot say which a fight was on; these two settle it. See
        // `region_hints`.
        "uiLanguage": ui_language(app_data_dir),
        "utcOffsetMinutes": chrono::Local::now().offset().local_minus_utc() / 60,
    });

    let response = client
        .post(format!("{}/api/logs", crate::account::base_url()))
        .timeout(std::time::Duration::from_secs(60))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| UploadFailure::retry(format!("Could not reach a2tools.app: {e}")))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let reply: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();

    if status.is_success() {
        let result: UploadResult = serde_json::from_value(reply)
            .map_err(|_| UploadFailure::retry("Unexpected reply from a2tools.app."))?;
        let mut meta = meta;
        meta.url = Some(result.url.clone());
        meta.visibility = Some(result.visibility.clone());
        let _ = std::fs::create_dir_all(slices_dir(app_data_dir));
        write_meta(app_data_dir, &record.id, &meta);
        return Ok(result);
    }
    let code = status.as_u16();
    let message = match (code, reply.get("error").and_then(|e| e.as_str())) {
        (401, _) => "Your sign-in has expired. Connect your account again in Settings.".into(),
        (403, Some("insufficient_scope")) => {
            "This sign-in was made before uploads existed. Sign out and connect again in \
             Settings to allow them."
                .into()
        }
        (429, _) => "Too many uploads in the last hour. Try again later.".into(),
        (_, _) => reply
            .get("message")
            .and_then(|m| m.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("Upload failed ({status}).")),
    };
    // A server that is down, busy or rate limiting may take it later; one that
    // refused the fight, or the sign-in, will refuse it again.
    let retryable = code == 408 || code == 429 || status.is_server_error();
    Err(UploadFailure { message, retryable, keyring_locked: false })
}

/// Standard base64. Small enough that a dependency is not worth having.
pub(crate) fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
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

    #[test]
    fn base64_matches_the_standard_vectors() {
        for (raw, want) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64(raw.as_bytes()), want);
        }
    }
}
