// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import Daevalog

// The Character window's Character sheet: the game's own stat sheet as the
// meter last read it, in the game's stat window groups and order. Each
// value sits right before its name, right-aligned, so a column reads down
// like a ledger. Groups fill columns top to bottom, as many columns as fit.
Flickable {
    id: page

    property var view: null

    readonly property var groups: view && view.stats ? view.stats : []
    readonly property int gap: 28
    readonly property int count: Math.max(1, Math.min(4, Math.floor((width - 2 * Theme.gutter + gap) / (300 + gap))))
    readonly property real columnWidth: (width - 2 * Theme.gutter - (count - 1) * gap) / count
    // The groups in reading order, cut into `count` columns of about the
    // same length; a group is never split.
    readonly property var columns: {
        const lines = g => g.rows.length + 2
        const total = groups.reduce((n, g) => n + lines(g), 0)
        const target = total / count
        const out = [[]]
        let filled = 0
        for (const g of groups) {
            const last = out[out.length - 1]
            if (last.length > 0 && out.length < count && filled + lines(g) / 2 > target) {
                out.push([g])
                filled = lines(g)
            } else {
                last.push(g)
                filled += lines(g)
            }
        }
        return out
    }

    contentHeight: sheet.implicitHeight + 32
    clip: true
    boundsBehavior: Flickable.StopAtBounds

    UiText {
        visible: !!page.view && page.view.stats === null
        x: Theme.gutter
        y: 16
        width: page.width - 2 * Theme.gutter
        text: I18n.tr("character.notRead")
        color: Theme.note
    }

    Row {
        id: sheet
        x: Theme.gutter
        y: 16
        spacing: page.gap
        Repeater {
            model: page.columns
            Column {
                id: column
                required property var modelData
                width: page.columnWidth
                spacing: 16
                Repeater {
                    model: column.modelData
                    Column {
                        id: group
                        required property var modelData
                        width: column.width
                        spacing: 0
                        UiText {
                            width: parent.width
                            height: 22
                            text: group.modelData.title
                            font.weight: Font.DemiBold
                        }
                        Rectangle {
                            width: parent.width
                            height: 1
                            color: Theme.hairline
                        }
                        Item {
                            width: 1
                            height: 3
                        }
                        Repeater {
                            model: group.modelData.rows
                            Row {
                                id: row
                                required property var modelData
                                width: group.width
                                height: 21
                                spacing: 10
                                UiText {
                                    width: 84
                                    height: parent.height
                                    horizontalAlignment: Text.AlignRight
                                    text: row.modelData.value
                                    numeric: true
                                    color: Theme.number
                                    font.weight: Font.DemiBold
                                }
                                UiText {
                                    id: statName
                                    width: parent.width - 94
                                    height: parent.height
                                    text: row.modelData.name
                                    muted: true
                                    UiTip {
                                        text: statName.truncated ? row.modelData.name : ""
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
