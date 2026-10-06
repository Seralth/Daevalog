// SPDX-License-Identifier: MIT

//! `Overlay`, the QML singleton for window state that is not drawing:
//! the click-through lock, layer shell, the layer-shell position and the
//! screenshot hook.

use core::pin::Pin;

use cxx_qt_lib::{QMargins, QString};

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("cxx-qt-lib/qmargins.h");
        type QMargins = cxx_qt_lib::QMargins;

        include!(<QtQuick/QQuickWindow>);
        type QQuickWindow;

        include!("daevalog-qml/cpp/shell.h");

        #[cxx_name = "grabWindowToFile"]
        unsafe fn grab_window_to_file(window: *mut QQuickWindow, path: &QString) -> bool;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(bool, locked)]
        #[qproperty(bool, layer_shell)]
        #[qproperty(QMargins, layer_margins)]
        #[qproperty(QString, screenshot_path, READ, CONSTANT)]
        type Overlay = super::OverlayRust;

        /// Moves a layer-shell window: its margins from the top-left corner.
        #[qinvokable]
        fn move_layer_by(self: Pin<&mut Overlay>, dx: i32, dy: i32);

        /// Saves the window to the --screenshot file; false if that failed.
        #[qinvokable]
        unsafe fn save_screenshot(self: &Overlay, window: *mut QQuickWindow) -> bool;
    }
}

/// Where a layer-shell meter starts, from the screen's top-left corner.
/// Normal windows are placed by the compositor instead.
const LAYER_START: (i32, i32) = (40, 160);

pub struct OverlayRust {
    locked: bool,
    layer_shell: bool,
    layer_margins: QMargins,
    screenshot_path: QString,
}

impl Default for OverlayRust {
    fn default() -> Self {
        let options = crate::options();
        Self {
            locked: options.locked,
            layer_shell: options.layer_shell,
            layer_margins: QMargins::new(LAYER_START.0, LAYER_START.1, 0, 0),
            screenshot_path: QString::from(options.screenshot.as_deref().unwrap_or_default()),
        }
    }
}

impl qobject::Overlay {
    fn move_layer_by(self: Pin<&mut Self>, dx: i32, dy: i32) {
        let (left, top) = moved(self.layer_margins(), dx, dy);
        self.set_layer_margins(QMargins::new(left, top, 0, 0));
    }

    unsafe fn save_screenshot(&self, window: *mut qobject::QQuickWindow) -> bool {
        let path = self.screenshot_path();
        if path.is_empty() || window.is_null() {
            return false;
        }
        // The pointer comes from QML and points at a live window.
        unsafe { qobject::grab_window_to_file(window, path) }
    }
}

/// The new top-left margins; the window never goes past the screen's
/// top or left edge.
fn moved(margins: &QMargins, dx: i32, dy: i32) -> (i32, i32) {
    (
        margins.left().saturating_add(dx).max(0),
        margins.top().saturating_add(dy).max(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_move_stops_at_the_top_left_edge() {
        let start = QMargins::new(40, 160, 0, 0);
        assert_eq!(moved(&start, 10, -20), (50, 140));
        assert_eq!(moved(&start, -100, -500), (0, 0));
    }
}
