// SPDX-License-Identifier: AGPL-3.0-only

//! `Menus`, the QML singleton that places and opens the app's own menus
//! (UiMenu.qml): the ⋯ menu, the mode list and every dropdown. They are
//! drawn in the app's theme, so no desktop theme changes them, and each is a
//! popup window of its own, so it can reach past the small meter window.

use cxx_qt_lib::{QPointF, QRectF, QSizeF};

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qpointf.h");
        type QPointF = cxx_qt_lib::QPointF;

        include!("cxx-qt-lib/qsizef.h");
        type QSizeF = cxx_qt_lib::QSizeF;

        include!("cxx-qt-lib/qrectf.h");
        type QRectF = cxx_qt_lib::QRectF;

        include!(<QtQuick/QQuickWindow>);
        type QQuickWindow;

        include!("daevalog-qml/cpp/menus.h");

        #[cxx_name = "mapRectToGlobal"]
        unsafe fn map_rect_to_global(window: *mut QQuickWindow, local: &QRectF) -> QRectF;

        #[cxx_name = "screenAreaAt"]
        unsafe fn screen_area_at(window: *mut QQuickWindow, point: &QPointF) -> QRectF;

        #[cxx_name = "setPopupAnchor"]
        unsafe fn set_popup_anchor(menu: *mut QQuickWindow, anchor: &QRectF, align_end: bool);

        #[cxx_name = "grabMenu"]
        unsafe fn grab_menu(menu: *mut QQuickWindow);
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        type Menus = super::MenusRust;

        /// Where the `size` menu opens for `anchor`, the control that opened
        /// it, in `window`'s coordinates: the menu's top-left on the screen.
        /// It opens below the anchor, its left edge on the anchor's left
        /// edge, or its right edge on the anchor's right edge when
        /// `align_end`.
        #[qinvokable]
        unsafe fn place(
            self: &Menus,
            menu: *mut QQuickWindow,
            window: *mut QQuickWindow,
            anchor: QRectF,
            size: QSizeF,
            align_end: bool,
        ) -> QPointF;

        /// Once the menu is shown: it takes the keys, and a press outside
        /// closes it (see menus.h).
        #[qinvokable]
        unsafe fn grab(self: &Menus, menu: *mut QQuickWindow);
    }
}

#[derive(Default)]
pub struct MenusRust;

/// A rectangle on the screen: left, top, width, height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Area {
    fn right(&self) -> f64 {
        self.x + self.width
    }

    fn bottom(&self) -> f64 {
        self.y + self.height
    }
}

impl From<&QRectF> for Area {
    fn from(rect: &QRectF) -> Self {
        Self { x: rect.x(), y: rect.y(), width: rect.width(), height: rect.height() }
    }
}

/// Where a `size` menu opens under `anchor` on a screen whose free part is
/// `screen`, where the app places it itself (X11): below the anchor, or
/// above it when it fits there and not below (or has more room above); its
/// left edge on the anchor's, or its right edge on the anchor's when
/// `align_end`; moved sideways and, when taller than the room, down to stay
/// on the screen. An empty screen means unknown: then it goes below.
pub fn place(anchor: Area, size: (f64, f64), screen: Area, align_end: bool) -> (f64, f64) {
    let (width, height) = size;
    let x = if align_end { anchor.right() - width } else { anchor.x };
    let below = anchor.bottom();
    if screen.width <= 0.0 || screen.height <= 0.0 {
        return (x, below);
    }
    let x = x.min(screen.right() - width).max(screen.x);
    let room_below = screen.bottom() - below;
    let room_above = anchor.y - screen.y;
    let y = if height <= room_below || room_below >= room_above {
        below.min(screen.bottom() - height)
    } else {
        anchor.y - height
    };
    (x, y.max(screen.y))
}

impl qobject::Menus {
    unsafe fn place(
        &self,
        menu: *mut qobject::QQuickWindow,
        window: *mut qobject::QQuickWindow,
        anchor: QRectF,
        size: QSizeF,
        align_end: bool,
    ) -> QPointF {
        // The pointers come from QML and point at live windows.
        let global = Area::from(&unsafe { qobject::map_rect_to_global(window, &anchor) });
        let size = (size.width(), size.height());
        if crate::overlay::qobject::platform_name().to_string().starts_with("wayland") {
            // The compositor places it; Qt before 6.6 asks for this spot.
            unsafe { qobject::set_popup_anchor(menu, &anchor, align_end) };
            let (x, y) = place(global, size, Area { x: 0.0, y: 0.0, width: 0.0, height: 0.0 }, align_end);
            return QPointF::new(x, y);
        }
        let middle = QPointF::new(global.x + global.width / 2.0, global.y + global.height / 2.0);
        let screen = Area::from(&unsafe { qobject::screen_area_at(window, &middle) });
        let (x, y) = place(global, size, screen, align_end);
        QPointF::new(x.round(), y.round())
    }

    unsafe fn grab(&self, menu: *mut qobject::QQuickWindow) {
        // As in `place`.
        unsafe { qobject::grab_menu(menu) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Area = Area { x: 0.0, y: 0.0, width: 1600.0, height: 900.0 };

    fn at(x: f64, y: f64, width: f64, height: f64) -> Area {
        Area { x, y, width, height }
    }

    #[test]
    fn below_the_anchor_on_its_left_or_right_edge() {
        let button = at(300.0, 100.0, 16.0, 16.0);
        assert_eq!(place(button, (180.0, 250.0), SCREEN, false), (300.0, 116.0));
        assert_eq!(place(button, (180.0, 250.0), SCREEN, true), (136.0, 116.0));
    }

    #[test]
    fn past_the_window_it_came_from() {
        // The footer's mode button of a short meter at 40,160: the list goes
        // on below the meter's bottom edge.
        let mode = at(120.0, 320.0, 50.0, 20.0);
        assert_eq!(place(mode, (160.0, 140.0), SCREEN, false), (120.0, 340.0));
    }

    #[test]
    fn above_when_it_does_not_fit_below() {
        let mode = at(120.0, 860.0, 50.0, 20.0);
        assert_eq!(place(mode, (160.0, 140.0), SCREEN, false), (120.0, 720.0));
    }

    #[test]
    fn the_side_with_more_room_when_it_fits_neither() {
        // 300 px fit neither above (200) nor below (180): below has less
        // room, so above, held on the screen.
        let field = at(10.0, 200.0, 220.0, 520.0);
        assert_eq!(place(field, (220.0, 300.0), SCREEN, false), (10.0, 0.0));
        let field = at(10.0, 100.0, 220.0, 500.0);
        assert_eq!(place(field, (220.0, 350.0), SCREEN, false), (10.0, 550.0));
    }

    #[test]
    fn sideways_onto_the_screen() {
        let right = at(1560.0, 10.0, 30.0, 20.0);
        assert_eq!(place(right, (200.0, 100.0), SCREEN, false), (1400.0, 30.0));
        let left = at(5.0, 10.0, 16.0, 16.0);
        assert_eq!(place(left, (200.0, 100.0), SCREEN, true), (0.0, 26.0));
    }

    #[test]
    fn on_a_second_screen_and_under_a_panel() {
        let screen = at(1600.0, 32.0, 1920.0, 1048.0);
        let button = at(1700.0, 40.0, 16.0, 16.0);
        assert_eq!(place(button, (180.0, 250.0), screen, true), (1600.0, 56.0));
        let low = at(2000.0, 1000.0, 60.0, 20.0);
        assert_eq!(place(low, (180.0, 250.0), screen, false), (2000.0, 750.0));
    }

    /// Every menu but the tray's is a UiMenu: the desktop's own menus
    /// (Qt.labs.platform) take the desktop's theme.
    #[test]
    fn only_the_tray_menu_is_the_desktops() {
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut checked = 0;
        for entry in std::fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "qml") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            checked += 1;
            let menus: Vec<_> = text.match_indices("Platform.Menu").map(|(at, _)| at).collect();
            if name != "Main.qml" {
                assert!(menus.is_empty(), "{name} uses the desktop's menu");
                continue;
            }
            let tray = text.find("Platform.SystemTrayIcon {").expect("Main.qml has the tray");
            assert!(menus.iter().all(|&at| at > tray), "a desktop menu outside the tray in Main.qml");
        }
        assert!(checked > 20, "the QML files are next to Cargo.toml");
    }

    #[test]
    fn unknown_screen_goes_below() {
        let empty = at(0.0, 0.0, 0.0, 0.0);
        let button = at(300.0, 880.0, 16.0, 16.0);
        assert_eq!(place(button, (180.0, 250.0), empty, true), (136.0, 896.0));
    }
}
