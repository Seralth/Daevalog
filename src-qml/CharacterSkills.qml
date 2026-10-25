// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Daevalog

// The Character window's Skills page: the class's skills in the game's
// order, Active on the left, Passive and Stigma on the right. Each with its
// level, the levels Daevanion and gear add, and its Specialty picks; a
// stigma with its tiers by the level that turns each on.
Flickable {
    id: page

    property var view: null

    readonly property var skills: view ? view.skills : null
    readonly property string job: view ? view.head.class : ""

    contentHeight: columns.implicitHeight + 32
    clip: true
    boundsBehavior: Flickable.StopAtBounds

    UiText {
        visible: !!page.view && page.skills === null
        x: Theme.gutter
        y: 16
        width: page.width - 2 * Theme.gutter
        text: I18n.tr("character.notRead")
        color: Theme.note
    }

    component Heading: UiText {
        height: 26
        verticalAlignment: Text.AlignTop
        font.weight: Font.DemiBold
        muted: true
    }

    // A box with a number in it: a Specialty option, or a stigma tier's
    // level; `lit` in the accent.
    component Mark: Rectangle {
        id: mark
        property string text
        property bool lit: false
        property bool quiet: false
        width: Math.max(18, markText.implicitWidth + 8)
        height: 18
        radius: 3
        color: lit ? Theme.accentTint : "transparent"
        border.width: 1
        border.color: lit ? Theme.accent : Theme.fieldEdge
        UiText {
            id: markText
            anchors.centerIn: parent
            text: mark.text
            numeric: true
            font.pixelSize: Theme.sizeSmall
            font.weight: mark.lit ? Font.DemiBold : Font.Normal
            color: mark.lit ? Theme.accent : (mark.quiet ? Theme.note : Theme.sub)
        }
    }

    // A mark and the text after it; the text elides at the row's width and
    // shows whole on rest.
    component Chip: Row {
        id: chip
        property string mark
        property bool lit: false
        property bool markQuiet: false
        property string text
        property bool quiet: false
        property real room: 300
        rightPadding: 14
        topPadding: 1
        bottomPadding: 1
        spacing: 6
        Mark {
            id: chipMark
            anchors.verticalCenter: parent.verticalCenter
            text: chip.mark
            lit: chip.lit
            quiet: chip.markQuiet
        }
        UiText {
            id: chipText
            visible: chip.text !== ""
            width: Math.min(implicitWidth, chip.room - chipMark.width - 20)
            text: chip.text
            color: chip.quiet ? Theme.note : Theme.name
            font.pixelSize: Theme.sizeSmall
            UiTip {
                text: chipText.truncated ? chip.text : ""
            }
        }
    }

    component SkillRow: Item {
        id: row
        property var skill: ({})
        readonly property bool learned: skill.learned === true
        readonly property var extra: [
            skill.board > 0 ? I18n.tr("character.skills.daevanion").replace("{n}", skill.board) : "",
            skill.gear > 0 ? I18n.tr("character.skills.gear").replace("{n}", skill.gear) : ""
        ].filter(s => s !== "")
        implicitHeight: Math.max(32, text.implicitHeight) + 12

        SkillIcon {
            y: 6
            size: 32
            code: row.skill.code || 0
            job: page.job
            opacity: row.learned ? 1 : 0.35
        }
        // The level right before the name.
        UiText {
            x: 38
            y: 6
            width: 30
            height: 20
            horizontalAlignment: Text.AlignRight
            text: row.learned ? String(row.skill.level) : "-"
            numeric: true
            color: row.learned ? Theme.number : Theme.note
            font.pixelSize: Theme.sizeTitle
            font.weight: Font.DemiBold
        }
        Column {
            id: text
            x: 78
            y: 6
            width: row.width - x
            spacing: 3
            UiText {
                width: parent.width
                height: 20
                text: row.skill.name || ""
                color: row.learned ? Theme.name : Theme.note
                font.weight: Font.DemiBold
            }
            UiText {
                visible: row.extra.length > 0
                width: parent.width
                text: row.extra.join("  ·  ")
                muted: true
                font.pixelSize: Theme.sizeSmall
            }
            // Specialty: the option in each slot, a slot the level has not
            // opened with the level that opens it.
            Flow {
                visible: (row.skill.slots || []).length > 0
                width: parent.width
                UiText {
                    height: 20
                    rightPadding: 10
                    text: I18n.tr("character.skills.specialty")
                    muted: true
                    font.pixelSize: Theme.sizeSmall
                }
                Repeater {
                    model: row.skill.slots || []
                    Chip {
                        required property var modelData
                        room: text.width
                        mark: modelData.option > 0 ? String(modelData.option) : ""
                        lit: modelData.option > 0
                        markQuiet: !modelData.open
                        quiet: modelData.option === 0
                        text: modelData.option > 0 ? modelData.text
                            : modelData.open ? I18n.tr("character.gear.empty")
                            : I18n.tr("character.level").replace("{n}", modelData.unlock)
                    }
                }
            }
            // A stigma's tiers: each by the level that turns it on, lit once
            // reached.
            Flow {
                visible: (row.skill.tiers || []).length > 0
                width: parent.width
                Repeater {
                    model: row.skill.tiers || []
                    Chip {
                        required property var modelData
                        room: text.width
                        mark: String(modelData.unlock)
                        lit: modelData.reached
                        quiet: !modelData.reached
                        text: modelData.text
                    }
                }
            }
        }
    }

    component Section: Column {
        id: section
        property string title
        property var rows: []
        visible: rows.length > 0
        spacing: 0
        Heading {
            width: section.width
            text: section.title
        }
        Repeater {
            model: section.rows
            SkillRow {
                required property var modelData
                width: section.width
                skill: modelData
            }
        }
    }

    RowLayout {
        id: columns
        visible: !!page.skills
        x: Theme.gutter
        y: 16
        width: page.width - 2 * Theme.gutter
        spacing: 32
        Section {
            Layout.fillWidth: true
            Layout.preferredWidth: 1
            Layout.alignment: Qt.AlignTop
            title: I18n.tr("character.skills.active")
            rows: page.skills ? page.skills.active : []
        }
        Column {
            Layout.fillWidth: true
            Layout.preferredWidth: 1
            Layout.alignment: Qt.AlignTop
            spacing: 20
            Section {
                width: parent.width
                title: I18n.tr("character.skills.passive")
                rows: page.skills ? page.skills.passive : []
            }
            Section {
                width: parent.width
                title: I18n.tr("character.skills.stigma")
                rows: page.skills ? page.skills.stigma : []
            }
        }
    }
}
