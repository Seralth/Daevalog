// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import Daevalog

// One gear slot of the Character window. What the game keeps in its item
// tooltips is on the card: the enhance level, Item Level, Manastone slots,
// every rolled line and +1 skill line. Resting on it shows the item's base
// stats. An empty slot shows its name, quiet.
Rectangle {
    id: card

    // A gear entry of the view: { slot, label, group, item }.
    property var slot: ({})
    // The character's class (a Theme class key), for skill icons.
    property string job: ""
    // The base stats shown without the pointer (a screenshot of them).
    property bool baseShown: false

    // A base stat in the hover card: the value first, right-aligned, then
    // its name.
    component Ledger: Row {
        id: ledger
        property var entry: ({})
        spacing: 8
        UiText {
            width: 76
            horizontalAlignment: Text.AlignRight
            text: ledger.entry.value
            numeric: true
            color: Theme.number
            font.weight: Font.DemiBold
        }
        UiText {
            width: 216
            text: ledger.entry.name
            muted: true
        }
    }

    readonly property var item: slot.item || null
    readonly property color gradeColor: item ? Theme.gradeColor(item.grade) : Theme.faint

    implicitHeight: Math.max(body.implicitHeight, 28) + 20
    radius: 6
    color: Theme.card
    border.width: 1
    border.color: Theme.hairline

    // The grade on the left edge too, so a column reads at a glance.
    Rectangle {
        visible: !!card.item
        x: 1
        y: 4
        width: 2
        height: parent.height - 8
        radius: 1
        color: card.gradeColor
    }

    GearGlyph {
        x: 12
        y: 10
        size: 28
        slot: card.slot.slot || ""
        color: card.gradeColor
    }

    Column {
        id: body
        x: 50
        y: 10
        width: parent.width - 60
        spacing: 3

        // The enhance level right after the name.
        Row {
            width: parent.width
            spacing: 6
            UiText {
                id: name
                width: Math.min(implicitWidth, parent.width - (enhance.visible ? enhance.implicitWidth + 6 : 0))
                text: card.item ? card.item.name : card.slot.label
                color: card.item ? card.gradeColor : Theme.note
                font.weight: card.item ? Font.DemiBold : Font.Normal
                UiTip {
                    text: name.truncated ? name.text : ""
                }
            }
            UiText {
                id: enhance
                visible: !!card.item && card.item.enhance !== null
                text: card.item ? "+" + card.item.enhance : ""
                numeric: true
                color: card.item && card.item.enhance > 0 ? Theme.number : Theme.note
                font.weight: Font.DemiBold
            }
        }

        // Slot · grade · Item Level, then a ring per Manastone slot.
        Row {
            width: parent.width
            spacing: 8
            UiText {
                width: Math.min(implicitWidth, parent.width - (sockets.visible ? sockets.width + 8 : 0))
                height: 16
                text: card.item ? [card.slot.label, card.item.gradeLabel,
                                   card.item.itemLevel !== null ? I18n.tr("character.itemLevel") + " " + card.item.itemLevel : ""]
                                  .filter(s => s !== "").join("  ·  ")
                                : I18n.tr("character.gear.empty")
                muted: !!card.item
                color: card.item ? Theme.sub : Theme.note
                font.pixelSize: Theme.sizeSmall
            }
            Item {
                id: sockets
                visible: !!card.item && card.item.sockets > 0
                width: ringRow.width
                height: 16
                Row {
                    id: ringRow
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 3
                    Repeater {
                        model: card.item ? card.item.sockets : 0
                        Rectangle {
                            width: 7
                            height: 7
                            radius: 3.5
                            color: "transparent"
                            border.width: 1
                            border.color: Theme.sub
                        }
                    }
                }
                UiTip {
                    text: card.item ? I18n.tr("character.gear.sockets").replace("{n}", card.item.sockets) : ""
                }
            }
        }

        UiText {
            visible: !!card.item && !card.item.read
            width: parent.width
            text: I18n.tr("character.gear.notRead")
            color: Theme.note
            font.pixelSize: Theme.sizeSmall
        }

        // The rolled lines: each value right after its name.
        // Flow spacing would gap the wrapped lines too: each keeps its own
        // room on the right instead.
        Flow {
            visible: !!card.item && card.item.lines.length > 0
            width: parent.width
            Repeater {
                model: card.item ? card.item.lines : []
                Row {
                    id: line
                    required property var modelData
                    rightPadding: 12
                    spacing: 4
                    UiText {
                        id: lineName
                        width: Math.min(implicitWidth, body.width - lineValue.implicitWidth - 16)
                        text: line.modelData.name
                        muted: true
                        UiTip {
                            text: lineName.truncated ? lineName.text : ""
                        }
                    }
                    UiText {
                        id: lineValue
                        text: line.modelData.value
                        numeric: true
                        color: Theme.number
                        font.weight: Font.DemiBold
                    }
                }
            }
        }

        // The +1 skill lines.
        Flow {
            visible: !!card.item && card.item.skills.length > 0
            width: parent.width
            Repeater {
                model: card.item ? card.item.skills : []
                Row {
                    id: skillLine
                    required property var modelData
                    rightPadding: 12
                    spacing: 4
                    SkillIcon {
                        anchors.verticalCenter: parent.verticalCenter
                        size: 16
                        code: skillLine.modelData.code
                        job: card.job
                    }
                    UiText {
                        width: Math.min(implicitWidth, body.width - 20 - skillLevel.implicitWidth - 20)
                        text: skillLine.modelData.name
                    }
                    UiText {
                        id: skillLevel
                        text: I18n.tr("character.gear.skillLevel").replace("{n}", skillLine.modelData.levels)
                        numeric: true
                        color: Theme.number
                        font.weight: Font.DemiBold
                    }
                }
            }
        }
    }

    // Base stats at the item's enhance level, and the lines every copy of
    // the item has, each value before its name.
    UiTipCard {
        active: !!card.item && (card.item.base.length > 0 || card.item.fixed.length > 0)
        forced: card.baseShown
        cardWidth: 300

        UiText {
            visible: !!card.item && card.item.base.length > 0
            width: 300
            text: card.item ? I18n.tr("character.gear.baseAt").replace("{n}", card.item.enhance) : ""
            color: Theme.note
            font.pixelSize: Theme.sizeSmall
        }
        Repeater {
            model: card.item ? card.item.base : []
            Ledger {
                required property var modelData
                entry: modelData
            }
        }
        Item {
            visible: !!card.item && card.item.base.length > 0 && card.item.fixed.length > 0
            width: 1
            height: 6
        }
        UiText {
            visible: !!card.item && card.item.fixed.length > 0
            width: 300
            text: I18n.tr("character.gear.base")
            color: Theme.note
            font.pixelSize: Theme.sizeSmall
        }
        Repeater {
            model: card.item ? card.item.fixed : []
            Ledger {
                required property var modelData
                entry: modelData
            }
        }
    }
}
