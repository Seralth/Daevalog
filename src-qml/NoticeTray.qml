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
// down to (a Field Boss or a Spacetime Rift) shows the minutes left where
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

    // Narrower than the meter, so it reads as a card behind it.
    readonly property int inset: 10
    readonly property int barHeight: 24
    readonly property int listMax: 260
    // The list's height at most: listMax, or less where the screen has less
    // room on the tray's side (set by place()).
    property int listRoom: listMax
    readonly property int listHeight: items.length > 0 ? Math.min(list.contentHeight, listRoom) : empty.implicitHeight + 12
    readonly property int wantedHeight: barHeight + (open ? listHeight + 4 : 0)
    readonly property bool wanted: meter !== null && meter.visible && meter.shown
    readonly property bool locked: Settings.locked
    readonly property bool wayland: Qt.platform.pluginName.startsWith("wayland")
    // A Wayland popup takes its place only when it is shown.
    readonly property bool showAgain: wayland

    // "top", "bottom", or "auto": the top, unless the largest open card has
    // no room there and the bottom has more. The side is picked for the
    // largest card, so it does not change when the card opens.
    readonly property string edge: Settings.trayEdge
    readonly property int fullHeight: barHeight + listMax + 4
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
        listRoom = room ? Math.max(38, Math.min(listMax, (below ? room.below : room.above) - barHeight - 4)) : listMax
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
    onLockedChanged: shapeInput()

    function toggle() {
        open = !open
    }

    component Cross: Item {
        property color color: Theme.hudSub
        implicitWidth: 8
        implicitHeight: 8
        Rectangle {
            anchors.centerIn: parent
            width: 9
            height: 1.5
            rotation: 45
            antialiasing: true
            color: parent.color
        }
        Rectangle {
            anchors.centerIn: parent
            width: 9
            height: 1.5
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
        radius: 6
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
            width: (clearAll.visible ? clearAll.x - 4 : chevron.x)
            height: parent.height
            TapHandler {
                enabled: !tray.locked
                onTapped: tray.toggle()
            }
        }

        Row {
            visible: !tray.open
            x: 8
            anchors.verticalCenter: parent.verticalCenter
            spacing: 6
            Repeater {
                model: tray.kinds
                Row {
                    id: kindMark
                    required property var modelData
                    spacing: 3
                    NoticeIcon {
                        kind: kindMark.modelData.kind
                        anchors.verticalCenter: parent.verticalCenter
                        width: 18
                        height: 18
                    }
                    HudText {
                        visible: kindMark.modelData.count > 1
                        text: kindMark.modelData.count
                        numeric: true
                        font.pixelSize: Theme.sizeRow
                        color: Theme.hudSub
                        anchors.verticalCenter: parent.verticalCenter
                    }
                }
            }
        }

        HudText {
            visible: tray.open
            x: 8
            anchors.verticalCenter: parent.verticalCenter
            width: clearAll.x - x - 8
            text: I18n.tr("notices.title")
            font.pixelSize: Theme.sizeRow
            color: Theme.hudSub
        }

        HudText {
            id: clearAll
            visible: tray.open && tray.items.length > 0
            anchors.right: chevron.left
            anchors.rightMargin: 10
            anchors.verticalCenter: parent.verticalCenter
            text: I18n.tr("notices.clearAll")
            font.pixelSize: Theme.sizeRow
            color: clearHover.hovered ? Theme.hudName : Theme.hudSub
            HoverHandler { id: clearHover }
            TapHandler { onTapped: Notices.clear() }
        }

        Item {
            id: chevron
            anchors.right: parent.right
            width: 26
            height: parent.height
            HoverHandler { id: chevronHover }
            // Locked too: the input region keeps it clickable.
            TapHandler { onTapped: tray.toggle() }
            Chevron {
                anchors.centerIn: parent
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
        y: tray.below ? 8 : tray.barHeight + 8
        x: 8
        width: parent.width - 16
        text: I18n.tr("notices.none")
        font.pixelSize: Theme.sizeRow
        color: Theme.meterNote
    }

    ListView {
        id: list
        visible: tray.open
        y: tray.below ? 2 : tray.barHeight + 2
        width: parent.width
        height: tray.listHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        model: tray.items

        delegate: Item {
            id: line
            required property var modelData
            width: ListView.view.width
            height: Math.max(38, words.implicitHeight + 10)

            // Centred on the whole notice, title and text, about two lines tall.
            NoticeIcon {
                id: mark
                x: 8
                anchors.verticalCenter: parent.verticalCenter
                width: 28
                height: 28
                kind: line.modelData.kind
            }
            Column {
                id: words
                x: mark.x + mark.width + 8
                y: 5
                width: time.x - x - 8
                HudText {
                    width: parent.width
                    text: line.modelData.title
                    font.pixelSize: Theme.sizeRow
                    font.weight: Font.DemiBold
                    color: Theme.hudName
                }
                HudText {
                    visible: text !== ""
                    width: parent.width
                    text: line.modelData.text
                    font.pixelSize: Theme.sizeRow
                    color: Theme.hudShare
                    wrapMode: Text.WordWrap
                    // A Field Boss notice holds a line per map.
                    maximumLineCount: line.modelData.until > 0 ? 6 : 3
                }
            }
            HudText {
                id: time
                anchors.right: dismiss.left
                anchors.rightMargin: 6
                y: 5
                text: line.modelData.until > 0
                    ? tray.countdown(line.modelData.until)
                    : I18n.clock(line.modelData.hour, line.modelData.minute, Settings.hour24)
                font.pixelSize: Theme.sizeRow
                color: Theme.meterNote
            }
            Item {
                id: dismiss
                anchors.right: parent.right
                y: 2
                width: 26
                height: 20
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
