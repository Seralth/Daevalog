// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import Daevalog

// A window of the meter other than the overlay (Details, History,
// Settings): a normal desktop window, so the desktop moves, sizes, tiles
// and closes it, and solid, since it is read off the game.
Window {
    id: window

    // The page shown, for windows with more than one ("record", "report").
    property string page: ""
    // The name its place and size are kept under ("details"); empty: none.
    property string placeName: ""
    // Off while something else places it (Show Details on monitor): that
    // place is the setting's, not the player's, and is not kept, as 1.x.
    property bool keepPlace: true
    property bool placed: false
    // Wayland lets no window read or pick its place: only the size.
    readonly property bool sizeOnly: Qt.platform.pluginName === "wayland"

    visible: false
    color: Theme.window

    function present(page) {
        if (page !== undefined)
            window.page = page
        restorePlace()
        show()
        raise()
        requestActivate()
    }

    // The place and size of the last time, on a screen that is there now.
    function restorePlace() {
        if (placed || placeName === "")
            return
        placed = true
        const screens = Array.from(Qt.application.screens).map(s => [s.virtualX, s.virtualY, s.width, s.height])
        const text = Windows.place(placeName, JSON.stringify(screens), minimumWidth, minimumHeight)
        if (text === "")
            return
        const saved = JSON.parse(text)
        width = saved.width
        height = saved.height
        if (!sizeOnly && saved.x !== null) {
            x = saved.x
            y = saved.y
        }
    }
    function keepPlaceNow() {
        if (placed && keepPlace && placeName !== "" && visibility !== Window.Maximized && visibility !== Window.FullScreen)
            Windows.keep(placeName, x, y, width, height, sizeOnly)
    }
    Timer {
        id: keepTimer
        interval: 600
        onTriggered: window.keepPlaceNow()
    }
    function placeMoved() {
        if (visible && placed)
            keepTimer.restart()
    }
    onXChanged: placeMoved()
    onYChanged: placeMoved()
    onWidthChanged: placeMoved()
    onHeightChanged: placeMoved()
    onVisibleChanged: {
        if (!visible)
            keepPlaceNow()
    }
}
