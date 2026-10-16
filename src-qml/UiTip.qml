// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import Daevalog

// A note shown under an item once the pointer rests on it, drawn above the
// rest of the window; it goes as soon as the pointer leaves. Put it inside
// the item it explains. The meter's notes wait longer (Theme.meterTipRest):
// the pointer crosses the meter on its way to other windows.
Item {
    id: tip

    property string text
    property alias delay: rest.delay

    anchors.fill: parent

    PointerRest {
        id: rest
        anchors.fill: parent
        delay: Theme.tipRest
        active: tip.text !== ""
        onRestedChanged: {
            if (rested)
                card.place()
        }
    }

    Rectangle {
        id: card
        parent: tip.Window.contentItem
        visible: rest.rested && placed
        property bool placed: false
        z: 1000
        width: note.width + 16
        height: note.implicitHeight + 10
        radius: 4
        color: Theme.window
        border.width: 1
        border.color: Theme.fieldEdge

        // Under the item, else over it; in a window too short for either
        // (the meter), as low as it fits.
        function place() {
            if (!parent)
                return
            const at = tip.mapToItem(parent, 0, tip.height + 4)
            const above = tip.mapToItem(parent, 0, 0).y - height - 4
            x = Math.max(4, Math.min(at.x, parent.width - width - 4))
            if (at.y + height <= parent.height - 4)
                y = at.y
            else if (above >= 4)
                y = above
            else
                y = Math.max(4, parent.height - height - 4)
            placed = true
        }

        UiText {
            id: note
            x: 8
            y: 5
            // No wider than the window: the meter can be 260 px.
            width: Math.min(300, implicitWidth, Math.max(120, (card.parent ? card.parent.width : 324) - 24))
            text: tip.text
            wrapMode: Text.WordWrap
            maximumLineCount: 8
            // The rows' size: at 11 px the text blurred.
            font.pixelSize: Theme.sizeRow
        }
    }
}
