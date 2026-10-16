//! Uploading a saved fight to a2tools.app.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::preview::{capture_files, cut_slice};
use super::slices::{gzip, load_sidecar, slice_bin_path, slice_folder, save_sidecar};
use crate::account::secret::{load_stored, Stored};
use crate::entity::fight_record::FightRecord;
use crate::version::{upload_version, UPLOAD_CLIENT};

/// A big slice on a slow link takes longer than the client's usual limit.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(60);

const NO_PACKETS: &str = "This fight has no packets saved, so it cannot be verified or uploaded. \
                          Fights recorded with this version and later can be.";

/// What a2tools.app answers to an upload that went through.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishOutcome {
    pub url: String,
    /// `public` or `private`; empty when the site leaves it out.
    #[serde(default)]
    pub visibility: String,
    /// The site had this fight already.
    #[serde(default)]
    pub duplicate: bool,
}

/// Why an upload did not go through.
#[derive(Debug, Clone)]
pub struct PublishError {
    /// Shown to the player whole.
    pub message: String,
    /// Whether the same upload may work later.
    pub retryable: bool,
    /// Nothing was sent: the keyring kept the token.
    pub keyring_locked: bool,
}

impl PublishError {
    fn fatal(message: impl Into<String>) -> Self {
        Self { message: message.into(), retryable: false, keyring_locked: false }
    }

    fn retryable(message: impl Into<String>) -> Self {
        Self { message: message.into(), retryable: true, keyring_locked: false }
    }
}

/// The upload of `record` with only the failure's message: the Upload button
/// shows it as it is.
pub async fn upload(
    client: &reqwest::Client,
    app_data_dir: &Path,
    ui_language: &str,
    record: &FightRecord,
) -> Result<PublishOutcome, String> {
    upload_detailed(client, app_data_dir, ui_language, record).await.map_err(|failure| failure.message)
}

/// Sends `record`'s slice with the account's token, and keeps the link in the
/// fight's meta when the site took it. One try; retries are the caller's.
pub async fn upload_detailed(
    client: &reqwest::Client,
    app_data_dir: &Path,
    ui_language: &str,
    record: &FightRecord,
) -> Result<PublishOutcome, PublishError> {
    let token = token_from(load_stored(app_data_dir))?;
    post_fight(client, &crate::account::server_root(), &token, app_data_dir, ui_language, record).await
}

/// Comes before the slice, so a player who is not signed in hears that
/// rather than anything about the fight.
fn token_from(stored: Stored) -> Result<String, PublishError> {
    match stored {
        Stored::Token(token) => Ok(token),
        Stored::Missing => Err(PublishError::fatal("Sign in under Settings → Account to upload fights.")),
        Stored::Locked => Err(PublishError {
            message: "The desktop keyring is locked. Unlock the keyring to upload fights.".into(),
            retryable: true,
            keyring_locked: true,
        }),
    }
}

/// Everything after the token: the body, the request, the answer, and the
/// link kept on success. The tests aim it at a listener of their own.
async fn post_fight(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    app_data_dir: &Path,
    ui_language: &str,
    record: &FightRecord,
) -> Result<PublishOutcome, PublishError> {
    let body = request_body(app_data_dir, ui_language, record)?;
    let response = client
        .post(format!("{base_url}/api/logs"))
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .timeout(UPLOAD_TIMEOUT)
        .body(body)
        .send()
        .await
        .map_err(|e| {
            PublishError::retryable(format!(
                "Could not reach a2tools.app ({}). Check your connection and try again.",
                with_causes(&e)
            ))
        })?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    let uploaded = judge_reply(status, &text)?;
    keep_link(app_data_dir, &record.id, &uploaded);
    Ok(uploaded)
}

/// reqwest's own text names only the URL; the reason is in its sources.
fn with_causes(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(inner) = cause {
        text = format!("{text}: {inner}");
        cause = inner.source();
    }
    text
}

/// What the site's answer means. A body that is not JSON reads as `null`.
fn judge_reply(status: u16, text: &str) -> Result<PublishOutcome, PublishError> {
    let reply: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        return serde_json::from_value(reply).map_err(|_| {
            PublishError::retryable(format!(
                "The reply from a2tools.app (status {status}) was not understood. Try again later."
            ))
        });
    }
    let retryable = matches!(status, 408 | 429 | 500..=599);
    let message = match (status, reply["error"].as_str()) {
        (401, _) => "Your sign-in to a2tools.app has expired (status 401). \
                     Connect the account again under Settings → Account."
            .to_string(),
        (403, Some("insufficient_scope")) => "Your sign-in to a2tools.app is from before uploads existed (status 403). \
                                              Sign out and connect again under Settings → Account to allow them."
            .to_string(),
        (429, _) => "Too many uploads to a2tools.app in the last hour (status 429). Try again later.".to_string(),
        _ => match reply["message"].as_str() {
            Some(said) => said.to_string(),
            None if retryable => format!("The upload to a2tools.app failed (status {status}). Try again later."),
            None => format!(
                "The upload to a2tools.app failed (status {status}). If it keeps happening, check for a meter update."
            ),
        },
    };
    Err(PublishError { message, retryable, keyring_locked: false })
}

/// Writes the link into the fight's meta, keeping its other fields. A fight
/// sent from a packet log has no `slices/` folder until now.
fn keep_link(app_data_dir: &Path, id: &str, uploaded: &PublishOutcome) {
    let mut meta = load_sidecar(app_data_dir, id);
    meta.url = Some(uploaded.url.clone());
    meta.visibility = Some(uploaded.visibility.clone());
    let _ = crate::platform::files::create_private_dir(&slice_folder(app_data_dir));
    save_sidecar(app_data_dir, id, &meta);
}

/// The upload body. Field order is the wire order, so it holds whatever
/// `serde_json` does with maps.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Outgoing<'a> {
    app_version: String,
    client: &'a str,
    client_version: String,
    fight_start_ms: i64,
    names: BTreeMap<String, &'a str>,
    slice: String,
    ui_language: &'a str,
    uploader_actor_id: Option<i32>,
    utc_offset_minutes: i32,
}

/// The text an upload of `record` sends, compact JSON on one line. No
/// numbers go: the site reads the fight from the slice itself. The UTC offset
/// and the interface language are how it tells Korea from Taiwan, whose
/// server ids are the same.
pub fn request_body(app_data_dir: &Path, ui_language: &str, record: &FightRecord) -> Result<String, PublishError> {
    let slice = gzipped_slice(app_data_dir, record)?;
    let version = upload_version();
    let body = Outgoing {
        app_version: version.clone(),
        client: UPLOAD_CLIENT,
        client_version: version,
        fight_start_ms: record.start_time_ms,
        // A repeated id keeps the later actor's name.
        names: record.actors.iter().map(|a| (a.actor_id.to_string(), a.nickname.as_str())).collect(),
        slice: base64(&slice),
        ui_language,
        uploader_actor_id: load_sidecar(app_data_dir, &record.id).uploader_actor_id,
        utc_offset_minutes: whole_minutes(chrono::Local::now().offset().local_minus_utc()),
    };
    serde_json::to_string(&body)
        .map_err(|e| PublishError::fatal(format!("Could not write the upload request for this fight ({e}).")))
}

/// The kept slice file's bytes, or else a slice cut from the packet logs and
/// gzipped here. A cut slice is sent only, never kept.
fn gzipped_slice(app_data_dir: &Path, record: &FightRecord) -> Result<Vec<u8>, PublishError> {
    if let Ok(kept) = std::fs::read(slice_bin_path(app_data_dir, &record.id)) {
        return Ok(kept);
    }
    let encoded = cut_slice(record, &capture_files(app_data_dir)).map_err(|_| PublishError::fatal(NO_PACKETS))?;
    gzip(&encoded).map_err(|e| PublishError::fatal(format!("Could not compress this fight's slice for upload ({e}).")))
}

/// Integer division drops the remainder toward zero, which is what the site
/// expects of a zone with stray seconds.
fn whole_minutes(offset_seconds: i32) -> i32 {
    offset_seconds / 60
}

/// RFC 4648 base64: standard alphabet, `=` padding, one line.
fn base64(data: &[u8]) -> String {
    let digit = |six_bits: u32| char::from(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"[(six_bits & 63) as usize]);
    let mut text = String::new();
    for triple in data.chunks(3) {
        let bits = triple.iter().zip([16, 8, 0]).fold(0, |acc, (byte, shift)| acc | u32::from(*byte) << shift);
        // n bytes fill n + 1 digits; the rest of the four are padding.
        let filled = triple.len() + 1;
        text.extend([18, 12, 6, 0].into_iter().take(filled).map(|shift| digit(bits >> shift)));
        text.extend(std::iter::repeat_n('=', 4 - filled));
    }
    text
}

/// What Preview upload wrote.
#[derive(Debug, Clone)]
pub struct UploadPreview {
    pub path: PathBuf,
    /// The request body's size: what an upload sends.
    pub bytes: usize,
}

/// Write the body an upload of `record` would send to `out_dir`, without
/// sending anything.
pub fn preview_upload(
    app_data_dir: &Path,
    ui_language: &str,
    record: &FightRecord,
    out_dir: &Path,
) -> Result<UploadPreview, String> {
    let body = request_body(app_data_dir, ui_language, record).map_err(|f| f.message)?;
    crate::platform::files::create_private_dir(out_dir).map_err(|e| e.to_string())?;
    // An older preview's files for this fight were not what an upload sends.
    for old in ["a2es", "a2es.gz", "upload.json"] {
        let _ = std::fs::remove_file(out_dir.join(format!("{}.{old}", record.id)));
    }
    let path = out_dir.join(format!("{}.upload-request.json", record.id));
    crate::platform::files::write_private(&path, body.as_bytes()).map_err(|e| e.to_string())?;
    Ok(UploadPreview { path, bytes: body.len() })
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::built::{self, Scratch};
    use crate::share::slices::SliceSidecar;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    const MADE_UP_TOKEN: &str = "tok-0000-made-up";
    /// Late 2023: far from the wall clock.
    const FIGHT_AT: i64 = 1_700_000_000_000;

    /// A saved boss fight starting at `FIGHT_AT`, with these actors.
    pub(in crate::share) fn saved_fight(id: &str, actors: &[(i32, &str)]) -> FightRecord {
        let actors: Vec<Value> =
            actors.iter().map(|(id, name)| serde_json::json!({ "actorId": id, "nickname": name })).collect();
        let actors = serde_json::from_value(Value::from(actors)).expect("actors");
        FightRecord { actors, ..built::fight(id, FIGHT_AT, 60_000) }
    }

    fn with_slice(tag: &str) -> Scratch {
        let dir = Scratch::new(&format!("upload.{tag}"));
        dir.put("slices/f1.a2es.gz", "the slice");
        dir
    }

    fn body_of(dir: &Scratch, language: &str, record: &FightRecord) -> Value {
        serde_json::from_str(&request_body(&dir.0, language, record).expect("a body")).expect("JSON")
    }

    /// Takes one request on 127.0.0.1, answers it with `status` and `reply`,
    /// and hands back what came in, byte for byte.
    fn answer_once(status: &str, reply: &str) -> (String, std::thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a local port");
        let at = format!("http://{}", listener.local_addr().expect("its address"));
        let answer = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
            reply.len()
        );
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("a request");
            let mut got = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = stream.read(&mut chunk).expect("request bytes");
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&chunk[..n]);
                if let Some(end) = got.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&got[..end]).to_ascii_lowercase();
                    let length: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .map_or(0, |v| v.trim().parse().expect("a length"));
                    if got.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            stream.write_all(answer.as_bytes()).expect("answer sent");
            got
        });
        (at, handle)
    }

    /// The production client is https only; this one differs from it in that alone.
    fn plain_client() -> reqwest::Client {
        reqwest::Client::builder().user_agent(crate::version::user_agent()).build().expect("a client")
    }

    #[test]
    fn base64_matches_the_rfc_vectors() {
        let vectors =
            [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")];
        for (plain, coded) in vectors {
            assert_eq!(base64(plain.as_bytes()), coded, "{plain:?}");
        }
        assert_eq!(base64(&[0xfb, 0xff, 0xbf]), "+/+/");
    }

    #[test]
    fn the_kept_slice_goes_as_its_file_bytes() {
        let dir = with_slice("kept");
        assert_eq!(body_of(&dir, "en", &saved_fight("f1", &[]))["slice"], "dGhlIHNsaWNl");
    }

    #[test]
    fn no_slice_and_no_packet_log_cannot_be_sent() {
        let dir = Scratch::new("upload.bare");
        let failure = request_body(&dir.0, "en", &saved_fight("f1", &[])).expect_err("nothing to send");
        assert!(!failure.retryable && !failure.keyring_locked);
        assert_eq!(failure.message, NO_PACKETS);
    }

    #[test]
    fn without_a_kept_slice_one_is_cut_from_the_packet_log_and_not_kept() {
        let dir = Scratch::new("upload.cut");
        let mut log = String::from("# Format: TIMESTAMP|STREAMKEY|HEX_DATA\n");
        for (at, n) in [(FIGHT_AT - 3_000, 1), (FIGHT_AT + 5_000, 2), (FIGHT_AT + 40_000, 3)] {
            let stamp = chrono::DateTime::from_timestamp_millis(at).expect("a time").to_rfc3339();
            log += &format!("{stamp}|Client:5000:13328|{}\n", built::to_hex(&built::tick(9001, 0, 9002, n, 1, 50, &[])));
        }
        dir.put("packets_20231114_221000.txt", log);
        let record = saved_fight("f1", &[(9002, "Abcd")]);
        let encoded = cut_slice(&record, &capture_files(&dir.0)).expect("the log covers the fight");
        let expected = base64(&gzip(&encoded).expect("gzip"));
        let body = body_of(&dir, "en", &record);
        assert_eq!(body["slice"], expected.as_str());
        assert_eq!(body["uploaderActorId"], Value::Null);
        assert!(!slice_folder(&dir.0).exists(), "a cut slice is not saved");
    }

    #[test]
    fn names_are_keyed_by_id_sorted_as_text_and_the_last_repeat_wins() {
        let dir = with_slice("names");
        let record = saved_fight("f1", &[(3001, "Abcd"), (10137, "Ef****h"), (42, "first"), (42, ""), (7, "名前")]);
        let body = request_body(&dir.0, "en", &record).expect("a body");
        assert!(body.contains(r#""names":{"10137":"Ef****h","3001":"Abcd","42":"","7":"名前"}"#), "{body}");
        assert_eq!(body_of(&dir, "en", &saved_fight("f1", &[]))["names"], serde_json::json!({}));
    }

    #[test]
    fn the_uploader_comes_from_the_meta() {
        let dir = with_slice("uploader");
        assert_eq!(body_of(&dir, "en", &saved_fight("f1", &[]))["uploaderActorId"], Value::Null);
        save_sidecar(&dir.0, "f1", &SliceSidecar { uploader_actor_id: Some(3001), ..SliceSidecar::default() });
        assert_eq!(body_of(&dir, "en", &saved_fight("f1", &[]))["uploaderActorId"], 3001);
    }

    #[test]
    fn the_body_is_exact_and_the_same_each_time() {
        let dir = with_slice("exact");
        save_sidecar(&dir.0, "f1", &SliceSidecar { uploader_actor_id: Some(3001), ..SliceSidecar::default() });
        let record = saved_fight("f1", &[(3001, "Abcd"), (3002, "Q\"\\/")]);
        let body = request_body(&dir.0, "", &record).expect("a body");
        let offset = serde_json::from_str::<Value>(&body).expect("JSON")["utcOffsetMinutes"].as_i64().expect("minutes");
        let version = upload_version();
        let expected = format!(
            r#"{{"appVersion":"{version}","client":"daevalog","clientVersion":"{version}","fightStartMs":1700000000000,"names":{{"3001":"Abcd","3002":"Q\"\\/"}},"slice":"dGhlIHNsaWNl","uiLanguage":"","uploaderActorId":3001,"utcOffsetMinutes":{offset}}}"#
        );
        assert_eq!(body, expected);
        assert_eq!(request_body(&dir.0, "", &record).expect("a body"), body);
    }

    #[test]
    fn the_offset_is_in_whole_minutes_toward_zero() {
        for (seconds, minutes) in [(32_400, 540), (-25_200, -420), (20_700, 345), (-12_600, -210), (-12_630, -210), (0, 0)] {
            assert_eq!(whole_minutes(seconds), minutes, "{seconds}");
        }
    }

    #[test]
    fn the_preview_is_the_body_an_upload_sends() {
        let dir = with_slice("preview");
        save_sidecar(&dir.0, "f1", &SliceSidecar { uploader_actor_id: Some(3001), ..SliceSidecar::default() });
        dir.put("share-preview/f1.upload.json", "older preview");
        let record = saved_fight("f1", &[(3001, "Abcd"), (3002, "Ab****d")]);
        let out = dir.0.join("share-preview");
        let written = preview_upload(&dir.0, "ko", &record, &out).expect("a preview");
        let text = std::fs::read_to_string(&written.path).expect("the preview file");
        assert_eq!(text, request_body(&dir.0, "ko", &record).expect("a body"));
        assert_eq!(written.bytes, text.len());
        assert!(!out.join("f1.upload.json").exists());
        let body: serde_json::Map<String, Value> = serde_json::from_str(&text).expect("JSON");
        let keys: Vec<&str> = body.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["appVersion", "client", "clientVersion", "fightStartMs", "names", "slice", "uiLanguage", "uploaderActorId", "utcOffsetMinutes"]
        );
        assert_eq!(body["slice"], "dGhlIHNsaWNl");
        assert_eq!(body["names"], serde_json::json!({ "3001": "Abcd", "3002": "Ab****d" }));
        assert_eq!(body["uploaderActorId"], 3001);
        assert_eq!(body["uiLanguage"], "ko");
    }

    #[test]
    fn stored_token_states_map_to_failures() {
        assert_eq!(token_from(Stored::Token(MADE_UP_TOKEN.into())).expect("a token"), MADE_UP_TOKEN);
        let missing = token_from(Stored::Missing).expect_err("no token");
        assert_eq!(missing.message, "Sign in under Settings → Account to upload fights.");
        assert!(!missing.retryable && !missing.keyring_locked);
        let locked = token_from(Stored::Locked).expect_err("locked");
        assert_eq!(locked.message, "The desktop keyring is locked. Unlock the keyring to upload fights.");
        assert!(locked.retryable && locked.keyring_locked);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn signed_out_hears_about_signing_in_and_nothing_changes() {
        let dir = Scratch::new("upload.signed-out");
        let message = upload(crate::account::client(), &dir.0, "en", &saved_fight("f1", &[])).await.expect_err("no token");
        assert_eq!(message, "Sign in under Settings → Account to upload fights.");

        let dir = with_slice("signed-out-meta");
        save_sidecar(&dir.0, "f1", &SliceSidecar { auto_attempts: 2, retry_at_ms: 5, ..SliceSidecar::default() });
        let before = dir.read("slices/f1.json");
        assert!(upload(crate::account::client(), &dir.0, "en", &saved_fight("f1", &[])).await.is_err());
        assert_eq!(dir.read("slices/f1.json"), before);
    }

    #[test]
    fn a_good_reply_becomes_the_result() {
        let ok = judge_reply(200, r#"{"url":"https://a2tools.app/logs/abc","visibility":"private","extra":1}"#).expect("ok");
        assert_eq!((ok.url.as_str(), ok.visibility.as_str(), ok.duplicate), ("https://a2tools.app/logs/abc", "private", false));
        let bare = judge_reply(201, r#"{"url":"u"}"#).expect("ok");
        assert_eq!((bare.visibility.as_str(), bare.duplicate), ("", false));
        assert!(judge_reply(200, r#"{"url":"u","duplicate":true}"#).expect("ok").duplicate);
    }

    #[test]
    fn a_success_status_with_a_strange_reply_is_tried_again() {
        for reply in ["not json", r#"{"visibility":"public"}"#, r#"{"url":null}"#, r#"{"url":"u","visibility":null}"#, ""] {
            let failure = judge_reply(200, reply).expect_err(reply);
            assert!(failure.retryable && !failure.keyring_locked, "{reply}");
            assert!(failure.message.contains("a2tools.app") && failure.message.contains("200"), "{}", failure.message);
        }
    }

    #[test]
    fn error_statuses_map_to_their_message_and_retry() {
        let expired = judge_reply(401, r#"{"error":"x","message":"m"}"#).expect_err("401");
        assert!(expired.message.contains("expired") && !expired.retryable);
        let scope = judge_reply(403, r#"{"error":"insufficient_scope"}"#).expect_err("403");
        assert!(scope.message.contains("before uploads existed") && !scope.retryable);
        for named in [&expired, &scope] {
            assert!(named.message.contains("Settings → Account"), "{}", named.message);
        }
        let busy = judge_reply(429, r#"{"message":"slow down"}"#).expect_err("429");
        assert!(busy.message.contains("in the last hour") && busy.retryable);

        let said = [
            (403, r#"{"error":"forbidden","message":"forbidden"}"#, "forbidden", false),
            (413, r#"{"error":"too_large","message":"The slice is larger than any real fight."}"#, "The slice is larger than any real fight.", false),
            (422, r#"{"error":"no_fight","message":"M"}"#, "M", false),
            (503, r#"{"message":"down"}"#, "down", true),
        ];
        for (status, reply, message, retryable) in said {
            let failure = judge_reply(status, reply).expect_err(reply);
            assert_eq!((failure.message.as_str(), failure.retryable, failure.keyring_locked), (message, retryable, false));
        }

        for (status, reply, retryable) in [(408, "", true), (500, "", true), (599, "oops", true), (404, r#"{"message":5}"#, false)] {
            let failure = judge_reply(status, reply).expect_err(reply);
            assert!(failure.message.contains(&status.to_string()), "{}", failure.message);
            assert!(failure.message.contains("a2tools.app"), "{}", failure.message);
            assert_eq!((failure.retryable, failure.keyring_locked), (retryable, false), "{status}");
        }
    }

    #[test]
    fn the_link_is_kept_in_the_meta_with_its_other_fields() {
        let dir = with_slice("link");
        let before = SliceSidecar { uploader_actor_id: Some(3001), auto_attempts: 3, retry_at_ms: 77, gave_up: true, ..SliceSidecar::default() };
        save_sidecar(&dir.0, "f1", &before);
        let uploaded = PublishOutcome { url: "https://a2tools.app/logs/abc".into(), visibility: "private".into(), duplicate: false };
        keep_link(&dir.0, "f1", &uploaded);
        let meta = load_sidecar(&dir.0, "f1");
        assert_eq!(meta.url.as_deref(), Some("https://a2tools.app/logs/abc"));
        assert_eq!(meta.visibility.as_deref(), Some("private"));
        assert_eq!((meta.uploader_actor_id, meta.auto_attempts, meta.retry_at_ms, meta.gave_up), (Some(3001), 3, 77, true));

        keep_link(&dir.0, "f1", &PublishOutcome { url: "u".into(), visibility: String::new(), duplicate: true });
        assert_eq!(load_sidecar(&dir.0, "f1").visibility.as_deref(), Some(""));
    }

    #[test]
    fn a_fight_sent_from_a_packet_log_gets_a_private_slices_folder() {
        let dir = Scratch::new("upload.from-log");
        keep_link(&dir.0, "f1", &PublishOutcome { url: "u".into(), visibility: "public".into(), duplicate: false });
        let text = String::from_utf8(dir.read("slices/f1.json").expect("the meta")).expect("text");
        assert!(text.contains(r#""uploaderActorId":null"#) && text.contains(r#""url":"u""#), "{text}");
        assert!(dir.read("slices/f1.a2es.gz").is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(slice_folder(&dir.0)).expect("the folder").permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_request_on_the_wire_is_the_documented_one() {
        let dir = with_slice("wire");
        save_sidecar(&dir.0, "f1", &SliceSidecar { uploader_actor_id: Some(3001), ..SliceSidecar::default() });
        let record = saved_fight("f1", &[(3001, "Abcd")]);
        let (at, server) = answer_once("200 OK", r#"{"url":"https://a2tools.app/logs/abc","visibility":"private"}"#);
        let result = post_fight(&plain_client(), &at, MADE_UP_TOKEN, &dir.0, "en", &record).await;
        let raw = server.join().expect("the listener");
        let expected_body = request_body(&dir.0, "en", &record).expect("a body");

        let end = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("a header end");
        let head = std::str::from_utf8(&raw[..end]).expect("ASCII head");
        let mut lines = head.split("\r\n");
        assert_eq!(lines.next(), Some("POST /api/logs HTTP/1.1"));
        let mut headers: Vec<(String, String)> = lines
            .map(|l| {
                let (name, value) = l.split_once(':').expect("a header");
                (name.to_ascii_lowercase(), value.trim().to_string())
            })
            .collect();
        headers.sort();
        let host = at.trim_start_matches("http://").to_string();
        let mut wanted = vec![
            ("accept".to_string(), "*/*".to_string()),
            ("authorization".to_string(), format!("Bearer {MADE_UP_TOKEN}")),
            ("content-length".to_string(), expected_body.len().to_string()),
            ("content-type".to_string(), "application/json".to_string()),
            ("host".to_string(), host),
            ("user-agent".to_string(), crate::version::user_agent()),
        ];
        wanted.sort();
        assert_eq!(headers, wanted);
        assert_eq!(&raw[end + 4..], expected_body.as_bytes());

        let result = result.expect("uploaded");
        assert_eq!((result.url.as_str(), result.visibility.as_str()), ("https://a2tools.app/logs/abc", "private"));
        let meta = load_sidecar(&dir.0, "f1");
        assert_eq!(meta.url.as_deref(), Some("https://a2tools.app/logs/abc"));
        assert_eq!((meta.visibility.as_deref(), meta.uploader_actor_id), (Some("private"), Some(3001)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_refused_upload_leaves_the_meta_alone() {
        let dir = with_slice("refused");
        save_sidecar(&dir.0, "f1", &SliceSidecar { auto_attempts: 1, retry_at_ms: 9, ..SliceSidecar::default() });
        let before = dir.read("slices/f1.json");
        let (at, server) = answer_once("503 Service Unavailable", r#"{"error":"busy","message":"down"}"#);
        let failure = post_fight(&plain_client(), &at, MADE_UP_TOKEN, &dir.0, "en", &saved_fight("f1", &[]))
            .await
            .expect_err("503");
        server.join().expect("the listener");
        assert_eq!((failure.message.as_str(), failure.retryable), ("down", true));
        assert_eq!(dir.read("slices/f1.json"), before);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn no_answer_at_all_is_tried_again() {
        let dir = with_slice("unreachable");
        let before = dir.read("slices/f1.json");
        let closed = TcpListener::bind("127.0.0.1:0").expect("a local port").local_addr().expect("its address");
        let failure = post_fight(&plain_client(), &format!("http://{closed}"), MADE_UP_TOKEN, &dir.0, "en", &saved_fight("f1", &[]))
            .await
            .expect_err("nobody listens");
        assert!(failure.retryable && !failure.keyring_locked);
        assert!(failure.message.starts_with("Could not reach a2tools.app ("), "{}", failure.message);
        assert_eq!(dir.read("slices/f1.json"), before);
    }
}
