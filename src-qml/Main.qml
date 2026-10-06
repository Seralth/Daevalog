// SPDX-License-Identifier: MIT
import QtQuick
import Qt.labs.platform as Platform
import Daevalog

// Starts the meter window and the tray icon. With --screenshot it saves the
// window as a PNG and quits instead.
QtObject {
    id: app

    property var meter: null
    readonly property bool screenshotMode: Overlay.screenshotPath !== ""

    function createMeter() {
        let component = null
        if (Overlay.layerShell) {
            component = Qt.createComponent("LayerMeterWindow.qml")
            if (component.status !== Component.Ready) {
                console.warn("Layer shell unavailable, using a normal window:", component.errorString())
                Overlay.layerShell = false
                component = null
            }
        }
        if (!component)
            component = Qt.createComponent("MeterWindow.qml")
        if (component.status !== Component.Ready) {
            console.error(component.errorString())
            Qt.exit(1)
            return
        }
        meter = component.createObject(app)
        meter.menuRequested.connect(() => windowMenu.open())
        meter.closeRequested.connect(closeMeter)
    }

    function toggleMeter() {
        if (meter)
            meter.visible = !meter.visible
    }

    // Without a tray there would be no way back, so close means quit.
    function closeMeter() {
        if (tray.available && tray.visible)
            meter.visible = false
        else
            Qt.quit()
    }

    component LockMenu: Platform.Menu {
        Platform.MenuItem {
            text: Overlay.locked ? qsTr("Unlock") : qsTr("Lock")
            onTriggered: Overlay.locked = !Overlay.locked
        }
        Platform.MenuSeparator {}
        Platform.MenuItem {
            text: qsTr("Quit")
            onTriggered: Qt.quit()
        }
    }

    readonly property Platform.SystemTrayIcon tray: Platform.SystemTrayIcon {
        visible: !app.screenshotMode
        icon.source: "qrc:/daevalog/icons/app.png"
        tooltip: "Daevalog"
        menu: LockMenu {}
        onActivated: reason => {
            if (reason === Platform.SystemTrayIcon.Trigger)
                app.toggleMeter()
        }
    }

    readonly property Platform.Menu windowMenu: LockMenu {}

    readonly property Timer screenshot: Timer {
        interval: 250
        running: app.screenshotMode && app.meter !== null
        onTriggered: {
            const saved = Overlay.saveScreenshot(app.meter)
            if (!saved)
                console.error("Could not save the screenshot to", Overlay.screenshotPath)
            Qt.exit(saved ? 0 : 1)
        }
    }

    Component.onCompleted: createMeter()
}
