// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Daevalog

// The Character window's Gear page: armor down the left; weapons, runes and
// the titles in the middle; accessories down the right. Scrolls when the
// cards outgrow the window.
Flickable {
    id: page

    property var view: null
    // The first worn item's base stats shown (--open character/base).
    property bool baseShown: false

    readonly property var gear: view && view.gear ? view.gear : []
    readonly property var titles: view ? view.titles : null
    readonly property string job: view ? view.head.class : ""
    readonly property string firstWorn: {
        const worn = gear.find(s => !!s.item)
        return worn ? worn.slot : ""
    }
    function slots(groups) {
        return gear.filter(s => groups.indexOf(s.group) >= 0)
    }

    contentHeight: columns.implicitHeight + 32
    clip: true
    boundsBehavior: Flickable.StopAtBounds

    component Cards: Column {
        id: cards
        property var groups: []
        Layout.fillWidth: true
        Layout.preferredWidth: 1
        Layout.alignment: Qt.AlignTop
        spacing: 8
        Repeater {
            model: page.slots(cards.groups)
            CharacterGearCard {
                required property var modelData
                width: cards.width
                slot: modelData
                job: page.job
                baseShown: page.baseShown && modelData.slot === page.firstWorn
            }
        }
    }

    RowLayout {
        id: columns
        x: Theme.gutter
        y: 16
        width: page.width - 2 * Theme.gutter
        spacing: 12

        Column {
            Layout.fillWidth: true
            Layout.preferredWidth: 1
            Layout.alignment: Qt.AlignTop
            spacing: 8
            // The game has not sent the gear yet.
            UiText {
                visible: !!page.view && page.view.gear === null
                width: parent.width
                text: I18n.tr("character.notRead")
                color: Theme.note
                wrapMode: Text.WordWrap
                maximumLineCount: 3
            }
            Cards {
                width: parent.width
                groups: ["armor"]
            }
        }

        Column {
            Layout.fillWidth: true
            Layout.preferredWidth: 1
            Layout.alignment: Qt.AlignTop
            spacing: 16
            Cards {
                width: parent.width
                groups: ["weapon"]
            }
            Cards {
                width: parent.width
                groups: ["rune"]
            }

            // The titles: how many owned, and the equipped ones with what
            // each gives while equipped.
            Rectangle {
                width: parent.width
                implicitHeight: titleList.implicitHeight + 20
                radius: 6
                color: Theme.card
                border.width: 1
                border.color: Theme.hairline

                Column {
                    id: titleList
                    x: 12
                    y: 10
                    width: parent.width - 24
                    spacing: 6
                    Row {
                        spacing: 8
                        UiText {
                            id: titlesHead
                            text: I18n.tr("character.titles")
                            font.weight: Font.DemiBold
                        }
                        UiText {
                            visible: !!page.titles
                            anchors.baseline: titlesHead.baseline
                            text: page.titles ? I18n.tr("character.titlesOwned").replace("{n}", Format.whole(page.titles.owned)) : ""
                            muted: true
                            font.pixelSize: Theme.sizeSmall
                        }
                    }
                    UiText {
                        visible: !page.titles || page.titles.equipped.length === 0
                        width: parent.width
                        text: page.titles ? I18n.tr("character.none") : I18n.tr("character.notRead")
                        color: Theme.note
                        wrapMode: Text.WordWrap
                        maximumLineCount: 3
                    }
                    Repeater {
                        model: page.titles ? page.titles.equipped : []
                        Column {
                            id: title
                            required property var modelData
                            width: titleList.width
                            spacing: 2
                            // The kind right after the name.
                            Row {
                                width: parent.width
                                spacing: 6
                                UiText {
                                    id: titleName
                                    width: Math.min(implicitWidth, parent.width - kind.implicitWidth - 6)
                                    text: title.modelData.name
                                }
                                UiText {
                                    id: kind
                                    anchors.baseline: titleName.baseline
                                    text: title.modelData.category
                                    muted: true
                                    font.pixelSize: Theme.sizeSmall
                                }
                            }
                            Flow {
                                width: parent.width
                                Repeater {
                                    model: title.modelData.effects
                                    Row {
                                        id: effect
                                        required property var modelData
                                        rightPadding: 12
                                        spacing: 4
                                        UiText {
                                            text: effect.modelData.name
                                            muted: true
                                            font.pixelSize: Theme.sizeSmall
                                        }
                                        UiText {
                                            text: effect.modelData.value
                                            numeric: true
                                            color: Theme.number
                                            font.pixelSize: Theme.sizeSmall
                                            font.weight: Font.DemiBold
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Cards {
            groups: ["accessory", "other"]
        }
    }
}
