// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import Daevalog

// A card shown under an item once the pointer rests on it, drawn above the
// rest of the window; it goes as soon as the pointer leaves. Put it inside
// the item it explains; its children are the card's content (UiTip's one
// text, or a short list of values).
Item {
    id: tip

    default property alias content: column.data
    // Off: nothing shows (nothing to list).
    property bool active: true
    property alias delay: rest.delay
    property real cardWidth: 280
    // Space between the card's edge and its content.
    property real padX: 10
    property real padY: 8
    // Shown without the pointer (a screenshot of it).
    property bool forced: false

    anchors.fill: parent

    // Placed again while forced: the page around it may still be laid out.
    Timer {
        interval: 200
        repeat: true
        running: tip.forced
        triggeredOnStart: true
        onTriggered: card.place()
    }

    PointerRest {
        id: rest
        anchors.fill: parent
        delay: Theme.tipRest
        active: tip.active
        onRestedChanged: {
            if (rested)
                card.place()
        }
    }

    Rectangle {
        id: card
        parent: tip.Window.contentItem
        visible: (rest.rested || tip.forced) && tip.active && placed
        property bool placed: false
        z: 1000
        width: tip.cardWidth + 2 * tip.padX
        height: column.implicitHeight + 2 * tip.padY
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

        Column {
            id: column
            x: tip.padX
            y: tip.padY
            width: tip.cardWidth
            spacing: 2
        }
    }
}
