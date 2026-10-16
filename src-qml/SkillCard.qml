// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import Daevalog

// The card beside the meter while the pointer rests on a row: that
// player's five biggest skills, as 1.x's hover card (Details.hover), read
// again each second while it shows. Each line has its damage first, then
// its share of the player's damage, then the skill, so no number sits
// across the card from its name.
Window {
    id: card

    property var player: null

    flags: Qt.ToolTip | Qt.FramelessWindowHint | Qt.WindowDoesNotAcceptFocus
    color: "transparent"
    width: 280
    height: content.implicitHeight + 16

    readonly property var hover: Details.hover !== "" ? JSON.parse(Details.hover) : null
    // The read for this player, not one still on its way for the last row.
    readonly property var view: hover && player && hover.player === player.entityId ? hover : null

    function read() {
        if (visible && player && player.entityId !== undefined)
            Details.readHover(player.entityId)
    }
    onPlayerChanged: read()
    onVisibleChanged: read()
    Timer {
        interval: 1000
        repeat: true
        running: card.visible && card.player !== null
        onTriggered: card.read()
    }

    Rectangle {
        anchors.fill: parent
        radius: 6
        color: Theme.window
        border.width: 1
        border.color: Theme.panelEdge
    }

    Column {
        id: content
        x: 10
        y: 8
        width: parent.width - 20
        spacing: 4

        Row {
            spacing: 6
            height: 22
            Image {
                anchors.verticalCenter: parent.verticalCenter
                width: 16
                height: 16
                source: card.player ? Theme.classIcon(card.player.job) : ""
                mipmap: true
            }
            UiText {
                anchors.verticalCenter: parent.verticalCenter
                text: card.player ? I18n.playerName(card.player) : ""
                font.weight: Font.DemiBold
            }
        }
        UiText {
            visible: !card.view || card.view.skills.length === 0
            height: 20
            text: !card.view ? I18n.tr("details.hoverTooltip.loading") : I18n.tr("details.hoverTooltip.empty")
            color: Theme.note
        }
        Repeater {
            model: card.view ? card.view.skills : []
            Item {
                id: skill
                required property var modelData
                width: content.width
                height: 20
                UiText {
                    id: damage
                    width: 56
                    height: parent.height
                    horizontalAlignment: Text.AlignRight
                    numeric: true
                    text: Format.rate(skill.modelData.damage)
                }
                UiText {
                    id: share
                    x: damage.width + 6
                    width: 40
                    height: parent.height
                    horizontalAlignment: Text.AlignRight
                    numeric: true
                    muted: true
                    text: card.view && card.view.total > 0 ? Format.share(skill.modelData.damage / card.view.total * 100) : ""
                }
                UiText {
                    x: share.x + share.width + 10
                    width: parent.width - x
                    height: parent.height
                    text: skill.modelData.name
                }
            }
        }
    }
}
