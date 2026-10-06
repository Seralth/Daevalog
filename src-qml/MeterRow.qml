// SPDX-License-Identifier: MIT
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
    // Bar length, 0 to 1, against the top row.
    required property real fill
    required property bool you

    readonly property color classColor: Theme.classColor(job)

    height: Theme.rowHeight

    Rectangle {
        anchors.fill: parent
        radius: 3
        color: Theme.rowEmpty
    }

    // Clipped, so the bar keeps the row's rounded start and ends square.
    Item {
        width: Math.round(row.width * Math.max(0, Math.min(1, row.fill)))
        height: parent.height
        clip: true

        Rectangle {
            width: row.width
            height: parent.height
            radius: 3
            color: Theme.barFill(row.classColor)
        }
        Rectangle {
            anchors.right: parent.right
            width: 2
            height: parent.height
            color: Theme.barEdge(row.classColor)
        }
    }

    // Your own row: a rail in the accent colour.
    Rectangle {
        visible: row.you
        width: 2
        height: parent.height
        color: Theme.accent
    }

    Image {
        x: Theme.padLeft
        anchors.verticalCenter: parent.verticalCenter
        width: Theme.iconSize
        height: Theme.iconSize
        source: Theme.classIcon(row.job)
        fillMode: Image.PreserveAspectFit
        smooth: true
        mipmap: true
    }

    HudText {
        x: Theme.padLeft + Theme.colIcon + Theme.colGap
        width: totalText.x - Theme.colGap - x
        height: parent.height
        text: row.name
        font.weight: Font.DemiBold
    }
    HudText {
        id: totalText
        anchors.right: rateText.left
        anchors.rightMargin: Theme.colGap
        width: Theme.colTotal
        height: parent.height
        horizontalAlignment: Text.AlignRight
        numeric: true
        text: row.total
        color: Theme.number
        font.weight: Font.Medium
    }
    HudText {
        id: rateText
        anchors.right: shareText.left
        anchors.rightMargin: Theme.colGap
        width: Theme.colRate
        height: parent.height
        horizontalAlignment: Text.AlignRight
        numeric: true
        text: row.rate
        color: Theme.number
        font.weight: Font.Medium
    }
    HudText {
        id: shareText
        anchors.right: parent.right
        anchors.rightMargin: Theme.padRight
        width: Theme.colShare
        height: parent.height
        horizontalAlignment: Text.AlignRight
        numeric: true
        text: row.share
        color: Theme.share
        font.weight: Font.Medium
    }
}
