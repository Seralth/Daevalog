//! Uploading a saved fight to a2tools.app.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::preview::{find_captures, gzip, slice_for};
use super::slices::{read_meta, slice_path, slices_dir, write_meta};
use crate::config::settings::Settings;
use crate::entity::fight_record::FightRecord;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadResult {
    pub url: String,
    #[serde(default)]
    pub visibility: String,
    #[serde(default)]
    pub duplicate: bool,
}

/// The meter's display language, as the settings hold it (`ko`, `en`, …).
///
/// Sent with an upload because a server id cannot tell Korea from Taiwan:
/// both number their servers 1001–1058 and 2001–2058. The language and the
/// computer's time zone are what the site has to go on; a player on Korean
/// servers almost always has one or the other Korean.
fn ui_language(settings: &Settings) -> String {
    // In memory: settings.json is written a moment after a change.
    settings.get("dpsMeter.language").unwrap_or_default()
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
    settings: &Settings,
    record: &FightRecord,
) -> Result<UploadResult, String> {
    upload_detailed(client, app_data_dir, settings, record).await.map_err(|f| f.message)
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
    pub(super) fn retry(message: impl Into<String>) -> Self {
        Self { message: message.into(), retryable: true, keyring_locked: false }
    }
    pub(super) fn fatal(message: impl Into<String>) -> Self {
        Self { message: message.into(), retryable: false, keyring_locked: false }
    }
    pub(super) fn locked(message: impl Into<String>) -> Self {
        Self { message: message.into(), retryable: true, keyring_locked: true }
    }
}

/// `upload`, saying whether a failure is worth retrying.
pub async fn upload_detailed(
    client: &reqwest::Client,
    app_data_dir: &Path,
    settings: &Settings,
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
        "client": crate::version::UPLOAD_CLIENT,
        "clientVersion": crate::version::upload_version(),
        "appVersion": crate::version::upload_version(),
        // Korea and Taiwan number their servers alike (10xx/20xx), so the
        // slice cannot say which a fight was on; these two settle it. See
        // `region_hints`.
        "uiLanguage": ui_language(settings),
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
        let _ = crate::platform::files::create_private_dir(&slices_dir(app_data_dir));
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
mod tests {
    #[test]
    fn the_upload_language_is_the_unsaved_setting() {
        let dir = std::env::temp_dir().join(format!("a2t-language-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), r#"{"dpsMeter.language":"en"}"#).unwrap();
        let settings = Settings::new(dir.clone());
        settings.set("dpsMeter.language", "ko");
        assert_eq!(ui_language(&settings), "ko");
        settings.remove("dpsMeter.language");
        assert_eq!(ui_language(&settings), "");
        drop(settings);
        let _ = std::fs::remove_dir_all(&dir);
    }

    use super::*;

    #[test]
    fn base64_matches_the_standard_vectors() {
        for (raw, want) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64(raw.as_bytes()), want);
        }
    }
}
