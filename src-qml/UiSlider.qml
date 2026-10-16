// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import Daevalog

// A value in a range, with the value written at its end. `moved` asks for
// a new value; the owner sets it.
Item {
    id: slider

    property real from: 0
    property real to: 100
    property real value: 0
    property real stepSize: 1
    // The value as shown: "60%", "13 px".
    property string valueText: Math.round(value) + "%"
    signal moved(real value)

    implicitWidth: 260
    implicitHeight: 20

    readonly property real ratio: Math.max(0, Math.min(1, (value - from) / (to - from)))

    Item {
        id: track
        width: slider.width - 56
        height: parent.height

        Rectangle {
            anchors.verticalCenter: parent.verticalCenter
            width: parent.width
            height: 4
            radius: 2
            color: Theme.hover
            Rectangle {
                width: parent.width * slider.ratio
                height: parent.height
                radius: 2
                color: Theme.accent
            }
        }
        Rectangle {
            anchors.verticalCenter: parent.verticalCenter
            x: track.width * slider.ratio - width / 2
            width: 14
            height: 14
            radius: 7
            color: Theme.accent
        }
        MouseArea {
            anchors.fill: parent
            anchors.margins: -4
            cursorShape: Qt.PointingHandCursor
            function pick(x) {
                const raw = slider.from + Math.max(0, Math.min(1, (x - 4) / track.width)) * (slider.to - slider.from)
                slider.moved(Math.round(raw / slider.stepSize) * slider.stepSize)
            }
            onPressed: mouse => pick(mouse.x)
            onPositionChanged: mouse => pick(mouse.x)
        }
    }
    UiText {
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        width: 44
        horizontalAlignment: Text.AlignRight
        numeric: true
        text: slider.valueText
    }
}
