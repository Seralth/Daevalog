// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import Daevalog

// A menu drawn in the app's own theme: the ⋯ menu, the mode list and every
// dropdown's options look the same on every desktop, whatever its theme.
// A popup window of its own (Menus), so it can reach past the window that
// opened it. Up and Down, Home, End, Enter or Space and Escape work in it.
Window {
    id: menu

    // UiMenuItem and UiMenuSeparator, in order.
    default property list<QtObject> items
    // The entries shown: `items`, or plain objects made from data, such as
    // a dropdown's options ({ text, checkable, checked, icon }).
    property var entries: items
    // Opens under the right end of the control that opened it, not its
    // left: the ⋯ menu at the meter's right edge.
    property bool alignEnd: false
    // At least this wide: a dropdown's list is as wide as its field.
    property real minimumListWidth: 0
    // The entry the keys or the pointer are on; -1 none.
    property int current: -1
    // An entry was chosen; its own `triggered` has been sent too.
    signal activated(int index)

    readonly property bool hasChecks: {
        for (let i = 0; i < entries.length; ++i) {
            if (entries[i].checkable === true)
                return true
        }
        return false
    }
    readonly property bool hasIcons: {
        for (let i = 0; i < entries.length; ++i) {
            if (entries[i].icon !== undefined && String(entries[i].icon) !== "")
                return true
        }
        return false
    }
    readonly property int pad: 4
    readonly property int gap: 3
    readonly property int gutter: hasChecks ? 22 : 0
    readonly property int iconGutter: hasIcons ? 22 : 0
    readonly property real listWidth: {
        let widest = 0
        // An index loop: Qt 6.4 cannot iterate a list property.
        for (let i = 0; i < list.children.length; ++i) {
            const row = list.children[i]
            if (row.visible)
                widest = Math.max(widest, row.implicitWidth)
        }
        return widest
    }
    // Long lists scroll past 14 entries.
    readonly property int maximumListHeight: 14 * Theme.tableRow

    flags: Qt.Popup | Qt.FramelessWindowHint
    color: "transparent"
    visible: false
    width: Math.ceil(Math.max(minimumListWidth, listWidth, 120))
    height: Math.min(list.implicitHeight, maximumListHeight) + 2 * pad

    // Open, or about to open (see open).
    property bool wanted: false
    property Item anchorItem: null
    onVisibleChanged: {
        if (!visible)
            wanted = false
    }

    // Opens under `item`, or closes the menu if it is open.
    function open(item) {
        if (visible || wanted) {
            keysFirst.stop()
            visible = false
            wanted = false
            return
        }
        const window = item.Window.window
        anchorItem = item
        transientParent = window
        wanted = true
        // A layer meter on Sway asks for the keys (wanted) and the menu opens
        // once it has them: Sway keeps the keys where they were when a menu
        // took its grab.
        if (Overlay.layerMenuKeys && window.layerSurface === true && !window.active)
            keysFirst.start()
        else
            openNow()
    }

    function openNow() {
        const item = anchorItem
        if (!item)
            return
        current = -1
        flick.contentY = 0
        // A small gap from the control, below it or above.
        const at = Menus.place(menu, transientParent, item.mapToItem(null, 0, -gap, item.width, item.height + 2 * gap),
                               Qt.size(width, height), alignEnd)
        x = at.x
        y = at.y
        visible = true
        Menus.grab(menu)
    }

    // Opens the menu when the meter has the keys, or after a moment without.
    Timer {
        id: keysFirst
        interval: 250
        onTriggered: menu.openNow()
    }
    Connections {
        target: menu.transientParent
        enabled: keysFirst.running
        function onActiveChanged() {
            if (menu.transientParent.active) {
                keysFirst.stop()
                menu.openNow()
            }
        }
    }

    function selectable(index) {
        const entry = entries[index]
        return entry !== undefined && entry !== null && entry.separator !== true && entry.visible !== false
    }

    // The next entry that can be chosen, `by` 1 down or -1 up, round the end.
    function step(by) {
        const count = entries.length
        let index = current < 0 ? (by > 0 ? -1 : count) : current
        for (let n = 0; n < count; ++n) {
            index = ((index + by) % count + count) % count
            if (selectable(index)) {
                current = index
                return
            }
        }
    }

    function trigger(index) {
        if (!selectable(index))
            return
        const entry = entries[index]
        // Closed first: what the entry opens takes the focus.
        visible = false
        if (typeof entry.triggered === "function")
            entry.triggered()
        activated(index)
    }

    // Every key press of the app while the menu is open (Menus).
    function takeKey(key) {
        switch (key) {
        case Qt.Key_Escape:
            visible = false
            break
        case Qt.Key_Down:
            step(1)
            break
        case Qt.Key_Up:
        case Qt.Key_Backtab:
            step(-1)
            break
        case Qt.Key_Tab:
            step(1)
            break
        case Qt.Key_Home:
        case Qt.Key_PageUp:
            current = -1
            step(1)
            break
        case Qt.Key_End:
        case Qt.Key_PageDown:
            current = -1
            step(-1)
            break
        case Qt.Key_Return:
        case Qt.Key_Enter:
        case Qt.Key_Space:
            trigger(current)
            break
        }
    }

    // The entry under the keys stays in view in a long list.
    onCurrentChanged: {
        const row = current >= 0 ? rows.itemAt(current) : null
        if (!row)
            return
        if (row.y < flick.contentY)
            flick.contentY = row.y
        else if (row.y + row.height > flick.contentY + flick.height)
            flick.contentY = row.y + row.height - flick.height
    }

    // A tick drawn from two strokes.
    component Tick: Item {
        implicitWidth: 10
        implicitHeight: 8
        Rectangle {
            x: 0.5
            y: 3.2
            width: 4.4
            height: 1.6
            radius: 0.8
            antialiasing: true
            color: Theme.accent
            transform: Rotation { origin.x: 0; origin.y: 0.8; angle: 45 }
        }
        Rectangle {
            x: 3.6
            y: 6.3
            width: 8.4
            height: 1.6
            radius: 0.8
            antialiasing: true
            color: Theme.accent
            transform: Rotation { origin.x: 0; origin.y: 0.8; angle: -52 }
        }
    }

    Rectangle {
        anchors.fill: parent
        radius: 6
        color: Theme.window
        border.width: 1
        border.color: Theme.fieldEdge
    }

    Flickable {
        id: flick
        x: 0
        y: menu.pad
        width: menu.width
        height: menu.height - 2 * menu.pad
        contentWidth: width
        contentHeight: list.implicitHeight
        interactive: contentHeight > height
        boundsBehavior: Flickable.StopAtBounds
        clip: interactive

        Column {
            id: list
            width: flick.width

            Repeater {
                id: rows
                model: menu.entries
                delegate: Item {
                    id: row
                    required property var modelData
                    required property int index
                    readonly property bool separator: modelData.separator === true
                    readonly property bool current: menu.current === index

                    visible: modelData.visible !== false
                    width: list.width
                    implicitWidth: separator ? 0 : menu.gutter + menu.iconGutter + label.implicitWidth + 26
                    implicitHeight: separator ? 9 : Theme.tableRow

                    Rectangle {
                        visible: row.separator
                        x: 8
                        y: 4
                        width: parent.width - 16
                        height: 1
                        color: Theme.hairline
                    }
                    Rectangle {
                        visible: row.current && !row.separator
                        anchors.fill: parent
                        anchors.leftMargin: 4
                        anchors.rightMargin: 4
                        radius: 3
                        color: Theme.menuCurrent
                    }
                    Tick {
                        visible: !row.separator && row.modelData.checked === true
                        x: 10
                        anchors.verticalCenter: parent.verticalCenter
                    }
                    Image {
                        visible: !row.separator && status === Image.Ready
                        x: 10 + menu.gutter
                        anchors.verticalCenter: parent.verticalCenter
                        width: 16
                        height: 16
                        source: row.separator || row.modelData.icon === undefined ? "" : row.modelData.icon
                        fillMode: Image.PreserveAspectFit
                        smooth: true
                        mipmap: true
                    }
                    UiText {
                        id: label
                        visible: !row.separator
                        x: 10 + menu.gutter + menu.iconGutter
                        width: parent.width - x - 14
                        height: parent.height
                        text: row.separator ? "" : row.modelData.text
                        font.weight: row.modelData.checked === true ? Font.DemiBold : Font.Normal
                    }

                    HoverHandler {
                        enabled: !row.separator
                        onHoveredChanged: {
                            if (hovered)
                                menu.current = row.index
                            else if (menu.current === row.index)
                                menu.current = -1
                        }
                    }
                    TapHandler {
                        enabled: !row.separator
                        onTapped: menu.trigger(row.index)
                    }
                }
            }
        }
    }
}
