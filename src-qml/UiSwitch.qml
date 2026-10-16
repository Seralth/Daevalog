// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import Daevalog

// On or off. It shows `checked` and asks for a change with `toggled`; the
// owner sets the setting, and the binding brings the new state back.
Item {
    id: toggle

    property bool checked: false
    signal toggled(bool on)

    implicitWidth: 30
    implicitHeight: 18
    // Disabled (`enabled: false`): faded, and it takes no click.
    opacity: enabled ? 1 : Theme.disabledOpacity

    Rectangle {
        anchors.fill: parent
        radius: height / 2
        color: toggle.checked ? Theme.accent : Theme.hover
        border.width: 1
        border.color: toggle.checked ? Theme.accent : Theme.fieldEdge
    }
    Rectangle {
        width: 12
        height: 12
        radius: 6
        y: 3
        x: toggle.checked ? toggle.width - width - 3 : 3
        color: toggle.checked ? Theme.onAccent : Theme.sub
    }

    HoverHandler {
        cursorShape: Qt.PointingHandCursor
    }
    TapHandler {
        onTapped: toggle.toggled(!toggle.checked)
    }
}
