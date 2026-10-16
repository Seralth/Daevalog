// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import Daevalog

// A text button. `primary` marks the one main action of a view with the
// accent; every other button stays quiet.
Rectangle {
    id: button

    property string text
    property bool primary: false
    property bool small: false
    signal clicked()

    implicitWidth: label.implicitWidth + (small ? 16 : 24)
    // Disabled (`enabled: false`, busy): faded, and it takes no click.
    opacity: enabled ? 1 : Theme.disabledOpacity
    implicitHeight: small ? 22 : 28
    radius: 4
    color: hover.hovered ? Theme.hover : "transparent"
    border.width: 1
    border.color: primary ? Theme.accent : Theme.fieldEdge

    UiText {
        id: label
        anchors.centerIn: parent
        text: button.text
        font.pixelSize: button.small ? Theme.sizeSmall : Theme.sizeRow
        font.weight: Font.Medium
        color: button.primary ? Theme.accent : Theme.name
    }

    HoverHandler {
        id: hover
        cursorShape: Qt.PointingHandCursor
    }
    TapHandler {
        onTapped: button.clicked()
    }
}
