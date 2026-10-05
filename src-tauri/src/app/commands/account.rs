//! Signing in to a2tools.app, and who this meter is signed in as.

use tauri::Emitter;

use super::system::open_url;
use crate::app::{sign_in, AppState};

/// Who, if anyone, this meter is signed in as.
///
/// Returns `None` when there is no token, or when the server says the one we
/// have is no longer valid — a token revoked from the website should stop
/// looking connected here at the next check, not at the next reinstall.
#[tauri::command]
pub(crate) async fn account_status(
    state: tauri::State<'_, AppState>,
) -> Result<Option<crate::account::AccountSummary>, String> {
    sign_in::account_status(&state).await
}

/// What the last `account_status` found, without asking the server again.
/// `None` when nothing has been checked yet this session.
#[tauri::command]
pub(crate) fn account_status_cached(
    state: tauri::State<'_, AppState>,
) -> Option<Option<crate::account::AccountSummary>> {
    state.account_seen.lock().clone()
}

/// Begin signing in, and return the code to show the player.
///
/// The meter has no browser and cannot hold a client secret, so this is the
/// Device Authorization Grant: the server hands back a short code, the player
/// approves it on a2tools.app, and a background task polls until they do. The
/// polling credential never reaches the webview — see `DeviceGrant`.
#[tauri::command]
pub(crate) async fn account_begin_link(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<crate::account::LinkPrompt, String> {
    let grant = crate::account::start(&state.http, &crate::account::device_label()).await?;
    let prompt = crate::account::LinkPrompt::from(&grant);

    // Open the browser straight onto the filled-in code. If it fails the player
    // still has the code and the URL in front of them.
    if crate::account::is_site_url(&grant.verification_uri_complete) {
        open_url(grant.verification_uri_complete.clone());
    } else {
        tracing::warn!("Not opening the sign-in page: the server sent a link outside a2tools.app");
    }

    let http = state.http.clone();
    let app_data_dir = state.app_data_dir.clone();
    tauri::async_runtime::spawn(async move {
        let outcome = crate::account::poll_until_decided(&http, &grant, &app_data_dir).await;
        let payload = match &outcome {
            Ok(()) => serde_json::json!({ "connected": true }),
            Err(why) => serde_json::json!({ "connected": false, "error": why }),
        };
        if let Err(e) = app.emit("account-changed", payload) {
            tracing::warn!("could not announce the account change: {e}");
        }
    });

    Ok(prompt)
}

/// Forget the token on this machine.
///
/// Local only, deliberately. Revoking it everywhere is a decision to make on the
/// website, where every device that holds a token is listed — a meter that could
/// silently revoke its own credential server-side would make "sign out on this
/// PC" and "this PC was stolen" the same button.
#[tauri::command]
pub(crate) fn account_sign_out(state: tauri::State<'_, AppState>) {
    sign_in::account_sign_out(&state);
}
