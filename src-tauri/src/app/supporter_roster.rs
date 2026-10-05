//! The supporter roster: where it comes from and how often it is read again.

use std::time::Duration;

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
pub(super) const ROSTER_POLL_OVERRIDE: Duration = Duration::from_secs(15);
pub(super) const ROSTER_POLL_PUBLISHED: Duration = Duration::from_secs(6 * 60 * 60);

/// Read the local override, if one is there.
pub(super) fn load_supporter_override(app_data_dir: &std::path::Path) -> Option<crate::supporters::Roster> {
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
pub(super) async fn fetch_supporter_roster(client: &reqwest::Client) -> Option<crate::supporters::Roster> {
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
