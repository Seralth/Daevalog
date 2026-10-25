// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import Daevalog

// One Daevanion board: its nodes on the game's grid, the ones taken in the
// accent (a skill node shows its skill's icon, faint until taken), each
// node's name on rest; beside the grid, what is taken. A board the game
// data does not know shows only that list.
Rectangle {
    id: card

    // A board of the view: { name, taken, total, rows, cols, cells, summary }.
    property var board: ({})
    // The character's class (a Theme class key), for skill icons.
    property string job: ""

    readonly property int cell: 20
    readonly property var cells: board.cells || []
    readonly property var summary: board.summary || []
    // The cells by place (row * 1000 + col), and the one under the pointer.
    readonly property var byPlace: {
        const places = {}
        for (const c of cells)
            places[c.row * 1000 + c.col] = c
        return places
    }
    property var pointed: null

    implicitHeight: Math.max(nodes.height, list.implicitHeight) + head.height + 30
    radius: 6
    color: Theme.card
    border.width: 1
    border.color: Theme.hairline

    // The count right after the name.
    Row {
        id: head
        x: 12
        y: 10
        width: parent.width - 24
        height: 22
        spacing: 8
        UiText {
            id: name
            width: Math.min(implicitWidth, parent.width - counted.implicitWidth - 8)
            text: card.board.name || ""
            font.pixelSize: Theme.sizeTitle
            font.weight: Font.DemiBold
        }
        UiText {
            id: counted
            anchors.baseline: name.baseline
            text: card.board.total !== null && card.board.total !== undefined
                ? I18n.tr("character.boards.nodes").replace("{taken}", card.board.taken).replace("{total}", card.board.total)
                : ""
            numeric: true
            muted: true
            font.pixelSize: Theme.sizeSmall
        }
    }

    Item {
        id: nodes
        x: 12
        y: head.y + head.height + 8
        width: (card.board.cols || 0) * card.cell
        height: (card.board.rows || 0) * card.cell

        Repeater {
            model: card.cells
            Item {
                id: node
                required property var modelData
                readonly property bool taken: modelData.taken
                readonly property bool core: modelData.kind === "stat" && (modelData.grade === "Unique" || modelData.grade === "Legend")
                x: modelData.col * card.cell
                y: modelData.row * card.cell
                width: card.cell
                height: card.cell

                Rectangle {
                    visible: node.modelData.kind !== "start"
                    anchors.centerIn: parent
                    width: card.cell - 3
                    height: card.cell - 3
                    radius: 3
                    color: node.taken ? Theme.accentTint : "transparent"
                    border.width: 1
                    border.color: node.taken ? Theme.accent : (node.core ? Theme.faint : Theme.fieldEdge)
                }
                // The start: a dot.
                Rectangle {
                    visible: node.modelData.kind === "start"
                    anchors.centerIn: parent
                    width: 8
                    height: 8
                    radius: 4
                    color: Theme.accent
                }
                SkillIcon {
                    visible: node.modelData.kind === "skill"
                    anchors.centerIn: parent
                    size: card.cell - 6
                    code: node.modelData.code
                    job: card.job
                    opacity: node.taken ? 1 : 0.35
                }
            }
        }

        // One note for the node under the pointer, not one per node: a
        // board has up to 153.
        HoverHandler {
            id: nodeHover
            onPointChanged: {
                if (hovered)
                    card.pointed = card.byPlace[Math.floor(point.position.y / card.cell) * 1000 + Math.floor(point.position.x / card.cell)] || null
            }
            onHoveredChanged: {
                if (!hovered)
                    card.pointed = null
            }
        }
        Item {
            visible: card.pointed !== null
            x: card.pointed ? card.pointed.col * card.cell : 0
            y: card.pointed ? card.pointed.row * card.cell : 0
            width: card.cell
            height: card.cell
            // The node's title, then what it gives: one stat or two.
            UiTip {
                text: card.pointed ? [card.pointed.title].concat(card.pointed.stats.map(s => s.name + "  " + s.value)).join("\n") : ""
            }
        }
    }

    // What is taken: how many nodes of each kind, the count first.
    Column {
        id: list
        x: nodes.width > 0 ? nodes.x + nodes.width + 16 : 12
        y: nodes.y
        width: card.width - x - 12
        spacing: 1
        Repeater {
            model: card.summary
            Row {
                id: entry
                required property var modelData
                width: list.width
                height: 19
                spacing: 8
                UiText {
                    width: 26
                    height: parent.height
                    horizontalAlignment: Text.AlignRight
                    text: String(entry.modelData.count)
                    numeric: true
                    color: Theme.number
                    font.weight: Font.DemiBold
                    font.pixelSize: Theme.sizeSmall
                }
                UiText {
                    id: entryName
                    width: parent.width - 34
                    height: parent.height
                    text: entry.modelData.title
                    muted: true
                    font.pixelSize: Theme.sizeSmall
                    UiTip {
                        text: entryName.truncated ? entry.modelData.title : ""
                    }
                }
            }
        }
    }
}
