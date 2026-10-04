//! Setup for the whole process, before the UI starts.
//!
//! WebKitGTK's DMA-BUF renderer hands its frames to the compositor as GPU
//! buffers, and some setups reject them: on NVIDIA under Wayland the window
//! never opens ("Error 71 (Protocol error) dispatching to Wayland display",
//! issue #7), and under XWayland GBM cannot allocate them ("Failed to create
//! GBM buffer"). 2.0.38 turned the renderer off for that. From WebKitGTK 2.54
//! a window drawn without it is mostly blank: only parts of the transparent
//! overlay paint (issue #8). Keeping the renderer and having it hand frames
//! over in shared memory avoids both. Tested on WebKitGTK 2.52.6 and 2.54.1,
//! AMD and NVIDIA, Wayland and XWayland. A value the player set themselves
//! for either variable is left alone.

const DISABLE: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";
const FORCE_SHM: &str = "WEBKIT_DMABUF_RENDERER_FORCE_SHM";

/// Returns a note for the log when it changed anything.
pub fn prepare() -> Option<String> {
    if std::env::var_os(DISABLE).is_some() || std::env::var_os(FORCE_SHM).is_some() {
        return None;
    }
    // SAFETY: called first thing in `run`, before any thread is started, so
    // nothing can be reading the environment concurrently.
    unsafe { std::env::set_var(FORCE_SHM, "1") };
    Some(format!("{FORCE_SHM}=1 (set by the meter; set {FORCE_SHM}=0 to hand frames over as GPU buffers)"))
}
