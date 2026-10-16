// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import Daevalog

// A sideways scroll bar for content wider than its view (a wide skill
// table, a long timeline): a thin track with a thumb as wide as the view's
// share of the content. Drag the thumb, or click the track to move by one
// view. Flat, in the theme's line colours; shown only when there is
// something to scroll.
Item {
    id: bar

    // The content's x at the view's left edge, 0 to `range`.
    property real position: 0
    // How far the content scrolls: its width less the view's.
    property real range: 0
    // The view's width, for the thumb's size.
    property real view: 1

    signal moved(real position)

    readonly property real thumbWidth: Math.max(32, width * view / Math.max(1, view + range))
    readonly property real travel: Math.max(1, width - thumbWidth)

    function moveTo(p) {
        bar.moved(Math.max(0, Math.min(bar.range, p)))
    }

    visible: range > 0
    implicitHeight: 10

    Rectangle {
        anchors.verticalCenter: parent.verticalCenter
        width: parent.width
        height: 6
        radius: 3
        color: Theme.rowEmpty
    }
    Rectangle {
        x: bar.range > 0 ? bar.position / bar.range * bar.travel : 0
        anchors.verticalCenter: parent.verticalCenter
        width: bar.thumbWidth
        height: 6
        radius: 3
        color: grip.pressed || grip.containsMouse ? Theme.sub : Theme.faint
    }
    MouseArea {
        id: grip
        anchors.fill: parent
        hoverEnabled: true
        // Where on the thumb it was taken; -1 for a click on the track.
        property real held: -1
        onPressed: mouse => {
            const thumbX = bar.range > 0 ? bar.position / bar.range * bar.travel : 0
            if (mouse.x >= thumbX && mouse.x <= thumbX + bar.thumbWidth)
                held = mouse.x - thumbX
            else {
                held = -1
                bar.moveTo(bar.position + (mouse.x < thumbX ? -bar.view : bar.view))
            }
        }
        onPositionChanged: mouse => {
            if (pressed && held >= 0)
                bar.moveTo((mouse.x - held) / bar.travel * bar.range)
        }
        onReleased: held = -1
    }
}
