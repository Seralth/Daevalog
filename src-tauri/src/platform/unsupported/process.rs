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
