// SPDX-License-Identifier: AGPL-3.0-only

//! `Overlay`, the QML singleton for window state that is not drawing:
//! the input area of a locked meter, layer shell, the layer-shell position
//! and its drag, and the screenshot hook. The lock itself is a setting
//! (`Settings.locked`), kept across restarts.

use core::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QColor, QMargins, QPointF, QRectF, QSizeF, QString};

use crate::layer_drag::{self, LayerDrag, Pos};

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("cxx-qt-lib/qmargins.h");
        type QMargins = cxx_qt_lib::QMargins;

        include!("cxx-qt-lib/qpointf.h");
        type QPointF = cxx_qt_lib::QPointF;

        include!("cxx-qt-lib/qsizef.h");
        type QSizeF = cxx_qt_lib::QSizeF;

        include!("cxx-qt-lib/qrectf.h");
        type QRectF = cxx_qt_lib::QRectF;

        include!(<QtQuick/QQuickWindow>);
        type QQuickWindow;

        include!("cxx-qt-lib/qcolor.h");
        type QColor = cxx_qt_lib::QColor;

        include!("daevalog-qml/cpp/shell.h");

        #[cxx_name = "grabWindowToFile"]
        unsafe fn grab_window_to_file(window: *mut QQuickWindow, path: &QString) -> bool;

        #[cxx_name = "captureWindows"]
        unsafe fn capture_windows(window: *mut QQuickWindow, beside: *mut QQuickWindow, clipboard: bool, background: &QColor) -> Vec<u8>;

        #[cxx_name = "watchWindow"]
        unsafe fn watch_window(window: *mut QQuickWindow);

        #[cxx_name = "monotonicMicros"]
        fn monotonic_micros() -> u64;

        #[cxx_name = "pointerEventTime"]
        fn pointer_event_time() -> u64;

        #[cxx_name = "pointerButtonDown"]
        fn pointer_button_down() -> bool;

        #[cxx_name = "pointerGlobalPosition"]
        fn pointer_global_position() -> QPointF;

        #[cxx_name = "releasePointer"]
        unsafe fn release_pointer(window: *mut QQuickWindow);

        #[cxx_name = "takeFrameTimes"]
        fn take_frame_times() -> Vec<u64>;

        #[cxx_name = "requestFrame"]
        unsafe fn request_frame(window: *mut QQuickWindow);

        #[cxx_name = "setInputArea"]
        unsafe fn set_input_area(window: *mut QQuickWindow, area: &QRectF);

        #[cxx_name = "platformName"]
        fn platform_name() -> QString;

        #[cxx_name = "canShapeInput"]
        fn can_shape_input() -> bool;

        #[cxx_name = "setPopupPlace"]
        unsafe fn set_popup_place(window: *mut QQuickWindow, anchor: &QRectF, below: bool, adjust: u32);

        #[cxx_name = "keepOffTaskbar"]
        unsafe fn keep_off_taskbar(window: *mut QQuickWindow);

        #[cxx_name = "setOffTaskbar"]
        unsafe fn set_off_taskbar(window: *mut QQuickWindow, off: bool);
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(bool, layer_shell)]
        #[qproperty(bool, input_region, READ, CONSTANT)]
        #[qproperty(bool, unmanaged, READ, CONSTANT)]
        #[qproperty(bool, minimize_hidden, READ, CONSTANT)]
        #[qproperty(bool, off_taskbar, READ, CONSTANT)]
        #[qproperty(bool, taskbar_setting, READ, CONSTANT)]
        #[qproperty(bool, layer_setting, READ, CONSTANT)]
        #[qproperty(bool, auto_hide_setting, READ, CONSTANT)]
        #[qproperty(bool, layer_menu_keys, READ, CONSTANT)]
        #[qproperty(bool, layer_popups_lag, READ, CONSTANT)]
        #[qproperty(QMargins, layer_margins)]
        #[qproperty(QString, screenshot_path, READ, CONSTANT)]
        #[qproperty(QString, start_view, READ, CONSTANT)]
        #[qproperty(i32, start_width, READ, CONSTANT)]
        type Overlay = super::OverlayRust;

        /// Notes the window's pointer event and frame times, for a layer
        /// drag. Once per window.
        #[qinvokable]
        unsafe fn watch_window(self: &Overlay, window: *mut QQuickWindow);

        /// Keeps the window out of the taskbar (`off_taskbar`); before its
        /// first show.
        #[qinvokable]
        unsafe fn keep_off_taskbar(self: &Overlay, window: *mut QQuickWindow);

        /// Keeps the window out of the taskbar while `off`, or lets it back
        /// (the "Keep out of the taskbar" setting); X11 only.
        #[qinvokable]
        unsafe fn set_off_taskbar(self: &Overlay, window: *mut QQuickWindow, off: bool);

        /// Whether the game is the window in front, as the backend tells
        /// (`platform::window_detector`), for auto-hide.
        #[qinvokable]
        fn game_in_front(self: &Overlay) -> bool;

        /// Only `area` of the window takes clicks; an empty area gives all
        /// of it back.
        #[qinvokable]
        unsafe fn shape_input(self: &Overlay, window: *mut QQuickWindow, area: QRectF);

        /// Where a Wayland popup goes when it is next shown: on the top or
        /// (`below`) the bottom edge of `anchor`, its parent's coordinates.
        /// Where it does not fit, the compositor may `slide` it onto the
        /// screen, or `flip` it to the other edge first; with neither it
        /// goes exactly where asked.
        #[qinvokable]
        unsafe fn place_popup(self: &Overlay, window: *mut QQuickWindow, anchor: QRectF, below: bool, slide: bool, flip: bool);

        /// A layer-shell drag starts: the pointer took hold of the `size`
        /// window at `grab`, in a `screen`-sized area (the screen less the
        /// bars, where the margins count).
        #[qinvokable]
        fn begin_layer_drag(self: Pin<&mut Overlay>, grab: QPointF, size: QSizeF, screen: QSizeF);

        /// The pointer moved to `pointer` (window coordinates) in a layer
        /// drag; moves the window under it.
        #[qinvokable]
        unsafe fn layer_drag_to(self: Pin<&mut Overlay>, window: *mut QQuickWindow, pointer: QPointF);

        #[qinvokable]
        fn end_layer_drag(self: Pin<&mut Overlay>);

        /// Keeps a `size` layer-shell window inside its `screen`-sized area.
        #[qinvokable]
        fn keep_layer_on_screen(self: Pin<&mut Overlay>, size: QSizeF, screen: QSizeF);

        /// Where the pointer is on the screen, from the mouse event being
        /// delivered; X11 only.
        #[qinvokable]
        fn pointer_on_screen(self: &Overlay) -> QPointF;

        /// Ends the press that started a system resize (see shell.h).
        #[qinvokable]
        unsafe fn release_pointer(self: &Overlay, window: *mut QQuickWindow);

        /// Saves the window to the --screenshot file; false if that failed.
        #[qinvokable]
        unsafe fn save_screenshot(self: &Overlay, window: *mut QQuickWindow) -> bool;

        /// A screenshot of `window`, with `beside` on its left when not null,
        /// on `background`, to the clipboard; with `to_file` also a new PNG
        /// in the screenshot folder (Settings), never over a file there.
        /// JSON: {"clipboard": it is there, "file": the PNG's path, or ""}.
        #[qinvokable]
        unsafe fn capture(self: &Overlay, window: *mut QQuickWindow, beside: *mut QQuickWindow, to_file: bool, background: QColor) -> QString;
    }
}

/// A screenshot's file name, as 1.x names it: AION2_DPS_20261008_031500.png.
fn screenshot_name(now: chrono::DateTime<chrono::Local>) -> String {
    format!("AION2_DPS_{}.png", now.format("%Y%m%d_%H%M%S"))
}

/// Where a layer-shell meter starts, from the screen's top-left corner.
/// Normal windows are placed by the compositor instead.
const LAYER_START: (i32, i32) = (40, 160);

pub struct OverlayRust {
    /// Whether the meter is a layer-shell surface: the setting, where Qt
    /// runs on Wayland and the compositor offers layer shell, until Main.qml
    /// finds layer-shell-qt missing.
    layer_shell: bool,
    /// Whether part of the window can take clicks alone (`shape_input`):
    /// on Wayland and X11.
    input_region: bool,
    /// Whether the meter is an X11 window the window manager does not
    /// manage, which places itself (`display::x11_unmanaged`).
    unmanaged: bool,
    /// Whether a hidden normal Wayland window is minimized too
    /// (`display::minimize_when_hidden`).
    minimize_hidden: bool,
    /// Whether the meter keeps out of the taskbar (`display::off_taskbar`).
    off_taskbar: bool,
    /// Whether "Keep out of the taskbar" can act (`display::taskbar_setting`).
    taskbar_setting: bool,
    /// Whether "Overlay: Wayland layer" can act (`display::layer_setting`).
    layer_setting: bool,
    /// Whether "Auto-hide meter" can act: where the backend sees which
    /// window is in front (`window_detector::SEES_FOREGROUND`).
    auto_hide_setting: bool,
    /// Whether a layer meter takes the keys while a menu of it is open
    /// (`display::layer_menu_takes_keys`).
    layer_menu_keys: bool,
    /// Whether a layer meter's popups stay behind when it moves
    /// (`display::layer_popups_lag`).
    layer_popups_lag: bool,
    layer_margins: QMargins,
    screenshot_path: QString,
    /// --open: the window and page to open at the start ("history/record").
    start_view: QString,
    /// --width: that window's width; 0 for its own.
    start_width: i32,
    drag: Option<Drag>,
}

struct Drag {
    moves: LayerDrag,
    /// Whether event time stamps share the monotonic clock; if not, events
    /// are timed by when they arrive.
    event_clock: bool,
    /// On Hyprland: where the window's top-left was on the screen at the
    /// start. The pointer then comes from Hyprland's socket, measured from
    /// there (see hyprland.rs).
    origin: Option<crate::hyprland::Point>,
}

impl Default for OverlayRust {
    fn default() -> Self {
        let options = crate::options();
        let platform = qobject::platform_name().to_string();
        let layer_shell = crate::settings_store::store().values().layer_shell
            && crate::display::compositor_has_layer_shell()
            && platform.starts_with("wayland");
        Self {
            layer_shell,
            input_region: qobject::can_shape_input(),
            unmanaged: crate::display::x11_unmanaged(&platform),
            minimize_hidden: crate::display::minimize_when_hidden(),
            off_taskbar: crate::display::off_taskbar(&platform),
            taskbar_setting: crate::display::taskbar_setting(&platform),
            layer_setting: crate::display::layer_setting(&platform),
            auto_hide_setting: daevalog::platform::window_detector::SEES_FOREGROUND,
            layer_menu_keys: crate::display::layer_menu_takes_keys(),
            layer_popups_lag: crate::display::layer_popups_lag(),
            // Where it was last dropped, else the starting place; kept on
            // screen once the window knows its size (`keep_layer_on_screen`).
            layer_margins: {
                let (x, y) = crate::settings_store::store().values().layer_position.unwrap_or(LAYER_START);
                QMargins::new(x, y, 0, 0)
            },
            screenshot_path: QString::from(options.screenshot.as_deref().unwrap_or_default()),
            start_view: QString::from(options.open.as_deref().unwrap_or_default()),
            start_width: options.width.unwrap_or(0),
            drag: None,
        }
    }
}

/// A size in whole pixels, rounded up.
fn pixels(size: &QSizeF) -> (i32, i32) {
    (size.width().ceil() as i32, size.height().ceil() as i32)
}

impl qobject::Overlay {
    unsafe fn watch_window(&self, window: *mut qobject::QQuickWindow) {
        // The pointer comes from QML and points at a live window.
        unsafe { qobject::watch_window(window) }
    }

    unsafe fn keep_off_taskbar(&self, window: *mut qobject::QQuickWindow) {
        // As in `watch_window`.
        unsafe { qobject::keep_off_taskbar(window) }
    }

    unsafe fn set_off_taskbar(&self, window: *mut qobject::QQuickWindow, off: bool) {
        // As in `watch_window`.
        unsafe { qobject::set_off_taskbar(window, off) }
    }

    fn game_in_front(&self) -> bool {
        daevalog::platform::window_detector::is_aion2_foreground()
    }

    unsafe fn shape_input(&self, window: *mut qobject::QQuickWindow, area: QRectF) {
        // As in `watch_window`.
        unsafe { qobject::set_input_area(window, &area) }
    }

    unsafe fn place_popup(&self, window: *mut qobject::QQuickWindow, anchor: QRectF, below: bool, slide: bool, flip: bool) {
        // xdg_positioner's constraint_adjustment: slide_x 1, slide_y 2, flip_y 8.
        let adjust = if slide { 1 | 2 } else { 0 } | if flip { 8 } else { 0 };
        // As in `watch_window`.
        unsafe { qobject::set_popup_place(window, &anchor, below, adjust) }
    }

    fn begin_layer_drag(mut self: Pin<&mut Self>, grab: QPointF, size: QSizeF, screen: QSizeF) {
        let margins = self.layer_margins();
        let start = (margins.left(), margins.top());
        let origin = if crate::display::pointer_from_socket() {
            crate::hyprland::layer_origin(crate::hyprland::METER_NAMESPACE)
        } else {
            None
        };
        let from_press = origin.is_some() || crate::display::pointer_from_press();
        let moves = LayerDrag::new(start, (grab.x(), grab.y()), pixels(&size), pixels(&screen), from_press);
        let event_clock =
            layer_drag::event_clock_is_monotonic(qobject::pointer_event_time(), qobject::monotonic_micros());
        // Frames before the drag carried nothing of it.
        qobject::take_frame_times();
        self.as_mut().rust_mut().drag = Some(Drag { moves, event_clock, origin });
    }

    unsafe fn layer_drag_to(mut self: Pin<&mut Self>, window: *mut qobject::QQuickWindow, pointer: QPointF) {
        // A drag with no button down missed its release.
        if !qobject::pointer_button_down() {
            self.as_mut().end_layer_drag();
            return;
        }
        let Some(drag) = self.as_mut().rust_mut().get_mut().drag.as_mut() else { return };
        // Taken here, not from a queued signal: the frames must be known
        // before this event is measured.
        for at in qobject::take_frame_times() {
            drag.moves.frame(at);
        }
        let time = if drag.event_clock {
            qobject::pointer_event_time()
        } else {
            qobject::monotonic_micros() / 1000
        };
        let pointer = match drag.origin {
            Some(origin) => {
                let Some(at) = crate::hyprland::cursor() else { return };
                (f64::from(at.0 - origin.0), f64::from(at.1 - origin.1))
            }
            None => (pointer.x(), pointer.y()),
        };
        let Some(to) = drag.moves.pointer(pointer, time as u32) else { return };
        self.as_mut().place_layer(to);
        if let Some(drag) = self.as_mut().rust_mut().get_mut().drag.as_mut() {
            drag.moves.sent(to, qobject::monotonic_micros());
        }
        // The margins go out with the next frame, and only then.
        unsafe { qobject::request_frame(window) }
    }

    fn end_layer_drag(mut self: Pin<&mut Self>) {
        let dragged = self.as_mut().rust_mut().drag.take().is_some();
        // Remembered for the next start, as 1.x's window kept its place;
        // also when the release was missed (`layer_drag_to`).
        if dragged {
            let margins = self.layer_margins();
            let place = format!("{},{}", margins.left().max(0), margins.top().max(0));
            crate::settings_store::store().set(crate::settings_store::Key::LayerPosition, &place);
        }
    }

    fn keep_layer_on_screen(self: Pin<&mut Self>, size: QSizeF, screen: QSizeF) {
        let margins = self.layer_margins();
        let pos = layer_drag::clamp((margins.left(), margins.top()), pixels(&size), pixels(&screen));
        self.place_layer(pos);
    }

    fn place_layer(self: Pin<&mut Self>, (left, top): Pos) {
        self.set_layer_margins(QMargins::new(left, top, 0, 0));
    }

    fn pointer_on_screen(&self) -> QPointF {
        qobject::pointer_global_position()
    }

    unsafe fn release_pointer(&self, window: *mut qobject::QQuickWindow) {
        // As in `watch_window`.
        unsafe { qobject::release_pointer(window) }
    }

    unsafe fn capture(&self, window: *mut qobject::QQuickWindow, beside: *mut qobject::QQuickWindow, to_file: bool, background: QColor) -> QString {
        // As in `watch_window`.
        let png = unsafe { qobject::capture_windows(window, beside, true, &background) };
        let clipboard = !png.is_empty();
        let file = (to_file && clipboard)
            .then(|| crate::settings_store::store().values().screenshot_folder())
            .flatten()
            .and_then(|dir| daevalog::platform::screenshot::write_new_png(&dir.join(screenshot_name(chrono::Local::now())), &png));
        let file = file.map(|path| path.display().to_string()).unwrap_or_default();
        QString::from(&serde_json::json!({ "clipboard": clipboard, "file": file }).to_string())
    }

    unsafe fn save_screenshot(&self, window: *mut qobject::QQuickWindow) -> bool {
        let path = self.screenshot_path();
        if path.is_empty() || window.is_null() {
            return false;
        }
        // As in `watch_window`.
        unsafe { qobject::grab_window_to_file(window, path) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn a_screenshot_is_named_as_1x_names_it() {
        let at = chrono::Local.with_ymd_and_hms(2026, 10, 8, 3, 15, 0).single().unwrap();
        assert_eq!(screenshot_name(at), "AION2_DPS_20261008_031500.png");
    }
}
