// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import Daevalog

// The notice tray: a card that sits on the meter's top edge and pulls up
// from it, or on its bottom edge and pulls down (Settings > Notifications >
// Notification tray). Closed, a slim bar shows an icon for each kind of notice
// waiting (Notices) and a chevron; open, the card lists them, newest first,
// each with its own dismiss, and a "Clear all". A new notice only adds its
// icon: nothing opens over the game by itself. A notice with a time to count
// down to (a boss or a Spacetime Rift) shows the minutes left where
// others show the time they came.
//
// Its own window, as the hover card, so the meter keeps its place and
// size: a popup of the meter, which on Wayland moves with it by itself.
Window {
    id: tray

    // Set by Main.
    property Window meter: null
    property bool open: false

    readonly property var items: JSON.parse(Notices.items)
    // Unix seconds, for the countdowns; kept current while the card is open.
    property real now: Date.now() / 1000
    Timer {
        interval: 10000
        repeat: true
        running: tray.visible && tray.open
        triggeredOnStart: true
        onTriggered: tray.now = Date.now() / 1000
    }
    function countdown(until) {
        const minutes = Math.ceil((until - now) / 60)
        return minutes > 0 ? I18n.tr("notices.countdown").replace("{minutes}", minutes) : I18n.tr("notices.now")
    }
    // Each kind once, newest first, with how many of it wait.
    readonly property var kinds: {
        const out = []
        for (let i = 0; i < items.length; i++) {
            const kind = items[i].kind
            const known = out.find(k => k.kind === kind)
            if (known)
                known.count++
            else
                out.push({ "kind": kind, "count": 1 })
        }
        return out
    }

    // Narrower than the meter, so it reads as a card behind it. Its sizes
    // follow the Meter size setting, as the meter's do (Theme.meter).
    readonly property int inset: Theme.meter.px(10)
    readonly property int barHeight: Theme.meter.px(24)
    // The open card shows a page of notices, whole: at most this many, from
    // the one at `first`. A line under them pages on (fit()).
    readonly property int maxRows: 3
    readonly property int moreHeight: Theme.meter.px(20)
    // A notice is at least this tall, and so is the list where the screen
    // has little room.
    readonly property int rowMin: Theme.meter.px(38)
    // A notice's text runs to this many lines at most: a countdown's (an
    // Abyss Boss notice holds a line per map, and a line may wrap) and any
    // other's (a pet's is three).
    readonly property int countdownLines: 6
    readonly property int textLines: 3
    // The open card's list at about its tallest, for picking the tray's
    // side: each notice a title and its text in the row font, with the
    // row's 10 px of margin; one countdown at its most lines and the others
    // at a pet's. Only an Abyss Boss notice runs that long, and two are
    // never on the tray at once: the game's bosses on both Reshanta maps
    // come at 21:00 and at 21:30, on different days. The titles are
    // DemiBold, which Pretendard sets as tall as the regular weight at every
    // Meter size (15.5, 17.9, 20.3 and 23.8 px), so the regular weight
    // measures both.
    FontMetrics {
        id: rowFont
        font.family: Theme.font
        font.pixelSize: Theme.meter.sizeRow
    }
    readonly property int listMax: (1 + countdownLines + (maxRows - 1) * (1 + textLines)) * Math.ceil(rowFont.height)
        + maxRows * Theme.meter.px(10) + moreHeight
    // The room for the list on the tray's side of the meter, where the
    // screen is known (set by place()); `noLimit` where it is not.
    readonly property int noLimit: 100000
    property int listRoom: noLimit
    property int first: 0
    // The notice at `first`, kept there as notices come and go.
    property int firstId: -1
    readonly property var page: items.slice(first, first + maxRows)
    // How many of the page fit (fit()).
    property int shown: 0
    readonly property int later: Math.max(0, items.length - first - shown)
    readonly property bool paged: first > 0 || later > 0
    readonly property int listHeight: items.length === 0 ? empty.implicitHeight + Theme.meter.px(12)
        : rowsBox.height + (paged ? moreHeight : 0)
    readonly property int wantedHeight: barHeight + (open ? listHeight + Theme.meter.px(4) : 0)
    readonly property bool wanted: meter !== null && meter.visible && meter.shown
    readonly property bool locked: Settings.locked
    readonly property bool wayland: Qt.platform.pluginName.startsWith("wayland")
    // A Wayland popup takes its place only when it is shown.
    readonly property bool showAgain: wayland

    // "top", "bottom", or "auto": the top, unless the largest open card has
    // no room there and the bottom has more. The side is picked for the
    // largest card, so it does not change when the card opens.
    readonly property string edge: Settings.trayEdge
    readonly property int fullHeight: barHeight + listMax + Theme.meter.px(4)
    // On the meter's bottom edge, pulling down.
    property bool below: false
    // What place() last asked for, so a Wayland popup is shown again only
    // when that changes. Not the meter's place: the popup moves with the
    // meter by itself, and one shown again at each step of a drag left
    // copies of itself along the way on Hyprland.
    property string asked: ""

    transientParent: meter
    flags: Qt.ToolTip | Qt.FramelessWindowHint | Qt.WindowDoesNotAcceptFocus
           | (locked && !Overlay.inputRegion ? Qt.WindowTransparentForInput : 0)
    color: "transparent"
    visible: false
    // Both set by place().
    width: 0
    height: barHeight

    // The room above and below the meter on its screen, where the meter
    // knows its place: a layer surface by its margins, which count from the
    // top of the area the bars leave; an X11 window by its position. A
    // normal Wayland window does not know where it is: null.
    function rooms() {
        if (meter.layerSurface) {
            const top = Overlay.layerMargins.top
            return { "above": top, "below": meter.placeArea.height - top - meter.height }
        }
        if (wayland)
            return null
        const top = meter.contentItem.mapToGlobal(0, 0).y - meter.screen.virtualY
        return { "above": top, "below": meter.screen.height - top - meter.height }
    }

    function place() {
        if (!wanted) {
            visible = false
            return
        }
        const room = rooms()
        // Auto without the room known asks the compositor: the open card on
        // the top edge, which it puts on the bottom edge where it does not
        // fit (onYChanged reads which it took); closed, the bar stays on the
        // edge the card took. With the room known the tray goes exactly
        // where asked: a compositor that slides it to fit measures from
        // where the meter is mid-animation (Hyprland) and leaves it over the
        // meter's header.
        const flip = !room && edge === "auto"
        const wantsBelow = edge === "bottom"
            || (edge === "auto" && (room ? room.above < fullHeight && room.below > room.above : !open && below))
        if (!flip)
            below = wantsBelow
        listRoom = room ? Math.max(rowMin, (below ? room.below : room.above) - barHeight - Theme.meter.px(4)) : noLimit
        const at = meter.contentItem.mapToGlobal(inset, wantsBelow ? meter.height : 0)
        const w = Math.max(0, meter.width - 2 * inset)
        const h = wantedHeight
        const key = [w, h, wantsBelow, wantsBelow || flip ? meter.height : 0].join()
        if (visible && showAgain && key !== asked)
            visible = false
        asked = key
        if (wayland)
            Overlay.placePopup(tray, Qt.rect(inset, 0, w, meter.height), wantsBelow, !room, flip)
        width = w
        height = h
        x = at.x
        y = wantsBelow ? at.y : at.y - h
        visible = true
        shapeInput()
    }
    // Auto without the room known: the edge the compositor gave the tray.
    onYChanged: {
        if (visible && wayland && !meter.layerSurface && edge === "auto")
            below = y + height / 2 > meter.y + meter.height / 2
    }
    // A Wayland popup that changes size slides off its place until it is
    // shown again: while the meter's width changes the tray hides, and it
    // comes back once the resize has settled.
    Timer {
        id: widthSettled
        interval: 150
        onTriggered: tray.place()
    }
    function meterWidthChanged() {
        if (!showAgain) {
            placeSoon()
            return
        }
        visible = false
        if (wanted)
            widthSettled.restart()
    }
    function placeSoon() {
        Qt.callLater(place)
    }
    onWantedChanged: placeSoon()
    onWantedHeightChanged: placeSoon()
    onMeterChanged: placeSoon()
    onEdgeChanged: placeSoon()
    Connections {
        target: tray.meter
        function onXChanged() { tray.placeSoon() }
        function onYChanged() { tray.placeSoon() }
        function onWidthChanged() { tray.meterWidthChanged() }
        // The bottom edge, and the room below it.
        function onHeightChanged() { tray.placeSoon() }
        function onPlaceAreaChanged() { tray.placeSoon() }
    }
    // A layer surface moves by its margins. Where its popups stay behind
    // when it moves (Hyprland), the tray is shown again once the meter has
    // stopped and its move animation has ended.
    Connections {
        target: Overlay
        function onLayerMarginsChanged() {
            tray.placeSoon()
            if (tray.meter && tray.meter.layerSurface && Overlay.layerPopupsLag && tray.visible)
                moveSettled.restart()
        }
    }
    Timer {
        id: moveSettled
        interval: 600
        onTriggered: {
            tray.visible = false
            tray.place()
        }
    }

    // Locked, only the chevron takes clicks, as the meter's lock control;
    // where no input region can be set, none of it does.
    function shapeInput() {
        if (Overlay.inputRegion && visible)
            Overlay.shapeInput(tray, locked ? chevron.mapToItem(null, 0, 0, chevron.width, chevron.height) : Qt.rect(0, 0, 0, 0))
    }
    onLockedChanged: {
        shapeInput()
        showFrom(0)
    }

    function toggle() {
        open = !open
    }

    component Cross: Item {
        property color color: Theme.hudSub
        implicitWidth: Theme.meter.part(8)
        implicitHeight: Theme.meter.part(8)
        Rectangle {
            anchors.centerIn: parent
            width: Theme.meter.part(9)
            height: Theme.meter.part(1.5)
            rotation: 45
            antialiasing: true
            color: parent.color
        }
        Rectangle {
            anchors.centerIn: parent
            width: Theme.meter.part(9)
            height: Theme.meter.part(1.5)
            rotation: -45
            antialiasing: true
            color: parent.color
        }
    }

    // The card: rounded on its far edge only; the other meets the meter.
    Rectangle {
        anchors.fill: parent
        anchors.topMargin: tray.below ? -radius : 0
        anchors.bottomMargin: tray.below ? 0 : -radius
        radius: Theme.meter.part(6)
        color: Theme.panel
        border.color: Theme.panelEdge
        border.width: 1
    }

    // The bar: the kinds waiting (closed) or the title (open), then
    // "Clear all" (open) and the chevron. On the card's far edge: it pulls
    // away from the meter as the card opens.
    Item {
        id: bar
        y: tray.below ? tray.height - height : 0
        width: parent.width
        height: tray.barHeight
        // Locked, the chevron's input region moves with it.
        onYChanged: tray.shapeInput()

        // The bar pulls the card up and down, as the chevron does; "Clear
        // all" and the chevron keep their own clicks.
        Item {
            width: (clearAll.visible ? clearAll.x - Theme.meter.px(4) : chevron.x)
            height: parent.height
            TapHandler {
                enabled: !tray.locked
                onTapped: tray.toggle()
            }
        }

        Row {
            visible: !tray.open
            x: Theme.meter.px(8)
            anchors.verticalCenter: parent.verticalCenter
            spacing: Theme.meter.px(6)
            Repeater {
                model: tray.kinds
                Row {
                    id: kindMark
                    required property var modelData
                    spacing: Theme.meter.px(3)
                    NoticeIcon {
                        kind: kindMark.modelData.kind
                        anchors.verticalCenter: parent.verticalCenter
                        width: Theme.meter.px(18)
                        height: Theme.meter.px(18)
                    }
                    HudText {
                        visible: kindMark.modelData.count > 1
                        text: kindMark.modelData.count
                        numeric: true
                        font.pixelSize: Theme.meter.sizeRow
                        color: Theme.hudSub
                        anchors.verticalCenter: parent.verticalCenter
                    }
                }
            }
        }

        HudText {
            visible: tray.open
            x: Theme.meter.px(8)
            anchors.verticalCenter: parent.verticalCenter
            width: clearAll.x - x - Theme.meter.px(8)
            text: I18n.tr("notices.title")
            font.pixelSize: Theme.meter.sizeRow
            color: Theme.hudSub
        }

        HudText {
            id: clearAll
            visible: tray.open && tray.items.length > 0
            anchors.right: chevron.left
            anchors.rightMargin: Theme.meter.px(10)
            anchors.verticalCenter: parent.verticalCenter
            text: I18n.tr("notices.clearAll")
            font.pixelSize: Theme.meter.sizeRow
            color: clearHover.hovered ? Theme.hudName : Theme.hudSub
            HoverHandler { id: clearHover }
            TapHandler { onTapped: Notices.clear() }
        }

        Item {
            id: chevron
            anchors.right: parent.right
            width: Theme.meter.px(26)
            height: parent.height
            HoverHandler { id: chevronHover }
            // Locked too: the input region keeps it clickable.
            TapHandler { onTapped: tray.toggle() }
            Chevron {
                anchors.centerIn: parent
                scale: Theme.meter.scale
                // Away from the meter to pull the card out, towards it to
                // put it away.
                turn: tray.open !== tray.below ? 0 : 180
                color: chevronHover.hovered ? Theme.hudName : Theme.hudSub
            }
        }
    }

    Rectangle {
        visible: tray.open
        y: tray.below ? bar.y - 1 : tray.barHeight
        x: 1
        width: parent.width - 2
        height: 1
        color: Theme.hairline
    }

    HudText {
        id: empty
        visible: tray.open && tray.items.length === 0
        y: tray.below ? Theme.meter.px(8) : tray.barHeight + Theme.meter.px(8)
        x: Theme.meter.px(8)
        width: parent.width - Theme.meter.px(16)
        text: I18n.tr("notices.none")
        font.pixelSize: Theme.meter.sizeRow
        color: Theme.meterNote
    }

    // How many notices of the page fit: all where they do, else as many as
    // fit above the line that pages on; at least one, cut where the screen
    // has no room for it.
    function fit() {
        const heights = []
        for (let i = 0; i < rows.count; i++) {
            const row = rows.itemAt(i)
            // Not built yet: its own onItemAdded calls fit() again.
            if (!row)
                return
            heights.push(row.height)
        }
        const fitting = room => {
            let n = 0
            let height = 0
            while (n < heights.length && height + heights[n] <= room)
                height += heights[n++]
            return n
        }
        let n = fitting(listRoom)
        // Not `paged`: that reads `shown`, which this sets.
        if (first > 0 || first + n < items.length)
            n = fitting(listRoom - moreHeight)
        shown = Math.max(1, n)
    }
    function fitSoon() {
        Qt.callLater(fit)
    }
    onListRoomChanged: fitSoon()

    function showFrom(index) {
        first = Math.max(0, Math.min(index, items.length - 1))
        firstId = first < items.length ? items[first].id : -1
    }
    // The first page stays on the newest notices; a later one keeps its top
    // notice as notices come, go or move to the top. The card opens, and the
    // meter locks, on the newest.
    onItemsChanged: {
        const at = first > 0 ? items.findIndex(n => n.id === firstId) : 0
        showFrom(at >= 0 ? at : first)
    }
    onOpenChanged: showFrom(0)

    Item {
        id: rowsBox
        visible: tray.open
        y: tray.below ? Theme.meter.px(2) : tray.barHeight + Theme.meter.px(2)
        width: parent.width
        // Cut only where the screen has no room for a single notice.
        height: Math.min(column.implicitHeight, tray.listRoom)
        clip: true

        // The wheel steps a notice at a time.
        property real turned: 0
        // One notch of a mouse wheel, in its eighths of a degree.
        readonly property int notch: 120
        WheelHandler {
            enabled: !tray.locked
            onWheel: event => {
                rowsBox.turned += event.angleDelta.y
                while (rowsBox.turned <= -rowsBox.notch) {
                    rowsBox.turned += rowsBox.notch
                    if (tray.later > 0)
                        tray.showFrom(tray.first + 1)
                }
                while (rowsBox.turned >= rowsBox.notch) {
                    rowsBox.turned -= rowsBox.notch
                    tray.showFrom(tray.first - 1)
                }
            }
        }

        Column {
            id: column
            width: parent.width
            Repeater {
                id: rows
                model: tray.page
                onItemAdded: tray.fitSoon()
                onItemRemoved: tray.fitSoon()
                Item {
                    id: line
                    required property var modelData
                    required property int index
                    width: column.width
                    height: Math.max(tray.rowMin, words.implicitHeight + Theme.meter.px(10))
                    visible: index < tray.shown
                    onHeightChanged: tray.fitSoon()

                    // Centred on the whole notice, title and text, about two lines tall.
                    NoticeIcon {
                        id: mark
                        x: Theme.meter.px(8)
                        anchors.verticalCenter: parent.verticalCenter
                        width: Theme.meter.px(28)
                        height: Theme.meter.px(28)
                        kind: line.modelData.kind
                    }
                    Column {
                        id: words
                        x: mark.x + mark.width + Theme.meter.px(8)
                        y: Theme.meter.px(5)
                        width: time.x - x - Theme.meter.px(8)
                        HudText {
                            width: parent.width
                            text: line.modelData.title
                            font.pixelSize: Theme.meter.sizeRow
                            font.weight: Font.DemiBold
                            color: Theme.hudName
                        }
                        HudText {
                            visible: text !== ""
                            width: parent.width
                            text: line.modelData.text
                            font.pixelSize: Theme.meter.sizeRow
                            color: Theme.hudShare
                            wrapMode: Text.WordWrap
                            maximumLineCount: line.modelData.until > 0 ? tray.countdownLines : tray.textLines
                        }
                    }
                    HudText {
                        id: time
                        anchors.right: dismiss.left
                        anchors.rightMargin: Theme.meter.px(6)
                        y: Theme.meter.px(5)
                        text: line.modelData.until > 0
                            ? tray.countdown(line.modelData.until)
                            : I18n.clock(line.modelData.hour, line.modelData.minute, Settings.hour24)
                        font.pixelSize: Theme.meter.sizeRow
                        color: Theme.meterNote
                    }
                    Item {
                        id: dismiss
                        anchors.right: parent.right
                        y: Theme.meter.px(2)
                        width: Theme.meter.px(26)
                        height: Theme.meter.px(20)
                        HoverHandler { id: dismissHover }
                        TapHandler {
                            enabled: !tray.locked
                            onTapped: Notices.dismiss(line.modelData.id)
                        }
                        Cross {
                            anchors.centerIn: parent
                            color: dismissHover.hovered ? Theme.hudName : Theme.hudSub
                        }
                    }
                }
            }
        }
    }

    // "3 more" pages down to them; on the last page, back to the top.
    HudText {
        visible: tray.open && tray.paged
        y: rowsBox.y + rowsBox.height + (tray.moreHeight - height) / 2
        x: Theme.meter.px(8)
        width: parent.width - Theme.meter.px(16)
        text: tray.later > 0 ? I18n.tr("notices.more").replace("{count}", tray.later) : I18n.tr("notices.top")
        font.pixelSize: Theme.meter.sizeRow
        color: moreHover.hovered ? Theme.hudName : Theme.meterNote
        HoverHandler { id: moreHover }
        TapHandler {
            enabled: !tray.locked
            onTapped: tray.showFrom(tray.later > 0 ? tray.first + tray.shown : 0)
        }
    }
}
