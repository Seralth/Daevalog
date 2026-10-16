// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick

// Whether the pointer has rested on the item for `delay` ms. Every move
// starts the wait again, so a pointer on its way past (to a button) never
// rests; leaving ends it at once. Once rested, moves keep it so.
Item {
    id: rest

    property int delay: 700
    // Off: nothing rests (a tooltip with no text).
    property bool active: true
    readonly property bool hovered: hover.hovered
    readonly property bool rested: done && hover.hovered && active
    property bool done: false

    HoverHandler {
        id: hover
        onHoveredChanged: {
            rest.done = false
            if (hovered && rest.active)
                wait.restart()
            else
                wait.stop()
        }
        onPointChanged: {
            if (hovered && rest.active && !rest.done)
                wait.restart()
        }
    }
    Timer {
        id: wait
        interval: rest.delay
        onTriggered: rest.done = true
    }
}
