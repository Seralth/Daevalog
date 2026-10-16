// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import Daevalog

// A table: a quiet head line, then rows. Each column is { key, label,
// width, right, muted, tip, icon }: width 0 takes the room left, `right`
// aligns numbers, `icon` puts the row's class icon ("class") or its skill
// icon ("skill", by the row's `code` and `job`) before the text.
Column {
    id: table

    property var columns: []
    property var rows: []
    // The text of a row's cell.
    property var cellText: (row, key) => row[key] === undefined ? "" : String(row[key])
    // A row's bar, as on the meter: (row) => 0-1, or a negative for none.
    property var barFill: null
    // How strong those bars are. Text sits on them, so the windows' tables
    // stay fainter than the meter (Theme.tableBarStrength).
    property real barStrength: Theme.tableBarStrength
    // A row's share bar in the room the columns leave: (row) => 0-1, the
    // top row full, as on the meter. With it, the width-0 column is only as
    // wide as its widest text, and the bar takes the rest of the line.
    property var shareFill: null
    // Cells that do not match the game's record: (row, key) => true.
    property var flagged: null
    property int selected: -1
    property int rowHeight: Theme.tableRow
    property int iconSize: 18
    // The rows' and the heads' text sizes (Details' Font size).
    property int textSize: Theme.sizeRow
    property int headSize: Theme.sizeSmall
    readonly property int headHeight: headSize + 13
    // Heads that sort: a click asks for `sortKey`'s column (headClicked);
    // the sorted head shows its order with a chevron.
    property bool sortable: false
    property string sortKey: ""
    property bool sortAscending: false
    signal headClicked(string key)
    // Rows with `expandable` get a chevron before the name; a click on it
    // asks to open or close the row (foldClicked). `sub` rows sit under
    // their skill.
    signal foldClicked(int index, var row)
    readonly property int foldWidth: rows.some(r => r.expandable) ? 16 : 0
    // With `scrollable`, a table narrower than its columns need keeps its
    // first column (the names) at `pinnedWidth` and scrolls the rest
    // sideways: a scroll bar under it, Shift and the wheel, or a sideways
    // swipe.
    property bool scrollable: false
    property int pinnedWidth: 180
    property real scrollX: 0
    readonly property bool scrolling: scrollable && columns.length > 1 && width < fixedWidth + pinnedWidth
    readonly property real scrollRange: scrolling ? fixedWidth + pinnedWidth - width : 0
    readonly property real scrollView: Math.max(1, width - 2 * pad - pinnedWidth - gap)
    onScrollRangeChanged: scrollX = Math.max(0, Math.min(scrollX, scrollRange))
    function scrollBy(delta) {
        scrollX = Math.max(0, Math.min(scrollRange, scrollX + delta))
    }
    signal rowClicked(int index, var row)
    // Rows that answer a click (rowClicked): only these shade under the
    // pointer and show a hand, so a row never promises what it cannot do.
    property bool clickable: false

    readonly property int gap: 12
    readonly property int pad: 8
    // A column's width grows with the text past the rows' usual size.
    function scaled(width) {
        return Math.round(width * Math.max(1, textSize / Theme.sizeRow))
    }
    readonly property int fixedWidth: {
        let sum = 0
        for (const column of columns)
            sum += scaled(column.width || 0)
        return sum + gap * Math.max(0, columns.length - 1) + pad * 2
    }

    function widthOf(column) {
        if (column.width > 0)
            return scaled(column.width)
        if (table.scrolling)
            return table.pinnedWidth
        const room = Math.max(80, table.width - fixedWidth)
        return shareFill ? Math.min(textWidth, room) : room
    }

    // The width-0 column's widest text, and the width the table needs with
    // that column so wide: a table given this width keeps its numbers next
    // to their names instead of across the window.
    readonly property real fitWidth: fixedWidth + textWidth
    readonly property real textWidth: {
        let widest = 80
        for (const column of columns) {
            if (column.width > 0)
                continue
            const icon = column.icon === "class" ? 24 : (column.icon === "skill" ? iconSize + 8 : 0)
            const fold = column.key === "name" ? foldWidth : 0
            widest = Math.max(widest, cellMetrics.advanceWidth(column.label || "") + (sortable ? 14 : 0))
            for (const row of rows)
                widest = Math.max(widest, cellMetrics.advanceWidth((row.sub && column.key === "name" ? subMark : "") + cellText(row, column.key)) + icon + fold)
        }
        return Math.ceil(widest) + 4
    }
    readonly property string subMark: "\u2022 "
    FontMetrics {
        id: cellMetrics
        font.family: Theme.font
        font.pixelSize: table.textSize
    }

    component Cell: Item {
        id: cell
        required property var column
        property var row: null
        property int rowIndex: -1
        property bool head: false
        property bool flag: false

        readonly property bool named: cell.column.key === "name" && !cell.head
        readonly property bool sorted: cell.head && table.sortable && cell.column.key === table.sortKey

        width: table.widthOf(column)
        height: head ? table.headHeight - 2 : table.rowHeight

        // The fold's chevron: right while closed, down while open.
        Item {
            id: fold
            width: cell.named ? table.foldWidth : 0
            height: parent.height
            Chevron {
                visible: cell.named && !!cell.row && !!cell.row.expandable
                anchors.centerIn: parent
                turn: cell.row && cell.row.expanded ? 0 : -90
                color: foldArea.containsMouse ? Theme.name : Theme.sub
            }
            MouseArea {
                id: foldArea
                anchors.fill: parent
                enabled: cell.named && !!cell.row && !!cell.row.expandable
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: table.foldClicked(cell.rowIndex, cell.row)
            }
        }
        Image {
            id: classIcon
            visible: !cell.head && cell.column.icon === "class"
            anchors.left: fold.right
            anchors.verticalCenter: parent.verticalCenter
            width: visible ? 16 : 0
            height: 16
            source: visible && cell.row ? Theme.classIcon(cell.row.job) : ""
            fillMode: Image.PreserveAspectFit
            mipmap: true
        }
        SkillIcon {
            id: skillIcon
            visible: !cell.head && cell.column.icon === "skill" && !!cell.row
            anchors.left: fold.right
            anchors.verticalCenter: parent.verticalCenter
            size: table.iconSize
            width: visible ? size : 0
            code: visible ? cell.row.code || 0 : 0
            job: visible ? cell.row.job || "" : ""
        }
        UiText {
            id: cellText
            anchors.left: parent.left
            anchors.leftMargin: fold.width + classIcon.width + skillIcon.width + (classIcon.visible || skillIcon.visible ? 8 : 0)
            anchors.right: parent.right
            anchors.rightMargin: cell.sorted && !cell.column.right ? 14 : 0
            height: parent.height
            horizontalAlignment: cell.column.right ? Text.AlignRight : Text.AlignLeft
            numeric: !!cell.column.right
            muted: cell.head || !!cell.column.muted
            font.pixelSize: cell.head ? table.headSize : table.textSize
            color: cell.flag ? Theme.warning : (cell.head || cell.column.muted || (cell.named && cell.row && cell.row.sub) ? Theme.sub : Theme.name)
            font.weight: cell.sorted ? Font.DemiBold : Font.Normal
            text: cell.head ? cell.column.label
                : (cell.named && cell.row && cell.row.sub ? table.subMark : "") + table.cellText(cell.row, cell.column.key)
        }
        // The sorted head's order, beside its label: down biggest first.
        Chevron {
            visible: cell.sorted
            anchors.verticalCenter: parent.verticalCenter
            x: cell.column.right ? Math.max(0, cell.width - cellText.contentWidth - width - 4) : Math.min(cell.width - width, cellText.x + cellText.contentWidth + 4)
            turn: table.sortAscending ? 180 : 0
            color: Theme.name
        }
        MouseArea {
            anchors.fill: parent
            enabled: cell.head && table.sortable
            cursorShape: enabled ? Qt.PointingHandCursor : Qt.ArrowCursor
            onClicked: table.headClicked(cell.column.key)
        }
        UiTip {
            text: cell.head ? (cell.column.tip || "") : ""
        }
    }

    Item {
        width: table.width
        height: table.headHeight
        Row {
            x: table.pad
            spacing: table.gap
            Repeater {
                model: table.scrolling ? table.columns.slice(0, 1) : table.columns
                Cell {
                    required property var modelData
                    column: modelData
                    head: true
                }
            }
            Item {
                visible: table.scrolling
                width: table.scrollView
                height: table.headHeight - 2
                clip: true
                Row {
                    x: -table.scrollX
                    spacing: table.gap
                    Repeater {
                        model: table.scrolling ? table.columns.slice(1) : []
                        Cell {
                            required property var modelData
                            column: modelData
                            head: true
                        }
                    }
                }
            }
        }
        Rectangle {
            anchors.bottom: parent.bottom
            width: parent.width
            height: 1
            color: Theme.hairline
        }
    }

    Repeater {
        model: table.rows
        Item {
            id: line
            required property var modelData
            required property int index
            readonly property bool chosen: index === table.selected
            readonly property real fill: table.barFill ? table.barFill(modelData) : -1
            readonly property color classColor: Theme.classColor(modelData.job || "")

            width: table.width
            height: table.rowHeight + 2

            Rectangle {
                anchors.fill: parent
                anchors.topMargin: 1
                anchors.bottomMargin: 1
                radius: 3
                color: line.chosen ? Theme.accentTint : (rowHover.hovered && table.clickable ? Theme.hover : (line.fill >= 0 ? Theme.rowEmpty : "transparent"))
            }
            // The bar, as on the meter: flat class colour, a solid edge.
            Item {
                visible: line.fill >= 0
                y: 1
                width: Math.round(parent.width * Math.max(0, Math.min(1, line.fill)))
                height: parent.height - 2
                clip: true
                Rectangle {
                    width: line.width
                    height: parent.height
                    radius: 3
                    color: Theme.barFill(line.classColor, table.barStrength)
                }
                Rectangle {
                    anchors.right: parent.right
                    width: 2
                    height: parent.height
                    color: Theme.barEdge(line.classColor)
                }
            }
            Rectangle {
                visible: line.chosen
                y: 1
                width: 2
                height: parent.height - 2
                color: Theme.accent
            }
            Row {
                id: cells
                x: table.pad
                spacing: table.gap
                y: 1
                Repeater {
                    model: table.scrolling ? table.columns.slice(0, 1) : table.columns
                    Cell {
                        required property var modelData
                        column: modelData
                        row: line.modelData
                        rowIndex: line.index
                        flag: table.flagged ? table.flagged(line.modelData, modelData.key) : false
                    }
                }
                Item {
                    visible: table.scrolling
                    width: table.scrollView
                    height: table.rowHeight
                    clip: true
                    Row {
                        x: -table.scrollX
                        spacing: table.gap
                        Repeater {
                            model: table.scrolling ? table.columns.slice(1) : []
                            Cell {
                                required property var modelData
                                column: modelData
                                row: line.modelData
                                rowIndex: line.index
                                flag: table.flagged ? table.flagged(line.modelData, modelData.key) : false
                            }
                        }
                    }
                }
            }
            // The share bar, after the last column: flat class colour and a
            // solid edge, as the meter's bars, half the line's height.
            Item {
                readonly property real share: table.shareFill ? Math.max(0, Math.min(1, table.shareFill(line.modelData))) : 0
                readonly property real room: table.width - x - table.pad
                visible: table.shareFill !== null && room > 24 && share > 0
                x: cells.x + cells.width + table.gap
                width: Math.max(2, Math.round(room * share))
                height: Math.round(table.rowHeight / 2)
                anchors.verticalCenter: parent.verticalCenter
                clip: true
                Rectangle {
                    width: parent.width + 3
                    height: parent.height
                    radius: 3
                    color: Theme.barFill(line.classColor)
                }
                Rectangle {
                    anchors.right: parent.right
                    width: 2
                    height: parent.height
                    color: Theme.barEdge(line.classColor)
                }
            }
            HoverHandler {
                id: rowHover
                enabled: table.clickable
                cursorShape: Qt.PointingHandCursor
            }
            TapHandler {
                enabled: table.clickable
                onTapped: table.rowClicked(line.index, line.modelData)
            }
        }
    }

    UiScrollBar {
        x: table.pad + table.pinnedWidth + table.gap
        width: table.scrollView
        position: table.scrollX
        range: table.scrollRange
        view: table.scrollView
        onMoved: p => table.scrollX = p
    }
    WheelHandler {
        target: null
        enabled: table.scrolling
        acceptedModifiers: Qt.ShiftModifier
        onWheel: event => table.scrollBy(-(event.angleDelta.y + event.angleDelta.x) / 2)
    }
    WheelHandler {
        target: null
        enabled: table.scrolling
        orientation: Qt.Horizontal
        onWheel: event => table.scrollBy(-event.angleDelta.x / 2)
    }
}
