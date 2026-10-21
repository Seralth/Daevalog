// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import Daevalog

// One player. The row's background is the bar, filled to the player's
// share; icon, name and numbers sit on top.
Item {
    id: row

    required property string job
    required property string name
    required property string total
    required property string rate
    required property string share
    // Bar length, 0 to 1, against the biggest row shown.
    required property real fill
    required property bool you
    required property bool unattributed
    // No name yet: `name` is the player's id.
    required property bool identifying
    // Shown with the class when other players' names are hidden.
    required property int number
    // Beside the name, for party members ("44K"); empty for the rest.
    required property string combatPower
    // 1.x's low-share colours: 0 none, 1 under 5% of the damage shown
    // (amber), 2 under 3% (coral), drawn as Settings > Meter > Low share
    // colour says: "row" tints the whole row and its bar, so a share near 0
    // shows it too (1.x recoloured the bar only); "number" colours the
    // share's number (`shareCell`); "off" neither.
    property int lowShare: 0
    property int shareCell: -1
    readonly property string lowLook: lowShare > 0 ? Settings.lowShare : "off"
    readonly property color lowColor: lowShare === 2 ? Theme.error : Theme.warning
    readonly property color barColor: lowLook === "row" ? lowColor : classColor

    // The number columns, left to right, as the tab sets them: their texts,
    // widths and kinds: "main", "share" (a percentage, a little dimmer) or
    // "second" (a secondary figure, dimmer and lighter).
    property var cells: [total, rate, share]
    property var widths: [64, 56, 40]
    property var kinds: ["main", "main", "share"]
    // Shown on the row's "-" cells when the pointer rests on one: why the
    // cell has no number.
    property string dashTip: ""

    signal clicked()
    signal pointerOver(bool on)

    readonly property color classColor: Theme.classColor(job)

    height: Theme.meter.rowHeight

    Rectangle {
        anchors.fill: parent
        radius: Theme.meter.part(3)
        color: row.lowLook === "row" ? Theme.barFill(row.lowColor, Theme.lowShareTint) : Theme.rowEmpty
    }

    // Clipped, so the bar keeps the row's rounded start and ends square.
    Item {
        width: Math.round(row.width * Math.max(0, Math.min(1, row.fill)))
        height: parent.height
        clip: true

        Rectangle {
            width: row.width
            height: parent.height
            radius: Theme.meter.part(3)
            color: Theme.barFill(row.barColor)
        }
        Rectangle {
            anchors.right: parent.right
            width: Theme.meter.px(2)
            height: parent.height
            color: Theme.barEdge(row.barColor)
        }
    }

    // Your own row: a rail in the accent colour.
    Rectangle {
        visible: row.you
        width: Theme.meter.px(2)
        height: parent.height
        color: Theme.accent
    }

    Image {
        x: Theme.meter.padLeft
        anchors.verticalCenter: parent.verticalCenter
        width: Theme.meter.iconSize
        height: Theme.meter.iconSize
        source: Theme.classIcon(row.job)
        fillMode: Image.PreserveAspectFit
        smooth: true
        mipmap: true
    }

    // The name, and the combat power right after it in the small type: the
    // name gives way first, so the power stays whole.
    Row {
        id: who
        x: Theme.meter.padLeft + Theme.meter.colIcon + Theme.meter.colGap
        height: parent.height
        spacing: Theme.meter.px(4)
        readonly property real room: numbers.x - Theme.meter.colGap - x
        HudText {
            width: Math.max(0, Math.min(implicitWidth, who.room - (power.visible ? power.implicitWidth + who.spacing : 0)))
            height: parent.height
            text: I18n.playerName(row)
            font.pixelSize: Theme.meter.sizeRow
            // The Bold settings: names DemiBold, numbers Medium, as approved;
            // normal weight when off.
            font.weight: Settings.boldNames ? Font.DemiBold : Font.Normal
        }
        HudText {
            id: power
            visible: row.combatPower !== ""
            height: parent.height
            numeric: true
            text: row.combatPower
            font.pixelSize: Theme.meter.sizeSmall
            // Apart from the name, in the theme's accent.
            color: Theme.accent
        }
    }

    Row {
        id: numbers
        anchors.right: parent.right
        anchors.rightMargin: Theme.meter.padRight
        height: parent.height
        spacing: Theme.meter.colGap

        Repeater {
            model: row.cells.length
            HudText {
                id: number
                required property int index
                width: row.widths[index] || Theme.meter.px(40)
                height: row.height
                horizontalAlignment: Text.AlignRight
                numeric: true
                font.pixelSize: Theme.meter.sizeRow
                // While a tab's texts change, a cell past the new list's end
                // still reads once before it goes.
                text: row.cells[index] !== undefined ? row.cells[index] : ""
                color: row.lowLook === "number" && index === row.shareCell ? row.lowColor
                    : row.kinds[index] === "main" ? Theme.number : Theme.hudShare
                // Bold DPS: the white numbers (damage and its rates), as
                // bold as Bold names; the shares stay plain.
                font.weight: Settings.boldDps && row.kinds[index] === "main" ? Font.DemiBold : Font.Normal
                UiTip {
                    delay: Theme.meterTipRest
                    text: number.text === "-" ? row.dashTip : ""
                }
            }
        }
    }

    HoverHandler {
        id: hover
        onHoveredChanged: row.pointerOver(hovered)
    }
    // A row that goes away under a resting pointer (a reload, a new fight)
    // never sees the pointer leave: its card goes with it.
    Component.onDestruction: if (hover.hovered) row.pointerOver(false)
    TapHandler {
        onTapped: row.clicked()
    }
}
