// SPDX-License-Identifier: AGPL-3.0-only

//! `Windows`, the QML singleton that remembers where Details, History and
//! Settings were and how big, as 1.x's `window.<name>.*` keys did (the layer
//! meter keeps its own place, `dpsMeter.layerPosition`). A saved place comes
//! back on a screen that is there now: a window left on a monitor that is
//! gone, or bigger than the screen, or smaller than the window allows, is
//! moved and sized to fit.
//!
//! Wayland lets no window read or pick its place, so there only the size is
//! kept; a place saved on X11 stays for the next X11 start.

use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        type Windows = super::WindowsRust;

        /// The saved place of window `name` fitted to `screens` (JSON
        /// [[x, y, width, height]], the first the primary) and the window's
        /// least size: JSON {"x", "y", "width", "height"}, x and y null when
        /// no place was saved; empty when nothing was saved.
        #[qinvokable]
        fn place(self: &Windows, name: &QString, screens: &QString, min_width: i32, min_height: i32) -> QString;

        /// Keeps window `name`'s place and size; with `size_only` (Wayland)
        /// its size, and the place saved before.
        #[qinvokable]
        fn keep(self: &Windows, name: &QString, x: i32, y: i32, width: i32, height: i32, size_only: bool);
    }
}

#[derive(Default)]
pub struct WindowsRust;

/// Room above a window's content for the desktop's title bar: a window
/// placed at the screen's top edge keeps its bar on the screen.
const TITLE_BAR: i32 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    fn overlap(&self, other: &Rect) -> i64 {
        let w = (self.x + self.width).min(other.x + other.width) - self.x.max(other.x);
        let h = (self.y + self.height).min(other.y + other.height) - self.y.max(other.y);
        if w > 0 && h > 0 { i64::from(w) * i64::from(h) } else { 0 }
    }
}

/// A saved window: its place, when one was known, and its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Saved {
    pub place: Option<(i32, i32)>,
    pub width: i32,
    pub height: i32,
}

/// "x,y,width,height"; x and y empty when only the size is known.
fn parse(raw: &str) -> Option<Saved> {
    let parts: Vec<&str> = raw.split(',').map(str::trim).collect();
    let [x, y, w, h] = parts.as_slice() else { return None };
    let (width, height) = (w.parse().ok()?, h.parse().ok()?);
    let place = match (x.parse(), y.parse()) {
        (Ok(x), Ok(y)) => Some((x, y)),
        _ => None,
    };
    Some(Saved { place, width, height })
}

fn write(saved: &Saved) -> String {
    match saved.place {
        Some((x, y)) => format!("{x},{y},{},{}", saved.width, saved.height),
        None => format!(",,{},{}", saved.width, saved.height),
    }
}

/// `saved` fitted to the screens: on the screen that holds most of it, else
/// centred on the first (the primary: its monitor is gone); no bigger than
/// that screen less the title bar, no smaller than `min` unless the screen
/// is. Without a saved place, only the size is fitted, on the first screen.
pub fn fit(saved: Saved, screens: &[Rect], min: (i32, i32)) -> Saved {
    let Some(first) = screens.first() else { return saved };
    let rect = saved.place.map(|(x, y)| Rect { x, y, width: saved.width, height: saved.height });
    let screen = rect
        .and_then(|r| screens.iter().filter(|s| s.overlap(&r) > 0).max_by_key(|s| s.overlap(&r)))
        .unwrap_or(first);
    let room = (screen.width.max(1), (screen.height - TITLE_BAR).max(1));
    let width = saved.width.max(min.0).min(room.0);
    let height = saved.height.max(min.1).min(room.1);
    let place = match rect {
        Some(r) if screen.overlap(&r) > 0 => (
            r.x.clamp(screen.x, screen.x + screen.width - width),
            r.y.clamp(screen.y + TITLE_BAR, screen.y + TITLE_BAR + room.1 - height),
        ),
        Some(_) => (screen.x + (screen.width - width) / 2, screen.y + TITLE_BAR + (room.1 - height) / 2),
        None => return Saved { place: None, width, height },
    };
    Saved { place: Some(place), width, height }
}

fn key(name: &str) -> String {
    format!("dpsMeter.window.{name}")
}

impl qobject::Windows {
    fn place(&self, name: &QString, screens: &QString, min_width: i32, min_height: i32) -> QString {
        let store = crate::settings_store::store();
        let Some(saved) = store.raw(&key(&name.to_string())).as_deref().and_then(parse) else {
            return QString::default();
        };
        let screens: Vec<Rect> = serde_json::from_str::<Vec<[i32; 4]>>(&screens.to_string())
            .unwrap_or_default()
            .into_iter()
            .map(|[x, y, width, height]| Rect { x, y, width, height })
            .filter(|s| s.width > 0 && s.height > 0)
            .collect();
        let fitted = fit(saved, &screens, (min_width, min_height));
        let json = serde_json::json!({
            "x": fitted.place.map(|p| p.0),
            "y": fitted.place.map(|p| p.1),
            "width": fitted.width,
            "height": fitted.height,
        });
        QString::from(&json.to_string())
    }

    fn keep(&self, name: &QString, x: i32, y: i32, width: i32, height: i32, size_only: bool) {
        if width <= 0 || height <= 0 {
            return;
        }
        let store = crate::settings_store::store();
        let key = key(&name.to_string());
        let place = if size_only { store.raw(&key).as_deref().and_then(parse).and_then(|s| s.place) } else { Some((x, y)) };
        let text = write(&Saved { place, width, height });
        if store.raw(&key).as_deref() != Some(text.as_str()) {
            store.set_raw(&key, &text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: Rect = Rect { x: 0, y: 0, width: 2560, height: 1440 };
    const RIGHT: Rect = Rect { x: 2560, y: 0, width: 1920, height: 1080 };
    const MIN: (i32, i32) = (760, 480);

    fn at(x: i32, y: i32, width: i32, height: i32) -> Saved {
        Saved { place: Some((x, y)), width, height }
    }

    #[test]
    fn a_place_on_a_screen_comes_back_as_it_was() {
        assert_eq!(fit(at(2700, 100, 1000, 760), &[LEFT, RIGHT], MIN), at(2700, 100, 1000, 760));
        assert_eq!(fit(at(100, 200, 1000, 760), &[LEFT, RIGHT], MIN), at(100, 200, 1000, 760));
    }

    #[test]
    fn a_window_on_a_monitor_that_is_gone_comes_to_the_primary() {
        let fitted = fit(at(3000, 300, 1000, 760), &[LEFT], MIN);
        assert_eq!(fitted, at(780, 32 + (1408 - 760) / 2, 1000, 760), "centred on the screen that is left");
    }

    #[test]
    fn a_window_half_off_the_edge_moves_on() {
        assert_eq!(fit(at(2200, 1300, 1000, 760), &[LEFT], MIN), at(1560, 680, 1000, 760));
        assert_eq!(fit(at(-400, -50, 1000, 760), &[LEFT], MIN), at(0, 32, 1000, 760), "the title bar stays on screen");
    }

    #[test]
    fn a_tiny_size_grows_to_the_least_the_window_allows() {
        assert_eq!(fit(at(100, 100, 12, 8), &[LEFT], MIN), at(100, 100, 760, 480));
    }

    #[test]
    fn a_size_bigger_than_the_screen_shrinks_to_it() {
        assert_eq!(fit(at(0, 0, 5000, 3000), &[RIGHT], MIN), at(2560, 32, 1920, 1048));
        let small = Rect { x: 0, y: 0, width: 640, height: 400 };
        assert_eq!(fit(at(0, 0, 1000, 760), &[small], MIN), at(0, 32, 640, 368), "a screen smaller than the least size wins");
    }

    #[test]
    fn a_size_without_a_place_is_fitted_alone() {
        let saved = Saved { place: None, width: 9000, height: 100 };
        assert_eq!(fit(saved, &[LEFT], MIN), Saved { place: None, width: 2560, height: 480 });
    }

    #[test]
    fn no_screens_leaves_it_as_saved() {
        assert_eq!(fit(at(5, 5, 10, 10), &[], MIN), at(5, 5, 10, 10));
    }

    #[test]
    fn saved_text_reads_back() {
        for saved in [at(-1920, 40, 1000, 760), Saved { place: None, width: 900, height: 600 }] {
            assert_eq!(parse(&write(&saved)), Some(saved));
        }
        assert_eq!(parse("1,2,3"), None);
        assert_eq!(parse("a,b,c,d"), None);
    }
}
