//! Window helpers on Linux. As in `../unsupported/window.rs`, except that the
//! pointer position can be read when the meter runs on GDK's X11 backend.
//!
//! The click-through lock needs the pointer's position outside the meter's
//! windows (`platform::window::cursor_position`). A native Wayland window
//! cannot read that, but the meter often runs under XWayland (`GDK_BACKEND=x11`),
//! and so does a Proton game unless `PROTON_ENABLE_WAYLAND` is set; an X11
//! client can read the global pointer while it is over any X11 window. With
//! a position, the lock as it is works: a player tested it on KDE Plasma
//! (issue #14). libX11 is loaded at run time, so nothing new is linked.

use x11_dl::xlib;

/// One X connection per thread that asks, closed when the thread ends. The
/// pointer watch runs on its own thread; gdk's pointer calls would need the
/// GTK main thread.
struct Conn {
    x: xlib::Xlib,
    display: *mut xlib::Display,
}

impl Drop for Conn {
    fn drop(&mut self) {
        unsafe { (self.x.XCloseDisplay)(self.display) };
    }
}

thread_local! {
    static CONN: Option<Conn> = {
        xlib::Xlib::open().ok().and_then(|x| {
            let display = unsafe { (x.XOpenDisplay)(std::ptr::null()) };
            (!display.is_null()).then_some(Conn { x, display })
        })
    };
}

/// Whether the meter's windows are on GDK's X11 backend (XWayland or X11).
fn on_x11() -> bool {
    let backend = std::env::var("GDK_BACKEND").unwrap_or_default();
    match backend.split(',').next().map(str::trim) {
        Some("x11") => true,
        Some("wayland") => false,
        _ => std::env::var_os("WAYLAND_DISPLAY").is_none_or(|v| v.is_empty()),
    }
}

/// Where the mouse pointer is, in screen pixels: on the X11 backend only. A
/// native Wayland window cannot read it, so there the lock is not offered.
/// The root coordinates are the space tao's `inner_position()` uses on X11,
/// so the lock button's hit test lines up.
pub fn cursor_position() -> Option<(i32, i32)> {
    query_pointer().map(|(x, y, _)| (x, y))
}

/// Whether the left mouse button is held: on the X11 backend only. Ends a
/// window-manager resize of a tool window (see `release_size`).
pub fn primary_button_down() -> Option<bool> {
    query_pointer().map(|(_, _, mask)| mask & xlib::Button1Mask != 0)
}

fn query_pointer() -> Option<(i32, i32, u32)> {
    if !on_x11() {
        return None;
    }
    CONN.with(|conn| {
        let c = conn.as_ref()?;
        let (mut root_ret, mut child) = (0, 0);
        let (mut rx, mut ry, mut wx, mut wy, mut mask) = (0, 0, 0, 0, 0);
        let ok = unsafe {
            let root = (c.x.XDefaultRootWindow)(c.display);
            (c.x.XQueryPointer)(
                c.display, root, &mut root_ret, &mut child, &mut rx, &mut ry, &mut wx, &mut wy, &mut mask,
            )
        };
        (ok != 0).then_some((rx, ry, mask))
    })
}

pub fn start_drag(window: &tauri::WebviewWindow) {
    let _ = window.start_dragging();
}

pub fn show_on_top_without_focus(window: &tauri::WebviewWindow) {
    let _ = window.show();
    let _ = window.set_always_on_top(true);
}

pub fn minimize_off_top(window: &tauri::WebviewWindow) {
    let _ = window.set_always_on_top(false);
    let _ = window.minimize();
}

/// Size a meter window and pin its size hints to that size. Window managers
/// only edge-tile or maximize a window whose minimum size is below its maximum
/// (KWin: `X11Window::isResizable`), so pinned hints keep KDE, GNOME and the
/// rest from snapping the overlay or its tool windows into a tile. The meter
/// still sizes its windows itself: every size change goes through here. Min
/// and max go in one call; set one at a time, the window manager sees a
/// minimum above the maximum in between and the window flickers.
pub fn set_size(window: &tauri::WebviewWindow, size: tauri::Size) {
    let (w, h) = match size {
        tauri::Size::Physical(s) => (
            tauri::PixelUnit::Physical(tauri::PhysicalUnit::new(s.width as i32)),
            tauri::PixelUnit::Physical(tauri::PhysicalUnit::new(s.height as i32)),
        ),
        tauri::Size::Logical(s) => (
            tauri::PixelUnit::Logical(tauri::LogicalUnit::new(s.width)),
            tauri::PixelUnit::Logical(tauri::LogicalUnit::new(s.height)),
        ),
    };
    let _ = window.set_size_constraints(tauri::WindowSizeConstraints {
        min_width: Some(w),
        min_height: Some(h),
        max_width: Some(w),
        max_height: Some(h),
    });
    let _ = window.set_size(size);
}

/// Unpin a tool window's size so the window manager can resize it, down to
/// `min` logical pixels. Window managers tile on a move, not on a resize, so
/// this is safe for the length of a resize; `set_size` pins it again after.
pub fn release_size(window: &tauri::WebviewWindow, min: tauri::LogicalSize<f64>) {
    let _ = window.set_size_constraints(tauri::WindowSizeConstraints {
        min_width: Some(tauri::PixelUnit::Logical(tauri::LogicalUnit::new(min.width))),
        min_height: Some(tauri::PixelUnit::Logical(tauri::LogicalUnit::new(min.height))),
        max_width: None,
        max_height: None,
    });
}
