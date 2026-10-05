//! Whether this meter is signed in, and signing out, for the account commands.

use super::AppState;

pub(crate) async fn account_status(state: &AppState) -> Result<Option<crate::account::AccountSummary>, String> {
    let who = match crate::account::whoami(&state.http, &state.app_data_dir).await {
        crate::account::AccountState::SignedIn(summary) => Some(summary),
        crate::account::AccountState::SignedOut => None,
        crate::account::AccountState::Unavailable(why) => return Err(why),
    };
    *state.account_seen.lock() = Some(who.clone());
    Ok(who)
}
