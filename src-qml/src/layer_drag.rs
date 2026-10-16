// SPDX-License-Identifier: AGPL-3.0-only

//! Moving a layer-shell window with the pointer.
//!
//! A layer surface has no compositor move: the app places it by its margins
//! from the screen's top-left corner. Pointer events come in window
//! coordinates, measured from wherever the window was when the compositor
//! sent them, and a new place reaches the compositor only with the window's
//! next frame. So for a while after each move, events still come from the
//! old place.
//!
//! Each event is therefore measured from the place that was on screen at
//! its time stamp. That gives the pointer's spot on the screen, and the
//! window goes there, less the spot inside the window where the drag took
//! hold of it. Nothing adds up from one event to the next, so a late or
//! repeated event cannot carry the window away.
//!
//! Sway is different: while the button is held, it measures the pointer from
//! where the window was at the press, for the whole drag. There every event
//! is measured from the drag's start. Hyprland measures it from where its
//! layer animation has got to, which the app cannot know; there the pointer
//! comes from Hyprland's socket, also measured from the drag's start (see
//! hyprland.rs).

/// A window position in screen pixels: its left and top margins.
pub type Pos = (i32, i32);

/// Keeps a `size` window inside a `screen`-sized area. A zero screen size
/// means unknown: then only the top and left edges hold.
pub fn clamp(pos: Pos, size: (i32, i32), screen: (i32, i32)) -> Pos {
    let axis = |at: i32, length: i32, room: i32| {
        let at = if room > 0 { at.min(room - length) } else { at };
        at.max(0)
    };
    (axis(pos.0, size.0, screen.0), axis(pos.1, size.1, screen.1))
}

/// Whether an event stamped `time` happened at or after `since`, both in
/// milliseconds on the compositor's event clock, which wraps at 2^32.
fn at_or_after(time: u32, since: u32) -> bool {
    time.wrapping_sub(since) as i32 >= 0
}

/// Whether the compositor stamps events with the monotonic clock: the
/// event being delivered (ms) is within a second of `now` (µs). If not,
/// events are timed by when they arrive.
pub fn event_clock_is_monotonic(event_ms: u64, now_us: u64) -> bool {
    let now_ms = (now_us / 1000) as u32;
    (now_ms.wrapping_sub(event_ms as u32) as i32).unsigned_abs() < 1000
}

/// A place the window was shown at, from `since` (ms, event clock) on;
/// `None` for the place the drag started from.
struct Place {
    pos: Pos,
    since: Option<u32>,
}

pub struct LayerDrag {
    /// Where the pointer took hold of the window, in window coordinates.
    grab: (f64, f64),
    /// Where the window was at the press.
    start: Pos,
    /// Whether events are measured from `start` all drag long (Sway).
    from_press: bool,
    size: (i32, i32),
    screen: (i32, i32),
    /// Places shown, oldest first; never empty.
    shown: Vec<Place>,
    /// Places sent but not yet carried by a frame, with when they were sent
    /// (µs, monotonic).
    sent: Vec<(Pos, u64)>,
}

impl LayerDrag {
    /// `from_press`: the pointer is measured from where the window was at
    /// the press (see `display::pointer_from_press` and
    /// `display::pointer_from_socket`).
    pub fn new(start: Pos, grab: (f64, f64), size: (i32, i32), screen: (i32, i32), from_press: bool) -> Self {
        Self {
            grab,
            start,
            from_press,
            size,
            screen,
            shown: vec![Place { pos: start, since: None }],
            sent: Vec::new(),
        }
    }

    /// Where the window should go for the pointer at `pointer` (window
    /// coordinates) in an event stamped `time` (ms); `None` when it is
    /// there already or on its way.
    pub fn pointer(&mut self, pointer: (f64, f64), time: u32) -> Option<Pos> {
        let from = if self.from_press { self.start } else { self.place_at(time) };
        let to = clamp(
            (
                from.0.saturating_add((pointer.0 - self.grab.0).round() as i32),
                from.1.saturating_add((pointer.1 - self.grab.1).round() as i32),
            ),
            self.size,
            self.screen,
        );
        (to != self.latest()).then_some(to)
    }

    /// The window was asked to go to `pos` at `now` (µs).
    pub fn sent(&mut self, pos: Pos, now: u64) {
        self.sent.push((pos, now));
    }

    /// A frame went out at `at` (µs): the last place sent before it is on
    /// screen from then on.
    pub fn frame(&mut self, at: u64) {
        let Some(last) = self.sent.iter().rposition(|&(_, sent)| sent <= at) else { return };
        let pos = self.sent[last].0;
        self.sent.drain(..=last);
        self.shown.push(Place { pos, since: Some((at / 1000) as u32) });
    }

    /// The newest place asked for.
    fn latest(&self) -> Pos {
        match self.sent.last() {
            Some(&(pos, _)) => pos,
            None => self.shown[self.shown.len() - 1].pos,
        }
    }

    /// The place on screen at `time`. Events arrive in order, so older
    /// places are dropped.
    fn place_at(&mut self, time: u32) -> Pos {
        let current = self
            .shown
            .iter()
            .rposition(|place| place.since.is_none_or(|since| at_or_after(time, since)))
            .unwrap_or(0);
        self.shown.drain(..current);
        self.shown[0].pos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (i32, i32) = (1920, 1080);
    const SIZE: (i32, i32) = (360, 200);

    /// A drag from (40, 160), held 100 px from the window's left edge.
    fn drag() -> LayerDrag {
        LayerDrag::new((40, 160), (100.0, 15.0), SIZE, SCREEN, false)
    }

    #[test]
    fn events_before_a_frame_do_not_add_up() {
        let mut drag = drag();
        // The window has not moved yet, so the pointer is 10, 20, 30 px right
        // of where it took hold. The old code moved by the whole drag again
        // on each event: 50, 70, 100.
        assert_eq!(drag.pointer((110.0, 15.0), 10), Some((50, 160)));
        drag.sent((50, 160), 10_100);
        assert_eq!(drag.pointer((120.0, 15.0), 11), Some((60, 160)));
        drag.sent((60, 160), 11_100);
        assert_eq!(drag.pointer((130.0, 15.0), 12), Some((70, 160)));
        drag.sent((70, 160), 12_100);
        // The same event again changes nothing.
        assert_eq!(drag.pointer((130.0, 15.0), 12), None);
    }

    #[test]
    fn events_after_a_frame_are_measured_from_the_new_place() {
        let mut drag = drag();
        assert_eq!(drag.pointer((130.0, 15.0), 10), Some((70, 160)));
        drag.sent((70, 160), 10_500);
        drag.frame(11_000);
        // On screen at 70 since 11 ms: the pointer is at 70 + 105 = 175.
        assert_eq!(drag.pointer((105.0, 15.0), 12), Some((75, 160)));
        drag.sent((75, 160), 12_500);
        // A pointer that stays put while the window catches up asks for
        // nothing new.
        drag.frame(13_000);
        assert_eq!(drag.pointer((100.0, 15.0), 14), None);
    }

    #[test]
    fn a_late_event_is_measured_from_the_place_of_its_time() {
        let mut drag = drag();
        assert_eq!(drag.pointer((130.0, 15.0), 10), Some((70, 160)));
        drag.sent((70, 160), 10_500);
        drag.frame(11_000);
        // Stamped before the frame, so measured from 40: the pointer was at
        // 40 + 135 = 175, which puts the window at 75, not 105.
        assert_eq!(drag.pointer((135.0, 15.0), 10), Some((75, 160)));
    }

    #[test]
    fn a_frame_carries_only_what_was_sent_before_it() {
        let mut drag = drag();
        drag.sent((70, 160), 12_000);
        drag.frame(11_000);
        // Still measured from 40 after that frame.
        assert_eq!(drag.pointer((110.0, 15.0), 12), Some((50, 160)));
        drag.sent((50, 160), 12_500);
        // A later frame carries both sends; the newest one is on screen.
        drag.frame(13_000);
        assert_eq!(drag.pointer((100.0, 15.0), 14), None);
        assert_eq!(drag.pointer((110.0, 15.0), 14), Some((60, 160)));
    }

    #[test]
    fn the_window_stays_on_the_screen() {
        let mut drag = drag();
        assert_eq!(drag.pointer((5000.0, 4000.0), 10), Some((1560, 880)));
        assert_eq!(drag.pointer((-500.0, -900.0), 11), Some((0, 0)));
        assert_eq!(clamp((40, 160), (2000, 1200), SCREEN), (0, 0));
        assert_eq!(clamp((-5, 2000), SIZE, (0, 0)), (0, 2000));
        assert_eq!(clamp((1700, 50), SIZE, SCREEN), (1560, 50));
    }

    /// A drag traced on Sway: pressed at
    /// (110.08, 16.25), then the pointer's window coordinates rose 5 px per
    /// event, by its travel, while frames moved the window. The old rule sent
    /// left margins 50, 65, 85, 110 ... and ran off to the corner.
    #[test]
    fn sway_measures_from_the_press() {
        let mut drag = LayerDrag::new((40, 160), (110.078125, 16.24609375), SIZE, SCREEN, true);
        let mut lefts = Vec::new();
        for step in 1..=20 {
            let pointer = (110.078125 + 5.0 * f64::from(step), 16.24609375 + 5.0 * f64::from(step));
            // A frame between every two events carries the last place.
            drag.frame(u64::from(step as u32) * 60_000 - 30_000);
            if let Some(to) = drag.pointer(pointer, step as u32 * 60) {
                drag.sent(to, u64::from(step as u32) * 60_000 + 1_000);
                lefts.push(to.0);
                assert_eq!(to.1 - 160, to.0 - 40, "moves along the pointer's diagonal");
            }
        }
        assert_eq!(lefts.first(), Some(&45));
        assert_eq!(lefts.last(), Some(&140), "the pointer's 100 px, no more");
        assert!(lefts.windows(2).all(|pair| pair[1] - pair[0] == 5));
        // Back to the press spot: back to the start.
        assert_eq!(drag.pointer((110.078125, 16.24609375), 1300), Some((40, 160)));
    }

    #[test]
    fn time_stamps_wrap() {
        assert!(at_or_after(3, u32::MAX - 5));
        assert!(!at_or_after(u32::MAX - 5, 3));
        let mut drag = drag();
        drag.sent((70, 160), 1);
        drag.frame(u64::from(u32::MAX - 5) * 1000);
        assert_eq!(drag.pointer((100.0, 15.0), 3), None, "measured from 70 after the wrap");
    }

    #[test]
    fn event_clock_check() {
        assert!(event_clock_is_monotonic(2_170_983, 2_171_200_000));
        assert!(!event_clock_is_monotonic(0, 2_171_200_000));
        assert!(!event_clock_is_monotonic(2_170_983, 9_000_000_000));
    }
}
