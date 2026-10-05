//! The desktop application: Tauri commands, capture wiring, window management.
//!
//! Split out of `lib.rs` so the crate can be built *without* it. The parser has
//! to compile to `wasm32-unknown-unknown` — that is how the server re-derives an
//! uploaded encounter with the same code the client ran — and nothing in this
//! file can: `tauri`, `libloading`, `reqwest` and the Windows API all fail on
//! that target. Everything here is behind the `desktop` feature; the parser core
//! is not, and `cargo check --no-default-features --target wasm32-unknown-unknown`
//! is what keeps it that way.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::capture::combat_port_detector::CombatPortDetector;
use crate::combat::data_storage::DataStorage;
use crate::combat::dps_calculator::{DetailsSource, DpsCalculator};
use crate::combat::ping_tracker::PingTracker;
use crate::config::settings::Settings;
use crate::history::fight_history::FightHistoryManager;
use crate::i18n::lookup::{NpcLookup, SkillLookup};

mod auto_upload;
mod capture_control;
mod commands;
mod data_folder;
mod drag_resize;
mod fights;
mod game_records;
mod links;
mod local_player;
mod meter;
mod overlay_lock;
mod page_support;
mod replay;
mod report;
mod screenshots;
mod setting_changes;
mod setup;
mod sharing;
mod sign_in;
mod tasks;
mod tool_windows;
mod tray_actions;

pub(crate) use overlay_lock::{overlay_lock_available, toggle_overlay_lock};
use overlay_lock::OverlayLock;
pub use setup::run;
pub(crate) use tray_actions::{open_history_from_tray, open_settings_from_tray, quit_from_tray};

/// Shared application state.
pub struct AppState {
    pub data_storage: Arc<DataStorage>,
    pub dps_calculator: Mutex<DpsCalculator>,
    /// Readers for the Details calls, which never take the meter's mutex.
    pub details: DetailsSource,
    pub ping_tracker: Arc<PingTracker>,
    pub port_detector: Arc<CombatPortDetector>,
    pub fight_history: FightHistoryManager,
    pub settings: Settings,
    pub skill_lookup: Arc<SkillLookup>,
    pub npc_lookup: Arc<NpcLookup>,
    pub app_data_dir: std::path::PathBuf,
    pub i18n_data_dir: Option<std::path::PathBuf>,
    /// One client, reused: anything periodic wants a pool rather than a TLS
    /// handshake every time.
    pub http: reqwest::Client,
    /// The header's suspend button: while set, the capture dispatcher drops
    /// every packet. Shared with it (`CaptureDispatcher::use_suspend_flag`).
    pub capture_suspended: Arc<std::sync::atomic::AtomicBool>,
    /// The overlay's click-through lock. See `apply_overlay_lock`.
    pub overlay_lock: Arc<OverlayLock>,
    /// What the last account check found: `None` until one has run, then
    /// `Some(None)` signed out or `Some(Some(_))` signed in. Settings shows it
    /// at once instead of "checking" for as long as the server takes.
    pub account_seen: Mutex<Option<Option<crate::account::AccountSummary>>>,
    /// The screenshot folder the meter's own picker returned. See
    /// `screenshots::capture_screenshot`.
    pub screenshot_folder: Mutex<Option<std::path::PathBuf>>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_csp_allows_every_inline_handler() {
        use sha2::{Digest, Sha256};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let read = |p: std::path::PathBuf| std::fs::read_to_string(p).unwrap();
        let conf: serde_json::Value =
            serde_json::from_str(&read(root.join("src-tauri/tauri.conf.json"))).unwrap();
        let mut pages = vec![read(root.join("index.html"))];
        // Every script, subfolders included.
        let mut dirs = vec![root.join("public/src/js")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() { dirs.push(path) } else { pages.push(read(path)) }
            }
        }
        // `onload="..."` and the like, in the page and in HTML the scripts build.
        let mut handlers = Vec::new();
        for page in &pages {
            let mut rest = page.as_str();
            while let Some(at) = rest.find(" on") {
                rest = &rest[at + 3..];
                let name = rest.bytes().take_while(|b| b.is_ascii_lowercase()).count();
                if name > 0 && rest[name..].starts_with("=\"") {
                    let body = &rest[name + 2..];
                    handlers.push(body[..body.find('"').unwrap()].to_string());
                }
            }
        }
        assert!(handlers.len() >= 5);
        for key in ["csp", "devCsp"] {
            let script_src = conf["app"]["security"][key]["script-src"].as_str().unwrap();
            for handler in &handlers {
                let hash = format!("'sha256-{}'", crate::share::base64(&Sha256::digest(handler.as_bytes())));
                assert!(script_src.contains(&hash), "{key} script-src has no {hash} for {handler}");
            }
        }
    }
}
