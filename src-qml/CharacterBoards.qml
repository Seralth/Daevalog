// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import Daevalog

// The Character window's Daevanion page: each of the class's boards as the
// game lays it out, the nodes taken in the accent, and beside it what is
// taken, by node: how many of each, the count before the name. Two boards
// side by side where they fit.
Flickable {
    id: page

    property var view: null

    readonly property var boards: view && view.boards ? view.boards : []
    readonly property string job: view ? view.head.class : ""
    readonly property int count: width >= 1120 ? 2 : 1
    readonly property real cardWidth: (width - 2 * Theme.gutter - (count - 1) * 12) / count

    contentHeight: grid.implicitHeight + 32
    clip: true
    boundsBehavior: Flickable.StopAtBounds

    UiText {
        visible: !!page.view && (page.view.boards === null || page.boards.length === 0)
        x: Theme.gutter
        y: 16
        width: page.width - 2 * Theme.gutter
        text: page.view && page.view.boards === null ? I18n.tr("character.notRead") : I18n.tr("character.none")
        color: Theme.note
    }

    Grid {
        id: grid
        x: Theme.gutter
        y: 16
        columns: page.count
        spacing: 12
        Repeater {
            model: page.boards
            CharacterBoard {
                required property var modelData
                width: page.cardWidth
                board: modelData
                job: page.job
            }
        }
    }
}
