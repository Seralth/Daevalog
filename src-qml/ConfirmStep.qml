// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick

// An action that asks once more: the first press arms it, a second press
// within `wait` ms does it. Waiting, or `cancel`, disarms it.
QtObject {
    id: step

    property int wait: 4000
    property bool armed: false
    signal confirmed()

    function press() {
        if (armed) {
            armed = false
            confirmed()
        } else {
            armed = true
        }
    }

    function cancel() {
        armed = false
    }

    readonly property Timer timer: Timer {
        interval: step.wait
        running: step.armed
        onTriggered: step.armed = false
    }
}
