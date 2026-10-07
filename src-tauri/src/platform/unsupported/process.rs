//! Setup for the whole process, before the UI starts. Nothing here.

/// Returns a note for the log when it changed anything.
pub fn prepare() -> Option<String> {
    None
}

/// No Wayland layer overlay here.
pub fn overlay_layer() -> bool {
    false
}

pub fn overlay_layer_by_default() -> bool {
    false
}

/// The system, for a bug report.
pub fn system_name() -> String {
    std::env::consts::OS.to_string()
}

/// The display backend the meter chose, for a bug report: none here.
pub fn display_backend() -> Option<String> {
    None
}

/// Whether the system writes times of day on a 24-hour clock: not read here.
pub fn clock_24h() -> Option<bool> {
    None
}
