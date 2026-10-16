// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import Daevalog

// One of a few choices side by side; the chosen one in the accent. Framed
// for settings, bare for tabs.
Rectangle {
    id: group

    // [{ label, value }]
    property var options: []
    property var current
    property bool framed: true
    property bool small: false
    signal picked(var value)

    implicitWidth: choices.implicitWidth + (framed ? 2 : 0)
    // Disabled (`enabled: false`): faded, and it takes no click.
    opacity: enabled ? 1 : Theme.disabledOpacity
    implicitHeight: choices.implicitHeight + (framed ? 2 : 0)
    radius: 4
    color: "transparent"
    border.width: framed ? 1 : 0
    border.color: Theme.fieldEdge

    Row {
        id: choices
        anchors.centerIn: parent
        spacing: group.framed ? 0 : 2

        Repeater {
            model: group.options
            Rectangle {
                id: choice
                required property var modelData
                readonly property bool active: modelData.value === group.current
                implicitWidth: label.implicitWidth + (group.small ? 14 : 20)
                implicitHeight: group.small ? 22 : 26
                radius: 3
                color: active ? Theme.accentTint : (hover.hovered ? Theme.hover : "transparent")
                UiText {
                    id: label
                    anchors.centerIn: parent
                    text: choice.modelData.label
                    font.pixelSize: group.small ? Theme.sizeSmall : Theme.sizeRow
                    font.weight: choice.active ? Font.DemiBold : Font.Normal
                    color: choice.active ? Theme.accent : Theme.sub
                }
                HoverHandler {
                    id: hover
                    cursorShape: Qt.PointingHandCursor
                }
                TapHandler {
                    onTapped: group.picked(choice.modelData.value)
                }
            }
        }
    }
}
