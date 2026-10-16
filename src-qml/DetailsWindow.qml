// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Qt.labs.platform as Platform
import Daevalog

// One player in one fight (Details! breakdown layout): skills on the left,
// the player's numbers and the chosen skill's hits on the right, targets
// below, then the DPS chart or the timeline. DMG, HEAL and TAKEN pick the
// numbers; with All players, TAKEN lists the boss's mechanics and who they
// hit. Every number is the Details object's; the window only lays them out.
AppWindow {
    id: details
    placeName: "details"

    // The backend's view of the pick, read once per change.
    readonly property var view: Details.view !== "" ? JSON.parse(Details.view) : null
    readonly property bool allPlayers: !view || view.player === 0
    // The player picked, as the Player list names them.
    readonly property var player: allPlayers ? null : entry(view.player)
    // "dmg", "heal" or "taken".
    property string mode: "dmg"
    // "chart" or "timeline".
    property string lower: "chart"
    property bool optionsOpen: false
    // The chosen skill row, by what it is: the list is read again every 2 s
    // and sorted by amount, so an index would land on another skill. Empty,
    // or a row that is gone: the top row.
    property string selectedKey: ""
    function rowKey(row) {
        return row ? row.code + "/" + (row.sourceCode || 0) + "/" + row.name : ""
    }
    readonly property int selectedSkill: Math.max(0, shownRows.findIndex(r => rowKey(r) === selectedKey))
    // Each table's order, {key, ascending}, for the session, as 1.x keeps
    // it: "dmg", "heal", "taken" and "mechanics" have their own columns.
    property var sorts: ({})
    readonly property string tableName: source.table
    readonly property var sort: sorts[tableName] || { "key": mode === "dmg" ? "dmg" : (mode === "heal" ? "heal" : "taken"), "ascending": false }
    function sortBy(key) {
        const next = Object.assign({}, sorts)
        next[tableName] = order.clicked(sort, key)
        sorts = next
    }
    // The skills opened on their damage over time, by name, as 1.x.
    property var openDots: ({})
    property bool openEveryDot: false
    function everyDot() {
        const all = {}
        for (const row of skills)
            all[row.name] = true
        return all
    }
    function fold(name) {
        const next = Object.assign({}, openDots)
        if (next[name])
            delete next[name]
        else
            next[name] = true
        openDots = next
    }
    readonly property SkillOrder order: SkillOrder {}
    readonly property NoData noData: NoData {}
    // The meter's window, for a screenshot with the meter (Main.qml).
    property var meterWindow: null
    // What the last Screenshot did, shown for 2 s; and the file it wrote.
    property string shotNote: ""
    property string shotFile: ""

    // The Screenshot button: to the clipboard, the meter beside Details and
    // a PNG in the folder as the options say, as 1.x.
    function screenshot() {
        const result = JSON.parse(Overlay.capture(details, Settings.screenshotWithMeter ? meterWindow : null,
            Settings.screenshotToFolder, Theme.window))
        if (!result.clipboard)
            shotNote = I18n.tr("details.screenshot.failed")
        else if (result.file !== "")
            shotNote = I18n.tr("details.screenshot.savedToBoth")
        else if (Settings.screenshotToFolder)
            shotNote = I18n.tr("details.screenshot.fileFailed")
        else
            shotNote = I18n.tr("details.screenshot.savedToClipboard")
        shotFile = result.file
        shotNoteTimer.restart()
    }
    Timer {
        id: shotNoteTimer
        interval: 2000
        onTriggered: details.shotNote = ""
    }
    // The desktop's own folder picker (Qt.labs.platform, which the tray
    // uses too).
    Platform.FolderDialog {
        id: folderDialog
        // On Windows a folder URL is file:///C:/..., the folder C:\...
        readonly property bool windowsPaths: Qt.platform.os === "windows"
        currentFolder: Settings.screenshotPlace === "" ? ""
            : windowsPaths ? "file:///" + Settings.screenshotPlace.replace(/\\/g, "/")
            : "file://" + Settings.screenshotPlace
        onAccepted: {
            const url = folder.toString()
            if (windowsPaths && url.startsWith("file:///"))
                Settings.screenshotFolder = decodeURIComponent(url.slice(8)).replace(/\//g, "\\")
            else if (url.startsWith("file://"))
                Settings.screenshotFolder = decodeURIComponent(url.slice(7))
        }
    }

    // The Details text size setting moves the whole type scale with it.
    readonly property int body: Settings.detailsFontSize
    readonly property int small: body - 2
    readonly property int titleSize: body + 2
    // The Player and Fight vs lists and their labels, as tall as the text needs.
    readonly property int fieldHeight: Math.max(28, body + 15)

    readonly property bool mechanics: source.table === "mechanics"
    // The Overview's column, wider for larger text.
    readonly property int overviewWidth: Math.max(280, Math.round(280 * body / Theme.sizeRow))
    readonly property bool overviewBeside: skillTable.fitWidth <= bodyColumn.width - overviewWidth - 20
    // The skill table's rows and the table they are for, both read from the
    // mode and the view in one binding: a binding that read them from two
    // could see the new table with the rows from before. Each shown row
    // keeps its `table` for the same reason.
    readonly property var source: {
        const table = mode === "taken" && (!view || view.player === 0) ? "mechanics" : mode
        return { "table": table, "rows": !view ? [] : (table === "mechanics" ? view.mechanics
            : (table === "dmg" ? view.damage.skills : (table === "heal" ? view.heal.skills : view.taken.rows))) }
    }
    readonly property var skills: source.rows
    // The table's rows: sorted, with the opened skills' damage over time.
    // One player's DMG and HEAL bars in that player's class colour: a
    // summon's skill has no class and would take the grey kept for
    // monsters. TAKEN rows are monsters'.
    readonly property var shownRows: order.arrange(
        player && (source.table === "dmg" || source.table === "heal") ? skills.map(r => Object.assign({}, r, { "job": player.job })) : skills,
        source.table, sort, tableColumns.map(c => c.key), openEveryDot ? everyDot() : openDots)
    readonly property var skill: shownRows[selectedSkill] || null
    readonly property real seconds: view ? view.damage.stats.battleMs / 1000 : 0
    readonly property var received: view ? view.taken.received : null
    readonly property real total: !view ? 0 : (mode === "dmg" ? view.damage.stats.damage
        : (mode === "heal" ? view.heal.stats.heal : (received ? received.damage : 0)))
    readonly property var perSecond: !view ? null : (mode === "dmg" ? view.damage.stats.perSecond
        : (mode === "heal" ? view.heal.stats.perSecond : view.taken.perSecond))

    // A player of the fight by id, as the backend lists them.
    function entry(id) {
        for (const p of view.players) {
            if (p.id === id)
                return p
        }
        return { "id": id, "name": String(id), "job": "", "number": 0, "you": false, "identifying": true, "unattributed": false }
    }
    function rate(v) {
        return v === null || v === undefined ? "-" : Format.rate(v)
    }

    // Live: read again every 2 s, as 1.x did; a replay once more at its end.
    // A saved fight does not change; paused, Details holds as the meter does.
    Timer {
        interval: 2000
        repeat: true
        running: details.visible && !Capture.suspended && !(Meter.replaying && Meter.replayDone) && !(details.view && details.view.saved)
        onTriggered: Details.refresh()
    }
    Connections {
        target: Meter
        function onReplayDoneChanged() {
            Details.refresh()
        }
    }
    // The language changed: the names again at once, a saved fight's too,
    // as 1.x redraws.
    Connections {
        target: Settings
        function onLanguageChanged() {
            if (details.visible)
                Details.refresh()
        }
    }

    title: I18n.tr("details.title")
    width: 1000
    height: 760
    minimumWidth: 760
    minimumHeight: 480
    color: Theme.window

    onPageChanged: {
        // --open details/<page>
        if (page === "heal" || page === "taken" || page === "dmg")
            mode = page
        else if (page === "mechanics")
            mode = "taken"
        else if (page === "timeline" || page === "row-timeline" || page === "saved-timeline") {
            lower = "timeline"
            scroller.contentY = Qt.binding(() => Math.max(0, scroller.contentHeight - scroller.height))
        } else if (page === "options")
            optionsOpen = true
        else if (page === "monitor")
            showMonitorBadge(true)
        else if (page === "sorted") {
            // For a shot: sorted by hits, every skill opened on its damage
            // over time.
            sorts = { "dmg": { "key": "hit", "ascending": false } }
            openEveryDot = true
        }
    }

    // A head's tooltip: the column's full name, what it shows, and for the
    // rates that leave DoT hits out, the note that says so.
    function tipOf(column) {
        return [column.name, column.about || "", column.dot ? I18n.tr("details.tooltip") : ""].filter(t => t !== "").join("\n")
    }
    function withTips(columns) {
        return columns.map(c => Object.assign({ "tip": tipOf(c) }, c))
    }
    // Each row's share bar, against the biggest row (full), as the meter.
    function withFill(rows) {
        const top = rows.reduce((m, r) => Math.max(m, r.amount || 0), 0)
        return rows.map(r => Object.assign({ "fill": top > 0 ? (r.amount || 0) / top : 0 }, r))
    }

    // The skill table's columns. DMG takes the Skill columns chosen in the
    // options; HEAL and TAKEN have their own short sets. `name` is the
    // column's full name (the options list it), `about` what it shows (the
    // options show it as the name's tooltip).
    readonly property var dmgColumns: withTips([
        { "key": "hit", "label": I18n.tr("details.skills.hits"), "name": I18n.tr("details.skills.hitsTooltip"), "about": I18n.tr("details.skillTips.hits") },
        { "key": "dmg", "label": I18n.tr("details.skills.dmg"), "name": I18n.tr("details.skills.dmgTooltip"), "about": I18n.tr("details.skillTips.dmg") },
        { "key": "dmgpct", "label": I18n.tr("details.skills.dmgPct"), "name": I18n.tr("details.skills.dmgPctTooltip"), "about": I18n.tr("details.skillTips.dmgPct") },
        { "key": "mhit", "label": I18n.tr("details.skills.multiHit"), "name": I18n.tr("details.skills.multiHitTooltip"), "about": I18n.tr("details.skillTips.multiHit") },
        { "key": "mdmg", "label": I18n.tr("details.skills.multiHitDamage"), "name": I18n.tr("details.skills.multiHitDamageTooltip"), "about": I18n.tr("details.skillTips.multiHitDamage") },
        { "key": "crit", "label": I18n.tr("details.skills.crit"), "name": I18n.tr("details.skills.critTooltip"), "about": I18n.tr("details.skillTips.crit"), "dot": true },
        { "key": "parry", "label": I18n.tr("details.skills.parry"), "name": I18n.tr("details.skills.parryTooltip"), "about": I18n.tr("details.skillTips.parry") },
        { "key": "perfect", "label": I18n.tr("details.skills.perfect"), "name": I18n.tr("details.skills.perfectTooltip"), "about": I18n.tr("details.skillTips.perfect"), "dot": true },
        { "key": "double", "label": I18n.tr("details.skills.double"), "name": I18n.tr("details.skills.doubleTooltip"), "about": I18n.tr("details.skillTips.double"), "dot": true },
        { "key": "back", "label": I18n.tr("details.skills.back"), "name": I18n.tr("details.skills.backTooltip"), "about": I18n.tr("details.skillTips.back"), "dot": true },
        { "key": "frontal", "label": I18n.tr("details.skills.frontal"), "name": I18n.tr("details.skills.frontalTooltip"), "about": I18n.tr("details.skillTips.frontal") },
        { "key": "block", "label": I18n.tr("details.skills.shieldBlock"), "name": I18n.tr("details.skills.shieldBlockTooltip"), "about": I18n.tr("details.skillTips.shieldBlock") },
        { "key": "perfectblock", "label": I18n.tr("details.skills.perfectBlock"), "name": I18n.tr("details.skills.perfectBlockTooltip"), "about": I18n.tr("details.skillTips.perfectBlock") },
        { "key": "ironwall", "label": I18n.tr("details.skills.ironWall"), "name": I18n.tr("details.skills.ironWallTooltip"), "about": I18n.tr("details.skillTips.ironWall") },
        { "key": "regeneration", "label": I18n.tr("details.skills.regeneration"), "name": I18n.tr("details.skills.regenerationTooltip"), "about": I18n.tr("details.skillTips.regeneration") },
        { "key": "miss", "label": I18n.tr("details.skills.miss"), "name": I18n.tr("details.skills.missTooltip"), "about": I18n.tr("details.skillTips.miss") },
        { "key": "resist", "label": I18n.tr("details.skills.resist"), "name": I18n.tr("details.skills.resistTooltip"), "about": I18n.tr("details.skillTips.resist") },
        { "key": "regen", "label": I18n.tr("details.skills.regen"), "name": I18n.tr("details.skills.regenTooltip"), "about": I18n.tr("details.skillTips.regen") },
        { "key": "mindmg", "label": I18n.tr("details.skills.minDmg"), "name": I18n.tr("details.skills.minDmgTooltip"), "about": I18n.tr("details.skillTips.minDmg") },
        { "key": "avgdmg", "label": I18n.tr("details.skills.avgDmg"), "name": I18n.tr("details.skills.avgDmgTooltip"), "about": I18n.tr("details.skillTips.avgDmg") },
        { "key": "maxdmg", "label": I18n.tr("details.skills.maxDmg"), "name": I18n.tr("details.skills.maxDmgTooltip"), "about": I18n.tr("details.skillTips.maxDmg") }
    ])
    // The columns chosen in the options, as the Settings object keeps them.
    readonly property var chosenColumns: JSON.parse(Settings.skillColumns)
    // Damage first, then the chosen ones in the 1.x order.
    readonly property var shownDmgColumns: {
        const first = ["dmg", "dmgpct", "hit"]
        const chosen = chosenColumns
        const list = []
        for (const key of first) {
            if (chosen.indexOf(key) >= 0)
                list.push(dmgColumns.find(c => c.key === key))
        }
        for (const column of dmgColumns) {
            if (first.indexOf(column.key) < 0 && chosen.indexOf(column.key) >= 0)
                list.push(column)
        }
        // A chosen column shows even when it is 0 on every row, as 1.x.
        return list
    }
    readonly property var nameColumn: mechanics ? {
        "key": "name", "label": I18n.tr("details.skills.mechanic"),
        "tip": I18n.tr("details.skills.mechanic") + "\n" + I18n.tr("details.skillTips.mechanic"), "width": 0, "icon": ""
    } : {
        "key": "name", "label": I18n.tr("details.skills.skill"), "width": 0, "icon": "skill",
        "tip": I18n.tr("details.skills.skillTooltip") + "\n" + (mode === "heal" ? I18n.tr("details.skillTips.healSkill")
            : (mode === "taken" ? I18n.tr("details.skillTips.takenSkill") : I18n.tr("details.skillTips.skill")))
    }
    readonly property var tableColumns: {
        let list
        if (mode === "dmg")
            list = shownDmgColumns
        else if (mode === "heal")
            list = withTips([
                { "key": "heal", "label": I18n.tr("details.skills.heal"), "name": I18n.tr("details.skills.healTooltip"), "about": I18n.tr("details.skillTips.heal") },
                { "key": "healpct", "label": I18n.tr("details.skills.healPct"), "name": I18n.tr("details.skills.healPctTooltip"), "about": I18n.tr("details.skillTips.healPct") },
                { "key": "ticks", "label": I18n.tr("details.skills.ticks"), "name": I18n.tr("details.skills.ticksTooltip"), "about": I18n.tr("details.skillTips.ticks") },
                { "key": "avgheal", "label": I18n.tr("details.skills.avgDmg"), "name": I18n.tr("details.skills.avgHealTooltip"), "about": I18n.tr("details.skillTips.avgHeal") }
            ])
        else if (mechanics)
            list = takenColumns.concat(withTips([
                { "key": "players", "label": I18n.tr("details.skills.playersHit"), "name": I18n.tr("details.skills.playersHit"), "about": I18n.tr("details.skills.playersHitTooltip"), "width": 80 }
            ]))
        else
            // The hit results any row has, as 1.x shows them.
            list = takenColumns.concat(view ? view.taken.results.map(key => resultColumns[key]) : [])
        return [nameColumn].concat(list.map(c => Object.assign({ "width": c.key === "dmg" || c.key === "heal" || c.key === "taken" ? 64 : 48, "right": true }, c)))
    }

    // Damage taken: its numbers, then (one player) the hit results.
    readonly property var takenColumns: withTips([
        { "key": "taken", "label": I18n.tr("details.skills.taken"), "name": I18n.tr("details.skills.takenTooltip"), "about": I18n.tr("details.skillTips.taken") },
        { "key": "takenpct", "label": I18n.tr("details.skills.takenPct"), "name": I18n.tr("details.skills.takenPctTooltip"), "about": I18n.tr("details.skillTips.takenPct") },
        { "key": "hits", "label": I18n.tr("details.skills.hitsTaken"), "name": I18n.tr("details.skills.hitsTakenTooltip"), "about": I18n.tr("details.skillTips.hitsTaken") }
    ])
    // Damage received's hit results: their columns, by the backend's names.
    readonly property var resultColumns: {
        const list = withTips([
            { "key": "shieldBlock", "label": I18n.tr("details.skills.shieldBlock"), "name": I18n.tr("details.skills.shieldBlockTooltip"), "about": I18n.tr("details.skillTips.shieldBlock") },
            { "key": "parry", "label": I18n.tr("details.skills.parry"), "name": I18n.tr("details.skills.parryTooltip"), "about": I18n.tr("details.skillTips.parry") },
            { "key": "perfectBlock", "label": I18n.tr("details.skills.perfectBlock"), "name": I18n.tr("details.skills.perfectBlockTooltip"), "about": I18n.tr("details.skillTips.perfectBlock") },
            { "key": "ironWall", "label": I18n.tr("details.skills.ironWall"), "name": I18n.tr("details.skills.ironWallTooltip"), "about": I18n.tr("details.skillTips.ironWall") },
            { "key": "regeneration", "label": I18n.tr("details.skills.regeneration"), "name": I18n.tr("details.skills.regenerationTooltip"), "about": I18n.tr("details.skillTips.regeneration") }
        ])
        const byKey = {}
        for (const column of list)
            byKey[column.key] = column
        return byKey
    }
    // A skill column's rate in the row's `rates`.
    readonly property var rateOf: ({
        "mhit": "multiHit", "crit": "crit", "parry": "parry", "perfect": "perfect", "double": "double", "back": "back",
        "frontal": "front", "block": "shieldBlock", "perfectblock": "perfectBlock", "ironwall": "ironWall", "regeneration": "regeneration"
    })

    // By the row's own table: right after a change of mode or player the
    // table can still hold the rows from before.
    function cell(row, key) {
        if (key === "name")
            return row.name
        if (row.table === "dmg" || row.table === "heal")
            return noData.skillCell(row, key, number(row, key))
        return number(row, key)
    }
    function number(row, key) {
        if (row.table === "taken" || row.table === "mechanics") {
            switch (key) {
            case "taken": return Format.rate(row.damage)
            case "takenpct": return Format.pct1(row.share)
            case "hits": return String(row.attacks)
            case "players": return String(row.players)
            }
            // A hit result's share of the hits; empty where it has none.
            const share = row.rates ? row.rates[key] : null
            return share === null || share === undefined ? "" : share + "%"
        }
        switch (key) {
        case "dmg":
        case "heal": return Format.rate(row.amount)
        case "dmgpct":
        case "healpct": return Format.pct1(row.share)
        case "hit":
        case "ticks": return String(row.hits)
        case "mdmg": return Format.rate(row.multiHitDamage)
        case "regen": return Format.rate(row.regen)
        case "mindmg": return Format.rate(row.min)
        case "avgdmg":
        case "avgheal": return Format.rate(row.avg)
        case "maxdmg": return Format.rate(row.max)
        case "miss": return String(row.miss)
        case "resist": return String(row.resist)
        }
        return row.rates[rateOf[key]] + "%"
    }


    // The overview: [label key, value] for the tab.
    readonly property var stats: {
        if (!view)
            return []
        if (mode === "heal") {
            const h = view.heal.stats
            return [["totalHealing", Format.whole(h.heal)], ["hps", rate(h.perSecond)], ["combatTime", Format.clock(h.battleMs / 1000)],
                ["healTicks", Format.whole(h.ticks)], ["hotTicks", Format.whole(h.hotTicks)], ["healSkills", String(h.skills)]]
                .map(p => [p[0], noData.healStat(p[0], h, p[1])])
        }
        if (mode === "taken") {
            const t = view.taken
            return [["takenTotal", received ? Format.whole(received.damage) : "-"], ["dtps", rate(t.perSecond)],
                ["takenHits", received ? Format.whole(received.attacks) : "-"]].concat(
                t.results.map(key => [key, Format.pct1(t.rates[key])]))
                .map(p => [p[0], noData.takenStat(received, p[1])])
        }
        const s = view.damage.stats
        const records = view.damage.skills.length > 0
        return [["totalDamage", Format.whole(s.damage)], ["contribution", s.contribution === null ? "-" : Format.pct1(s.contribution)],
            ["hits", Format.whole(s.hits)], ["critRate", Format.pct1(s.critRate)], ["perfectRate", Format.pct1(s.perfectRate)],
            ["doubleRate", Format.pct1(s.doubleRate)], ["backRate", Format.pct1(s.backRate)], ["parryRate", Format.pct1(s.parryRate)],
            ["combatTime", Format.clock(s.battleMs / 1000)], ["multiHitDamage", Format.whole(s.multiHitDamage)],
            ["multiHitHits", Format.pct1(s.multiHitRate)], ["maxHp", s.maxHp > 0 ? Format.whole(s.maxHp) : "-"], ["regen", Format.whole(s.regen)],
            ["deaths", s.deaths === null ? "-" : Format.whole(s.deaths)]]
            .map(p => [p[0], noData.damageStat(p[0], s, records, p[1])])
    }
    // The DMG overview's "-" for hits and their rates: the game sent only
    // damage over time ticks.
    readonly property bool ticksOnly: !!view && mode === "dmg" && view.damage.stats.hits === 0 && view.damage.stats.damage > 0

    readonly property string playerLabel: player ? I18n.playerName(player) : I18n.tr("details.history.allPlayers")
    readonly property string classLabel: {
        switch (player ? player.job : "") {
        case "gladiator": return I18n.tr("classes.GLADIATOR")
        case "templar": return I18n.tr("classes.TEMPLAR")
        case "ranger": return I18n.tr("classes.RANGER")
        case "assassin": return I18n.tr("classes.ASSASSIN")
        case "sorcerer": return I18n.tr("classes.SORCERER")
        case "spiritmaster": return I18n.tr("classes.ELEMENTALIST")
        case "cleric": return I18n.tr("classes.CLERIC")
        case "chanter": return I18n.tr("classes.CHANTER")
        case "fighter": return I18n.tr("classes.FIGHTER")
        }
        return ""
    }
    readonly property string summary: {
        if (!view)
            return ""
        const amount = Format.amount(total)
        const rate = details.rate(perSecond)
        let numbers
        if (mode === "dmg")
            numbers = I18n.tr("details.summary.damage")
        else if (mode === "heal")
            numbers = I18n.tr("details.summary.heal")
        else
            numbers = I18n.tr("details.summary.taken")
        numbers = numbers.replace("{amount}", amount).replace("{rate}", rate)
        const fight = I18n.tr("details.summary.fight").replace("{time}", Format.clock(seconds)).replace("{target}", view.title || "#" + view.target)
        return (allPlayers ? [numbers, fight] : [classLabel, numbers, fight]).join("  ·  ")
    }

    component Heading: UiText {
        font.pixelSize: details.small
        font.weight: Font.DemiBold
        muted: true
    }

    // A value and its label on one line: the value first, right-aligned in
    // a column as wide as its list's widest value, the label right after
    // it, so the eye never crosses the column for a number (Visual rules).
    component Pair: Item {
        id: pair
        property string label
        property string value
        property real valueWidth: 60
        // Shown when the pointer rests on the value.
        property string tip: ""
        width: parent ? parent.width : 140
        height: details.body + 7
        UiText {
            width: pair.valueWidth
            height: parent.height
            horizontalAlignment: Text.AlignRight
            numeric: true
            text: pair.value
            font.pixelSize: details.body
            UiTip {
                text: pair.tip
            }
        }
        UiText {
            x: pair.valueWidth + 10
            width: parent.width - x
            height: parent.height
            text: pair.label
            muted: true
            font.pixelSize: details.body
        }
    }

    // The width of a Pair list's value column: its widest value.
    FontMetrics {
        id: valueMetrics
        font.family: Theme.font
        font.pixelSize: details.body
        // The values' own digits: fixed-width, as UiText's numeric.
        Component.onCompleted: Theme.tabularDigits(valueMetrics)
    }
    function valueColumn(values) {
        return Math.ceil(Math.max(24, ...values.map(v => valueMetrics.advanceWidth(v)))) + 6
    }

    // The ⋯ mark, drawn as on the meter.
    component Dots: Item {
        id: dots
        property bool active: false
        signal clicked()
        implicitWidth: 28
        implicitHeight: 28
        Rectangle {
            anchors.fill: parent
            radius: 4
            color: dots.active ? Theme.accentTint : (dotsHover.hovered ? Theme.hover : "transparent")
            border.width: 1
            border.color: Theme.fieldEdge
        }
        Row {
            anchors.centerIn: parent
            spacing: 2.5
            Repeater {
                model: 3
                Rectangle {
                    width: 3
                    height: 3
                    radius: 1.5
                    color: dots.active ? Theme.accent : Theme.name
                }
            }
        }
        HoverHandler {
            id: dotsHover
            cursorShape: Qt.PointingHandCursor
        }
        TapHandler {
            onTapped: dots.clicked()
        }
    }

    // Header: who and what, then the pickers.
    Rectangle {
        id: header
        width: parent.width
        height: headerColumn.implicitHeight + 24
        color: "transparent"

        Column {
            id: headerColumn
            x: Theme.gutter
            y: 12
            width: parent.width - 2 * Theme.gutter
            spacing: 10

            RowLayout {
                width: parent.width
                spacing: 10
                Image {
                    visible: !details.allPlayers
                    Layout.preferredWidth: 22
                    Layout.preferredHeight: 22
                    source: details.player ? Theme.classIcon(details.player.job) : ""
                    mipmap: true
                }
                UiText {
                    text: details.playerLabel
                    font.pixelSize: details.titleSize
                    font.weight: Font.DemiBold
                }
                UiText {
                    Layout.fillWidth: true
                    text: details.summary
                    muted: true
                    font.pixelSize: details.body
                }
                UiSegmented {
                    options: [
                        { "label": I18n.tr("details.mode.dmg"), "value": "dmg" },
                        { "label": I18n.tr("details.mode.heal"), "value": "heal" },
                        { "label": I18n.tr("details.mode.taken"), "value": "taken" }
                    ]
                    current: details.mode
                    onPicked: value => {
                        details.mode = value
                        details.selectedKey = ""
                    }
                }
                UiText {
                    visible: details.shotNote !== ""
                    text: details.shotNote
                    color: Theme.note
                    font.pixelSize: details.small
                }
                UiButton {
                    text: I18n.tr("details.screenshotButton")
                    onClicked: details.screenshot()
                    UiTip {
                        text: details.shotFile !== "" ? I18n.tr("details.screenshot.captured") + ": " + details.shotFile
                            : I18n.tr("details.screenshot.ariaLabel")
                    }
                }
                UiButton {
                    text: I18n.tr("details.historyBtn")
                    onClicked: details.historyRequested()
                }
                Dots {
                    active: details.optionsOpen
                    onClicked: details.optionsOpen = !details.optionsOpen
                    UiTip {
                        text: I18n.tr("details.settings.ariaLabel")
                    }
                }
            }

            Row {
                spacing: 10
                UiText {
                    height: details.fieldHeight
                    text: I18n.tr("details.player")
                    muted: true
                    font.pixelSize: details.body
                }
                UiDropdown {
                    implicitWidth: 200
                    textSize: details.body
                    options: [{ "label": I18n.tr("details.history.allPlayers"), "value": 0 }].concat(
                        details.view ? details.view.players.map(p => ({ "label": I18n.playerName(p), "value": p.id })) : [])
                    current: details.view ? details.view.player : 0
                    onPicked: value => {
                        details.selectedKey = ""
                        Details.pickPlayer(value)
                    }
                }
                Item {
                    width: 10
                    height: 1
                }
                UiText {
                    height: details.fieldHeight
                    text: I18n.tr("details.fightVs")
                    muted: true
                    font.pixelSize: details.body
                }
                UiDropdown {
                    implicitWidth: 220
                    textSize: details.body
                    // The targets the shown players hit, and the one picked.
                    readonly property var targets: !details.view ? [] : details.view.targets.concat(
                        details.view.target === 0 || details.view.targets.some(t => t.id === details.view.target) ? []
                            : [{ "id": details.view.target, "name": details.view.title }])
                    // A saved fight has one target, as in 1.x: no "All",
                    // which snapped back to it.
                    options: (details.view && details.view.saved ? [] : [{ "label": I18n.tr("details.all"), "value": 0 }]).concat(
                        targets.map(t => ({ "label": t.name || "#" + t.id, "value": t.id })))
                    current: details.view ? details.view.target : 0
                    onPicked: value => {
                        details.selectedKey = ""
                        Details.pickTarget(value)
                    }
                }
                // The dungeon's difficulty, in History's mark, then when the
                // fight started (a saved one: when it was fought), as 1.x's
                // title.
                Item {
                    readonly property var title: !details.view ? null : (details.view.saved || details.view.live || null)
                    readonly property string tier: title && title.tier ? title.tier : ""
                    visible: tier !== ""
                    width: tierBadge.width + 10
                    height: details.fieldHeight
                    Rectangle {
                        id: tierBadge
                        x: 10
                        anchors.verticalCenter: parent.verticalCenter
                        width: tierText.implicitWidth + 10
                        height: 17
                        radius: 3
                        color: "transparent"
                        border.width: 1
                        border.color: Theme.fieldEdge
                        UiText {
                            id: tierText
                            anchors.centerIn: parent
                            text: parent.parent.tier
                            font.pixelSize: Theme.sizeSmall
                            color: Theme.sub
                        }
                    }
                }
                UiText {
                    visible: text !== ""
                    height: details.fieldHeight
                    leftPadding: 10
                    text: details.view && details.view.saved ? details.view.saved.date
                        : details.view && details.view.live ? details.view.live.date : ""
                    muted: true
                    font.pixelSize: details.body
                }
            }
        }
        Rectangle {
            anchors.bottom: parent.bottom
            width: parent.width
            height: 1
            color: Theme.hairline
        }
    }

    signal historyRequested()

    Flickable {
        id: scroller
        anchors.top: header.bottom
        anchors.bottom: parent.bottom
        width: parent.width
        contentHeight: bodyColumn.implicitHeight + 24
        clip: true
        boundsBehavior: Flickable.StopAtBounds

        Column {
            id: bodyColumn
            x: Theme.gutter
            y: 12
            width: scroller.width - 2 * Theme.gutter
            spacing: 18

            // The Overview beside the skills while the table, names in full,
            // fits beside it; under the table when it does not (many
            // columns, large text), its lists side by side.
            GridLayout {
                width: parent.width
                columns: details.overviewBeside ? 2 : 1
                columnSpacing: 20
                rowSpacing: 18

                // Skills (or, for every player's damage taken, the mechanics).
                // In an Item: a table's width comes from its place, never
                // from its rows.
                Item {
                    Layout.fillWidth: true
                    Layout.alignment: Qt.AlignTop
                    implicitHeight: skillTable.implicitHeight
                UiTable {
                    id: skillTable
                    width: parent.width
                    // Too many columns for the window: the names stay, the
                    // numbers scroll sideways.
                    scrollable: true
                    pinnedWidth: Math.min(textWidth, 230)
                    columns: details.tableColumns
                    rows: details.shownRows
                    sortable: true
                    sortKey: details.sort.key
                    sortAscending: details.sort.ascending
                    onHeadClicked: key => details.sortBy(key)
                    onFoldClicked: (index, row) => details.fold(row.name)
                    cellText: (row, key) => details.cell(row, key)
                    iconSize: Settings.detailsIconSize
                    rowHeight: Math.max(Theme.tableRow, Settings.detailsIconSize + 6, details.body + 13)
                    textSize: details.body
                    headSize: details.small
                    selected: details.selectedSkill
                    // Each row's share of the tab's total, as 1.x's skill
                    // and TAKEN rows; the mechanics have no bar.
                    barFill: row => details.mechanics ? -1 : (row.fill !== undefined ? row.fill : row.share / 100)
                    clickable: true
                    onRowClicked: index => details.selectedKey = details.rowKey(details.shownRows[index])
                }
                }

                // The player's numbers, then the chosen skill's hits.
                Flow {
                    id: overview
                    Layout.preferredWidth: details.overviewBeside ? details.overviewWidth : -1
                    Layout.fillWidth: !details.overviewBeside
                    Layout.alignment: Qt.AlignTop
                    spacing: details.overviewBeside ? 14 : 28

                    Column {
                        id: statList
                        width: details.overviewWidth
                        visible: !details.mechanics
                        spacing: 0
                        readonly property real valueWidth: details.valueColumn(details.stats.map(s => s[1]))
                        Heading {
                            text: I18n.tr("details.statsTitle")
                            height: 22
                        }
                        Repeater {
                            model: details.mechanics ? [] : details.stats
                            Pair {
                                required property var modelData
                                valueWidth: statList.valueWidth
                                label: {
                                    switch (modelData[0]) {
                                    case "totalDamage": return I18n.tr("details.stats.totalDamage")
                                    case "contribution": return I18n.tr("details.stats.contribution")
                                    case "hits": return I18n.tr("details.stats.hits")
                                    case "critRate": return I18n.tr("details.stats.critRate")
                                    case "perfectRate": return I18n.tr("details.stats.perfectRate")
                                    case "doubleRate": return I18n.tr("details.stats.doubleRate")
                                    case "backRate": return I18n.tr("details.stats.backRate")
                                    case "parryRate": return I18n.tr("details.stats.parryRate")
                                    case "combatTime": return I18n.tr("details.stats.combatTime")
                                    case "multiHitDamage": return I18n.tr("details.stats.multiHitDamage")
                                    case "multiHitHits": return I18n.tr("details.stats.multiHitHits")
                                    case "maxHp": return I18n.tr("details.stats.maxHp")
                                    case "regen": return I18n.tr("details.stats.regen")
                                    case "deaths": return I18n.tr("details.stats.deaths")
                                    case "totalHealing": return I18n.tr("details.stats.totalHealing")
                                    case "hps": return I18n.tr("details.stats.hps")
                                    case "healTicks": return I18n.tr("details.stats.healTicks")
                                    case "hotTicks": return I18n.tr("details.stats.hotTicks")
                                    case "healSkills": return I18n.tr("details.stats.healSkills")
                                    case "takenTotal": return I18n.tr("details.stats.takenTotal")
                                    case "dtps": return I18n.tr("details.stats.dtps")
                                    case "takenHits": return I18n.tr("details.stats.takenHits")
                                    case "shieldBlock": return I18n.tr("details.skills.shieldBlockTooltip")
                                    case "parry": return I18n.tr("details.skills.parryTooltip")
                                    case "perfectBlock": return I18n.tr("details.skills.perfectBlockTooltip")
                                    case "ironWall": return I18n.tr("details.skills.ironWallTooltip")
                                    case "regeneration": return I18n.tr("details.skills.regenerationTooltip")
                                    }
                                    return ""
                                }
                                value: modelData[1]
                                tip: details.ticksOnly && value === "-" && (details.noData.damageByHit.indexOf(modelData[0]) >= 0
                                    || modelData[0] === "multiHitDamage") ? I18n.tr("meter.noHitsTip") : ""
                            }
                        }
                    }

                    // The chosen skill's hits: normal and critical, then the
                    // game's own hit types.
                    Column {
                        id: hitList
                        width: details.overviewWidth
                        visible: shown
                        spacing: 0
                        // HEAL and TAKEN rows have no hit types or min and max.
                        readonly property bool shown: details.skill !== null && details.skill.table === "dmg"
                        // A damage over time row counts ticks, not hits.
                        readonly property bool ticks: shown && details.skill.dot === true
                        readonly property var minAvgMax: !shown ? "" : details.noData.hitsAndTicks(details.skill) === 0 ? "-"
                            : [details.skill.min, details.skill.avg, details.skill.max].map(v => Format.rate(v)).join(" / ")
                        readonly property var hitTypes: shown ? [["back", details.skill.back], ["perfect", details.skill.perfect],
                            ["double", details.skill.double], ["parry", details.skill.parry], ["frontal", details.skill.front]] : []
                        readonly property real valueWidth: shown ? details.valueColumn([String(details.skill.hits - details.skill.crit),
                            String(details.skill.crit), minAvgMax].concat(hitTypes.map(t => String(t[1])))) : 60
                        Rectangle {
                            width: parent.width
                            height: 1
                            color: Theme.hairline
                        }
                        Row {
                            height: 28
                            width: parent.width
                            spacing: 8
                            SkillIcon {
                                anchors.verticalCenter: parent.verticalCenter
                                size: 20
                                code: details.skill ? details.skill.code || 0 : 0
                                job: details.skill ? details.skill.job || "" : ""
                            }
                            Heading {
                                height: parent.height
                                width: parent.width - 28
                                text: details.skill ? details.skill.name : ""
                                color: Theme.name
                            }
                        }
                        // The backend keeps each skill's lowest and highest
                        // hit, not each hit: one min / avg / max for all.
                        Pair {
                            label: hitList.ticks ? I18n.tr("details.hits.ticks") : I18n.tr("details.hits.normal")
                            value: details.skill ? String(details.skill.hits - details.skill.crit) : ""
                            valueWidth: hitList.valueWidth
                        }
                        Pair {
                            label: hitList.ticks ? I18n.tr("details.hits.criticalTicks") : I18n.tr("details.hits.critical")
                            value: details.skill ? String(details.skill.crit) : ""
                            valueWidth: hitList.valueWidth
                        }
                        Pair {
                            label: I18n.tr("details.hits.minAvgMax")
                            value: hitList.minAvgMax
                            valueWidth: hitList.valueWidth
                        }
                        Heading {
                            height: 28
                            text: I18n.tr("details.hits.types")
                        }
                        Repeater {
                            model: hitList.hitTypes
                            Pair {
                                required property var modelData
                                valueWidth: hitList.valueWidth
                                label: {
                                    switch (modelData[0]) {
                                    case "back": return I18n.tr("details.hitTypes.back")
                                    case "perfect": return I18n.tr("details.hitTypes.perfect")
                                    case "double": return I18n.tr("details.hitTypes.double")
                                    case "parry": return I18n.tr("details.hitTypes.parry")
                                    case "frontal": return I18n.tr("details.hitTypes.frontal")
                                    }
                                    return ""
                                }
                                value: String(modelData[1])
                            }
                        }
                    }

                    // Every player's damage taken: who the chosen mechanic hit.
                    Column {
                        width: details.overviewWidth
                        visible: details.mechanics
                        spacing: 0
                        Heading {
                            height: 22
                            width: parent.width
                            text: details.skill ? details.skill.name : ""
                            color: Theme.name
                        }
                        UiTable {
                            width: parent.width
                            rowHeight: Math.max(Theme.tableRow, details.body + 13)
                            textSize: details.body
                            headSize: details.small
                            columns: [
                                { "key": "name", "label": I18n.tr("details.player"), "width": 0, "icon": "class",
                                    "tip": I18n.tr("details.player") + "\n" + I18n.tr("details.tips.mechanicPlayer") },
                                { "key": "hits", "label": I18n.tr("details.skills.hitsTaken"), "width": 40, "right": true,
                                    "tip": I18n.tr("details.skills.hitsTakenTooltip") + "\n" + I18n.tr("details.tips.mechanicHits") },
                                { "key": "taken", "label": I18n.tr("details.skills.taken"), "width": 60, "right": true,
                                    "tip": I18n.tr("details.skills.takenTooltip") + "\n" + I18n.tr("details.tips.mechanicDamage") }
                            ]
                            // The chosen row's own table, as cell().
                            rows: details.skill && details.skill.table === "mechanics" ? details.skill.hit.map(h => {
                                const p = details.entry(h.id)
                                return { "name": I18n.playerName(p), "job": p.job, "hits": h.attacks, "taken": Format.rate(h.damage) }
                            }) : []
                        }
                    }
                }
            }

            // Targets (DMG, HEAL) or what hit the player (TAKEN): the names
            // as wide as the longest, so each total sits next to its name,
            // and a share bar in the room left.
            UiTable {
                width: parent.width
                visible: !details.mechanics
                rowHeight: Math.max(Theme.tableRow, details.body + 13)
                textSize: details.body
                headSize: details.small
                shareFill: row => row.fill
                columns: details.withTips(details.mode === "taken" ? [
                    { "key": "name", "label": I18n.tr("details.targets.sources"), "name": I18n.tr("details.targets.sources"), "about": I18n.tr("details.tips.sources"), "width": 0 },
                    { "key": "value", "label": I18n.tr("meter.columns.total"), "name": I18n.tr("details.skills.takenTooltip"), "about": I18n.tr("details.tips.sourceDamage"), "width": 80, "right": true },
                    { "key": "share", "label": I18n.tr("meter.columns.pct"), "name": I18n.tr("details.skills.takenPctTooltip"), "about": I18n.tr("details.tips.sourceShare"), "width": 60, "right": true }
                ] : details.mode === "heal" ? [
                    { "key": "name", "label": I18n.tr("details.targets.title"), "name": I18n.tr("details.targets.title"), "about": I18n.tr("details.tips.healTargets"), "width": 0 },
                    { "key": "value", "label": I18n.tr("meter.columns.total"), "name": I18n.tr("details.skills.healTooltip"), "about": I18n.tr("details.tips.targetHeal"), "width": 80, "right": true },
                    { "key": "share", "label": I18n.tr("meter.columns.pct"), "name": I18n.tr("details.skills.healPctTooltip"), "about": I18n.tr("details.tips.targetHealShare"), "width": 60, "right": true }
                ] : [
                    { "key": "name", "label": I18n.tr("details.targets.title"), "name": I18n.tr("details.targets.title"), "about": I18n.tr("details.tips.targets"), "width": 0 },
                    { "key": "value", "label": I18n.tr("meter.columns.total"), "name": I18n.tr("details.skills.dmgTooltip"), "about": I18n.tr("details.tips.targetDamage"), "width": 80, "right": true },
                    { "key": "share", "label": I18n.tr("meter.columns.pct"), "name": I18n.tr("details.skills.dmgPctTooltip"), "about": I18n.tr("details.tips.targetShare"), "width": 60, "right": true }
                ])
                rows: details.withFill(!details.view ? []
                    : details.mode === "heal"
                    // Whom the shown healing went to.
                    ? details.view.heal.targets.map(t => ({
                        "name": I18n.playerName(t), "job": t.job,
                        "value": Format.rate(t.amount), "share": Format.pct1(t.share), "amount": t.amount }))
                    : (details.mode === "taken"
                        ? details.view.taken.sources.map(s => ({
                            "name": s.code > 0 ? s.name || "#" + s.code : "-", "value": Format.rate(s.damage), "share": Format.pct1(s.share), "amount": s.damage }))
                        : details.view.targets.map(t => ({ "name": t.name || "#" + t.id, "value": Format.rate(t.damage), "share": Format.pct1(t.share), "amount": t.damage }))))
            }

            // The DPS chart or the timeline, on one time axis.
            Column {
                width: parent.width
                spacing: 8
                UiSegmented {
                    framed: false
                    options: [
                        {
                            "label": details.mode === "heal" ? I18n.tr("details.chart.hps")
                                : (details.mode === "taken" ? I18n.tr("details.chart.dtps") : I18n.tr("details.dpsChart")),
                            "value": "chart"
                        },
                        { "label": I18n.tr("details.timeline"), "value": "timeline" }
                    ]
                    current: details.lower
                    onPicked: value => details.lower = value
                }
                DpsChart {
                    visible: details.lower === "chart"
                    width: parent.width
                    height: 200
                    textSize: details.small
                    player: details.player
                    // Damage done, healing done or damage received, at the
                    // times they landed.
                    readonly property var chart: !details.view ? null : (details.mode === "heal" ? details.view.healChart
                        : (details.mode === "taken" ? details.view.takenChart : details.view.chart))
                    seconds: chart && chart.battleMs > 0 ? chart.battleMs / 1000 : details.seconds
                    playerPoints: chart ? chart.player : []
                    partyPoints: chart ? chart.party : []
                }
                Timeline {
                    visible: details.lower === "timeline"
                    width: parent.width
                    textSize: details.small
                    player: details.player
                    lanes: details.view ? details.view.lanes : []
                    tracks: details.view && details.view.tracks ? details.view.tracks : ({ "buffs": [], "stats": [] })
                    seconds: details.view && details.view.chart.battleMs > 0 ? details.view.chart.battleMs / 1000 : details.seconds
                }
            }
        }
    }

    // The Details options, over the overview column (280 px, also when the
    // overview sits under a wide table) and the window's edge, so the skill
    // table keeps the gap it has to the overview.
    Rectangle {
        id: options
        visible: details.optionsOpen
        anchors.top: header.bottom
        anchors.bottom: parent.bottom
        anchors.right: parent.right
        width: 280 + Theme.gutter
        color: Theme.window
        border.width: 1
        border.color: Theme.panelEdge

        Flickable {
            anchors.fill: parent
            anchors.margins: 1
            contentHeight: optionsColumn.implicitHeight + 24
            clip: true
            boundsBehavior: Flickable.StopAtBounds

            Column {
                id: optionsColumn
                x: 14
                y: 12
                width: parent.width - 28
                spacing: 6

                UiText {
                    text: I18n.tr("details.settings.title")
                    font.pixelSize: Theme.sizeTitle
                    font.weight: Font.DemiBold
                    height: 26
                }
                UiText {
                    text: I18n.tr("details.fontSize.label")
                    muted: true
                }
                UiSlider {
                    width: parent.width
                    from: 11
                    to: 20
                    value: Settings.detailsFontSize
                    valueText: Math.round(value) + " px"
                    onMoved: value => Settings.detailsFontSize = value
                }
                UiText {
                    text: I18n.tr("details.iconSize.label")
                    muted: true
                }
                UiSlider {
                    width: parent.width
                    from: 20
                    to: 56
                    value: Settings.detailsIconSize
                    valueText: Math.round(value) + " px"
                    onMoved: value => Settings.detailsIconSize = value
                }
                Item {
                    width: 1
                    height: 6
                }
                Heading {
                    text: I18n.tr("details.screenshots.label")
                    height: 22
                }
                SwitchLine {
                    label: I18n.tr("details.screenshots.includeMeter")
                    checked: Settings.screenshotWithMeter
                    onToggled: on => Settings.screenshotWithMeter = on
                }
                SwitchLine {
                    label: I18n.tr("details.screenshots.saveToFolder")
                    checked: Settings.screenshotToFolder
                    onToggled: on => Settings.screenshotToFolder = on
                }
                // The folder, while files are saved: chosen, else the
                // default (Pictures/Daevalog DPS Meter).
                Row {
                    visible: Settings.screenshotToFolder
                    width: parent.width
                    spacing: 8
                    UiButton {
                        id: chooseFolder
                        small: true
                        text: I18n.tr("details.screenshots.chooseFolder")
                        onClicked: folderDialog.open()
                    }
                    UiText {
                        width: parent.width - chooseFolder.width - 8
                        height: chooseFolder.height
                        text: Settings.screenshotPlace
                        color: Theme.note
                        font.pixelSize: details.small
                        elide: Text.ElideMiddle
                        UiTip {
                            text: Settings.screenshotPlace
                        }
                    }
                }
                Item {
                    width: 1
                    height: 6
                }
                Heading {
                    text: I18n.tr("details.columns.label")
                    height: 22
                }
                Repeater {
                    model: details.dmgColumns
                    SwitchLine {
                        required property var modelData
                        label: modelData.name
                        tip: modelData.about || ""
                        checked: details.chosenColumns.indexOf(modelData.key) >= 0
                        // The columns are the DMG table's: from HEAL or
                        // TAKEN, a change goes to DMG, where it shows.
                        onToggled: on => {
                            const list = details.chosenColumns.filter(k => k !== modelData.key)
                            if (on)
                                list.push(modelData.key)
                            Settings.skillColumns = JSON.stringify(list)
                            if (details.mode !== "dmg") {
                                details.mode = "dmg"
                                details.selectedKey = ""
                            }
                        }
                    }
                }
            }
        }
    }

    component SwitchLine: Item {
        id: line
        property string label
        // What the line's name alone does not say, shown while the pointer rests on it.
        property string tip
        property bool checked
        signal toggled(bool on)
        width: parent ? parent.width : 280
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

    // "Show Details on monitor" placed the window (Main.qml
    // applyDetailsMonitor): the screen's number and name in its middle, held
    // 2 s and faded out over 1 s, as 1.x's 3 s badge. --open details/monitor
    // holds it, for a shot.
    function showMonitorBadge(hold) {
        monitorFade.stop()
        monitorBadge.opacity = 1
        if (!hold)
            monitorFade.start()
    }
    SequentialAnimation {
        id: monitorFade
        PauseAnimation { duration: 2000 }
        OpacityAnimator { target: monitorBadge; to: 0; duration: 1000 }
    }
    Rectangle {
        id: monitorBadge
        readonly property var screens: Array.from(Qt.application.screens)
        readonly property int number: screens.findIndex(s => s.name === details.screen.name) + 1
        opacity: 0
        visible: opacity > 0
        anchors.centerIn: parent
        width: Math.max(monitorNumber.implicitWidth, monitorText.implicitWidth) + 64
        height: monitorNumber.implicitHeight + monitorText.implicitHeight + 40
        radius: 8
        color: Theme.surface
        border.width: 1
        border.color: Theme.fieldEdge
        UiText {
            id: monitorNumber
            anchors.horizontalCenter: parent.horizontalCenter
            y: 12
            text: String(parent.number)
            font.pixelSize: 120
            font.weight: Font.DemiBold
            color: Theme.number
        }
        UiText {
            id: monitorText
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.top: monitorNumber.bottom
            text: [I18n.tr("settings.detailsMonitor.badge") + " " + parent.number, details.screen.name,
                details.screen.width + "×" + details.screen.height].filter(t => t !== "").join(" · ").toUpperCase()
            font.pixelSize: Theme.sizeTitle
            font.letterSpacing: 2
            color: Theme.sub
        }
    }
}
