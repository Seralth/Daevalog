//! The supporter roster: where it comes from and how often it is read again.

use std::time::Duration;

use tauri::Manager;

use super::AppState;

/// Where the supporter roster lives. The same bucket the installer is served
/// from, so it costs no new infrastructure and is already cached at the edge.
const SUPPORTER_ROSTER_URL: &str = "https://cdn.a2tools.app/patrons-v1.bin";

/// A roster placed here overrides the downloaded one.
///
/// Build it with `a2t-roster names.txt -o <this path>`. It exists so the gold
/// name can be exercised before anything is published — otherwise the only way
/// to see the feature is to ship a real roster to every user and hope it looks
/// right.
///
/// Deliberately not restricted to debug builds. It changes nothing but the
/// colour of a name **on this machine**: the roster is only ever read locally,
/// is never uploaded, and no number, ordering or leaderboard depends on it. So
/// there is nothing here to cheat, and it doubles as the way to reproduce a
/// "why is this person gold" report.
const SUPPORTER_ROSTER_OVERRIDE: &str = "patrons-local.bin";

/// How often to look again. The override is polled quickly so dropping the file
/// in shows up while you are still looking at the meter; the published roster
/// changes rarely enough that six hours is generous.
const ROSTER_POLL_OVERRIDE: Duration = Duration::from_secs(15);
const ROSTER_POLL_PUBLISHED: Duration = Duration::from_secs(6 * 60 * 60);

/// Read the local override, if one is there.
fn load_supporter_override(app_data_dir: &std::path::Path) -> Option<crate::supporters::Roster> {
    let path = app_data_dir.join(SUPPORTER_ROSTER_OVERRIDE);
    let bytes = std::fs::read(&path).ok()?;
    match crate::supporters::Roster::parse(&bytes) {
        Some(roster) => {
            tracing::info!(
                "Supporter roster OVERRIDE in use: {} entries from {} — delete the file to \
                 go back to the published roster",
                roster.len(),
                path.display()
            );
            Some(roster)
        }
        None => {
            // Say so loudly: a malformed override looks exactly like "the
            // feature is broken" from the outside.
            tracing::warn!(
                "{} is not a supporter roster — rebuild it with `a2t-roster names.txt -o {}`",
                path.display(),
                path.display()
            );
            None
        }
    }
}

/// Download and parse the supporter roster.
///
/// Every failure is `None` and nobody renders gold. That is deliberate: this is
/// a cosmetic, and there is no version of "the CDN is down" that should produce
/// a visible error, a retry storm, or a wrong answer.
async fn fetch_supporter_roster(client: &reqwest::Client) -> Option<crate::supporters::Roster> {
    let response = client
        .get(SUPPORTER_ROSTER_URL)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let bytes = response.bytes().await.ok()?;
    // A roster for ten thousand supporters is about 80 KB; anything far past
    // that is not one of ours.
    if bytes.len() > 8 * 1024 * 1024 {
        return None;
    }
    crate::supporters::Roster::parse(&bytes)
}

pub(super) fn spawn_roster_poll(app: &tauri::AppHandle) {
    // Supporter roster: fetched, never queried. See `crate::supporters`
    // — asking the server "is this player a supporter?" would hand it a
    // list of who you play with, every fight, for a cosmetic.
    let handle_roster = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut had_override = false;
        loop {
            let mut wait = ROSTER_POLL_PUBLISHED;
            if let Some(state) = handle_roster.try_state::<AppState>() {
                // The override wins when present, and is re-read every
                // pass so editing it takes effect without a restart.
                match load_supporter_override(&state.app_data_dir) {
                    Some(roster) => {
                        had_override = true;
                        wait = ROSTER_POLL_OVERRIDE;
                        state.data_storage.set_supporters(roster);
                    }
                    None => {
                        let just_lost_override = had_override;
                        if had_override {
                            tracing::info!("Supporter roster override removed");
                            had_override = false;
                        }
                        match fetch_supporter_roster(&state.http).await {
                            Some(roster) => {
                                tracing::info!(
                                    "Supporter roster: {} entries",
                                    roster.len()
                                );
                                state.data_storage.set_supporters(roster);
                            }
                            None => {
                                tracing::debug!("Supporter roster unavailable");
                                // Keep looking for the override often, so
                                // dropping the file in works on a machine
                                // that has never reached the CDN.
                                wait = ROSTER_POLL_OVERRIDE;
                                // Only wipe the roster if the override we
                                // were using has just gone away. Clearing
                                // on any failed fetch would mean one CDN
                                // hiccup removes every supporter's gold
                                // until the next successful poll.
                                if just_lost_override {
                                    state
                                        .data_storage
                                        .set_supporters(Default::default());
                                }
                            }
                        }
                    }
                }
            }
            tokio::time::sleep(wait).await;
        }
    });
}
