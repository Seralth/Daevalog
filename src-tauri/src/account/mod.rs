//! The a2tools.app account: the HTTP client the meter talks through, the
//! device sign-in, and who is signed in. The token itself lives in `secret`.

pub mod secret;

use std::future::Future;
use std::path::Path;
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── The HTTP client and where it points; only the base URL names the site ──

const PRODUCTION_URL: &str = "https://a2tools.app";

/// A fresh client with the meter's settings. Https only: a token or an upload
/// never leaves over a plain connection, and a plain URL fails before any
/// connection is made.
pub fn http_client() -> reqwest::Client {
    let built = reqwest::Client::builder()
        .user_agent(crate::version::user_agent())
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build();
    match built {
        Ok(made) => made,
        Err(e) => {
            // A client without these settings would be worse than none.
            tracing::error!("The HTTP client could not be built: {e}");
            panic!("the HTTP client could not be built: {e}");
        }
    }
}

/// The client a front end without one of its own keeps for the whole run.
/// Built on first use, so its user agent carries a release line set at the
/// start (`version::set_release`).
pub fn client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(http_client)
}

/// Where requests go: `A2TOOLS_API` in a debug build, else production.
pub fn server_root() -> String {
    let set = std::env::var("A2TOOLS_API").ok();
    api_base(dev_override(set.as_deref(), cfg!(debug_assertions)))
}

/// The override is for development: a release build ignores it, so a stray
/// environment variable cannot send the token and uploads elsewhere.
fn dev_override(value: Option<&str>, debug_build: bool) -> Option<&str> {
    if debug_build || value.is_none() {
        return value;
    }
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| tracing::warn!("A2TOOLS_API is ignored in a release build; using {PRODUCTION_URL}"));
    None
}

/// The token goes to this server in a header, so an override must be https.
/// Anything else falls back to production.
fn api_base(override_url: Option<&str>) -> String {
    let Some(value) = override_url.map(str::trim).filter(|v| !v.is_empty()) else {
        return PRODUCTION_URL.to_string();
    };
    match reqwest::Url::parse(value) {
        Ok(url) if url.scheme() == "https" && url.host_str().is_some() => {
            value.trim_end_matches('/').to_string()
        }
        _ => {
            tracing::warn!("A2TOOLS_API is not an https URL; using {PRODUCTION_URL}");
            PRODUCTION_URL.to_string()
        }
    }
}

// ── The site ───────────────────────────────────────────────────────────────

/// Must match the host of `PRODUCTION_URL`.
const SITE_HOST: &str = "a2tools.app";

/// The name the site knows the meter by. Not a secret: it limits a token to
/// the scopes the site grants the meter.
const METER_CLIENT_NAME: &str = "dps-meter";

/// Whether a link from the network may be opened in the browser: an https
/// page on a2tools.app or one of its subdomains, with no user, password or
/// port that could make it land somewhere else.
pub fn is_site_url(url: &str) -> bool {
    let Ok(link) = reqwest::Url::parse(url) else {
        return false;
    };
    let on_site = link.host_str().is_some_and(|host| {
        host.strip_suffix(SITE_HOST).is_some_and(|front| front.is_empty() || front.ends_with('.'))
    });
    // The parser drops a default port, so `:443` passes, and keeps a host's
    // trailing dot, so `a2tools.app.` fails.
    let no_detour = link.username().is_empty() && link.password().is_none() && link.port().is_none();
    link.scheme() == "https" && no_detour && on_site
}

/// What the site lists this sign-in as. It names the system, never the
/// machine.
pub fn device_label() -> String {
    label_for_system(std::env::consts::OS)
}

fn label_for_system(os: &str) -> String {
    let system = match os {
        "linux" => "Linux",
        "windows" => "Windows",
        "macos" => "macOS",
        other => other,
    };
    format!("Daevalog DPS Meter on {system}")
}

/// Every account request is small; a server that has not answered by then
/// will not.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// A JSON request to the site, answered with its status and body text. A body
/// that cannot be read counts as empty.
async fn post_to_site(client: &reqwest::Client, path: &str, body: String) -> reqwest::Result<(StatusCode, String)> {
    let reply = client
        .post(format!("{}{path}", server_root()))
        .timeout(REQUEST_TIMEOUT)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await?;
    let status = reply.status();
    Ok((status, reply.text().await.unwrap_or_default()))
}

// ── Device sign-in (RFC 8628, with the site's differences) ─────────────────

/// The site's answer to a sign-in start: the code the player approves in the
/// browser, and the one the meter polls with. `device_code` is a credential,
/// so it never serializes, and nothing logs it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignInGrant {
    #[serde(skip_serializing)]
    pub device_code: String,
    pub user_code: String,
    pub expires_in: i64,
    pub interval: i64,
    pub verification_uri: String,
    pub verification_uri_complete: String,
}

fn start_body(device_label: &str) -> String {
    serde_json::json!({ "client": METER_CLIENT_NAME, "device_label": device_label }).to_string()
}

fn poll_body(device_code: &str) -> String {
    serde_json::json!({ "device_code": device_code }).to_string()
}

/// Asks the site for a sign-in code. One try; the player starts again on a
/// failure.
pub async fn start(client: &reqwest::Client, device_label: &str) -> Result<SignInGrant, String> {
    match post_to_site(client, "/api/device/code", start_body(device_label)).await {
        Ok((status, text)) => grant_from_reply(status, &text),
        Err(e) => Err(format!("Could not reach a2tools.app ({e}). Check your connection and try again.")),
    }
}

fn grant_from_reply(status: StatusCode, text: &str) -> Result<SignInGrant, String> {
    if !status.is_success() {
        return Err(format!("a2tools.app turned the sign-in down ({status}). Try again later."));
    }
    serde_json::from_str(text)
        .map_err(|e| format!("The reply from a2tools.app could not be read ({e}). Try again later."))
}

/// What one poll of the token endpoint said.
#[derive(Debug, PartialEq)]
enum TokenReply {
    Approved(String),
    Pending,
    SlowDown,
    Expired,
    Denied,
    /// Nothing decided; worth asking again. Carries what went wrong.
    TryAgain(String),
}

/// A token wins over everything; then the RFC's error words. The site says
/// "keep polling" with a 428, which still holds when a proxy page replaces
/// the body.
fn read_token_reply(status: StatusCode, text: &str) -> TokenReply {
    let fields: serde_json::Map<String, Value> = serde_json::from_str(text).unwrap_or_default();
    if let Some(token) = fields.get("access_token").and_then(Value::as_str).filter(|t| !t.is_empty()) {
        return TokenReply::Approved(token.to_string());
    }
    let word = fields.get("error").and_then(Value::as_str);
    match (word, status.as_u16()) {
        (Some("authorization_pending"), _) | (None, 428) => TokenReply::Pending,
        (Some("slow_down"), _) => TokenReply::SlowDown,
        (Some("expired_token"), _) => TokenReply::Expired,
        (Some("access_denied"), _) => TokenReply::Denied,
        (Some(other), _) => TokenReply::TryAgain(other.into()),
        (None, code) => TokenReply::TryAgain(format!("HTTP {code}")),
    }
}

async fn ask_for_token(client: &reqwest::Client, device_code: &str) -> TokenReply {
    match post_to_site(client, "/api/device/token", poll_body(device_code)).await {
        Ok((status, text)) => read_token_reply(status, &text),
        Err(e) => TokenReply::TryAgain(e.to_string()),
    }
}

const CODE_EXPIRED: &str = "The sign-in code ran out before it was approved. Start the sign-in again.";
const SIGN_IN_DECLINED: &str = "The sign-in was declined on a2tools.app. Start it again if that was a mistake.";

/// The server's poll interval, kept within reason.
fn starting_interval(server_secs: i64) -> Duration {
    Duration::from_secs(server_secs.clamp(1, 60) as u64)
}

/// How long the code is polled for, kept within reason.
fn code_lifetime(server_secs: i64) -> Duration {
    Duration::from_secs(server_secs.clamp(60, 3600) as u64)
}

/// Polls until the player decides in the browser, then stores the token.
pub async fn poll_until_decided(client: &reqwest::Client, grant: &SignInGrant, app_data_dir: &Path) -> Result<(), String> {
    let code = grant.device_code.as_str();
    let deadline = Instant::now() + code_lifetime(grant.expires_in);
    let token = await_decision(
        starting_interval(grant.interval),
        deadline,
        tokio::time::sleep,
        move || ask_for_token(client, code),
    )
    .await?;
    secret::save(app_data_dir, &token)
        .map_err(|why| format!("The sign-in worked, but its token could not be stored securely: {why}"))?;
    tracing::info!("The a2tools.app account is connected");
    Ok(())
}

/// The poll loop, with the wait and the poll passed in so tests run it
/// without a clock or a server. Each poll comes one interval after the last
/// check; a slow-down adds 5 s for good, as the RFC asks.
async fn await_decision<W, Waited, A, Asked>(
    mut every: Duration,
    deadline: Instant,
    mut wait: W,
    mut ask: A,
) -> Result<String, String>
where
    W: FnMut(Duration) -> Waited,
    Waited: Future<Output = ()>,
    A: FnMut() -> Asked,
    Asked: Future<Output = TokenReply>,
{
    while Instant::now() < deadline {
        wait(every).await;
        match ask().await {
            TokenReply::Approved(token) => return Ok(token),
            TokenReply::Pending => {}
            TokenReply::SlowDown => every += Duration::from_secs(5),
            TokenReply::TryAgain(why) => tracing::debug!("A sign-in poll failed; polling again: {why}"),
            TokenReply::Expired => return Err(CODE_EXPIRED.to_string()),
            TokenReply::Denied => return Err(SIGN_IN_DECLINED.to_string()),
        }
    }
    Err(CODE_EXPIRED.to_string())
}

// ── Who is signed in ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum AccountState {
    SignedIn(AccountSummary),
    SignedOut,
    /// Not known now; the stored token is kept. Says why, for Settings.
    Unavailable(String),
}

/// The account as `/api/me` describes it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub supporter: bool,
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Every other key of the reply; the site's `display_name` lands here.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

const KEYRING_LOCKED: &str =
    "The desktop keyring is locked, so the account cannot be checked. Unlock the keyring and open Settings again.";

/// Who the stored token belongs to. A token the site no longer accepts is
/// deleted; any other trouble keeps it.
pub async fn whoami(client: &reqwest::Client, app_data_dir: &Path) -> AccountState {
    let token = match secret::load_stored(app_data_dir) {
        secret::Stored::Token(token) => token,
        secret::Stored::Missing => return AccountState::SignedOut,
        secret::Stored::Locked => return AccountState::Unavailable(KEYRING_LOCKED.to_string()),
    };
    let sent = client
        .get(format!("{}/api/me", server_root()))
        .timeout(REQUEST_TIMEOUT)
        .bearer_auth(&token)
        .send()
        .await;
    let Ok(reply) = sent else {
        return AccountState::Unavailable("Could not reach a2tools.app to check the account.".to_string());
    };
    let status = reply.status();
    let state = state_from_me(status, &reply.text().await.unwrap_or_default());
    if matches!(state, AccountState::SignedOut) {
        tracing::info!("a2tools.app no longer accepts the stored token; signing out");
        secret::clear(app_data_dir);
    }
    state
}

fn state_from_me(status: StatusCode, text: &str) -> AccountState {
    if status == StatusCode::UNAUTHORIZED {
        return AccountState::SignedOut;
    }
    if !status.is_success() {
        return AccountState::Unavailable(format!("a2tools.app could not check the account ({status})."));
    }
    match serde_json::from_str(text) {
        Ok(summary) => AccountState::SignedIn(summary),
        Err(_) => AccountState::Unavailable("Unexpected reply from a2tools.app.".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::built::Scratch;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    #[tokio::test(flavor = "current_thread")]
    async fn the_http_client_refuses_plain_http() {
        let err = super::http_client().get("http://127.0.0.1:9/").send().await.unwrap_err();
        assert!(err.is_builder(), "{err}");
    }

    #[test]
    fn the_api_override_is_for_debug_builds_only() {
        assert_eq!(dev_override(Some("https://staging.example.org"), true), Some("https://staging.example.org"));
        assert_eq!(dev_override(Some("https://staging.example.org"), false), None);
        assert_eq!(dev_override(None, false), None);
        assert_eq!(api_base(dev_override(Some("https://staging.example.org"), false)), "https://a2tools.app");
    }

    #[test]
    fn only_an_https_override_moves_the_base() {
        assert_eq!(api_base(None), "https://a2tools.app");
        assert_eq!(api_base(Some("")), "https://a2tools.app");
        assert_eq!(api_base(Some("https://staging.example.org/")), "https://staging.example.org");
        for bad in ["http://localhost:8787", "ftp://a2tools.app", "a2tools.app"] {
            assert_eq!(api_base(Some(bad)), "https://a2tools.app", "{bad}");
        }
    }

    #[test]
    fn the_base_follows_the_build_kind() {
        let base = |value, debug| api_base(dev_override(value, debug));
        assert_eq!(base(Some("https://staging.example.org/"), true), "https://staging.example.org");
        assert_eq!(base(Some("http://localhost:8787"), true), "https://a2tools.app");
    }

    #[test]
    fn site_links_open_only_on_a2tools_over_https() {
        for good in [
            "https://a2tools.app/link?code=ABCD-1234",
            "https://logs.a2tools.app/abc",
            "https://A2Tools.app/logs/abc",
            "https://a2tools.app",
            "https://a2tools.app:443/x",
            "https://a2tools.app:443/",
            "HTTPS://a2tools.app/",
            "https://a.b.a2tools.app/",
        ] {
            assert!(is_site_url(good), "{good} should open");
        }
        for bad in [
            "http://a2tools.app/link",
            "https://a2tools.app.example.com/",
            "https://evila2tools.app/",
            "https://a2tools.app@example.com/",
            "https://user@a2tools.app/",
            "https://:pw@a2tools.app/",
            "https://a2tools.app:8443/",
            "https://a2tools.app./",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "",
            "not a url",
            "https://127.0.0.1/",
        ] {
            assert!(!is_site_url(bad), "{bad} should not open");
        }
    }

    #[test]
    fn the_label_names_the_system_only() {
        assert_eq!(label_for_system("linux"), "Daevalog DPS Meter on Linux");
        assert_eq!(label_for_system("windows"), "Daevalog DPS Meter on Windows");
        assert_eq!(label_for_system("macos"), "Daevalog DPS Meter on macOS");
        assert_eq!(label_for_system("freebsd"), "Daevalog DPS Meter on freebsd");
        #[cfg(target_os = "linux")]
        assert_eq!(device_label(), "Daevalog DPS Meter on Linux");
    }

    #[test]
    fn request_bodies_are_compact_json_in_key_order() {
        assert_eq!(
            start_body("Daevalog DPS Meter on Linux"),
            r#"{"client":"dps-meter","device_label":"Daevalog DPS Meter on Linux"}"#
        );
        assert_eq!(start_body("a \"b\""), r#"{"client":"dps-meter","device_label":"a \"b\""}"#);
        assert_eq!(poll_body("made-up-code"), r#"{"device_code":"made-up-code"}"#);
    }

    const GRANT_JSON: &str = r#"{"device_code":"d","user_code":"ABCD-1234","expires_in":600,"interval":5,"verification_uri":"https://a2tools.app/link","verification_uri_complete":"https://a2tools.app/link?code=ABCD-1234","extra":1}"#;

    #[test]
    fn a_grant_needs_all_six_fields_with_whole_seconds() {
        let grant = grant_from_reply(StatusCode::OK, GRANT_JSON).unwrap();
        assert_eq!(grant.device_code, "d");
        assert_eq!(grant.user_code, "ABCD-1234");
        assert_eq!((grant.expires_in, grant.interval), (600, 5));
        assert_eq!(grant.verification_uri, "https://a2tools.app/link");
        assert_eq!(grant.verification_uri_complete, "https://a2tools.app/link?code=ABCD-1234");
        assert!(grant_from_reply(StatusCode::OK, &GRANT_JSON.replace(r#""interval":5,"#, "")).is_err());
        assert!(grant_from_reply(StatusCode::OK, &GRANT_JSON.replace("600", r#""600""#)).is_err());
        assert!(grant_from_reply(StatusCode::OK, "").is_err());
    }

    #[test]
    fn a_refused_start_names_the_status() {
        let why = grant_from_reply(StatusCode::SERVICE_UNAVAILABLE, GRANT_JSON).unwrap_err();
        assert!(why.contains("503 Service Unavailable"), "{why}");
    }

    #[test]
    fn a_grant_never_serializes_its_polling_code() {
        let with_secret = GRANT_JSON.replace(r#""device_code":"d""#, r#""device_code":"SECRET-DEVICE-CODE""#);
        let grant = grant_from_reply(StatusCode::OK, &with_secret).unwrap();
        assert_eq!(grant.device_code, "SECRET-DEVICE-CODE");
        let json = serde_json::to_string(&grant).unwrap();
        assert!(!json.contains("SECRET-DEVICE-CODE") && !json.contains("device_code"), "{json}");
        assert!(json.contains("ABCD-1234"), "{json}");
        let keys: Vec<String> = serde_json::from_str::<serde_json::Map<String, Value>>(&json).unwrap().keys().cloned().collect();
        assert_eq!(keys, ["expires_in", "interval", "user_code", "verification_uri", "verification_uri_complete"]);
    }

    fn reply(code: u16, body: &str) -> TokenReply {
        read_token_reply(StatusCode::from_u16(code).unwrap(), body)
    }

    #[test]
    fn a_token_in_the_reply_is_an_approval() {
        let approved = reply(200, r#"{"access_token":"tok_made_up_1","token_type":"bearer"}"#);
        assert_eq!(approved, TokenReply::Approved("tok_made_up_1".into()));
        // Even beside an error word.
        assert_eq!(reply(400, r#"{"access_token":"t","error":"slow_down"}"#), TokenReply::Approved("t".into()));
    }

    #[test]
    fn the_rfc_error_words_map_one_to_one() {
        let words = [("slow_down", TokenReply::SlowDown), ("expired_token", TokenReply::Expired), ("access_denied", TokenReply::Denied)];
        for (word, meaning) in words {
            assert_eq!(reply(400, &format!(r#"{{"error":"{word}"}}"#)), meaning, "{word}");
        }
    }

    #[test]
    fn a_428_means_keep_polling_whatever_its_body() {
        for body in [r#"{"error":"authorization_pending"}"#, "<html>gateway</html>", r#"{"error":7}"#] {
            assert_eq!(reply(428, body), TokenReply::Pending, "{body}");
        }
    }

    #[test]
    fn anything_else_is_worth_another_poll() {
        let undecided = [
            (200, r#"{"access_token":""}"#, "HTTP 200"),
            (200, r#"{"access_token":5}"#, "HTTP 200"),
            (500, r#"{"error":"no database bound"}"#, "no database bound"),
            (502, "", "HTTP 502"),
            (400, "[]", "HTTP 400"),
        ];
        for (code, body, why) in undecided {
            assert_eq!(reply(code, body), TokenReply::TryAgain(why.to_string()), "{code} {body}");
        }
    }

    #[test]
    fn the_server_timings_are_kept_within_reason() {
        let secs = Duration::from_secs;
        assert_eq!(starting_interval(0), secs(1));
        assert_eq!(starting_interval(-3), secs(1));
        assert_eq!(starting_interval(5), secs(5));
        assert_eq!(starting_interval(120), secs(60));
        assert_eq!(code_lifetime(30), secs(60));
        assert_eq!(code_lifetime(600), secs(600));
        assert_eq!(code_lifetime(7200), secs(3600));
    }

    /// Runs the poll loop on scripted replies, without waiting; gives the
    /// outcome and the waits asked for. Each wait is followed by one poll.
    async fn scripted(every: u64, deadline: Instant, replies: Vec<TokenReply>) -> (Result<String, String>, Vec<u64>) {
        let waits = RefCell::new(Vec::new());
        let queue = RefCell::new(VecDeque::from(replies));
        let outcome = await_decision(
            Duration::from_secs(every),
            deadline,
            |d: Duration| {
                waits.borrow_mut().push(d.as_secs());
                std::future::ready(())
            },
            || std::future::ready(queue.borrow_mut().pop_front().expect("polled past the script")),
        )
        .await;
        (outcome, waits.into_inner())
    }

    fn later() -> Instant {
        Instant::now() + Duration::from_secs(600)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn each_slow_down_adds_five_seconds_for_good() {
        use TokenReply::*;
        let (outcome, waits) = scripted(5, later(), vec![SlowDown, SlowDown, Approved("t".into())]).await;
        assert_eq!(outcome, Ok("t".into()));
        assert_eq!(waits, [5, 10, 15]);
        let (_, waits) = scripted(60, later(), vec![SlowDown, Pending, Approved("t".into())]).await;
        assert_eq!(waits, [60, 65, 65]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn polling_goes_on_through_pending_and_failures() {
        use TokenReply::*;
        let (outcome, waits) = scripted(5, later(), vec![Pending, Pending, Approved("tok_made_up_2".into())]).await;
        assert_eq!((outcome, waits.len()), (Ok("tok_made_up_2".into()), 3));
        let (outcome, waits) = scripted(5, later(), vec![TryAgain("timed out".into()), Approved("t".into())]).await;
        assert_eq!((outcome, waits.len()), (Ok("t".into()), 2));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_decline_or_expiry_ends_at_once() {
        use TokenReply::*;
        let (outcome, waits) = scripted(5, later(), vec![Denied]).await;
        assert_eq!((outcome, waits.len()), (Err(SIGN_IN_DECLINED.into()), 1));
        let (outcome, waits) = scripted(5, later(), vec![Pending, Expired]).await;
        assert_eq!((outcome, waits.len()), (Err(CODE_EXPIRED.into()), 2));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_passed_deadline_polls_no_more() {
        let (outcome, waits) = scripted(5, Instant::now(), vec![]).await;
        assert_eq!(outcome, Err(CODE_EXPIRED.to_string()));
        assert!(waits.is_empty());
    }

    fn me(code: u16, body: &str) -> AccountState {
        state_from_me(StatusCode::from_u16(code).unwrap(), body)
    }

    #[test]
    fn the_me_reply_decides_the_state() {
        assert!(matches!(me(401, ""), AccountState::SignedOut));
        let AccountState::Unavailable(why) = me(500, "") else { panic!() };
        assert_eq!(why, "a2tools.app could not check the account (500 Internal Server Error).");
        let AccountState::Unavailable(why) = me(599, "") else { panic!() };
        assert_eq!(why, "a2tools.app could not check the account (599 <unknown status code>).");
        let AccountState::SignedIn(summary) = me(200, r#"{"display_name":"Made Up","supporter":true}"#) else { panic!() };
        assert!(summary.supporter);
        for bad in ["", "<html>", r#"{"supporter":"yes"}"#] {
            let AccountState::Unavailable(why) = me(200, bad) else { panic!("{bad}") };
            assert_eq!(why, "Unexpected reply from a2tools.app.");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn no_token_means_signed_out_without_asking() {
        let dir = Scratch::new("whoami-missing");
        assert!(matches!(whoami(client(), &dir).await, AccountState::SignedOut));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "current_thread")]
    async fn a_locked_token_is_not_checked_or_dropped() {
        let dir = Scratch::new("whoami-locked");
        // A reference to an item no keyring holds reads as locked.
        std::fs::write(dir.join("credentials.dat"), "secret-service:no-such-item").unwrap();
        let AccountState::Unavailable(why) = whoami(client(), &dir).await else { panic!() };
        assert_eq!(
            why,
            "The desktop keyring is locked, so the account cannot be checked. Unlock the keyring and open Settings again."
        );
    }

    #[test]
    fn the_summary_takes_either_name_and_keeps_the_rest() {
        let read = |json: &str| serde_json::from_str::<AccountSummary>(json);
        let summary = read(r#"{"display_name":"Abcd","supporter":true,"scopes":["profile","sync"],"id":7}"#).unwrap();
        assert_eq!(summary.display_name, None);
        assert!(summary.supporter);
        assert_eq!(summary.scopes, ["profile", "sync"]);
        assert_eq!(Value::Object(summary.extra), serde_json::json!({ "display_name": "Abcd", "id": 7 }));
        let summary = read(r#"{"displayName":"Abcd"}"#).unwrap();
        assert_eq!(summary.display_name.as_deref(), Some("Abcd"));
        assert!(!summary.supporter && summary.scopes.is_empty() && summary.extra.is_empty());
        let summary = read("{}").unwrap();
        assert!(summary.display_name.is_none() && !summary.supporter && summary.scopes.is_empty() && summary.extra.is_empty());
        assert!(read(r#"{"supporter":"yes"}"#).is_err());
    }

    #[test]
    fn the_summary_writes_its_own_fields_first() {
        let summary: AccountSummary = serde_json::from_str(r#"{"id":7,"displayName":"Abcd","scopes":["sync"]}"#).unwrap();
        assert_eq!(
            serde_json::to_string(&summary).unwrap(),
            r#"{"displayName":"Abcd","supporter":false,"scopes":["sync"],"id":7}"#
        );
    }
}
