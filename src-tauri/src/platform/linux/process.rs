//! Setup for the whole process, before the UI starts.
//!
//! WebKitGTK's DMA-BUF renderer shares GPU buffers with the compositor, and
//! on some Wayland setups (NVIDIA drivers especially) the compositor rejects
//! them: the window never opens and GTK reports "Error 71 (Protocol error)
//! dispatching to Wayland display" (issue #7, KDE Plasma on CachyOS). Turning
//! that renderer off is the usual fix for WebKitGTK apps. The meter draws a
//! light UI, so the GPU path buys it nothing it would miss. A value the
//! player set themselves is left alone, so `=0` turns it back on.

const DMABUF: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";

/// Returns a note for the log when it changed anything.
pub fn prepare() -> Option<String> {
    if std::env::var_os(DMABUF).is_some() {
        return None;
    }
    // SAFETY: called first thing in `run`, before any thread is started, so
    // nothing can be reading the environment concurrently.
    unsafe { std::env::set_var(DMABUF, "1") };
    Some(format!("{DMABUF}=1 (set by the meter; set it to 0 to use WebKit's GPU renderer)"))
}
