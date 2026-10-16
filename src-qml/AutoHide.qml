// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick

// Auto-hide, as the 1.x backend does it (app/tasks.rs): while on, every
// tick after the first 20 asks whether the game or the meter is in front.
// After three ticks with neither, `hidden` turns on; the next tick with
// either turns it off. The owner hides and shows the meter by it.
QtObject {
    id: autoHide

    property bool enabled: false
    // Asked each tick: whether the game is the window in front.
    property var gameInFront: () => true
    // Whether the meter itself has focus.
    property bool meterActive: false
    property int tick: 500

    property bool hidden: false
    property int ticks: 0
    property int away: 0

    function step() {
        ticks++
        if (ticks <= 20)
            return
        if (gameInFront() || meterActive) {
            away = 0
            hidden = false
        } else if (++away >= 3) {
            hidden = true
        }
    }

    onEnabledChanged: {
        ticks = 0
        away = 0
        hidden = false
    }

    readonly property Timer timer: Timer {
        interval: autoHide.tick
        repeat: true
        running: autoHide.enabled
        onTriggered: autoHide.step()
    }
}
