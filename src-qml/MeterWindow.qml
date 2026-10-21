// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Daevalog

// The meter overlay: frameless, see-through, kept on top; the lock makes
// it click-through, all but the ⋯ button.
Window {
    id: meter

    // Set by LayerMeterWindow on Wayland. A layer surface ignores system
    // move and resize: it moves by its margins and grows from its top-left.
    property bool layerSurface: false
    // On KDE X11 the meter is a window KWin does not manage, the only kind
    // it keeps above a fullscreen game. Nothing moves it but the meter: it
    // goes where the pointer is and grows from its top-left.
    readonly property bool unmanaged: Overlay.unmanaged
    readonly property bool placesItself: layerSurface || unmanaged

    // Shown, or hidden from the tray or ✕. On Wayland a hidden meter keeps
    // its surface, draws nothing and takes no clicks: a surface made again
    // goes wherever the compositor puts a new window, and COSMIC closes the
    // connection when a layer surface goes. On KDE and COSMIC a hidden
    // normal window is minimized too. Elsewhere it hides.
    property bool shown: true
    readonly property bool hidesInPlace: Qt.platform.pluginName.startsWith("wayland")
    readonly property bool hiddenInPlace: !shown && hidesInPlace
    // A normal Wayland window shown again asks to come to the front: the
    // compositor raises and focuses a window on activation, with the token
    // the tray hands over. Hidden, it keeps its taskbar entry: a window made
    // again goes wherever the compositor puts a new one, and the entry is a
    // way back that needs no tray.
    onShownChanged: {
        if (!hidesInPlace || layerSurface)
            return
        if (shown)
            requestActivate()
        else if (Overlay.minimizeHidden)
            showMinimized()
    }
    // Activated from the taskbar or the window list: shown again.
    onActiveChanged: {
        if (active && !shown)
            shown = true
    }

    // The lock lives in the ⋯ menu, or on its own button when "Show lock
    // button" is on; that one control still takes clicks while the meter is
    // locked. On Wayland and X11 the window's input region shrinks to it;
    // elsewhere a locked meter takes no clicks and unlocks from the tray.
    readonly property Item lockControl: Settings.showLockButton ? lockButton : menuButton
    readonly property bool hasInputRegion: Overlay.inputRegion
    // Set by Main: the lock can be undone here (an input region or a tray).
    property bool canLock: true
    // Set by Main: ✕ hides the meter to the tray; without a tray it quits.
    property bool closeHides: true
    readonly property size screenSize: Qt.size(Screen.width, Screen.height)
    // Where a layer surface's margins count: set from LayerArea, the screen
    // less the bars; the whole screen until the compositor has told.
    property size layerArea: Qt.size(0, 0)
    readonly property size placeArea: layerArea.width > 0 ? layerArea : screenSize

    // The tab: 0 DPS, 1 Heal, 2 Tank. Tabs pick the numbers, the mode picks
    // the fight; every tab lists every player.
    property int tab: 0
    // Set once the user resizes; until then the window fits its rows.
    // A double-click on an edge clears it (fitToRows).
    property bool userSized: false
    // Armed at the start; the first height change the meter did not make
    // is set back, then it is off (see onHeightChanged).
    property bool restoreHeight: false

    readonly property bool hasBoss: Meter.bossHealth >= 0
    // Every size below is Theme.meter's, at the Meter size setting; the
    // 1 px lines and the border stay 1 px.
    readonly property int naturalHeight: 2 + Theme.meter.headerHeight + (Capture.notice ? notice.height : 0)
        + (hasBoss ? Theme.meter.bossBand : 0) + (Theme.slim ? 0 : Theme.meter.headsHeight)
        + rowList.implicitHeight + Theme.meter.px(10) + Theme.meter.footerHeight

    // Each tab's number columns, as Settings > Meter > Columns chose them
    // (any number of them), in the list's order: head, tooltip (the full
    // name, then what the column shows), width, kind. Four or more sit a
    // little closer, and the meter's minimum width grows with them;
    // a DPS column is wider when Round DPS is off and it shows the whole
    // number. The Heal and Tank rows are stand-ins.
    readonly property var columnInfo: ({
        "dps": {
            "total": { "head": I18n.tr("meter.columns.total"), "tip": I18n.tr("settings.encColumns.total") + "\n" + I18n.tr("meter.columnTips.total"), "widths": [64, 54], "kind": "main" },
            "encDps": { "head": I18n.tr("meter.columns.encDps"), "tip": I18n.tr("settings.columns.dps") + "\n" + I18n.tr("meter.columnTips.dps"), "widths": [56, 46], "kind": "main", "rate": true },
            "dps": { "head": I18n.tr("meter.columns.dps"), "tip": I18n.tr("settings.encColumns.dps") + "\n" + I18n.tr("meter.columnTips.activeDps"), "widths": [56, 46], "kind": "main", "rate": true },
            "pct": { "head": I18n.tr("meter.columns.pct"), "tip": I18n.tr("settings.encColumns.pct") + "\n" + I18n.tr("meter.columnTips.pct"), "widths": [40, 38], "kind": "share" },
            "crit": { "head": I18n.tr("meter.columns.crit"), "tip": I18n.tr("settings.encColumns.crit") + "\n" + I18n.tr("meter.columnTips.crit"), "widths": [40, 38], "kind": "share" },
            "last10": { "head": I18n.tr("meter.columns.last10"), "tip": I18n.tr("settings.encColumns.last10") + "\n" + I18n.tr("meter.columnTips.last").replace("{n}", "10"), "widths": [56, 46], "kind": "main", "rate": true },
            "last30": { "head": I18n.tr("meter.columns.last30"), "tip": I18n.tr("settings.encColumns.last30") + "\n" + I18n.tr("meter.columnTips.last").replace("{n}", "30"), "widths": [56, 46], "kind": "main", "rate": true },
            "last60": { "head": I18n.tr("meter.columns.last60"), "tip": I18n.tr("settings.encColumns.last60") + "\n" + I18n.tr("meter.columnTips.last").replace("{n}", "60"), "widths": [56, 46], "kind": "main", "rate": true },
            "maxHit": { "head": I18n.tr("meter.columns.maxHit"), "tip": I18n.tr("settings.encColumns.maxHit") + "\n" + I18n.tr("meter.columnTips.maxHit"), "widths": [56, 50], "kind": "main" },
            "hits": { "head": I18n.tr("meter.columns.hits"), "tip": I18n.tr("settings.encColumns.hits") + "\n" + I18n.tr("meter.columnTips.hits"), "widths": [44, 40], "kind": "share" }
        },
        // Heal: what the healer did, then, quiet, what each player took.
        "heal": {
            "healed": { "head": I18n.tr("meter.columns.healed"), "tip": I18n.tr("settings.columns.healed") + "\n" + I18n.tr("meter.columnTips.healed"), "widths": [64, 54], "kind": "main" },
            "hps": { "head": I18n.tr("meter.columns.hps"), "tip": I18n.tr("settings.columns.hps") + "\n" + I18n.tr("meter.columnTips.hps"), "widths": [56, 46], "kind": "main" },
            "pct": { "head": I18n.tr("meter.columns.share"), "tip": I18n.tr("settings.columns.healPct") + "\n" + I18n.tr("meter.columnTips.healPct"), "widths": [40, 38], "kind": "share" },
            "taken": { "head": I18n.tr("meter.columns.taken"), "tip": I18n.tr("settings.columns.taken") + "\n" + I18n.tr("meter.columnTips.healTaken"), "widths": [56, 50], "kind": "second" }
        },
        "tank": {
            "taken": { "head": I18n.tr("meter.columns.taken"), "tip": I18n.tr("settings.columns.taken") + "\n" + I18n.tr("meter.columnTips.taken"), "widths": [64, 54], "kind": "main" },
            "dtps": { "head": I18n.tr("meter.columns.dtps"), "tip": I18n.tr("settings.columns.dtps") + "\n" + I18n.tr("meter.columnTips.dtps"), "widths": [56, 46], "kind": "main" },
            "pct": { "head": I18n.tr("meter.columns.share"), "tip": I18n.tr("settings.columns.takenPct") + "\n" + I18n.tr("meter.columnTips.takenPct"), "widths": [40, 38], "kind": "share" },
            "block": { "head": I18n.tr("details.skills.shieldBlock"), "tip": I18n.tr("settings.columns.block") + "\n" + I18n.tr("meter.columnTips.block"), "widths": [44, 40], "kind": "share" },
            "parry": { "head": I18n.tr("details.skills.parry"), "tip": I18n.tr("settings.columns.parry") + "\n" + I18n.tr("meter.columnTips.parry"), "widths": [44, 40], "kind": "share" }
        }
    })
    readonly property var tabKeys: (JSON.parse(Settings.meterColumns) || {})[["dps", "heal", "tank"][tab]] || []
    readonly property var columns: tabKeys.map(key => {
        const info = columnInfo[["dps", "heal", "tank"][tab]][key]
        const width = Theme.meter.px(info.widths[tabKeys.length > 3 ? 1 : 0] + (info.rate && !Settings.roundDps ? 12 : 0))
        return { "head": info.head, "tip": info.tip, "width": width, "kind": info.kind }
    })
    readonly property var columnWidths: columns.map(c => c.width)
    readonly property var columnKinds: columns.map(c => c.kind)
    readonly property var healerColumns: [
        { "head": I18n.tr("meter.columns.healed"), "tip": I18n.tr("settings.columns.healed") + "\n" + I18n.tr("meter.columnTips.healed"), "width": Theme.meter.px(64), "kind": "main" },
        { "head": I18n.tr("meter.columns.hps"), "tip": I18n.tr("settings.columns.hps") + "\n" + I18n.tr("meter.columnTips.hps"), "width": Theme.meter.px(56), "kind": "main" },
        { "head": I18n.tr("meter.columns.share"), "tip": I18n.tr("settings.columns.healPct") + "\n" + I18n.tr("meter.columnTips.healerPct"), "width": Theme.meter.px(40), "kind": "share" }
    ]
    readonly property string groupTotal: [Meter.groupTotal, party ? party.healTotal : "", party ? party.tankTotal : ""][tab]
    // The group's damage beside its DPS, as 1.x, DPS tab only, as Settings >
    // Meter > Group damage places it: in the footer after the rate (1.x's
    // Beta layout), or with the rate on a line of its own under the DPS rows,
    // at their right edge (1.x's Classic layout).
    readonly property bool showsDamage: Settings.showTotal && tab === 0 && groupTotal !== "" && Meter.groupDamage !== ""
    readonly property bool damageLine: showsDamage && Settings.groupDamage === "line"

    // The Heal and Tank tabs and the split view's healers, as the Meter
    // gives them. The rows sit in list models, so a new reading changes them
    // in place and a row under the pointer stays; their texts come from
    // `party` by index. Each player keeps their row, as on the DPS tab
    // (meter.rs `row_steps`): rows that left go, rows that changed place
    // move, new ones go in at their place, then each takes its numbers.
    readonly property var party: Meter.party ? JSON.parse(Meter.party) : null
    ListModel { id: healRows }
    ListModel { id: tankRows }
    ListModel { id: healerRows }
    onPartyChanged: {
        sync(healRows, party ? party.heal : [])
        sync(tankRows, party ? party.tank : [])
        sync(healerRows, party ? party.healers : [])
    }
    function sync(model, rows) {
        const ids = new Set(rows.map(r => r.entityId))
        for (let i = model.count - 1; i >= 0; i--) {
            if (!ids.has(model.get(i).entityId))
                model.remove(i)
        }
        for (let i = 0; i < rows.length; i++) {
            const row = Object.assign({}, rows[i])
            delete row.texts
            let at = -1
            for (let j = i; j < model.count; j++) {
                if (model.get(j).entityId === row.entityId) {
                    at = j
                    break
                }
            }
            if (at < 0) {
                model.insert(i, row)
            } else {
                if (at !== i)
                    model.move(at, i, 1)
                model.set(i, row)
            }
        }
        if (model.count > rows.length)
            model.remove(rows.length, model.count - rows.length)
    }
    // The texts of a Heal (1) or Tank (2) row, or of a healer (3).
    function partyTexts(tab, index) {
        if (!party)
            return []
        const row = [null, party.heal, party.tank, party.healers][tab][index]
        return row ? row.texts : []
    }

    // The DPS rows' texts follow the columns and Round DPS; the Meter
    // writes them again at once, also when no reading comes.
    Connections {
        target: Settings
        function onMeterColumnsChanged() { Meter.redraw() }
        function onRoundDpsChanged() { Meter.redraw() }
    }

    // One of the meter's menus is open or opening (Main.qml).
    property bool menuOpen: false

    // ⋯ > Save screenshot: the meter alone to the clipboard, and for 2 s
    // the header says what happened, as Details' Screenshot button does.
    property string shotNote: ""
    function screenshot() {
        const result = JSON.parse(Overlay.capture(meter, null, false, Theme.window))
        shotNote = result.clipboard ? I18n.tr("details.screenshot.savedToClipboard") : I18n.tr("details.screenshot.failed")
        shotNoteTimer.restart()
    }
    Timer {
        id: shotNoteTimer
        interval: 2000
        onTriggered: meter.shotNote = ""
    }

    signal menuRequested(Item button)
    signal closeRequested()
    signal modeMenuRequested(Item button)
    // A row was clicked: Details for that player.
    signal detailsRequested(var player)
    // The pointer rests on a row, or left it (row null).
    signal rowHovered(var player, Item row)

    width: 360
    // Wide enough for the icon, 60 px of name and every column the rows
    // show (the split view's healers too), as MeterRow lays them out; the
    // 1 px border on each side; never below 260.
    readonly property int rowsMinimum: {
        const need = widths => widths.reduce((sum, w) => sum + w, 0) + widths.length * Theme.meter.colGap
        const healers = Settings.splitView && tab === 0 ? need(healerColumns.map(c => c.width)) : 0
        return 2 + Theme.meter.padLeft + Theme.meter.colIcon + Theme.meter.colGap + Theme.meter.px(60) + Theme.meter.padRight
            + Math.max(need(columnWidths), healers)
    }
    minimumWidth: Math.max(Theme.meter.px(260), rowsMinimum)
    // The width that gives names about the room three columns leave at
    // 360 px: the start, and a change of columns, grow to it.
    readonly property int roomyWidth: Math.max(Theme.meter.px(360), rowsMinimum + Theme.meter.px(70))
    onRowsMinimumChanged: {
        if (width < rowsMinimum)
            width = rowsMinimum
    }
    // A new meter size keeps the window's proportions: its width, and a
    // height the user set, grow or shrink with everything in it.
    property real sizedAt: 1
    Connections {
        target: Theme.meter
        function onScaleChanged() {
            const k = Theme.meter.scale / meter.sizedAt
            meter.sizedAt = Theme.meter.scale
            meter.width = Math.max(meter.minimumWidth, Math.round(meter.width * k))
            if (meter.userSized)
                meter.height = Math.max(meter.minimumHeight, Math.round(meter.height * k))
        }
    }
    Connections {
        target: Settings
        function onMeterColumnsChanged() {
            if (meter.width < meter.roomyWidth)
                meter.width = meter.roomyWidth
        }
    }
    minimumHeight: 2 + Theme.meter.headerHeight + Theme.meter.footerHeight
    visible: shown || hidesInPlace
    color: "transparent"
    title: I18n.tr("header.title") + (System.devBuild ? " (dev)" : "")
    flags: Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint
           | ((Settings.locked && !hasInputRegion) || hiddenInPlace ? Qt.WindowTransparentForInput : 0)
           | (unmanaged ? Qt.BypassWindowManagerHint : 0)

    Component.onCompleted: {
        // Not bindings: a resize by the user must stick.
        width = roomyWidth
        height = naturalHeight
        sizedAt = Theme.meter.scale
        restoreHeight = true
        Overlay.watchWindow(meter)
        shapeInput()
    }
    onNaturalHeightChanged: {
        if (!userSized)
            height = naturalHeight
    }
    // A compositor's first configure can carry the height from before the
    // rows came (KWin, COSMIC): the rows' height goes back, once, so a
    // compositor that keeps its own size is not fought.
    onHeightChanged: {
        if (userSized || !restoreHeight || height === naturalHeight)
            return
        restoreHeight = false
        Qt.callLater(() => {
            if (!userSized)
                height = naturalHeight
        })
    }

    // An unmanaged window's top-left goes where the pointer is, less where
    // the drag took hold of it, on the pointer's screen.
    function placeAt(pointer, grab) {
        let x = Math.round(pointer.x - grab.x)
        let y = Math.round(pointer.y - grab.y)
        for (const s of Array.from(Qt.application.screens)) {
            if (pointer.x >= s.virtualX && pointer.x < s.virtualX + s.width
                    && pointer.y >= s.virtualY && pointer.y < s.virtualY + s.height) {
                x = Math.max(s.virtualX, Math.min(x, s.virtualX + s.width - width))
                y = Math.max(s.virtualY, Math.min(y, s.virtualY + s.height - height))
            }
        }
        meter.x = x
        meter.y = y
    }

    function shapeInput() {
        if (!hasInputRegion)
            return
        const c = lockControl
        Overlay.shapeInput(meter, Settings.locked ? c.mapToItem(null, 0, 0, c.width, c.height) : Qt.rect(0, 0, 0, 0))
    }

    // The ⋯ button moves when the window's width changes, and the lock
    // moves to its own button and back with "Show lock button".
    onLockControlChanged: shapeInput()
    Connections {
        target: meter.lockControl
        function onXChanged() { meter.shapeInput() }
        function onYChanged() { meter.shapeInput() }
    }
    Connections {
        target: Settings
        function onLockedChanged() { meter.shapeInput() }
    }

    component HeaderButton: Item {
        id: button
        property bool hovered: hover.hovered
        // Off: a mark that only shows a state.
        property bool clickable: true
        signal clicked()
        implicitWidth: Theme.meter.px(16)
        implicitHeight: Theme.meter.px(16)
        HoverHandler { id: hover; enabled: button.clickable }
        TapHandler { enabled: button.clickable; onTapped: button.clicked() }
    }

    component SmallText: HudText {
        font.pixelSize: Theme.meter.sizeSmall
        color: Theme.hudSub
    }

    // The heads do not sort, as Details' do: each head's tooltip ends with
    // the rows' order and where to turn it round.
    readonly property string orderTip: I18n.tr("meter.columnTips.order").replace("{lowest}", I18n.tr("meter.sortLowestFirst"))

    // A heads line over rows: the first column's name, then the number
    // columns, right-aligned on the rows' own columns. Each head's tooltip
    // shows once the pointer rests on it (unlocked only).
    component ColumnHeads: Item {
        id: heads
        property string first
        property string firstTip
        property var columns: []
        // A hairline above, where a second table starts (split view).
        property bool rule: false

        Rectangle {
            visible: heads.rule
            y: Theme.meter.px(2)
            width: parent.width
            height: 1
            color: Theme.hairline
        }
        Item {
            anchors.fill: parent
            anchors.leftMargin: Theme.meter.px(6)
            anchors.rightMargin: Theme.meter.px(6)
            anchors.topMargin: heads.rule ? Theme.meter.px(4) : 0
            SmallText {
                x: Theme.meter.padLeft + Theme.meter.colIcon + Theme.meter.colGap
                height: parent.height
                text: heads.first
                UiTip {
                    delay: Theme.meterTipRest
                    text: heads.firstTip + "\n" + meter.orderTip
                }
            }
            Row {
                anchors.right: parent.right
                anchors.rightMargin: Theme.meter.padRight
                height: parent.height
                spacing: Theme.meter.colGap
                Repeater {
                    model: heads.columns
                    SmallText {
                        id: head
                        required property var modelData
                        width: modelData.width
                        height: parent.height
                        horizontalAlignment: Text.AlignRight
                        text: modelData.head
                        UiTip {
                            delay: Theme.meterTipRest
                            text: head.modelData.tip + "\n" + meter.orderTip
                        }
                    }
                }
            }
        }
    }

    // The healers' table under DPS, when the split view is on; its head
    // line shows from the start, with a note until a healer qualifies.
    readonly property bool splitShown: Settings.splitView && tab === 0
    readonly property bool split: splitShown && healerRows.count > 0

    // The live loop reads Details for the Heal and Tank rows only while
    // they show: their tabs, or the split view's healers under DPS.
    Binding {
        target: Meter
        property: "partyShown"
        value: meter.tab !== 0 || Settings.splitView
    }

    // The player of a meter row, for Details and the hover card.
    // The DPS row at `index` as a click on it hands it to Details; null when
    // there is none (--open details/row).
    function rowPlayer(index) {
        const row = dpsRows.itemAt(index)
        return row ? playerOf(row) : null
    }

    function playerOf(row) {
        return {
            "entityId": row.entityId, "job": row.job, "name": row.name, "number": row.number, "you": row.you,
            "identifying": row.identifying, "unattributed": row.unattributed
        }
    }

    // An edge or corner that resizes the window.
    component ResizeGrip: Item {
        id: grip
        required property int edges
        readonly property bool growsOnly: !(edges & (Qt.LeftEdge | Qt.TopEdge))
        // Where the window and the pointer were at the start, for a meter
        // that sizes itself.
        property rect startRect
        property point startPointer

        visible: meter.shown
        // A layer surface grows from its top-left corner only.
        enabled: !Settings.locked && (!meter.layerSurface || growsOnly)

        HoverHandler {
            cursorShape: {
                const e = grip.edges
                if (e === (Qt.LeftEdge | Qt.TopEdge) || e === (Qt.RightEdge | Qt.BottomEdge))
                    return Qt.SizeFDiagCursor
                if (e === (Qt.RightEdge | Qt.TopEdge) || e === (Qt.LeftEdge | Qt.BottomEdge))
                    return Qt.SizeBDiagCursor
                return (e & (Qt.LeftEdge | Qt.RightEdge)) ? Qt.SizeHorCursor : Qt.SizeVerCursor
            }
        }
        DragHandler {
            target: null
            // The header's drag lies under the top edge and corners; it
            // must not take a resize over.
            grabPermissions: PointerHandler.TakeOverForbidden
            onActiveChanged: {
                if (!active)
                    return
                meter.userSized = true
                if (meter.placesItself) {
                    grip.startRect = Qt.rect(meter.x, meter.y, meter.width, meter.height)
                    // The press, on the screen: the window has not moved yet.
                    const at = Overlay.pointerOnScreen()
                    grip.startPointer = Qt.point(at.x - centroid.scenePosition.x + centroid.scenePressPosition.x,
                                                 at.y - centroid.scenePosition.y + centroid.scenePressPosition.y)
                } else if (meter.startSystemResize(grip.edges)) {
                    Overlay.releasePointer(meter)
                }
            }
            // A layer surface keeps its top-left corner, so the pointer's
            // travel in the window is the size change.
            onTranslationChanged: {
                if (active && meter.layerSurface)
                    meter.resizeFrom(grip.startRect, grip.edges, translation)
            }
            // An unmanaged window's left and top edges move it under the
            // pointer, so there the travel is measured on the screen.
            onCentroidChanged: {
                if (!active || !meter.unmanaged)
                    return
                const at = Overlay.pointerOnScreen()
                meter.resizeFrom(grip.startRect, grip.edges,
                                 Qt.point(at.x - grip.startPointer.x, at.y - grip.startPointer.y))
            }
        }
        // A double-click fits the height to the rows again.
        TapHandler {
            onDoubleTapped: meter.fitToRows()
        }
    }

    // The height fits the rows and follows them again, as before the first
    // resize; the width stays.
    function fitToRows() {
        userSized = false
        height = naturalHeight
    }

    // A meter that sizes itself: the start rectangle's `edges` move by
    // `travel`, down to the smallest size; the other edges stay put.
    function resizeFrom(start, edges, travel) {
        let left = start.x
        let top = start.y
        let right = start.x + start.width
        let bottom = start.y + start.height
        if (edges & Qt.LeftEdge)
            left = Math.min(Math.round(left + travel.x), right - minimumWidth)
        if (edges & Qt.RightEdge)
            right = Math.max(Math.round(right + travel.x), left + minimumWidth)
        if (edges & Qt.TopEdge)
            top = Math.min(Math.round(top + travel.y), bottom - minimumHeight)
        if (edges & Qt.BottomEdge)
            bottom = Math.max(Math.round(bottom + travel.y), top + minimumHeight)
        // One request, so the far edges do not jump.
        if (unmanaged)
            setGeometry(left, top, right - left, bottom - top)
        else {
            width = right - left
            height = bottom - top
        }
    }

    Rectangle {
        id: panel
        anchors.fill: parent
        visible: meter.shown
        radius: Theme.meter.part(6)
        color: Theme.panel
        border.color: Theme.panelEdge
        border.width: 1

        ColumnLayout {
            anchors.fill: parent
            anchors.margins: 1
            spacing: 0

            // Header: state dot, fight time, target, enrage countdown, time
            // to kill.
            Item {
                Layout.fillWidth: true
                Layout.preferredHeight: Theme.meter.headerHeight

                DragHandler {
                    id: headerDrag
                    // Where an unmanaged window's drag took hold of it.
                    property point grab
                    target: null
                    enabled: !Settings.locked
                    onActiveChanged: {
                        if (meter.unmanaged) {
                            if (active) {
                                headerDrag.grab = centroid.scenePressPosition
                                meter.placeAt(Overlay.pointerOnScreen(), headerDrag.grab)
                            }
                        } else if (!meter.layerSurface) {
                            if (active)
                                meter.startSystemMove()
                        } else if (active) {
                            Overlay.beginLayerDrag(centroid.scenePressPosition,
                                Qt.size(meter.width, meter.height), meter.placeArea)
                            Overlay.layerDragTo(meter, centroid.scenePosition)
                        } else {
                            Overlay.endLayerDrag()
                        }
                    }
                    // A layer surface is placed from the pointer's spot on
                    // the screen; see layer_drag.rs.
                    onCentroidChanged: {
                        if (!active)
                            return
                        if (meter.layerSurface)
                            Overlay.layerDragTo(meter, centroid.scenePosition)
                        else if (meter.unmanaged)
                            meter.placeAt(Overlay.pointerOnScreen(), headerDrag.grab)
                    }
                }

                RowLayout {
                    anchors.fill: parent
                    anchors.leftMargin: Theme.meter.px(10)
                    anchors.rightMargin: Theme.meter.px(10)
                    spacing: Theme.meter.px(8)

                    // The state dot, on a dark ring like the text's outline.
                    Rectangle {
                        implicitWidth: Theme.meter.part(7)
                        implicitHeight: Theme.meter.part(7)
                        radius: Theme.meter.part(3.5)
                        color: Capture.suspended ? Theme.meterNote : Theme.accent
                        Rectangle {
                            z: -1
                            anchors.centerIn: parent
                            width: Theme.meter.part(9)
                            height: Theme.meter.part(9)
                            radius: Theme.meter.part(4.5)
                            color: Theme.outline
                        }
                    }
                    HudText {
                        numeric: true
                        text: Meter.fightTime
                        color: Theme.number
                        font.pixelSize: Theme.meter.sizeRow
                        font.weight: Font.DemiBold
                    }
                    // As wide as the name, the countdown and the time to kill
                    // right after it. In whole pixels: the layout's, a
                    // fraction short, elided it.
                    HudText {
                        Layout.fillWidth: true
                        Layout.maximumWidth: Math.ceil(implicitWidth)
                        text: meter.shotNote !== "" ? meter.shotNote
                            : Capture.suspended ? I18n.tr("battleTime.suspended") : I18n.targetName(Meter.mode, Meter.target)
                        color: meter.shotNote !== "" || Capture.suspended ? Theme.meterNote : Theme.hudName
                        font.pixelSize: Theme.meter.px(Settings.targetNameSize)
                        font.weight: Font.DemiBold
                    }
                    // The time left until the boss's timed enrage, in the
                    // health bar's colours as it runs down.
                    HudText {
                        visible: Meter.enrage !== "" && !Capture.suspended
                        numeric: true
                        text: Meter.enrage
                        color: Theme.pingColors[Meter.enrageLevel]
                        font.pixelSize: Theme.meter.sizeRow
                        font.weight: Font.DemiBold
                    }
                    // The time to kill, then a tick while it comes before the
                    // enrage (green) or a cross when not (red); Pretendard has
                    // both glyphs (U+2713, U+2717).
                    HudText {
                        visible: Meter.bossEta !== "" && !Capture.suspended
                        numeric: true
                        text: Meter.bossEta + (Meter.killMark !== "" ? " " + Meter.killMark : "")
                        color: Meter.killMark !== "" ? Theme.pingColors[Meter.killMarkLevel] : Theme.number
                        font.pixelSize: Theme.meter.sizeRow
                        font.weight: Font.DemiBold
                        UiTip {
                            delay: Theme.meterTipRest
                            text: I18n.tr("meter.timeToKillTip")
                        }
                    }
                    Item {
                        Layout.fillWidth: true
                    }
                    // Drawn, not glyphs, so each mark can sit on a dark copy, like the
                    // text's outline.
                    // Suspend and lock sit here only when their "Show ... button"
                    // setting is on; otherwise they are in the ⋯ menu (the
                    // padlock still shows while locked).
                    HeaderButton {
                        id: suspendButton
                        visible: Settings.showSuspendButton
                        onClicked: Capture.suspended = !Capture.suspended
                        UiTip {
                            delay: Theme.meterTipRest
                            text: Capture.suspended ? I18n.tr("meter.resume") : I18n.tr("meter.suspend") + "\n" + I18n.tr("meter.suspendTip")
                        }
                        readonly property color ink: hovered ? Theme.hudName : Theme.hudSub
                        // Two bars, each on a dark copy one pixel wider on
                        // every side; paused, a play mark to press instead.
                        Repeater {
                            model: Capture.suspended ? [] : [-3, 3]
                            Rectangle {
                                required property int modelData
                                anchors.centerIn: parent
                                anchors.horizontalCenterOffset: Theme.meter.part(modelData)
                                width: Theme.meter.part(4)
                                height: Theme.meter.part(12)
                                radius: Theme.meter.part(1)
                                color: Theme.outline
                            }
                        }
                        Repeater {
                            model: Capture.suspended ? [] : [-3, 3]
                            Rectangle {
                                required property int modelData
                                anchors.centerIn: parent
                                anchors.horizontalCenterOffset: Theme.meter.part(modelData)
                                width: Theme.meter.part(2)
                                height: Theme.meter.part(10)
                                color: suspendButton.ink
                            }
                        }
                        // The play mark: a triangle on its dark copy.
                        Canvas {
                            visible: Capture.suspended
                            anchors.fill: parent
                            readonly property color ink: suspendButton.ink
                            // Drawn again at the new size, never stretched.
                            readonly property real size: Theme.meter.scale
                            onInkChanged: requestPaint()
                            onVisibleChanged: requestPaint()
                            onSizeChanged: requestPaint()
                            onPaint: {
                                const g = getContext("2d")
                                g.reset()
                                g.scale(size, size)
                                g.beginPath()
                                g.moveTo(4.5, 3)
                                g.lineTo(13, 8)
                                g.lineTo(4.5, 13)
                                g.closePath()
                                g.lineJoin = "round"
                                g.lineWidth = 2
                                g.strokeStyle = Theme.css(Theme.outline)
                                g.stroke()
                                g.fillStyle = Theme.css(ink)
                                g.fill()
                            }
                        }
                    }
                    // A locked meter always shows the padlock, so it does not
                    // look frozen; it is a button only with "Show lock button".
                    HeaderButton {
                        id: lockButton
                        visible: Settings.locked || (Settings.showLockButton && meter.canLock)
                        clickable: Settings.showLockButton
                        onClicked: Settings.locked = !Settings.locked
                        UiTip {
                            delay: Theme.meterTipRest
                            text: !lockButton.clickable ? ""
                                : Settings.locked ? I18n.tr("meter.unlock") : I18n.tr("meter.lock") + "\n" + I18n.tr("meter.lockTip")
                        }
                        // A padlock: a shackle over a body, on a dark copy one
                        // pixel wider on every side. Unlocked, the shackle
                        // rises 2 px and its right leg leaves the body.
                        readonly property color ink: hovered || Settings.locked ? Theme.hudName : Theme.hudSub
                        // The middle of the shackle's arc, where its legs start.
                        readonly property real arcMiddle: Theme.meter.part(Settings.locked ? 5 : 3)
                        // The arc is the top half of a ring; the dark copy's
                        // half reaches a pixel lower, so an open end has one too.
                        Item {
                            x: Theme.meter.part(3)
                            y: lockButton.arcMiddle - Theme.meter.part(5)
                            width: Theme.meter.part(10)
                            height: Theme.meter.part(6)
                            clip: true
                            Rectangle {
                                width: Theme.meter.part(10)
                                height: Theme.meter.part(10)
                                radius: Theme.meter.part(5)
                                color: "transparent"
                                border.width: Theme.meter.part(3.5)
                                border.color: Theme.outline
                            }
                        }
                        Repeater {
                            model: Settings.locked ? [3, 9.5] : [3]
                            Rectangle {
                                required property real modelData
                                x: Theme.meter.part(modelData)
                                y: lockButton.arcMiddle
                                width: Theme.meter.part(3.5)
                                height: Theme.meter.part(7) - y
                                color: Theme.outline
                            }
                        }
                        Item {
                            x: Theme.meter.part(4)
                            y: lockButton.arcMiddle - Theme.meter.part(4)
                            width: Theme.meter.part(8)
                            height: Theme.meter.part(4)
                            clip: true
                            Rectangle {
                                width: Theme.meter.part(8)
                                height: Theme.meter.part(8)
                                radius: Theme.meter.part(4)
                                color: "transparent"
                                border.width: Theme.meter.part(1.5)
                                border.color: lockButton.ink
                            }
                        }
                        Repeater {
                            model: Settings.locked ? [4, 10.5] : [4]
                            Rectangle {
                                required property real modelData
                                x: Theme.meter.part(modelData)
                                y: lockButton.arcMiddle
                                width: Theme.meter.part(1.5)
                                height: Theme.meter.part(7) - y
                                color: lockButton.ink
                            }
                        }
                        Rectangle {
                            x: Theme.meter.part(2.5)
                            y: Theme.meter.part(6)
                            width: Theme.meter.part(11)
                            height: Theme.meter.part(8)
                            radius: Theme.meter.part(1.5)
                            color: lockButton.ink
                            border.width: 1
                            border.color: Theme.outline
                        }
                    }
                    HeaderButton {
                        id: menuButton
                        onClicked: meter.menuRequested(menuButton)
                        UiTip {
                            delay: Theme.meterTipRest
                            text: I18n.tr("meter.more")
                        }
                        Rectangle {
                            anchors.centerIn: parent
                            width: Theme.meter.part(12)
                            height: Theme.meter.part(4)
                            radius: Theme.meter.part(2)
                            color: Theme.outline
                        }
                        Row {
                            anchors.centerIn: parent
                            spacing: Theme.meter.part(2)
                            Repeater {
                                model: 3
                                Rectangle {
                                    width: Theme.meter.part(2)
                                    height: Theme.meter.part(2)
                                    radius: Theme.meter.part(1)
                                    color: menuButton.hovered ? Theme.hudName : Theme.hudSub
                                }
                            }
                        }
                    }
                    HeaderButton {
                        id: closeButton
                        onClicked: meter.closeRequested()
                        UiTip {
                            delay: Theme.meterTipRest
                            text: meter.closeHides ? I18n.tr("meter.hideToTray") : I18n.tr("tray.quit")
                        }
                        Repeater {
                            model: [45, -45]
                            Rectangle {
                                required property int modelData
                                anchors.centerIn: parent
                                width: Theme.meter.part(13)
                                height: Theme.meter.part(3.5)
                                radius: Theme.meter.part(1.75)
                                rotation: modelData
                                antialiasing: true
                                color: Theme.outline
                            }
                        }
                        Repeater {
                            model: [45, -45]
                            Rectangle {
                                required property int modelData
                                anchors.centerIn: parent
                                width: Theme.meter.part(11)
                                height: Theme.meter.part(1.5)
                                rotation: modelData
                                antialiasing: true
                                color: closeButton.hovered ? Theme.hudName : Theme.hudSub
                            }
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

            // Capture cannot start: a notice under the header until dismissed.
            Rectangle {
                id: notice
                Layout.fillWidth: true
                Layout.preferredHeight: noticeText.implicitHeight + Theme.meter.px(12)
                visible: Capture.notice
                color: "transparent"
                HudText {
                    id: noticeText
                    x: Theme.meter.px(10)
                    anchors.verticalCenter: parent.verticalCenter
                    width: parent.width - dismiss.width - Theme.meter.px(26)
                    // On Windows capture needs Npcap, which the installer
                    // cannot bring: a click opens its site.
                    readonly property bool npcap: Qt.platform.os === "windows"
                    text: npcap ? I18n.tr("connection.npcapMissing") : I18n.tr("connection.captureUnavailable")
                    color: Theme.warning
                    font.pixelSize: Theme.meter.sizeSmall
                    wrapMode: Text.WordWrap
                    // The help's address whole, also at 260 px in German.
                    maximumLineCount: 4
                    HoverHandler {
                        enabled: noticeText.npcap
                        cursorShape: Qt.PointingHandCursor
                    }
                    TapHandler {
                        enabled: noticeText.npcap
                        onTapped: Qt.openUrlExternally("https://npcap.com")
                    }
                }
                HudText {
                    id: dismiss
                    anchors.right: parent.right
                    anchors.rightMargin: Theme.meter.px(10)
                    anchors.verticalCenter: parent.verticalCenter
                    text: I18n.tr("connection.dismiss")
                    font.pixelSize: Theme.meter.sizeSmall
                    color: dismissHover.hovered ? Theme.hudName : Theme.hudSub
                    HoverHandler { id: dismissHover }
                    TapHandler { onTapped: Capture.notice = false }
                }
                Rectangle {
                    anchors.bottom: parent.bottom
                    width: parent.width
                    height: 1
                    color: Theme.hairline
                }
            }

            // The target's health, where the meter knows it (BOSS, TARGET):
            // the ping's green, yellow and red, by health left: green above
            // 50%, yellow down to 25%, red below.
            Rectangle {
                id: healthBar
                readonly property real share: Math.max(0, Meter.bossHealth)
                Layout.fillWidth: true
                Layout.preferredHeight: Theme.meter.bossBand
                visible: meter.hasBoss
                color: Theme.bossTrack
                Rectangle {
                    width: Math.round(parent.width * healthBar.share)
                    height: parent.height
                    color: Theme.pingColors[healthBar.share > 0.5 ? 1 : healthBar.share >= 0.25 ? 2 : 3]
                    opacity: 0.75
                }
                // Its text, as Settings > Meter > Boss health text places it:
                // the percent, the health left and the most in the middle
                // (1.x's Classic layout), or the percent at the left end and
                // the health at the right (1.x's Beta layout).
                readonly property string percent: Math.round(healthBar.share * 100) + "%"
                readonly property bool centred: Settings.healthText === "centred"
                // Slim's line is too thin for text.
                Item {
                    anchors.fill: parent
                    visible: !Theme.slim
                    HudText {
                        visible: !healthBar.centred
                        x: Theme.meter.px(8)
                        height: parent.height
                        numeric: true
                        text: healthBar.percent
                        font.pixelSize: Theme.meter.sizeSmall
                        font.weight: Font.DemiBold
                        color: Theme.hudSub
                    }
                    HudText {
                        visible: !healthBar.centred
                        anchors.horizontalCenter: parent.horizontalCenter
                        height: parent.height
                        numeric: true
                        text: Meter.bossHpText
                        font.pixelSize: Theme.meter.sizeSmall
                        // Small text on the meter stays full white (Visual rules).
                        color: Theme.hudSub
                    }
                    HudText {
                        visible: healthBar.centred
                        anchors.horizontalCenter: parent.horizontalCenter
                        height: parent.height
                        numeric: true
                        text: healthBar.percent + "  " + Meter.bossHpText
                        font.pixelSize: Theme.meter.sizeSmall
                        font.weight: Font.DemiBold
                        color: Theme.hudSub
                    }
                }
            }

            // Column heads, on the rows' columns. Slim mode has none.
            ColumnHeads {
                Layout.fillWidth: true
                Layout.preferredHeight: Theme.meter.headsHeight
                visible: !Theme.slim
                first: I18n.tr("meter.columns.name")
                firstTip: I18n.tr("meter.columns.name") + "\n" + I18n.tr("meter.columnTips.name")
                columns: meter.columns
            }

            // Players: the tab's rows; in the split view, healers below.
            Item {
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                Column {
                    id: rowList
                    x: Theme.meter.px(6)
                    y: Theme.meter.px(4)
                    width: parent.width - 2 * x
                    spacing: Theme.meter.rowGap
                    // DPS: the backend's rows.
                    Repeater {
                        id: dpsRows
                        model: meter.tab === 0 ? Meter : 0
                        delegate: MeterRow {
                            id: dpsRow
                            // Details opens on this player.
                            required property int entityId
                            // The chosen columns' texts, from the Meter.
                            required property var texts
                            required property real shareValue
                            required property bool ticksOnly
                            width: rowList.width
                            dashTip: ticksOnly ? I18n.tr("meter.noHitsTip") : ""
                            lowShare: shareValue < 3 ? 2 : shareValue < 5 ? 1 : 0
                            // Without the % column, the first number.
                            shareCell: Math.max(0, meter.tabKeys.indexOf("pct"))
                            cells: texts
                            widths: meter.columnWidths
                            kinds: meter.columnKinds
                            onClicked: meter.detailsRequested(meter.playerOf(dpsRow))
                            onPointerOver: on => meter.rowHovered(on ? meter.playerOf(dpsRow) : null, dpsRow)
                        }
                    }
                    // Heal and Tank.
                    Repeater {
                        model: [0, healRows, tankRows][meter.tab]
                        delegate: MeterRow {
                            id: partyRow
                            required property int index
                            // Details opens on this player.
                            required property int entityId
                            width: rowList.width
                            cells: meter.partyTexts(meter.tab, index)
                            widths: meter.columnWidths
                            kinds: meter.columnKinds
                            onClicked: meter.detailsRequested(meter.playerOf(partyRow))
                            onPointerOver: on => meter.rowHovered(on ? meter.playerOf(partyRow) : null, partyRow)
                        }
                    }
                    // Own line: the group's rate and damage under the DPS rows.
                    Item {
                        visible: meter.damageLine
                        width: rowList.width
                        height: Theme.meter.px(14)
                        Row {
                            anchors.right: parent.right
                            anchors.rightMargin: Theme.meter.padRight
                            height: parent.height
                            spacing: Theme.meter.px(5)
                            SmallText {
                                height: parent.height
                                numeric: true
                                text: meter.groupTotal
                                color: Theme.number
                                font.weight: Font.DemiBold
                            }
                            SmallText {
                                height: parent.height
                                text: "·"
                            }
                            SmallText {
                                height: parent.height
                                numeric: true
                                text: Meter.groupDamage
                            }
                        }
                    }
                    ColumnHeads {
                        width: rowList.width + 2 * rowList.x
                        x: -rowList.x
                        height: Theme.meter.headsHeight + Theme.meter.px(4)
                        visible: meter.splitShown
                        rule: true
                        first: I18n.tr("meter.columns.healer")
                        firstTip: I18n.tr("meter.columns.healer") + "\n" + I18n.tr("meter.columnTips.healer")
                        columns: meter.healerColumns
                    }
                    SmallText {
                        visible: meter.splitShown && !meter.split
                        x: Theme.meter.padLeft + Theme.meter.colIcon + Theme.meter.colGap
                        height: Theme.meter.rowHeight
                        text: I18n.tr("meter.noHealer")
                        color: Theme.meterNote
                    }
                    Repeater {
                        model: meter.split ? healerRows : 0
                        delegate: MeterRow {
                            id: healerRow
                            required property int index
                            required property int entityId
                            width: rowList.width
                            cells: meter.partyTexts(3, index)
                            widths: meter.healerColumns.map(c => c.width)
                            kinds: meter.healerColumns.map(c => c.kind)
                            onClicked: meter.detailsRequested(meter.playerOf(healerRow))
                            onPointerOver: on => meter.rowHovered(on ? meter.playerOf(healerRow) : null, healerRow)
                        }
                    }
                }
            }

            // Footer: tabs and mode on the left, group total and ping right.
            Item {
                Layout.fillWidth: true
                Layout.preferredHeight: Theme.meter.footerHeight

                Rectangle {
                    width: parent.width
                    height: 1
                    color: Theme.hairline
                }

                RowLayout {
                    anchors.fill: parent
                    anchors.leftMargin: Theme.meter.px(8)
                    anchors.rightMargin: Theme.meter.px(8)
                    spacing: Theme.meter.px(2)

                    Repeater {
                        model: [I18n.tr("meter.tabs.dps"), I18n.tr("meter.tabs.heal"), I18n.tr("meter.tabs.tank")]
                        Rectangle {
                            id: tabButton
                            required property string modelData
                            required property int index
                            readonly property bool active: meter.tab === index
                            implicitWidth: tabLabel.implicitWidth + Theme.meter.px(14)
                            implicitHeight: tabLabel.implicitHeight + Theme.meter.px(6)
                            radius: Theme.meter.part(3)
                            color: active ? Theme.accentTint : "transparent"
                            SmallText {
                                id: tabLabel
                                anchors.centerIn: parent
                                text: tabButton.modelData
                                color: tabButton.active ? Theme.accent : Theme.hudSub
                                font.weight: tabButton.active ? Font.DemiBold : Font.Normal
                            }
                            TapHandler {
                                onTapped: meter.tab = tabButton.index
                            }
                        }
                    }
                    // The mode is the first to give way when the footer is
                    // short of room (ENTRENAMIENTO): the header names the
                    // fight too. It opens the list of the five modes.
                    Rectangle {
                        id: modeButton
                        Layout.leftMargin: Theme.meter.px(8)
                        Layout.fillWidth: true
                        // Whole pixels: the layout rounds a filling item down,
                        // which would cut the name short.
                        Layout.preferredWidth: Math.ceil(modeLabel.implicitWidth) + 2 * modeLabel.anchors.leftMargin
                        Layout.maximumWidth: Layout.preferredWidth
                        implicitHeight: modeLabel.implicitHeight + Theme.meter.px(6)
                        radius: Theme.meter.part(3)
                        color: modeHover.hovered ? Theme.rowEmpty : "transparent"
                        border.color: Theme.hairline
                        border.width: 1
                        SmallText {
                            id: modeLabel
                            anchors.fill: parent
                            anchors.leftMargin: Theme.meter.px(6)
                            anchors.rightMargin: Theme.meter.px(6)
                            horizontalAlignment: Text.AlignHCenter
                            text: I18n.modeName(Meter.mode)
                        }
                        HoverHandler {
                            id: modeHover
                        }
                        TapHandler {
                            onTapped: meter.modeMenuRequested(modeButton)
                        }
                    }
                    Item {
                        Layout.fillWidth: true
                    }
                    SmallText {
                        numeric: true
                        visible: Settings.showTotal && !meter.damageLine
                        text: meter.groupTotal
                        color: Theme.number
                        font.weight: Font.DemiBold
                    }
                    // Footer: the damage after the rate.
                    SmallText {
                        visible: meter.showsDamage && !meter.damageLine
                        Layout.leftMargin: Theme.meter.px(3)
                        text: "·"
                    }
                    SmallText {
                        visible: meter.showsDamage && !meter.damageLine
                        Layout.leftMargin: Theme.meter.px(3)
                        numeric: true
                        text: Meter.groupDamage
                    }
                    // The number itself is coloured by its band; "--ms" in
                    // the note colour until the first ping comes.
                    SmallText {
                        Layout.leftMargin: Theme.meter.px(8)
                        numeric: true
                        visible: Settings.showPing
                        text: Meter.ping !== "" ? Meter.ping : "--ms"
                        color: Theme.pingColors[Meter.pingLevel]
                    }
                }
            }
        }
    }

    // Resize grips along the edges, corners on top.
    ResizeGrip { edges: Qt.LeftEdge; x: 0; y: 10; width: 5; height: parent.height - 20 }
    ResizeGrip { edges: Qt.RightEdge; x: parent.width - 5; y: 10; width: 5; height: parent.height - 20 }
    ResizeGrip { edges: Qt.TopEdge; x: 10; y: 0; width: parent.width - 20; height: 4 }
    ResizeGrip { edges: Qt.BottomEdge; x: 10; y: parent.height - 5; width: parent.width - 20; height: 5 }
    ResizeGrip { edges: Qt.LeftEdge | Qt.TopEdge; x: 0; y: 0; width: 10; height: 10 }
    ResizeGrip { edges: Qt.RightEdge | Qt.TopEdge; x: parent.width - 10; y: 0; width: 10; height: 10 }
    ResizeGrip { edges: Qt.LeftEdge | Qt.BottomEdge; x: 0; y: parent.height - 10; width: 10; height: 10 }
    ResizeGrip { edges: Qt.RightEdge | Qt.BottomEdge; x: parent.width - 10; y: parent.height - 10; width: 10; height: 10 }
}
