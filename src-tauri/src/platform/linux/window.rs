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
    let main = window.label() == "main";
    if main {
        *INPUT_RECT.lock() = rect;
    }
    let window = window.clone();
    super::dialog::on_gtk_thread(move || {
        let Ok(gtk_window) = window.gtk_window() else { return false };
        if main {
            apply_input_region(&gtk_window);
        } else {
            set_window_region(&gtk_window, rect);
        }
        true
    })
    .unwrap_or(false)
}

fn set_window_region(gtk_window: &gtk::ApplicationWindow, rect: Option<(f64, f64, f64, f64, f64)>) {
    use gtk::prelude::*;
    match rect {
        None => gtk_window.input_shape_combine_region(None),
        Some((x, y, w, h, css_scale)) => {
            let (x, y, w, h) = window_rect((x, y, w, h), css_scale, gtk_window.scale_factor());
            let region = gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(x, y, w, h));
            gtk_window.input_shape_combine_region(Some(&region));
        }
    }
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
    // A layer surface cannot be minimized; it is hidden instead.
    if is_layer(window) {
        let _ = window.hide();
        return;
    }
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
    let size = if layer_active(window) { layer_size(window, size) } else { size };
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
    if layer_active(window) {
        if border() > 0 {
            let window = window.clone();
            super::dialog::on_gtk_thread(move || {
                use gtk::prelude::*;
                // A layer surface takes the window's natural size, which the
                // border margins add to.
                if let (Ok(vbox), Some((w, h))) = (window.default_vbox(), *CONTENT_SIZE.lock()) {
                    vbox.set_size_request(w, h);
                }
                if let Ok(gtk_window) = window.gtk_window() {
                    apply_input_region(&gtk_window);
                }
            });
        }
        keep_layer_drawing_soon(window);
    }
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

/// The overlay as a Wayland layer surface (wlr-layer-shell), above every
/// window including a fullscreen game, through gtk-layer-shell. The library is
/// loaded at run time: without it, on X11, or on a compositor without the
/// protocol (GNOME), the overlay stays a normal window.
mod layer {
    use std::ffi::c_char;
    use std::sync::OnceLock;

    pub type Window = *mut gtk::ffi::GtkWindow;
    pub const LAYER_OVERLAY: i32 = 3;
    pub const EDGE_LEFT: i32 = 0;
    pub const EDGE_TOP: i32 = 2;
    pub const KEYBOARD_NONE: i32 = 0;

    pub struct Api {
        pub is_supported: unsafe extern "C" fn() -> i32,
        pub init_for_window: unsafe extern "C" fn(Window),
        pub is_layer_window: unsafe extern "C" fn(Window) -> i32,
        pub set_namespace: unsafe extern "C" fn(Window, *const c_char),
        pub set_layer: unsafe extern "C" fn(Window, i32),
        pub set_anchor: unsafe extern "C" fn(Window, i32, i32),
        pub set_margin: unsafe extern "C" fn(Window, i32, i32),
        pub get_margin: unsafe extern "C" fn(Window, i32) -> i32,
        pub set_exclusive_zone: unsafe extern "C" fn(Window, i32),
        pub set_keyboard_mode: unsafe extern "C" fn(Window, i32),
        _lib: libloading::Library,
    }

    pub fn api() -> Option<&'static Api> {
        static API: OnceLock<Option<Api>> = OnceLock::new();
        API.get_or_init(load).as_ref()
    }

    fn load() -> Option<Api> {
        // SAFETY: the symbols are gtk-layer-shell's documented C API, typed as
        // in gtk-layer-shell.h; the library stays loaded for the process.
        unsafe {
            let lib = ["libgtk-layer-shell.so.0", "libgtk-layer-shell.so"]
                .iter()
                .find_map(|name| libloading::Library::new(name).ok())?;
            macro_rules! sym {
                ($name:literal) => {
                    *lib.get(concat!($name, "\0").as_bytes()).ok()?
                };
            }
            let is_supported = sym!("gtk_layer_is_supported");
            let init_for_window = sym!("gtk_layer_init_for_window");
            let is_layer_window = sym!("gtk_layer_is_layer_window");
            let set_namespace = sym!("gtk_layer_set_namespace");
            let set_layer = sym!("gtk_layer_set_layer");
            let set_anchor = sym!("gtk_layer_set_anchor");
            let set_margin = sym!("gtk_layer_set_margin");
            let get_margin = sym!("gtk_layer_get_margin");
            let set_exclusive_zone = sym!("gtk_layer_set_exclusive_zone");
            let set_keyboard_mode = sym!("gtk_layer_set_keyboard_mode");
            Some(Api {
                is_supported, init_for_window, is_layer_window, set_namespace, set_layer,
                set_anchor, set_margin, get_margin, set_exclusive_zone, set_keyboard_mode,
                _lib: lib,
            })
        }
    }
}

/// Set once the overlay became a layer surface; only the overlay ever is one.
static LAYER_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn layer_active(window: &tauri::WebviewWindow) -> bool {
    window.label() == "main" && LAYER_ACTIVE.load(std::sync::atomic::Ordering::Relaxed)
}

fn layer_ptr(gtk_window: &gtk::ApplicationWindow) -> layer::Window {
    use gtk::glib::translate::ToGlibPtr;
    use gtk::prelude::*;
    gtk_window.upcast_ref::<gtk::Window>().to_glib_none().0
}

/// Commit the overlay's surface. GTK holds a layer surface's frames until the
/// compositor confirms a size change, and the compositor confirms one only
/// after a commit, so a resize made right after mapping waited forever.
fn commit_layer_surface(window: &gtk::ApplicationWindow) {
    use gtk::glib::translate::ToGlibPtr;
    use gtk::prelude::*;
    use std::ffi::c_void;
    type GetSurface = unsafe extern "C" fn(*mut gtk::gdk::ffi::GdkWindow) -> *mut c_void;
    type GetVersion = unsafe extern "C" fn(*mut c_void) -> u32;
    type Marshal = unsafe extern "C" fn(*mut c_void, u32, *const c_void, u32, u32, ...) -> *mut c_void;
    static SYMBOLS: std::sync::OnceLock<Option<(GetSurface, GetVersion, Marshal)>> = std::sync::OnceLock::new();
    // SAFETY: GDK's Wayland backend and libwayland-client are loaded with GTK;
    // the types match gdkwayland.h and wayland-client-core.h.
    let symbols = SYMBOLS.get_or_init(|| unsafe {
        let this = libloading::os::unix::Library::this();
        let get_surface: GetSurface = *this.get(b"gdk_wayland_window_get_wl_surface\0").ok()?;
        let get_version: GetVersion = *this.get(b"wl_proxy_get_version\0").ok()?;
        let marshal: Marshal = *this.get(b"wl_proxy_marshal_flags\0").ok()?;
        Some((get_surface, get_version, marshal))
    });
    let (Some((get_surface, get_version, marshal)), Some(gdk_window)) = (symbols, window.window()) else { return };
    const WL_SURFACE_COMMIT: u32 = 6;
    unsafe {
        let surface = get_surface(gdk_window.to_glib_none().0);
        if !surface.is_null() {
            marshal(surface, WL_SURFACE_COMMIT, std::ptr::null(), get_version(surface), 0);
        }
    }
}

/// Keep a layer overlay drawing after a size change or a remap.
///
/// GtkWindow freezes a window's updates when it asks for a new size and thaws
/// them when the matching configure event arrives. A layer surface never
/// delivers that event, so the overlay stopped drawing for good (its frame
/// clock never ran). Commit the surface so the compositor answers the size,
/// then thaw once more while the frame clock stays still.
fn keep_layer_drawing(window: gtk::ApplicationWindow, tries: u32) {
    use gtk::prelude::*;
    commit_layer_surface(&window);
    let Some(gdk_window) = window.window() else { return };
    let Some(clock) = gdk_window.frame_clock() else { return };
    let before = clock.frame_counter();
    gdk_window.invalidate_rect(None, true);
    clock.request_phase(gtk::gdk::FrameClockPhase::PAINT);
    gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(120), move || {
        if clock.frame_counter() != before || tries == 0 {
            return;
        }
        thaw_toplevel_updates(&gdk_window);
        keep_layer_drawing(window, tries - 1);
    });
}

fn thaw_toplevel_updates(gdk_window: &gtk::gdk::Window) {
    use gtk::glib::translate::ToGlibPtr;
    type Thaw = unsafe extern "C" fn(*mut gtk::gdk::ffi::GdkWindow);
    static THAW: std::sync::OnceLock<Option<Thaw>> = std::sync::OnceLock::new();
    // SAFETY: exported by GDK 3 (gdkwindow.h); takes the toplevel GdkWindow.
    let thaw = THAW.get_or_init(|| unsafe {
        Some(*libloading::os::unix::Library::this().get(b"gdk_window_thaw_toplevel_updates_libgtk_only\0").ok()?)
    });
    if let Some(thaw) = thaw {
        unsafe { thaw(gdk_window.to_glib_none().0) };
    }
}

/// `keep_layer_drawing` once GTK has applied a change (a remap, a resize).
fn keep_layer_drawing_soon(window: &tauri::WebviewWindow) {
    let window = window.clone();
    gtk::glib::MainContext::default().invoke(move || {
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(40), move || {
            if let Ok(gtk_window) = window.gtk_window() {
                keep_layer_drawing(gtk_window, 3);
            }
        });
    });
}

/// Make the overlay a layer surface at `pos` (logical pixels from the top-left
/// of the output) when `enabled` and the session supports it. Must run before
/// the overlay is first shown. Returns whether the overlay is one.
pub fn init_overlay_layer(window: &tauri::WebviewWindow, enabled: bool, pos: (i32, i32)) -> bool {
    if !enabled {
        return false;
    }
    let window = window.clone();
    let done = super::dialog::on_gtk_thread(move || {
        use gtk::prelude::*;
        let Some(api) = layer::api() else {
            tracing::warn!("Wayland layer: gtk-layer-shell is not installed; normal window");
            return false;
        };
        // SAFETY: gtk-layer-shell C API on the GTK thread, on a live window.
        if unsafe { (api.is_supported)() } == 0 {
            tracing::info!("Wayland layer: this session has no layer-shell; normal window");
            return false;
        }
        let Ok(gtk_window) = window.gtk_window() else { return false };
        let w = layer_ptr(&gtk_window);
        if gtk_window.is_realized() {
            tracing::warn!("Wayland layer: the overlay was already shown; normal window");
            return false;
        }
        if pointer_leaves_on_drag() {
            if let Ok(vbox) = window.default_vbox() {
                vbox.set_margin_start(DRAG_BORDER);
                vbox.set_margin_end(DRAG_BORDER);
                vbox.set_margin_top(DRAG_BORDER);
                vbox.set_margin_bottom(DRAG_BORDER);
                BORDER.store(DRAG_BORDER, std::sync::atomic::Ordering::Relaxed);
            }
        }
        let b = border();
        unsafe {
            (api.init_for_window)(w);
            (api.set_namespace)(w, c"daevalog-overlay".as_ptr());
            (api.set_layer)(w, layer::LAYER_OVERLAY);
            (api.set_anchor)(w, layer::EDGE_LEFT, 1);
            (api.set_anchor)(w, layer::EDGE_TOP, 1);
            // Margins count from the monitor's edges, not from panels.
            (api.set_exclusive_zone)(w, -1);
            // The game keeps the keyboard; the overlay is used with the mouse.
            (api.set_keyboard_mode)(w, layer::KEYBOARD_NONE);
            (api.set_margin)(w, layer::EDGE_LEFT, pos.0.max(0) - b);
            (api.set_margin)(w, layer::EDGE_TOP, pos.1.max(0) - b);
        }
        // A web view that was in the window before it became a layer surface
        // does not paint (WebKitGTK 2.52, gtk-layer-shell 0.10) until the
        // surface is mapped a second time. Remap it once, shortly after.
        let remapped = std::cell::Cell::new(false);
        gtk_window.connect_map(move |window| {
            if remapped.replace(true) {
                return;
            }
            let window = window.clone();
            gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || {
                window.hide();
                window.show_all();
                let window = window.clone();
                gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(40), move || {
                    // A place saved on a larger monitor would be out of reach.
                    if let Some((x, y)) = layer_position(&window) {
                        place_overlay(&window, x, y);
                    }
                    keep_layer_drawing(window, 3);
                });
            });
        });
        LAYER_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
        tracing::info!("Wayland layer: overlay is a layer surface at {},{}", pos.0, pos.1);
        true
    });
    done.unwrap_or(false)
}

/// Whether `window` is a layer surface.
pub fn is_layer(window: &tauri::WebviewWindow) -> bool {
    layer_active(window)
}

/// Whether the compositor takes the pointer away from the overlay in the
/// middle of a drag once it leaves the overlay. Hyprland does, and sends the
/// overlay a release then; KWin and Sway keep the pointer with the surface the
/// button went down on.
fn pointer_leaves_on_drag() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some_and(|v| !v.is_empty())
}

/// Where the pointer leaves the overlay mid-drag, the overlay has a clear
/// border this wide around the meter. It takes the mouse only during a drag,
/// so a fast drag stays on the overlay while it catches up.
const DRAG_BORDER: i32 = 128;
static BORDER: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static DRAGGING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn border() -> i32 {
    BORDER.load(std::sync::atomic::Ordering::Relaxed)
}

/// The page's last input-region request (the lock button), and the meter's
/// size in window pixels for the border's input region.
static INPUT_RECT: parking_lot::Mutex<Option<(f64, f64, f64, f64, f64)>> = parking_lot::Mutex::new(None);
static CONTENT_SIZE: parking_lot::Mutex<Option<(i32, i32)>> = parking_lot::Mutex::new(None);

/// Apply the overlay's input region on the GTK thread: the page's rectangle
/// (the lock button) or, with a border, the meter itself; all of the window
/// in a drag.
fn apply_input_region(gtk_window: &gtk::ApplicationWindow) {
    use gtk::prelude::*;
    let b = border();
    let rect = match *INPUT_RECT.lock() {
        Some((x, y, w, h, css_scale)) => {
            let (x, y, w, h) = window_rect((x, y, w, h), css_scale, gtk_window.scale_factor());
            Some((x + b, y + b, w, h))
        }
        None if b > 0 && !DRAGGING.load(std::sync::atomic::Ordering::Relaxed) => {
            CONTENT_SIZE.lock().map(|(w, h)| (b, b, w, h))
        }
        None => None,
    };
    match rect {
        None => gtk_window.input_shape_combine_region(None),
        Some((x, y, w, h)) => {
            let region = gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(x, y, w, h));
            gtk_window.input_shape_combine_region(Some(&region));
        }
    }
}

/// The position that keeps a `size` overlay inside a `monitor`-sized area.
fn clamp_position((x, y): (i32, i32), (w, h): (i32, i32), monitor: Option<(i32, i32)>) -> (i32, i32) {
    match monitor {
        Some((mw, mh)) => (x.min(mw - w).max(0), y.min(mh - h).max(0)),
        None => (x.max(0), y.max(0)),
    }
}

/// The overlay's monitor in layout pixels.
fn monitor_area(gtk_window: &gtk::ApplicationWindow) -> Option<gtk::gdk::Rectangle> {
    use gtk::prelude::*;
    Some(gtk_window.display().monitor_at_window(&gtk_window.window()?)?.geometry())
}

/// Where the meter is on its monitor: the layer margins plus the border.
fn layer_position(gtk_window: &gtk::ApplicationWindow) -> Option<(i32, i32)> {
    let api = layer::api()?;
    let w = layer_ptr(gtk_window);
    let b = border();
    // SAFETY: gtk-layer-shell C API on the GTK thread, on a live window.
    unsafe {
        ((api.is_layer_window)(w) != 0)
            .then(|| ((api.get_margin)(w, layer::EDGE_LEFT) + b, (api.get_margin)(w, layer::EDGE_TOP) + b))
    }
}

/// Put the meter at `x`, `y` on its monitor and commit at once, not at GTK's
/// next frame: the next drag step is measured against it.
fn place_overlay(gtk_window: &gtk::ApplicationWindow, x: i32, y: i32) -> Option<(i32, i32)> {
    use gtk::prelude::*;
    let api = layer::api()?;
    let w = layer_ptr(gtk_window);
    let b = border();
    let (width, height) = gtk_window.size();
    let monitor = monitor_area(gtk_window).map(|area| (area.width(), area.height()));
    let (x, y) = clamp_position((x, y), (width - 2 * b, height - 2 * b), monitor);
    // SAFETY: as in `layer_position`.
    unsafe {
        if (api.is_layer_window)(w) == 0 {
            return None;
        }
        (api.set_margin)(w, layer::EDGE_LEFT, x - b);
        (api.set_margin)(w, layer::EDGE_TOP, y - b);
    }
    commit_layer_surface(gtk_window);
    Some((x, y))
}

/// Hyprland animates a layer surface to each new place, and measures the
/// pointer from where the animation is, so the page cannot tell from its
/// events where the pointer is. Hyprland's socket can: the drag follows it.
mod hyprland {
    use std::io::{Read, Write};

    /// The pointer in layout pixels, from Hyprland's `cursorpos` request.
    pub fn cursor() -> Option<(i32, i32)> {
        let signature = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
        let path = std::path::Path::new(&runtime).join("hypr").join(signature).join(".socket.sock");
        let mut socket = std::os::unix::net::UnixStream::connect(path).ok()?;
        let timeout = Some(std::time::Duration::from_millis(100));
        socket.set_read_timeout(timeout).ok()?;
        socket.set_write_timeout(timeout).ok()?;
        socket.write_all(b"cursorpos").ok()?;
        let mut reply = String::new();
        socket.read_to_string(&mut reply).ok()?;
        parse(&reply)
    }

    pub(super) fn parse(reply: &str) -> Option<(i32, i32)> {
        let (x, y) = reply.trim().split_once(',')?;
        Some((x.trim().parse::<f64>().ok()?.round() as i32, y.trim().parse::<f64>().ok()?.round() as i32))
    }
}

/// During a drag on Hyprland: the pointer's offset from the meter's corner,
/// in layout pixels, with the monitor's origin added.
static HYPRLAND_GRAB: parking_lot::Mutex<Option<(i32, i32)>> = parking_lot::Mutex::new(None);

/// The page starts dragging the layer overlay: the border takes the mouse.
/// Returns whether pointer events during the drag measure from where the
/// overlay was when it began: Sway keeps that place for the whole drag, KWin
/// measures from the overlay's current place.
pub fn begin_layer_drag(window: &tauri::WebviewWindow) -> bool {
    set_layer_dragging(window, true);
    std::env::var_os("SWAYSOCK").is_some_and(|v| !v.is_empty())
        || std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.split(':').any(|d| d.eq_ignore_ascii_case("sway")))
}

/// The page saw the end of the drag.
pub fn end_layer_drag(window: &tauri::WebviewWindow) {
    set_layer_dragging(window, false);
}

fn set_layer_dragging(window: &tauri::WebviewWindow, dragging: bool) {
    if !layer_active(window) {
        return;
    }
    DRAGGING.store(dragging, std::sync::atomic::Ordering::Relaxed);
    let window = window.clone();
    super::dialog::on_gtk_thread(move || {
        let Ok(gtk_window) = window.gtk_window() else { return };
        let grab = || {
            let (cx, cy) = hyprland::cursor()?;
            let (mx, my) = layer_position(&gtk_window)?;
            let area = monitor_area(&gtk_window)?;
            Some((cx - mx - area.x(), cy - my - area.y()))
        };
        *HYPRLAND_GRAB.lock() = if dragging && pointer_leaves_on_drag() { grab() } else { None };
        if border() > 0 {
            apply_input_region(&gtk_window);
        }
    });
}

/// Put a layer-surface overlay at `x`, `y` logical pixels on its monitor, or
/// on Hyprland under the pointer. A layer surface has no compositor move; its
/// margins are its position. Returns where it went, kept on the monitor.
pub fn place_overlay_layer(window: &tauri::WebviewWindow, x: i32, y: i32) -> Option<(i32, i32)> {
    if !layer_active(window) {
        return None;
    }
    let window = window.clone();
    super::dialog::on_gtk_thread(move || {
        let gtk_window = window.gtk_window().ok()?;
        let grab = *HYPRLAND_GRAB.lock();
        let (x, y) = match grab.zip(hyprland::cursor()) {
            Some(((gx, gy), (cx, cy))) => (cx - gx, cy - gy),
            None => (x, y),
        };
        place_overlay(&gtk_window, x, y)
    })
    .flatten()
}

/// The position of a layer-surface overlay in logical pixels, or `None` for a
/// normal window (whose position the window manager knows).
pub fn overlay_layer_position(window: &tauri::WebviewWindow) -> Option<(i32, i32)> {
    if !layer_active(window) {
        return None;
    }
    let window = window.clone();
    super::dialog::on_gtk_thread(move || layer_position(&window.gtk_window().ok()?)).flatten()
}

/// The window size for a `size` meter on the layer overlay: no larger than the
/// room from its place to the monitor's right and bottom edges, plus the
/// border. Also keeps the meter's size for the input region.
fn layer_size(window: &tauri::WebviewWindow, size: tauri::Size) -> tauri::Size {
    let scale = window.scale_factor().unwrap_or(1.0);
    let want = size.to_logical::<f64>(scale);
    let window = window.clone();
    let room = super::dialog::on_gtk_thread(move || {
        let gtk_window = window.gtk_window().ok()?;
        let area = monitor_area(&gtk_window)?;
        let (x, y) = layer_position(&gtk_window)?;
        Some((f64::from((area.width() - x).max(1)), f64::from((area.height() - y).max(1))))
    })
    .flatten();
    let (rw, rh) = room.unwrap_or((f64::MAX, f64::MAX));
    let (w, h) = (want.width.min(rw), want.height.min(rh));
    *CONTENT_SIZE.lock() = Some((w.ceil() as i32, h.ceil() as i32));
    let b = f64::from(border());
    if b == 0.0 && w == want.width && h == want.height {
        return size;
    }
    tauri::Size::Logical(tauri::LogicalSize::new(w + 2.0 * b, h + 2.0 * b))
}

#[cfg(test)]
mod layer_tests {
    use super::{clamp_position, hyprland};

    #[test]
    fn hyprland_cursor_replies_parse() {
        assert_eq!(hyprland::parse("640, 400\n"), Some((640, 400)));
        assert_eq!(hyprland::parse("-12.6, 3.4"), Some((-13, 3)));
        assert_eq!(hyprland::parse("ok"), None);
    }

    #[test]
    fn the_layer_overlay_stays_on_its_monitor() {
        assert_eq!(clamp_position((100, 50), (400, 300), Some((1920, 1080))), (100, 50));
        assert_eq!(clamp_position((-30, -5), (400, 300), Some((1920, 1080))), (0, 0));
        assert_eq!(clamp_position((1800, 1000), (400, 300), Some((1920, 1080))), (1520, 780));
        // Larger than the monitor: pinned to the top-left corner.
        assert_eq!(clamp_position((10, 10), (2000, 1200), Some((1920, 1080))), (0, 0));
        assert_eq!(clamp_position((-1, 5000), (400, 300), None), (0, 5000));
    }
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
