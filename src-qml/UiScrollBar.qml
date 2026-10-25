// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import Daevalog

// A scroll bar for content larger than its view: sideways for a wide skill
// table or a long timeline, upright (`vertical`) for a page taller than its
// window. A thin track with a thumb as long as the view's share of the
// content. Drag the thumb, or click the track to move by one view. Flat, in
// the theme's line colours; shown only when there is something to scroll.
Item {
    id: bar

    // Upright, along the view's right edge, instead of sideways.
    property bool vertical: false
    // The content's offset at the view's start edge, 0 to `range`.
    property real position: 0
    // How far the content scrolls: its length less the view's.
    property real range: 0
    // The view's length, for the thumb's size.
    property real view: 1

    signal moved(real position)

    readonly property real length: vertical ? height : width
    readonly property real thumbLength: Math.max(32, length * view / Math.max(1, view + range))
    readonly property real travel: Math.max(1, length - thumbLength)
    readonly property real thumbAt: range > 0 ? position / range * travel : 0

    function moveTo(p) {
        bar.moved(Math.max(0, Math.min(bar.range, p)))
    }

    visible: range > 0
    implicitHeight: vertical ? 0 : 10
    implicitWidth: vertical ? 10 : 0

    Rectangle {
        anchors.centerIn: parent
        width: bar.vertical ? 6 : parent.width
        height: bar.vertical ? parent.height : 6
        radius: 3
        color: Theme.rowEmpty
    }
    Rectangle {
        x: bar.vertical ? (parent.width - width) / 2 : bar.thumbAt
        y: bar.vertical ? bar.thumbAt : (parent.height - height) / 2
        width: bar.vertical ? 6 : bar.thumbLength
        height: bar.vertical ? bar.thumbLength : 6
        radius: 3
        color: grip.pressed || grip.containsMouse ? Theme.sub : Theme.scrollThumb
    }
    MouseArea {
        id: grip
        anchors.fill: parent
        hoverEnabled: true
        // Where on the thumb it was taken; -1 for a click on the track.
        property real held: -1
        onPressed: mouse => {
            const at = bar.vertical ? mouse.y : mouse.x
            if (at >= bar.thumbAt && at <= bar.thumbAt + bar.thumbLength)
                held = at - bar.thumbAt
            else {
                held = -1
                bar.moveTo(bar.position + (at < bar.thumbAt ? -bar.view : bar.view))
            }
        }
        onPositionChanged: mouse => {
            if (pressed && held >= 0)
                bar.moveTo(((bar.vertical ? mouse.y : mouse.x) - held) / bar.travel * bar.range)
        }
        onReleased: held = -1
    }
}
