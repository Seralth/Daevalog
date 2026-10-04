//! Window helpers on Linux. As in `../unsupported/window.rs`, except that the
//! pointer button can be read when the meter runs on GDK's X11 backend, and
//! the click-through lock works through the window's input region.
//!
//! The lock lets only the lock button's rectangle take the mouse
//! (`set_input_region`). GDK applies that with the X shape extension on X11
//! and as the surface's input region on Wayland, so the lock needs no pointer
//! position and works on native Wayland too. libX11 is loaded at run time, so
//! nothing new is linked.

use x11_dl::xlib;

/// GNOME uses compositor resizing on both native Wayland and X11.
pub fn compositor_resize_supported(window: &tauri::WebviewWindow) -> bool {
    if !super::process::is_gnome() {
        return false;
    }
    use gtk::prelude::*;
    let window = window.clone();
    super::dialog::on_gtk_thread(move || {
        window.gtk_window().ok().is_some_and(|w| {
            matches!(w.display().type_().name(), "GdkWaylandDisplay" | "GdkX11Display")
        })
    }).unwrap_or(false)
}

/// Compositor grabs clear client pointer focus. Reject synthetic WebKit
/// hover events until the pointer returns to this window.
pub fn resize_pointer_down(window: &tauri::WebviewWindow) -> Option<bool> {
    use gtk::{gdk, prelude::*};
    let window = window.clone();
    super::dialog::on_gtk_thread(move || {
        let gtk_window = window.gtk_window().ok()?;
        let surface = gtk_window.window()?;
        let pointer = gtk_window.display().default_seat()?.pointer()?;
        let hit = pointer.window_at_position().0?;
        if hit.toplevel() != surface.toplevel() {
            return None;
        }
        Some(surface.device_position(&pointer).3.contains(gdk::ModifierType::BUTTON1_MASK))
    }).flatten()
}

/// Commit unpinned size hints before requesting a compositor resize.
/// WebKit animation frames do not guarantee a GTK surface commit.
pub async fn prepare_resize(
    window: &tauri::WebviewWindow,
    min: tauri::LogicalSize<f64>,
) -> Result<(), String> {
    use gtk::{gdk, glib, prelude::*};
    let window = window.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let (clock, signal) = super::dialog::on_gtk_thread(move || {
        let gtk_window = window.gtk_window().map_err(|e| e.to_string())?;
        let clock = gtk_window.frame_clock().ok_or("Window has no frame clock")?;
        let geometry = gdk::Geometry::new(
            min.width.ceil() as i32, min.height.ceil() as i32,
            0, 0, 0, 0, 0, 0, 0.0, 0.0, gdk::Gravity::NorthWest,
        );
        gtk_window.set_geometry_hints(None::<&gtk::Widget>, Some(&geometry), gdk::WindowHints::MIN_SIZE);
        let sender = std::cell::RefCell::new(Some(tx));
        let signal = clock.connect_local("after-paint", true, move |_| {
            if let Some(tx) = sender.borrow_mut().take() {
                let _ = tx.send(());
            }
            None
        });
        gtk_window.queue_resize();
        gtk_window.queue_draw();
        clock.request_phase(gdk::FrameClockPhase::LAYOUT | gdk::FrameClockPhase::PAINT | gdk::FrameClockPhase::AFTER_PAINT);
        Ok::<_, String>((glib::SendWeakRef::from(clock.downgrade()), signal))
    }).ok_or("GTK thread unavailable")??;
    // Hidden windows may never paint; detach the handler even on timeout.
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), rx).await;
    super::dialog::on_gtk_thread(move || {
        if let Some(clock) = clock.upgrade() {
            clock.disconnect(signal);
        }
    });
    result.map_err(|_| "Timed out applying resize hints".to_string())?
        .map_err(|_| "Window closed before resize".to_string())
}

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

/// Not used here: the lock works through the input region instead.
pub fn cursor_position() -> Option<(i32, i32)> {
    None
}

/// The click-through lock works through the input region here.
pub fn input_region_supported() -> bool {
    true
}

/// Let only `rect` of the window take the mouse, or the whole window with
/// `None`. `rect` is the page's CSS-pixel x, y, width, height and its
/// devicePixelRatio.
pub fn set_input_region(window: &tauri::WebviewWindow, rect: Option<(f64, f64, f64, f64, f64)>) -> bool {
    use gtk::prelude::*;
    let window = window.clone();
    super::dialog::on_gtk_thread(move || {
        let Ok(gtk_window) = window.gtk_window() else { return false };
        match rect {
            None => gtk_window.input_shape_combine_region(None),
            Some((x, y, w, h, css_scale)) => {
                let (x, y, w, h) = window_rect((x, y, w, h), css_scale, gtk_window.scale_factor());
                let region = gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(x, y, w, h));
                gtk_window.input_shape_combine_region(Some(&region));
            }
        }
        true
    })
    .unwrap_or(false)
}

/// The window-pixel rectangle that covers a CSS-pixel rectangle, rounded
/// outward. GDK window pixels are CSS pixels times the page's zoom, which is
/// devicePixelRatio over GDK's own scale.
fn window_rect((x, y, w, h): (f64, f64, f64, f64), css_scale: f64, gdk_scale: i32) -> (i32, i32, i32, i32) {
    let f = css_scale / f64::from(gdk_scale.max(1));
    let f = if f.is_finite() && f > 0.0 { f } else { 1.0 };
    let (left, top) = ((x * f).floor(), (y * f).floor());
    let (right, bottom) = (((x + w) * f).ceil(), ((y + h) * f).ceil());
    (left as i32, top as i32, (right - left).max(1.0) as i32, (bottom - top).max(1.0) as i32)
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
    if window.label() == "main" && super::process::is_gnome() {
        let window = window.clone();
        super::dialog::on_gtk_thread(move || {
            use gtk::prelude::*;
            let Ok(window) = window.gtk_window() else { return };
            let context = window.style_context();
            if context.has_class("a2tools-overlay") { return; }
            let provider = gtk::CssProvider::new();
            if provider.load_from_data(b"window.a2tools-overlay decoration { box-shadow: none; margin: 0; border: 0; }").is_ok() {
                gtk::StyleContext::add_provider_for_screen(
                    &window.display().default_screen(), &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
                context.add_class("a2tools-overlay");
            }
        });
    }
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

/// The libraries Tauri's tray loads at runtime, in the order it tries them.
const TRAY_LIBRARIES: &[&str] = &["libayatana-appindicator3.so.1", "libappindicator3.so.1"];

/// Whether a tray library is installed. Tauri panics when it builds a tray
/// icon without one, so the meter checks first and goes without a tray.
pub fn tray_available() -> bool {
    // SAFETY: loading a shared library runs its initialisers; these are the
    // libraries Tauri itself loads for the tray.
    first_loadable(TRAY_LIBRARIES, |name| unsafe { libloading::Library::new(name) }.is_ok()).is_some()
}

fn first_loadable<'a>(names: &[&'a str], load: impl Fn(&str) -> bool) -> Option<&'a str> {
    names.iter().copied().find(|name| load(name))
}

#[cfg(test)]
mod tray_tests {
    use super::*;

    #[test]
    fn the_tray_needs_one_of_its_libraries() {
        assert_eq!(first_loadable(TRAY_LIBRARIES, |_| false), None);
        assert_eq!(first_loadable(TRAY_LIBRARIES, |n| n == "libappindicator3.so.1"), Some("libappindicator3.so.1"));
        assert_eq!(first_loadable(TRAY_LIBRARIES, |_| true), Some("libayatana-appindicator3.so.1"));
    }
}

#[cfg(test)]
mod tests {
    use super::window_rect;

    #[test]
    fn the_lock_region_covers_the_button_in_window_pixels() {
        assert_eq!(window_rect((300.0, 4.0, 24.0, 24.0), 1.0, 1), (300, 4, 24, 24));
        // HiDPI: GDK scales by 2 and so does the page; window pixels stay CSS pixels.
        assert_eq!(window_rect((300.0, 4.0, 24.0, 24.0), 2.0, 2), (300, 4, 24, 24));
        // Page zoom 1.25: fractions round outward, never cutting the button.
        assert_eq!(window_rect((300.5, 4.2, 24.0, 24.0), 1.25, 1), (375, 5, 31, 31));
        // A bad scale falls back to 1, and an empty rect still keeps one pixel.
        assert_eq!(window_rect((10.0, 10.0, 0.0, 0.0), f64::NAN, 1), (10, 10, 1, 1));
    }
}
