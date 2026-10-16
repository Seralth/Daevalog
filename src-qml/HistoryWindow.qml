// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Daevalog

// Saved fights (LOA Logs layout): filters and the list on the left, the
// chosen fight on the right with its party table, its game record when it
// has one, and the upload, preview and delete actions. Everything shown is
// the History object's; this window only lays it out.
AppWindow {
    id: history
    placeName: "history"

    // 0 DPS, 1 Heal, 2 Tank, 3 game record: the meter's tabs, plus the record.
    property int tab: 0

    // Details on the chosen fight, by its id.
    signal detailsRequested(string fight)

    title: I18n.tr("history.title")
    width: 1100
    height: 700
    minimumWidth: 860
    minimumHeight: 480

    readonly property var list: History.list !== "" ? JSON.parse(History.list) : null
    readonly property var fight: History.fight !== "" ? JSON.parse(History.fight) : null
    readonly property var records: fight ? fight.records : []
    readonly property var lastRecord: records.length > 0 ? records[records.length - 1] : null
    // An upload's or a preview's outcome for the fight shown.
    readonly property var note: History.note !== "" ? JSON.parse(History.note) : null
    readonly property bool busy: !!note && (note.kind === "uploading" || note.kind === "previewing")
    function noteText(n) {
        if (!n)
            return ""
        switch (n.kind) {
        case "uploading":
            return I18n.tr("history.uploadWorking")
        case "uploaded":
            return (n.private ? I18n.tr("history.uploadDonePrivate") : I18n.tr("history.uploadDonePublic")).replace("{url}", n.url)
        case "previewing":
            return I18n.tr("history.previewWorking")
        case "previewed":
            return I18n.tr("history.previewRequestDone").replace("{file}", n.file).replace("{sent}", n.sent).replace("{dir}", n.dir)
        }
        return n.message || ""
    }
    // A folder as a file URL, each part escaped. On Windows the drive
    // ("C:") starts the path.
    function fileUrl(path) {
        if (Qt.platform.os === "windows")
            return "file:///" + path.split(/[\\/]/).map((part, i) => i === 0 ? part : encodeURIComponent(part)).join("/")
        return "file://" + path.split("/").map(encodeURIComponent).join("/")
    }
    // An upload's link goes to the clipboard; a preview's folder opens.
    Connections {
        target: History
        function onNoteChanged() {
            const n = history.note
            if (n && n.kind === "uploaded")
                System.copy(n.url)
            else if (n && n.kind === "previewed")
                Qt.openUrlExternally(history.fileUrl(n.dir))
        }
    }
    // The party table's players for the tab, and the one opened below it.
    readonly property var partyRows: !fight || tab === 3 ? [] : [fight.dps, fight.healRows, fight.tank][tab]
    readonly property var player: History.player !== "" ? JSON.parse(History.player) : null
    readonly property var playerRow: player ? partyRows.find(r => r.id === player.id) || null : null
    // Everything read: the list, its game records and the chosen fight.
    // `history.list`: a bare `list` is the fight list's item (id: list).
    readonly property bool settled: History.ready && History.recordsChecked
        && (History.chosen === "" ? !history.list || history.list.shown === 0 : !!fight && fight.id === History.chosen)
        && (page !== "player" || !!player || History.chosen === "" || (!!fight && partyRows.length === 0))

    // --open history/heal, history/tank, history/record.
    onPageChanged: tab = Math.max(0, ["", "heal", "tank", "record"].indexOf(page))
    onVisibleChanged: {
        if (visible) {
            History.refresh()
            return
        }
        // Closed: the filters start empty next time, as 1.x's.
        History.boss = ""
        History.job = ""
        History.date = ""
        History.onlyRecords = false
        pages.reset()
        deleteStep.cancel()
    }
    // 30 fights at first, every group open; again when a filter changes.
    HistoryPages {
        id: pages
    }
    readonly property var shownList: pages.cut(history.list ? history.list.groups : [])
    Connections {
        target: History
        // Opened on the game record page: the newest fight that has one.
        function onRecordsCheckedChanged() {
            if (!History.recordsChecked || history.page !== "record" || !history.list)
                return
            for (const group of history.list.groups) {
                const withRecord = group.fights.find(f => !!f.record)
                if (withRecord) {
                    History.choose(withRecord.id)
                    return
                }
            }
        }
        function onChosenChanged() {
            deleteStep.cancel()
            if (history.tab === 3 && history.page !== "record")
                history.tab = 0
        }
        function onBossChanged() {
            pages.reset()
        }
        function onJobChanged() {
            pages.reset()
        }
        function onDateChanged() {
            pages.reset()
        }
        function onOnlyRecordsChanged() {
            pages.reset()
        }
        function onShowTrainingChanged() {
            pages.reset()
        }
        function onLayoutChanged() {
            pages.reset()
        }
        // An auto-upload finished: the fight's Upload becomes Open log.
        function onUploadedElsewhere() {
            if (history.visible)
                History.refresh()
        }
        // --open history/player: the fight's first player opened below.
        function onFightChanged() {
            if (history.page === "player" && history.fight && History.player === "" && history.partyRows.length > 0)
                History.openPlayer(history.partyRows[0].id)
        }
    }
    // Display Time or the language changed in Settings: every time of day
    // and every name again, at once.
    Connections {
        target: Settings
        function onHour24Changed() {
            History.redraw()
        }
        function onLanguageChanged() {
            History.redraw()
        }
    }
    // Delete asks once more before it deletes.
    ConfirmStep {
        id: deleteStep
        onConfirmed: History.deleteChosen()
    }

    readonly property var layouts: [
        { "label": I18n.tr("history.viewList"), "value": "list" },
        { "label": I18n.tr("history.viewDungeon"), "value": "dungeon" },
        { "label": I18n.tr("history.viewGrouped"), "value": "grouped" }
    ]

    function players(f) {
        if (!f || f.players === 0)
            return ""
        return f.players === 1 ? I18n.tr("history.solo") : I18n.tr("history.players").replace("{n}", f.players)
    }

    function rate(v) {
        return v === null || v === undefined ? "-" : Format.rate(v)
    }

    function count(v) {
        return v === null || v === undefined ? "-" : String(v)
    }

    readonly property NoData noData: NoData {}

    // 1.x's "open in a new place" mark: a box open at its top right and an
    // arrow out of it, drawn (one font, and it has no such glyph).
    component LinkMark: Item {
        id: mark
        readonly property color ink: markHover.hovered ? Theme.accent : Theme.sub
        width: 12
        height: 12
        HoverHandler {
            id: markHover
            cursorShape: Qt.PointingHandCursor
        }
        Rectangle { x: 0; y: 3; width: 1.5; height: 9; color: mark.ink }
        Rectangle { x: 0; y: 10.5; width: 9; height: 1.5; color: mark.ink }
        Rectangle { x: 0; y: 3; width: 5; height: 1.5; color: mark.ink }
        Rectangle { x: 7.5; y: 7; width: 1.5; height: 5; color: mark.ink }
        Rectangle { x: 7; y: 0; width: 5; height: 1.5; color: mark.ink }
        Rectangle { x: 10.5; y: 0; width: 1.5; height: 5; color: mark.ink }
        Rectangle {
            x: 4.5; y: 3.6; width: 8; height: 1.5
            rotation: -45
            antialiasing: true
            color: mark.ink
        }
    }

    // A small word in a box: Live, Training, a dungeon's difficulty.
    component Badge: Rectangle {
        id: badge
        property string text
        property bool lit: false
        property string tip: ""
        implicitWidth: Math.min(badgeText.implicitWidth + 10, 180)
        implicitHeight: 17
        radius: 3
        color: "transparent"
        border.width: 1
        border.color: lit ? Theme.accent : Theme.fieldEdge
        UiText {
            id: badgeText
            anchors.centerIn: parent
            width: Math.min(implicitWidth, 170)
            text: badge.text
            font.pixelSize: Theme.sizeSmall
            color: badge.lit ? Theme.accent : Theme.sub
        }
        UiTip {
            text: badge.tip
        }
    }

    component SwitchLine: Item {
        id: line
        property string label
        property string tip
        property bool checked
        signal toggled(bool on)
        width: parent ? parent.width : 300
        height: 26
        // The switch first and its name right after it, as a checkbox, so
        // no gap lies between them.
        UiSwitch {
            id: lineSwitch
            anchors.verticalCenter: parent.verticalCenter
            checked: line.checked
            onToggled: on => line.toggled(on)
        }
        UiText {
            x: lineSwitch.width + 10
            width: parent.width - x
            height: parent.height
            text: line.label
            UiTip {
                text: line.tip
            }
        }
    }

    // A fight's game records in a few words, for the list.
    function recordState(record) {
        if (!record)
            return ""
        if (!record.compared)
            return I18n.tr("gameRecord.notCompared")
        if (!record.differingRows)
            return I18n.tr("gameRecord.matches")
        return record.differingRows === 1 ? I18n.tr("gameRecord.differOne")
            : I18n.tr("gameRecord.differ").replace("{n}", record.differingRows)
    }

    RowLayout {
        anchors.fill: parent
        spacing: 0

        // Left: filters, view, the list.
        Rectangle {
            Layout.preferredWidth: 380
            Layout.fillHeight: true
            color: Theme.surface

            Column {
                id: filters
                x: 14
                y: 14
                width: parent.width - 28
                spacing: 8

                UiDropdown {
                    width: parent.width
                    options: [{ "label": I18n.tr("history.allBosses"), "value": "" }]
                        .concat(history.list ? history.list.bosses : [])
                    current: History.boss
                    onPicked: value => History.boss = value
                }
                Row {
                    width: parent.width
                    spacing: 8
                    UiDropdown {
                        width: (parent.width - 8) / 2
                        options: [{ "label": I18n.tr("history.allClasses"), "value": "" }]
                            .concat(history.list ? history.list.jobs.map(j => ({ "label": j.label, "value": j.value,
                                                                                "icon": Theme.classIcon(j.icon) })) : [])
                        current: History.job
                        onPicked: value => History.job = value
                    }
                    UiDropdown {
                        width: (parent.width - 8) / 2
                        options: [{ "label": I18n.tr("history.allDays"), "value": "" }]
                            .concat(history.list ? history.list.dates.map(d => ({ "label": d, "value": d })) : [])
                        current: History.date
                        onPicked: value => History.date = value
                    }
                }
                // The layouts side by side, or as a list where their names
                // are too long for that (German, French, Russian ...).
                UiSegmented {
                    id: layouts
                    visible: implicitWidth <= parent.width
                    options: history.layouts
                    current: History.layout
                    onPicked: value => History.layout = value
                }
                UiDropdown {
                    visible: !layouts.visible
                    width: parent.width
                    options: history.layouts
                    current: History.layout
                    onPicked: value => History.layout = value
                }
                SwitchLine {
                    label: I18n.tr("history.showTrainingBattles")
                    checked: History.showTraining
                    onToggled: on => History.showTraining = on
                }
                SwitchLine {
                    label: I18n.tr("gameRecord.filter")
                    tip: I18n.tr("gameRecord.filterTip")
                    checked: History.onlyRecords
                    onToggled: on => History.onlyRecords = on
                }
            }
            Rectangle {
                id: listRule
                anchors.top: filters.bottom
                anchors.topMargin: 12
                width: parent.width
                height: 1
                color: Theme.hairline
            }

            Flickable {
                anchors.top: listRule.bottom
                anchors.bottom: parent.bottom
                width: parent.width
                contentHeight: list.implicitHeight + 12
                clip: true
                boundsBehavior: Flickable.StopAtBounds

                Column {
                    id: list
                    width: parent.width

                    UiText {
                        visible: History.ready && (!history.list || history.list.shown === 0)
                        x: 14
                        height: 40
                        text: I18n.tr("history.empty")
                        color: Theme.note
                    }

                    Repeater {
                        model: history.shownList.groups
                        Column {
                            id: group
                            required property var modelData
                            width: list.width

                            // The heading folds its group open and closed.
                            Item {
                                visible: group.modelData.title !== ""
                                width: parent.width
                                height: 30
                                Rectangle {
                                    anchors.fill: parent
                                    color: headHover.hovered ? Theme.hover : "transparent"
                                }
                                // The count right after the heading.
                                Row {
                                    x: 14
                                    width: parent.width - 28
                                    height: parent.height
                                    spacing: 8
                                    Chevron {
                                        anchors.bottom: parent.bottom
                                        anchors.bottomMargin: 9
                                        turn: group.modelData.folded ? -90 : 0
                                    }
                                    UiText {
                                        width: Math.min(implicitWidth, parent.width - 98)
                                        height: parent.height
                                        verticalAlignment: Text.AlignBottom
                                        bottomPadding: 4
                                        text: group.modelData.title
                                        muted: true
                                        font.pixelSize: Theme.sizeSmall
                                        font.weight: Font.DemiBold
                                    }
                                    UiText {
                                        height: parent.height
                                        verticalAlignment: Text.AlignBottom
                                        bottomPadding: 4
                                        text: group.modelData.count === 1 ? I18n.tr("history.fightCountOne")
                                            : I18n.tr("history.fightCount").replace("{n}", group.modelData.count)
                                        muted: true
                                        font.pixelSize: Theme.sizeSmall
                                    }
                                }
                                HoverHandler {
                                    id: headHover
                                    cursorShape: Qt.PointingHandCursor
                                }
                                TapHandler {
                                    onTapped: pages.toggle(group.modelData.title)
                                }
                            }

                            Repeater {
                                model: group.modelData.fights
                                FightItem {
                                    width: group.width
                                }
                            }
                        }
                    }

                    Item {
                        visible: history.shownList.more
                        width: parent.width
                        height: 48
                        UiButton {
                            anchors.centerIn: parent
                            text: I18n.tr("history.showMore")
                            onClicked: pages.more()
                        }
                    }
                }
            }
        }
        Rectangle {
            Layout.preferredWidth: 1
            Layout.fillHeight: true
            color: Theme.hairline
        }

        // Right: the chosen fight.
        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true

            // History is the overview; Details has the breakdown of the
            // chosen fight: every number, the DPS chart and the timeline.
            UiButton {
                id: detailsButton
                visible: !!history.fight
                anchors.right: parent.right
                anchors.rightMargin: Theme.gutter
                y: 14
                text: I18n.tr("details.title")
                onClicked: history.detailsRequested(history.fight.id)
            }

            Column {
                id: fightHead
                visible: !!history.fight
                x: Theme.gutter
                y: 14
                width: parent.width - 2 * Theme.gutter
                spacing: 4

                Row {
                    spacing: 8
                    UiText {
                        // Clear of the Details button on the right.
                        width: Math.min(implicitWidth, fightHead.width - detailsButton.width - 16
                            - (trainBadge.visible ? trainBadge.width + 8 : 0))
                        elide: Text.ElideRight
                        text: history.fight ? history.fight.boss : ""
                        font.pixelSize: Theme.sizeTitle
                        font.weight: Font.DemiBold
                    }
                    Badge {
                        id: trainBadge
                        anchors.verticalCenter: parent.verticalCenter
                        visible: !!history.fight && history.fight.train
                        text: I18n.tr("history.trainBadge")
                    }
                }
                UiText {
                    width: parent.width
                    text: history.fight ? [history.fight.dungeon, history.fight.date, history.players(history.fight)]
                        .filter(s => s !== "").join("  ·  ") : ""
                    muted: true
                }
                Item {
                    width: 1
                    height: 8
                }
                Row {
                    spacing: 28
                    Repeater {
                        model: history.fight ? [
                            [I18n.tr("details.stats.combatTime"), Format.clock(history.fight.battleMs / 1000)],
                            [I18n.tr("details.stats.totalDamage"), Format.amount(history.fight.damage)],
                            [I18n.tr("history.partyDps"), history.rate(history.fight.perSecond)],
                            [I18n.tr("details.stats.totalHealing"), Format.amount(history.fight.heal)],
                            [I18n.tr("details.stats.takenTotal"), history.fight.received ? Format.amount(history.fight.received.damage) : "-"],
                            [I18n.tr("details.stats.deaths"), history.count(history.fight.deaths)]
                        ] : []
                        Column {
                            required property var modelData
                            spacing: 2
                            UiText {
                                text: parent.modelData[0]
                                muted: true
                                font.pixelSize: Theme.sizeSmall
                            }
                            UiText {
                                text: parent.modelData[1]
                                numeric: true
                                font.pixelSize: Theme.sizeTitle
                                font.weight: Font.DemiBold
                            }
                        }
                    }
                }
                Item {
                    width: 1
                    height: 8
                }
                RowLayout {
                    width: parent.width
                    spacing: 8
                    UiSegmented {
                        framed: false
                        options: [
                            { "label": I18n.tr("meter.tabs.dps"), "value": 0 },
                            { "label": I18n.tr("meter.tabs.heal"), "value": 1 },
                            { "label": I18n.tr("meter.tabs.tank"), "value": 2 }
                        ].concat(history.records.length > 0 ? [{ "label": I18n.tr("gameRecord.title"), "value": 3 }] : [])
                        current: history.tab
                        onPicked: value => history.tab = value
                    }
                    Item {
                        Layout.fillWidth: true
                    }
                    // Uploading is always the player's own choice: no button
                    // here is drawn as the one to press. Once uploaded, the
                    // button opens the fight's log on a2tools.app.
                    UiButton {
                        visible: !!history.fight && !history.fight.train && !history.fight.live
                        // Busy uploading or previewing: greyed, as it takes no click.
                        enabled: !history.busy
                        text: history.fight && history.fight.uploaded ? I18n.tr("history.openLog") : I18n.tr("history.upload")
                        onClicked: {
                            if (history.fight.uploaded) {
                                const url = History.logUrl()
                                if (url !== "")
                                    Qt.openUrlExternally(url)
                            } else {
                                History.uploadChosen()
                            }
                        }
                        UiTip {
                            text: history.fight && history.fight.uploaded ? I18n.tr("history.uploadOpen") : I18n.tr("history.uploadTip")
                        }
                    }
                    // The meter keeps the link and never asks the site again,
                    // so a log deleted on a2tools.app still shows Open log:
                    // this sends the fight anyway, and its new link replaces
                    // the kept one.
                    UiButton {
                        visible: !!history.fight && !history.fight.train && !history.fight.live && history.fight.uploaded
                        enabled: !history.busy
                        text: I18n.tr("history.uploadAgain")
                        onClicked: History.uploadChosen()
                        UiTip {
                            text: I18n.tr("history.uploadAgainTip")
                        }
                    }
                    UiButton {
                        visible: !!history.fight && !history.fight.live
                        enabled: !history.busy
                        text: I18n.tr("history.previewUpload")
                        onClicked: History.previewChosen()
                        UiTip {
                            text: I18n.tr("history.previewRequestTip")
                        }
                    }
                    UiButton {
                        visible: !!history.fight && !history.fight.live
                        text: deleteStep.armed ? I18n.tr("history.deleteConfirm") : I18n.tr("history.delete")
                        onClicked: deleteStep.press()
                    }
                }
                UiText {
                    visible: text !== ""
                    width: parent.width
                    text: history.noteText(history.note)
                    color: history.note && history.note.kind === "failed" ? Theme.warning : Theme.note
                    font.pixelSize: Theme.sizeSmall
                    wrapMode: Text.WordWrap
                    textFormat: Text.PlainText
                }
            }
            Rectangle {
                id: fightRule
                visible: !!history.fight
                anchors.top: fightHead.bottom
                anchors.topMargin: 8
                width: parent.width
                height: 1
                color: Theme.hairline
            }

            Flickable {
                visible: !!history.fight
                anchors.top: fightRule.bottom
                anchors.bottom: recordLine.visible ? recordLine.top : parent.bottom
                width: parent.width
                contentHeight: fightBody.implicitHeight + 24
                clip: true
                boundsBehavior: Flickable.StopAtBounds

                Item {
                    id: fightBody
                    x: Theme.gutter
                    y: 10
                    width: parent.width - 2 * Theme.gutter
                    implicitHeight: history.tab === 3 ? record.implicitHeight
                        : partyTable.implicitHeight + (breakdown.visible ? 18 + breakdown.implicitHeight : 0)

                    UiTable {
                        id: partyTable
                        visible: history.tab !== 3
                        width: parent.width
                        rowHeight: 30
                        columns: [
                            [
                                { "key": "name", "label": I18n.tr("meter.columns.name"), "tip": I18n.tr("meter.columns.name") + "\n" + I18n.tr("meter.columnTips.name"), "width": 0, "icon": "class" },
                                { "key": "total", "label": I18n.tr("meter.columns.total"), "tip": I18n.tr("settings.encColumns.total") + "\n" + I18n.tr("meter.columnTips.total"), "width": 70, "right": true },
                                { "key": "rate", "label": I18n.tr("meter.columns.encDps"), "tip": I18n.tr("settings.columns.dps") + "\n" + I18n.tr("meter.columnTips.dps"), "width": 60, "right": true },
                                { "key": "share", "label": I18n.tr("meter.columns.pct"), "tip": I18n.tr("settings.encColumns.pct") + "\n" + I18n.tr("meter.columnTips.pct"), "width": 46, "right": true },
                                { "key": "crit", "label": I18n.tr("meter.columns.crit"), "tip": I18n.tr("settings.encColumns.crit") + "\n" + I18n.tr("meter.columnTips.crit"), "width": 46, "right": true },
                                { "key": "hits", "label": I18n.tr("meter.columns.hits"), "tip": I18n.tr("settings.encColumns.hits") + "\n" + I18n.tr("meter.columnTips.hits"), "width": 46, "right": true }
                            ],
                            [
                                { "key": "name", "label": I18n.tr("meter.columns.name"), "tip": I18n.tr("meter.columns.name") + "\n" + I18n.tr("meter.columnTips.name"), "width": 0, "icon": "class" },
                                { "key": "total", "label": I18n.tr("meter.columns.healed"), "tip": I18n.tr("settings.columns.healed") + "\n" + I18n.tr("meter.columnTips.healed"), "width": 70, "right": true },
                                { "key": "rate", "label": I18n.tr("meter.columns.hps"), "tip": I18n.tr("settings.columns.hps") + "\n" + I18n.tr("meter.columnTips.hps"), "width": 60, "right": true },
                                { "key": "share", "label": I18n.tr("meter.columns.pct"), "tip": I18n.tr("settings.columns.healPct") + "\n" + I18n.tr("meter.columnTips.healPct"), "width": 46, "right": true }
                            ],
                            [
                                { "key": "name", "label": I18n.tr("meter.columns.name"), "tip": I18n.tr("meter.columns.name") + "\n" + I18n.tr("meter.columnTips.name"), "width": 0, "icon": "class" },
                                { "key": "total", "label": I18n.tr("meter.columns.taken"), "tip": I18n.tr("settings.columns.taken") + "\n" + I18n.tr("meter.columnTips.taken"), "width": 70, "right": true },
                                { "key": "rate", "label": I18n.tr("meter.columns.dtps"), "tip": I18n.tr("settings.columns.dtps") + "\n" + I18n.tr("meter.columnTips.dtps"), "width": 60, "right": true },
                                { "key": "share", "label": I18n.tr("meter.columns.pct"), "tip": I18n.tr("settings.columns.takenPct") + "\n" + I18n.tr("meter.columnTips.takenPct"), "width": 46, "right": true },
                                { "key": "block", "label": I18n.tr("details.skills.shieldBlock"), "tip": I18n.tr("settings.columns.block") + "\n" + I18n.tr("meter.columnTips.block"), "width": 46, "right": true },
                                { "key": "parry", "label": I18n.tr("details.skills.parry"), "tip": I18n.tr("settings.columns.parry") + "\n" + I18n.tr("meter.columnTips.parry"), "width": 46, "right": true },
                                { "key": "deaths", "label": I18n.tr("details.stats.deaths"), "width": 54, "right": true }
                            ],
                            []
                        ][history.tab]
                        rows: {
                            if (!history.fight || history.tab === 3)
                                return []
                            const list = [history.fight.dps, history.fight.healRows, history.fight.tank][history.tab]
                            return list.map(r => ({
                                "name": I18n.playerName(r), "job": r.job, "fill": r.fill,
                                "total": history.noData.tankRow(r, Format.amount(r.amount)), "rate": history.noData.tankRow(r, history.rate(r.perSecond)),
                                "share": history.noData.tankRow(r, Format.share(r.share)), "crit": r.critRate === undefined ? "" : history.noData.byHits(r.hits, Format.pct(r.critRate)),
                                "hits": r.hits === undefined ? "" : history.noData.byHits(r.hits, String(r.hits)),
                                "block": history.noData.tankRow(r, String(r.shieldBlock)), "parry": history.noData.tankRow(r, String(r.parry)), "deaths": history.count(r.deaths)
                            }))
                        }
                        barFill: row => row.fill
                        // A click opens that player's skills, heals or damage
                        // received below; on the open one, closes it.
                        selected: history.playerRow ? history.partyRows.indexOf(history.playerRow) : -1
                        clickable: true
                        onRowClicked: index => {
                            const id = history.partyRows[index].id
                            if (history.player && history.player.id === id)
                                History.closePlayer()
                            else
                                History.openPlayer(id)
                        }
                    }

                    // The opened player, as Details shows them picked: DPS
                    // their skills, Heal their heals, Tank what hit them.
                    Column {
                        id: breakdown
                        visible: history.tab !== 3 && !!history.playerRow
                        y: partyTable.implicitHeight + 18
                        width: parent.width
                        spacing: 6
                        UiText {
                            text: history.playerRow ? I18n.playerName(history.playerRow) : ""
                            font.weight: Font.DemiBold
                        }
                        UiTable {
                            width: parent.width
                            rowHeight: 26
                            columns: [
                                [
                                    { "key": "name", "label": I18n.tr("details.skills.skill"), "width": 0, "icon": "skill" },
                                    { "key": "amount", "label": I18n.tr("details.skills.dmg"), "tip": I18n.tr("details.skills.dmgTooltip") + "\n" + I18n.tr("details.skillTips.dmg"), "width": 64, "right": true },
                                    { "key": "share", "label": I18n.tr("details.skills.dmgPct"), "tip": I18n.tr("details.skills.dmgPctTooltip") + "\n" + I18n.tr("details.skillTips.dmgPct"), "width": 52, "right": true },
                                    { "key": "hits", "label": I18n.tr("details.skills.hits"), "tip": I18n.tr("details.skills.hitsTooltip") + "\n" + I18n.tr("details.skillTips.hits"), "width": 48, "right": true },
                                    { "key": "crit", "label": I18n.tr("details.skills.crit"), "tip": I18n.tr("details.skills.critTooltip") + "\n" + I18n.tr("details.skillTips.crit"), "width": 48, "right": true },
                                    { "key": "avg", "label": I18n.tr("details.skills.avgDmg"), "tip": I18n.tr("details.skills.avgDmgTooltip") + "\n" + I18n.tr("details.skillTips.avgDmg"), "width": 56, "right": true }
                                ],
                                [
                                    { "key": "name", "label": I18n.tr("details.skills.skill"), "width": 0, "icon": "skill" },
                                    { "key": "amount", "label": I18n.tr("details.skills.heal"), "tip": I18n.tr("details.skills.healTooltip") + "\n" + I18n.tr("details.skillTips.heal"), "width": 64, "right": true },
                                    { "key": "share", "label": I18n.tr("details.skills.healPct"), "tip": I18n.tr("details.skills.healPctTooltip") + "\n" + I18n.tr("details.skillTips.healPct"), "width": 52, "right": true },
                                    { "key": "hits", "label": I18n.tr("details.skills.ticks"), "tip": I18n.tr("details.skills.ticksTooltip") + "\n" + I18n.tr("details.skillTips.ticks"), "width": 48, "right": true },
                                    { "key": "avg", "label": I18n.tr("details.skills.avgDmg"), "tip": I18n.tr("details.skills.avgHealTooltip") + "\n" + I18n.tr("details.skillTips.avgHeal"), "width": 56, "right": true }
                                ],
                                [
                                    { "key": "name", "label": I18n.tr("details.skills.mechanic"), "tip": I18n.tr("details.skills.mechanic") + "\n" + I18n.tr("details.skillTips.mechanic"), "width": 0 },
                                    { "key": "amount", "label": I18n.tr("details.skills.taken"), "tip": I18n.tr("details.skills.takenTooltip") + "\n" + I18n.tr("details.skillTips.taken"), "width": 64, "right": true },
                                    { "key": "share", "label": I18n.tr("details.skills.takenPct"), "tip": I18n.tr("details.skills.takenPctTooltip") + "\n" + I18n.tr("details.skillTips.takenPct"), "width": 52, "right": true },
                                    { "key": "hits", "label": I18n.tr("details.skills.hitsTaken"), "tip": I18n.tr("details.skills.hitsTakenTooltip") + "\n" + I18n.tr("details.skillTips.hitsTaken"), "width": 48, "right": true }
                                ],
                                []
                            ][history.tab]
                            rows: {
                                const p = history.player
                                if (!p || history.tab === 3)
                                    return []
                                if (history.tab === 2)
                                    return p.taken.map(r => ({ "name": r.source ? r.name + " · " + r.source : r.name, "fill": r.share / 100,
                                        "amount": Format.rate(r.damage), "share": Format.pct1(r.share), "hits": String(r.attacks) }))
                                const list = history.tab === 1 ? p.heals : p.skills
                                return list.map(r => ({ "name": r.name, "code": r.code, "job": r.job, "fill": r.fill,
                                    "amount": Format.rate(r.amount), "share": Format.pct1(r.share), "hits": String(r.hits),
                                    "crit": history.noData.skillCell(r, "crit", r.rates.crit + "%"),
                                    "avg": history.noData.skillCell(r, "avgdmg", Format.rate(r.avg)) }))
                            }
                            barFill: row => row.fill
                        }
                    }

                    GameRecordView {
                        id: record
                        visible: history.tab === 3
                        width: parent.width
                        records: history.records
                    }
                }
            }

            // The fight's latest game record in one line, under the party table.
            Rectangle {
                id: recordLine
                visible: !!history.lastRecord && history.tab !== 3
                anchors.bottom: parent.bottom
                anchors.bottomMargin: 12
                x: Theme.gutter
                width: parent.width - 2 * Theme.gutter
                height: 36
                radius: 4
                color: "transparent"
                border.width: 1
                border.color: Theme.hairline
                Row {
                    x: 10
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 10
                    Badge {
                        id: recordBadge
                        anchors.verticalCenter: parent.verticalCenter
                        text: I18n.tr("gameRecord.title")
                    }
                    UiText {
                        anchors.verticalCenter: parent.verticalCenter
                        // What is left of the line, so a long language elides instead of running out of it.
                        width: recordLine.width - 30 - recordBadge.width
                        readonly property var r: history.lastRecord
                        text: !r ? "" : !r.compared
                            ? I18n.tr("gameRecord.noComparison")
                            : record.verdictOf(r) + "  ·  " + I18n.tr("gameRecord.damageGame") + " " + Format.whole(r.gameTotal)
                                + "  ·  " + I18n.tr("gameRecord.damageMeter") + " " + Format.whole(r.meterTotal)
                        muted: true
                    }
                }
            }
        }
    }

    // One saved fight in the list.
    component FightItem: Item {
        id: item
        required property var modelData
        readonly property bool chosen: modelData.id === History.chosen
        readonly property bool byBoss: History.layout === "grouped"
        height: 52

        Rectangle {
            anchors.fill: parent
            color: item.chosen ? Theme.accentTint : (itemHover.hovered ? Theme.hover : "transparent")
        }
        Rectangle {
            visible: item.chosen
            width: 2
            height: parent.height
            color: Theme.accent
        }
        // The fight's DPS right after its name, and yours at the end of the
        // second line, so no number sits across the list from its fight.
        Column {
            x: 14
            anchors.verticalCenter: parent.verticalCenter
            width: parent.width - 28
            spacing: 4
            Row {
                spacing: 6
                width: parent.width
                UiText {
                    text: item.byBoss ? item.modelData.date : item.modelData.boss
                    font.weight: Font.DemiBold
                    width: Math.min(implicitWidth, parent.width - 150)
                }
                UiText {
                    numeric: true
                    text: history.rate(item.modelData.perSecond)
                    font.weight: Font.DemiBold
                    leftPadding: 4
                    rightPadding: 4
                }
                Badge {
                    anchors.verticalCenter: parent.verticalCenter
                    visible: item.modelData.live
                    lit: true
                    text: I18n.tr("history.liveBadge")
                }
                // The dungeon heading says it already.
                Badge {
                    anchors.verticalCenter: parent.verticalCenter
                    visible: !!item.modelData.tier && History.layout !== "dungeon"
                    text: item.modelData.tier ? item.modelData.tier.label : ""
                }
                Badge {
                    anchors.verticalCenter: parent.verticalCenter
                    visible: item.modelData.train
                    text: I18n.tr("history.trainBadge")
                }
                Badge {
                    anchors.verticalCenter: parent.verticalCenter
                    visible: !!item.modelData.record
                    text: I18n.tr("gameRecord.title")
                    tip: history.recordState(item.modelData.record)
                }
                // Uploaded: 1.x's link mark, which opens the fight's log.
                LinkMark {
                    anchors.verticalCenter: parent.verticalCenter
                    visible: item.modelData.uploaded
                    UiTip {
                        text: I18n.tr("history.uploadOpen")
                    }
                    TapHandler {
                        onTapped: {
                            History.choose(item.modelData.id)
                            const url = History.logUrl()
                            if (url !== "")
                                Qt.openUrlExternally(url)
                        }
                    }
                }
            }
            // Up to 12 class icons: as many as there is room for.
            Row {
                spacing: 8
                width: parent.width
                clip: true
                UiText {
                    height: 14
                    text: (item.byBoss ? "" : item.modelData.date + "  ·  ") + Format.clock(item.modelData.durationMs / 1000)
                    muted: true
                    font.pixelSize: Theme.sizeSmall
                }
                UiText {
                    height: 14
                    numeric: true
                    muted: true
                    font.pixelSize: Theme.sizeSmall
                    text: item.modelData.you ? I18n.tr("history.yourRank").replace("{dps}", history.rate(item.modelData.you.perSecond))
                        .replace("{rank}", item.modelData.you.rank) : Format.amount(item.modelData.totalDamage)
                }
                Row {
                    spacing: 2
                    anchors.verticalCenter: parent.verticalCenter
                    Repeater {
                        model: item.modelData.jobs
                        Image {
                            required property string modelData
                            width: 13
                            height: 13
                            source: Theme.classIcon(modelData)
                            mipmap: true
                            opacity: 0.85
                        }
                    }
                }
            }
        }
        HoverHandler {
            id: itemHover
            cursorShape: Qt.PointingHandCursor
        }
        TapHandler {
            onTapped: History.choose(item.modelData.id)
        }
    }
}
