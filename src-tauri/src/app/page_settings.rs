//! The one-time move of old settings out of the page's own storage.
//!
//! Up to this build the page kept a copy of every setting in its WebKit
//! storage and read that copy whenever settings.json had no value, so the
//! Settings page could show a value the backend did not use (the layer switch
//! on while the meter ran as a normal window). settings.json is now the only
//! store. On its first start the page hands its old values over once and
//! removes them.
//!
//! The rule for each key:
//! - settings.json has it: settings.json wins. It is what the backend used,
//!   and every page start copied it over the page's copy.
//! - only the page has it: it moves, because the page acted on it (or sent it
//!   to the backend at startup), so the meter keeps doing what it did. A key
//!   only the backend acts on stays out: the backend never used the page's
//!   copy, so moving it would change what the meter does.

use std::collections::HashMap;

use crate::config::settings::Settings;

/// Keys the backend reads from settings.json on its own. The page never sends
/// them at startup, so a copy only the page has was never in effect.
const BACKEND_ONLY: &[&str] = &[
    // The display setup (platform/linux/process.rs).
    "dpsMeter.waylandLayer",
    // Uploads (share/auto_upload.rs) and the tray (tray.rs).
    "dpsMeter.autoUpload",
    "dpsMeter.startInTray",
    "dpsMeter.hideFromTaskbar",
    // Read at startup or by the meter tick (app/setup.rs, app/tasks.rs).
    "dpsMeter.autoHideMeter",
    "dpsMeter.encounterTimeoutSec",
    "dpsMeter.saveRawPackets",
    "dpsMeter.hotkey",
    "dpsMeter.toggleWindowHotkey",
    "dpsMeter.lockHotkey",
    // Saved by nothing now: the backend picks the capture device.
    "dpsMeter.manualDevice",
];

/// Whether a value only the page has moves into settings.json.
fn moves(key: &str) -> bool {
    // Window places and backend.* are written by the backend alone; the page
    // only held copies.
    !key.starts_with("window.") && !key.starts_with("backend.") && !BACKEND_ONLY.contains(&key)
}

/// Moves the page's old values into settings.json. Returns the keys moved.
pub(super) fn adopt(settings: &Settings, page: HashMap<String, String>) -> Vec<String> {
    settings.insert_missing(page.into_iter().filter(|(key, _)| moves(key)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("a2-page-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn page(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// A data folder from an older build: some values in settings.json only,
    /// some in the page's storage only, some in both with different values.
    #[test]
    fn settings_json_wins_and_page_only_values_move_unless_only_the_backend_reads_them() {
        let dir = folder("mixed");
        std::fs::write(
            dir.join("settings.json"),
            r#"{"dpsMeter.theme": "frost", "dpsMeter.roundDps": "false", "window.x": "10"}"#,
        )
        .unwrap();
        let settings = Settings::new(dir.clone());
        let moved = adopt(
            &settings,
            page(&[
                // In both, different: settings.json wins.
                ("dpsMeter.theme", "ember"),
                ("dpsMeter.roundDps", "true"),
                ("window.x", "99"),
                // Only in the page's storage, and the page acted on it: moves.
                ("dpsMeter.showPing", "false"),
                ("dpsMeter.displayMode", "both"),
                ("dpsMeter.userName", "Tester"),
                ("historyViewMode", "list"),
                // Only in the page's storage, and only the backend reads it: stays out.
                ("dpsMeter.waylandLayer", "true"),
                ("dpsMeter.autoUpload", "true"),
                ("window.y", "20"),
                ("backend.screenshotFolder", "/tmp"),
            ]),
        );
        assert_eq!(moved, ["dpsMeter.displayMode", "dpsMeter.showPing", "dpsMeter.userName", "historyViewMode"]);
        settings.flush().unwrap();
        let file = crate::config::settings::read_file(&dir.join("settings.json"));
        let value = |key: &str| file.get(key).map(String::as_str);
        assert_eq!(value("dpsMeter.theme"), Some("frost"));
        assert_eq!(value("dpsMeter.roundDps"), Some("false"));
        assert_eq!(value("window.x"), Some("10"));
        assert_eq!(value("dpsMeter.showPing"), Some("false"));
        assert_eq!(value("dpsMeter.displayMode"), Some("both"));
        assert_eq!(value("dpsMeter.userName"), Some("Tester"));
        assert_eq!(value("historyViewMode"), Some("list"));
        assert_eq!(value("dpsMeter.waylandLayer"), None);
        assert_eq!(value("dpsMeter.autoUpload"), None);
        assert_eq!(value("window.y"), None);
        assert_eq!(value("backend.screenshotFolder"), None);
        drop(settings);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The second start finds the page's storage empty and moves nothing.
    #[test]
    fn an_empty_page_storage_changes_nothing() {
        let dir = folder("empty");
        std::fs::write(dir.join("settings.json"), r#"{"dpsMeter.theme": "frost"}"#).unwrap();
        let settings = Settings::new(dir.clone());
        assert!(adopt(&settings, HashMap::new()).is_empty());
        assert_eq!(settings.get_all(), page(&[("dpsMeter.theme", "frost")]));
        drop(settings);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
