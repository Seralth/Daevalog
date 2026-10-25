//! The character part of the replay report (`A2_REPLAY_CHARACTER=1`): your own
//! character's latest state as one JSON object (`capture::character_report`).

use std::path::PathBuf;

use crate::capture::character_report::{self, CaptureText, Names};

/// Replay the logs (paths joined by ':', oldest first) and return the state
/// at the end as pretty JSON. `A2_GAME_DATA` names the game data folder.
pub(super) fn report(paths: &str) -> String {
    // Without the game's tables every name stays an id.
    let names = std::env::var_os("A2_GAME_DATA")
        .filter(|v| !v.is_empty())
        .map_or_else(Names::default, |root| Names::load(&PathBuf::from(root)));
    let mut texts = Vec::new();
    for path in paths.split(':').filter(|p| !p.is_empty()) {
        match std::fs::read_to_string(path) {
            Ok(text) => texts.push((path, text)),
            Err(_) => eprintln!("character: {path} not readable"),
        }
    }
    let logs: Vec<CaptureText> = texts.iter().map(|(name, text)| CaptureText { name, text }).collect();
    serde_json::to_string_pretty(&character_report::report(&logs, &names)).expect("a Value serialises")
}
