// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import QtTest
import ".."

// A window closed from outside shows again after the retry, once a screen
// with a size is there; its own visible binding still hides it afterwards;
// a window that should hide, or one closed after the quit began, stays
// closed.
Item {
    id: root
    property bool screen: true
    property bool shown: true
    // As on Wayland, where the meter hides in place and stays visible.
    property bool hidesInPlace: true

    QtObject {
        id: app
        // Its change signal stands in for a screen coming or going.
        property var screens: []
        signal aboutToQuit()
    }

    Window {
        id: win
        width: 100
        height: 100
        visible: root.shown || root.hidesInPlace
    }

    Reopen {
        id: reopen
        window: win
        wanted: root.shown || root.hidesInPlace
        retry: 50
        application: app
        screenReady: () => root.screen
    }

    TestCase {
        name: "Reopen"

        function init() {
            reopen.quitting = false
            reopen.retry = 50
            root.screen = true
            root.shown = true
            root.hidesInPlace = true
            win.show()
            tryCompare(win, "visible", true)
        }

        function test_a_window_closed_from_outside_shows_again() {
            win.close()
            compare(win.visible, false)
            tryCompare(win, "visible", true, 2000)
        }

        function test_it_waits_for_a_screen_with_a_size() {
            root.screen = false
            win.close()
            wait(300)
            compare(win.visible, false, "only a placeholder screen")
            root.screen = true
            tryCompare(win, "visible", true, 2000)
        }

        function test_a_new_screen_shows_it_at_once() {
            reopen.retry = 60000
            root.screen = false
            win.close()
            wait(100)
            root.screen = true
            app.screensChanged()
            compare(win.visible, true)
        }

        function test_its_own_binding_still_hides_it() {
            root.hidesInPlace = false
            win.close()
            tryCompare(win, "visible", true, 2000)
            root.shown = false
            compare(win.visible, false)
            wait(300)
            compare(win.visible, false, "a hide the meter asked for stays")
        }

        function test_a_window_that_should_hide_stays_closed() {
            root.hidesInPlace = false
            root.shown = false
            compare(win.visible, false)
            wait(300)
            compare(win.visible, false)
        }

        function test_windows_closing_after_the_quit_began_stay_closed() {
            app.aboutToQuit()
            win.close()
            wait(300)
            compare(win.visible, false)
        }
    }
}
