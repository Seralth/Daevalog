// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick

// Shows a layer-shell window again after the compositor closed it. A
// compositor closes a layer surface when its output goes away (a switch to
// a text console, a monitor unplugged), and the layer shell then closes the
// window; nothing brings it back on its own. While the window is closed but
// `wanted`, this shows it again when the screens change and every `retry`
// ms, once a screen with a size is there. Windows closing while the
// application quits stay closed.
QtObject {
    id: reopen

    property Window window: null
    // Whether the window should be showing now: its own `visible` binding's
    // value, which a close from outside does not change.
    property bool wanted: true
    property int retry: 2000
    // The application object whose screens and quit this follows.
    property QtObject application: Qt.application
    // Whether a screen to show on is there. With no output left, Qt shows
    // windows on a placeholder screen that has no size.
    property var screenReady: () => {
        const screens = reopen.application.screens
        for (let i = 0; i < screens.length; i++) {
            if (screens[i].width > 0 && screens[i].height > 0)
                return true
        }
        return false
    }
    property bool quitting: false
    readonly property bool closed: window !== null && wanted && !window.visible && !quitting

    // show() goes through the window's setter, so its own `visible` binding
    // stays in place for the hides the meter asks for later.
    function attempt() {
        if (closed && screenReady())
            window.show()
    }

    readonly property Timer timer: Timer {
        interval: reopen.retry
        repeat: true
        running: reopen.closed
        onTriggered: reopen.attempt()
    }

    readonly property Connections follow: Connections {
        target: reopen.application
        function onScreensChanged() {
            reopen.attempt()
        }
        function onAboutToQuit() {
            reopen.quitting = true
        }
    }
}
