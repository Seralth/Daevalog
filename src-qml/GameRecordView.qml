// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Daevalog

// The game's own Combat Analysis records of a fight against the meter's
// numbers, skill by skill. A row that differs means the meter read the
// fight differently: the game's number is the right one.
Column {
    id: view

    // The fight's records (History's `records`), oldest first.
    property var records: []
    // The record shown: the newest, as 1.x opens.
    property int picked: 0
    // "meter", "game" or "both"; "game" alone where no packets were kept.
    property string numbersShown: "both"
    property bool onlyDiffer: false

    readonly property var record: records.length > 0 ? records[Math.max(0, Math.min(picked, records.length - 1))] : null
    readonly property bool compared: !!record && record.compared
    // [key, count index]: -1 is the damage.
    readonly property var keys: [["dmg", -1], ["hit", 0], ["crit", 1], ["perfect", 2], ["double", 3], ["back", 5]]
    readonly property var rows: {
        if (!record)
            return []
        const listed = record.rows.map((r, i) => ({ "row": r, "at": i })).filter(e =>
            numbersShown === "game" ? e.row.inGame
            : numbersShown === "meter" ? e.row.inMeter
            : !onlyDiffer || !e.row.same)
        // Most damage first on the side shown, as 1.x sorts its table.
        const side = numbersShown === "game" ? "game" : "meter"
        listed.sort((a, b) => (b.row[side].damage - a.row[side].damage) || (a.at - b.at))
        return listed.map(e => e.row)
    }

    // Which records these are: History hands over a new array whenever it
    // reads the fight again (after an upload, say), and only other records
    // reset the choices.
    readonly property string recordsKey: records.map(r => r.from + "/" + r.to + "/" + r.target).join(" ")

    // A new fight: its newest record, both sides when there are packets.
    onRecordsKeyChanged: {
        picked = records.length - 1
        const newest = records.length > 0 ? records[records.length - 1] : null
        numbersShown = newest && newest.compared ? "both" : "game"
        onlyDiffer = false
    }

    function verdictOf(r) {
        const n = r.rows.length
        return r.matching === n ? I18n.tr("gameRecord.allMatch").replace("{n}", n)
            : I18n.tr("gameRecord.someMatch").replace("{x}", r.matching).replace("{n}", n)
    }

    function value(numbers, i) {
        return i < 0 ? numbers.damage : numbers.counts[i]
    }

    // A count column is wide enough for "meter  game N" where a shown row
    // differs in it.
    function widthOf(key, narrow) {
        if (numbersShown !== "both")
            return narrow
        const i = keys.find(k => k[0] === key)[1]
        return rows.some(r => value(r.meter, i) !== value(r.game, i)) ? 84 : narrow
    }

    function cell(row, key) {
        if (key === "name")
            return row.name
        const i = keys.find(k => k[0] === key)[1]
        const show = v => i < 0 ? Format.whole(v) : String(v)
        const meter = value(row.meter, i), game = value(row.game, i)
        if (numbersShown === "meter")
            return show(meter)
        if (numbersShown === "game")
            return show(game)
        if (meter === game)
            return show(meter)
        return show(meter) + "  " + I18n.tr("gameRecord.gameTag").replace("{n}", show(game))
    }

    spacing: 12

    RowLayout {
        width: parent.width
        spacing: 10
        // Only the game's numbers when no packets were saved: no choice to
        // offer, and the note below says why.
        UiText {
            visible: view.compared
            text: I18n.tr("gameRecord.switch")
            muted: true
        }
        UiSegmented {
            visible: view.compared
            options: [
                { "label": I18n.tr("gameRecord.meter"), "value": "meter" },
                { "label": I18n.tr("gameRecord.game"), "value": "game" },
                { "label": I18n.tr("gameRecord.both"), "value": "both" }
            ]
            current: view.numbersShown
            onPicked: value => view.numbersShown = value
        }
        // Two records over one fight: which one.
        UiDropdown {
            visible: view.records.length > 1
            Layout.preferredWidth: 170
            options: view.records.map((r, i) => ({ "label": r.from + " – " + r.to, "value": i }))
            current: view.picked
            onPicked: value => view.picked = value
        }
        Item {
            Layout.fillWidth: true
        }
        UiText {
            visible: view.compared && view.numbersShown === "both"
            text: I18n.tr("gameRecord.onlyDiffer")
        }
        UiSwitch {
            visible: view.compared && view.numbersShown === "both"
            checked: view.onlyDiffer
            onToggled: on => view.onlyDiffer = on
        }
        Item {
            width: 6
        }
        // The record's bug report as plain text (1.x's gameRecord.js), and
        // "Copied" for a moment.
        UiButton {
            id: copyButton
            property bool copied: false
            text: copied ? I18n.tr("gameRecord.copied") : I18n.tr("gameRecord.copy")
            onClicked: {
                if (!view.record || !view.record.report)
                    return
                System.copy(view.record.report)
                copied = true
                copiedTimer.restart()
            }
            Timer {
                id: copiedTimer
                interval: 1500
                onTriggered: copyButton.copied = false
            }
        }
    }

    // Both totals on the left, the meter's and the game's in two columns so
    // each game number sits under the other; the verdict, the result, at the
    // top right. Where both do not fit side by side, the verdict goes above.
    Item {
        id: summary
        width: parent.width
        readonly property bool fits: totals.implicitWidth + verdict.implicitWidth + 24 <= width
        implicitHeight: fits ? Math.max(totals.implicitHeight, verdict.implicitHeight) : verdict.implicitHeight + 6 + totals.implicitHeight
        UiText {
            id: verdict
            x: summary.fits ? summary.width - implicitWidth : 0
            text: !view.record ? "" : view.compared ? view.verdictOf(view.record) : I18n.tr("gameRecord.noComparison")
            font.weight: Font.DemiBold
            color: view.compared && view.record.matching !== view.record.rows.length ? Theme.warning : Theme.name
        }
        Grid {
            id: totals
            y: summary.fits ? 0 : verdict.implicitHeight + 6
            // Without the meter's numbers (no packets kept), the game's alone.
            columns: view.compared ? 2 : 1
            columnSpacing: 32
            rowSpacing: 4
            UiText {
                visible: view.compared
                numeric: true
                text: I18n.tr("gameRecord.damageMeter") + "  " + (view.record ? Format.whole(view.record.meterTotal) : "")
                muted: true
            }
            UiText {
                numeric: true
                text: I18n.tr("gameRecord.damageGame") + "  " + (view.record ? Format.whole(view.record.gameTotal) : "")
                muted: true
            }
            UiText {
                visible: view.compared
                numeric: true
                text: I18n.tr("gameRecord.takenMeter") + "  " + (view.record ? Format.whole(view.record.meterTaken) : "")
                muted: true
            }
            UiText {
                numeric: true
                text: I18n.tr("gameRecord.takenGame") + "  " + (view.record ? Format.whole(view.record.gameTaken) : "")
                muted: true
            }
        }
    }
    // What the record covers.
    UiText {
        width: parent.width
        wrapMode: Text.WordWrap
        maximumLineCount: 3
        muted: true
        font.pixelSize: Theme.sizeSmall
        text: {
            const r = view.record
            if (!r)
                return ""
            let covers = I18n.tr("gameRecord.covers").replace("{s}", r.covers)
            if (r.otherFights === 1)
                covers += ", " + I18n.tr("gameRecord.alsoCoversOne")
            else if (r.otherFights > 1)
                covers += ", " + I18n.tr("gameRecord.alsoCovers").replace("{n}", r.otherFights)
            return [I18n.tr("gameRecord.window").replace("{from}", r.from).replace("{to}", r.to),
                covers + ".", I18n.tr("gameRecord.targetOnly").replace("{target}", r.target)].join(" ")
        }
    }

    UiTable {
        width: parent.width
        columns: [
            { "key": "name", "label": I18n.tr("details.skills.skill"), "width": 0, "icon": "skill" },
            { "key": "dmg", "label": I18n.tr("details.skills.dmg"), "tip": I18n.tr("gameRecord.columnTip"), "width": view.widthOf("dmg", 80) > 80 ? 150 : 80, "right": true },
            { "key": "hit", "label": I18n.tr("details.skills.hits"), "width": view.widthOf("hit", 44), "right": true },
            { "key": "crit", "label": I18n.tr("details.skills.crit"), "width": view.widthOf("crit", 44), "right": true },
            { "key": "perfect", "label": I18n.tr("details.skills.perfect"), "width": view.widthOf("perfect", 44), "right": true },
            { "key": "double", "label": I18n.tr("details.skills.double"), "width": view.widthOf("double", 44), "right": true },
            { "key": "back", "label": I18n.tr("details.skills.back"), "width": view.widthOf("back", 44), "right": true }
        ]
        rows: view.rows
        cellText: (row, key) => view.cell(row, key)
        flagged: (row, key) => {
            if (key === "name" || view.numbersShown !== "both")
                return false
            const i = view.keys.find(k => k[0] === key)[1]
            return view.value(row.meter, i) !== view.value(row.game, i)
        }
    }

    UiText {
        width: parent.width
        wrapMode: Text.WordWrap
        maximumLineCount: 3
        muted: true
        font.pixelSize: Theme.sizeSmall
        text: I18n.tr("gameRecord.note")
    }
}
