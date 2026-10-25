// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import Qt.labs.platform as Platform
import Daevalog

// Starts the meter window and the tray icon, and opens the other windows
// (Details, History, Settings, Character) when asked. With --screenshot it saves the
// window --open names (the meter by default) as a PNG and quits instead.
QtObject {
    id: app

    property var meter: null
    // The area a layer meter keeps inside (LayerArea.qml).
    property var layerArea: null
    property var details: null
    property var history: null
    property var settings: null
    property var character: null
    property var card: null
    // A locked meter can be unlocked again: from its own ⋯ or lock button,
    // which the input region keeps clickable, from the tray, or with the
    // lock key the desktop gave. Elsewhere the lock is not offered, as in
    // 1.x, or the meter would take no click until the next start.
    readonly property bool canLock: Overlay.inputRegion || (tray.available && tray.visible) || Hotkey.state === "bound"
    // The window --screenshot saves.
    property var shown: null
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
        // Nothing places an unmanaged window: it starts where a layer
        // surface does. A meter kept off the taskbar on X11 is shown once
        // that is set up.
        const offTaskbar = Overlay.offTaskbar || (Settings.hideFromTaskbar && Overlay.taskbarSetting)
        meter = component.createObject(app, Overlay.unmanaged ? { "x": 40, "y": 160 }
                                            : offTaskbar ? { "shown": false } : {})
        keepOffTaskbar(meter)
        if (offTaskbar)
            meter.shown = true
        // "Start in tray": hidden as the tray hides it, where there is a
        // tray to bring it back from.
        if (Settings.startInTray && tray.available && tray.visible)
            meter.shown = false
        meter.menuRequested.connect(button => meterMenu.open(button))
        meter.modeMenuRequested.connect(button => modeMenu.open(button))
        meter.menuOpen = Qt.binding(() => meterMenu.wanted || modeMenu.wanted)
        meter.closeRequested.connect(closeMeter)
        if (Overlay.layerShell) {
            layerArea = make("LayerArea.qml")
            if (layerArea)
                meter.layerArea = Qt.binding(() => layerArea.size)
        }
        meter.detailsRequested.connect(player => openDetails(player))
        meter.rowHovered.connect(showCard)
        // The meter hidden or on another tab under a resting pointer.
        meter.shownChanged.connect(() => showCard(null, null))
        meter.tabChanged.connect(() => showCard(null, null))
        meter.canLock = Qt.binding(() => app.canLock)
        meter.closeHides = Qt.binding(() => tray.available && tray.visible)
        keepUnlockable()
        // A screenshot asks the desktop nothing.
        if (!screenshotMode) {
            Hotkey.start(Settings.lockHotkey)
            makeNoticeTray()
        }
    }

    // The notice tray on the meter's top edge (NoticeTray.qml).
    property var noticeTray: null
    function makeNoticeTray() {
        if (noticeTray)
            return
        noticeTray = make("NoticeTray.qml")
        if (noticeTray)
            noticeTray.meter = meter
    }

    // A saved lock, or one set from Settings, only where it can be undone.
    function keepUnlockable() {
        if (Settings.locked && !canLock)
            Settings.locked = false
    }
    // The global lock key (Settings > Keybinds), through the desktop.
    readonly property Connections lockKey: Connections {
        target: Hotkey
        function onPressed() {
            Settings.locked = !Settings.locked
        }
    }
    readonly property Connections lockGuard: Connections {
        target: Settings
        function onLockedChanged() {
            app.keepUnlockable()
        }
    }

    // The other windows are made the first time they open.
    function make(file) {
        const component = Qt.createComponent(file)
        if (component.status !== Component.Ready) {
            console.error(component.errorString())
            return null
        }
        const window = component.createObject(app)
        // Details, History and Settings; the card is a tooltip window.
        if (window && typeof window.present === "function")
            keepOffTaskbar(window)
        return window
    }

    // "Keep out of the taskbar", for the meter and the other windows; the
    // meter keeps out anyway where it must (Overlay.offTaskbar). X11 only:
    // Wayland lets no app ask.
    function keepOffTaskbar(window) {
        Overlay.setOffTaskbar(window, Settings.hideFromTaskbar || (window === meter && Overlay.offTaskbar))
    }
    readonly property Connections taskbar: Connections {
        target: Settings
        function onHideFromTaskbarChanged() {
            for (const window of [app.meter, app.details, app.history, app.settings, app.character]) {
                if (window)
                    app.keepOffTaskbar(window)
            }
        }
    }

    // "Auto-hide": the meter hides while neither the game nor the meter is
    // in front, and comes back when one is; only a meter it hid.
    property bool autoHidden: false
    readonly property AutoHide autoHide: AutoHide {
        enabled: Settings.autoHide && Overlay.autoHideSetting && !app.screenshotMode
        gameInFront: () => Overlay.gameInFront()
        meterActive: app.meter ? app.meter.active : false
        onHiddenChanged: {
            if (!app.meter)
                return
            if (hidden && app.meter.shown) {
                app.autoHidden = true
                app.meter.shown = false
            } else if (!hidden && app.autoHidden) {
                app.autoHidden = false
                app.meter.shown = true
            }
        }
    }

    // Details on a meter row's player and the fight the meter shows; a
    // stand-in row (History, the Heal and Tank tabs) has no player in the
    // fight, so every player.
    function openDetails(player, page) {
        if (!details) {
            details = make("DetailsWindow.qml")
            details.historyRequested.connect(() => openHistory())
            details.meterWindow = Qt.binding(() => app.meter)
            // Closing the window "Show Details on monitor" keeps open turns
            // the setting off, as in 1.x, or it would open again next start.
            // A place on the monitor is the setting's: not kept.
            details.keepPlace = Qt.binding(() => !app.monitorDetails)
            details.closing.connect(() => {
                if (app.monitorDetails) {
                    // Next time, where the player last put it.
                    details.placed = false
                    app.monitorDetails = false
                    Settings.detailsMonitor = ""
                }
            })
        }
        if (player)
            showDetailsOf(player.entityId !== undefined ? player.entityId : 0)
        details.present(page)
        return details
    }

    // "Show Details on monitor" (X11 only; Wayland lets no window pick its
    // screen): Details stays open on that screen, on every player and every
    // target, as 1.x's live window. A screen that is not connected opens
    // nothing and keeps the setting (1.x turned it off), so it comes back
    // with the screen at the next start.
    property bool monitorDetails: false
    function applyDetailsMonitor() {
        if (app.screenshotMode || Qt.platform.pluginName === "wayland")
            return
        const screen = Array.from(Qt.application.screens).find(s => s.name === Settings.detailsMonitor)
        if (!screen) {
            if (app.monitorDetails && details) {
                app.monitorDetails = false
                details.close()
            }
            return
        }
        const window = openDetails(null)
        showDetailsOf(0)
        Details.pickTarget(0)
        app.monitorDetails = true
        window.screen = screen
        window.x = screen.virtualX + Math.max(0, (screen.width - window.width) / 2)
        window.y = screen.virtualY + Math.max(0, (screen.height - window.height) / 2)
        // Which screen this is, on the screen itself, as 1.x.
        window.showMonitorBadge(false)
    }
    readonly property Connections monitorSetting: Connections {
        target: Settings
        function onDetailsMonitorChanged() { app.applyDetailsMonitor() }
    }

    // Details on a fight saved in History, on you (every player when the
    // fight does not name you). Opening Details from the meter goes back to
    // the live fight.
    function openSavedDetails(fight) {
        const window = openDetails(null)
        Details.openSaved(fight, -1)
        return window
    }

    // `id` 0 is every player, -1 you.
    function showDetailsOf(id) {
        if (details)
            details.selectedKey = ""
        Details.open(id, Meter.targetId, Meter.target !== "", Settings.trainingMode === "all")
    }

    function openHistory(page) {
        if (!history) {
            history = make("HistoryWindow.qml")
            history.detailsRequested.connect(fight => openSavedDetails(fight))
        }
        history.present(page)
        return history
    }

    function openSettings(page) {
        if (!settings)
            settings = make("SettingsWindow.qml")
        settings.present(page)
        return settings
    }

    function openCharacter(page) {
        if (!character)
            character = make("CharacterWindow.qml")
        character.present(page)
        return character
    }

    // The card of a row's top skills, beside the meter while the pointer
    // rests on the row.
    function showCard(player, row) {
        if (!player || Settings.locked) {
            if (card)
                card.visible = false
            return
        }
        if (!card)
            card = make("SkillCard.qml")
        card.player = player
        const at = row.mapToGlobal(row.width + 12, 0)
        card.x = at.x
        card.y = at.y
        card.visible = true
    }

    // A second start of the meter asks for it (cpp/instance.cpp).
    function showMeter() {
        if (!meter)
            return
        meter.shown = true
        meter.raise()
        meter.requestActivate()
    }

    function toggleMeter() {
        if (meter)
            meter.shown = !meter.shown
    }

    // Without a tray there would be no way back, so close means quit; the
    // ✕ button's tooltip says which (MeterWindow `closeHides`).
    function closeMeter() {
        if (tray.available && tray.visible)
            meter.shown = false
        else
            Qt.quit()
    }

    // --open window/page: "meter/tank", "details/timeline", "settings/report".
    function openStartView() {
        const parts = Overlay.startView.split("/")
        const page = parts[1] || ""
        switch (parts[0]) {
        case "details": {
            // A saved fight from History: the newest the list shows (fill a
            // scratch data folder with --replay --save-fights first);
            // "saved-timeline" on its timeline.
            if (page === "saved" || page === "saved-timeline") {
                const openNewest = () => {
                    if (!History.ready)
                        return
                    History.readyChanged.disconnect(openNewest)
                    const list = History.list !== "" ? JSON.parse(History.list) : { "groups": [] }
                    const newest = list.groups.map(g => g.fights[0]).find(f => !!f)
                    if (newest)
                        Details.openSaved(newest.id, -1)
                }
                History.readyChanged.connect(openNewest)
                History.refresh()
                openNewest()
                return openDetails(null, page)
            }
            // With --replay, once the replay has ended: you, every player
            // for the mechanics, the meter's second row as a click on it
            // opens it ("row"; its first with one row, you with none, so a
            // screenshot never waits for a row that is not there; "row-timeline"
            // on its timeline), or you on every target ("every-target").
            const show = () => {
                if (page === "row" || page === "row-timeline") {
                    const row = meter.rowPlayer(1) || meter.rowPlayer(0)
                    if (row) {
                        meter.detailsRequested(row)
                        return
                    }
                }
                showDetailsOf(page === "mechanics" ? 0 : -1)
                if (page === "every-target")
                    Details.pickTarget(0)
            }
            if (Meter.replaying && !Meter.replayDone)
                Meter.replayDoneChanged.connect(show)
            else
                show()
            return openDetails(null, page)
        }
        case "history":
            return openHistory(page)
        case "settings":
            return openSettings(page || "appearance")
        case "character":
            return openCharacter(page)
        }
        // meter/split and meter/slim are settings, on for this run (main.rs).
        if (page === "heal")
            meter.tab = 1
        else if (page === "tank")
            meter.tab = 2
        else if (page === "notice")
            Capture.notice = true
        else if (page === "paused")
            Capture.suspended = true
        else if (page === "tray" || page === "tray-closed") {
            Notices.add("", "warning", "Sample warning", "A longer line of text under the title, which wraps onto a second line in a narrow tray.")
            Notices.add("", "info", "Sample notice", "One line of text")
            Notices.add("", "info", "Sample notice with no text", "")
            Notices.add("pet:1003", "pet", "Forest Specter soul", "22/25 (3 to level 2), 78 to max")
            const now = Math.floor(Date.now() / 1000)
            const at = (name, time) => I18n.tr("notices.spawn.at").replace("{name}", name).replace("{time}", time)
            Notices.addUntil("spawn:rift:1", "rift", "Spacetime Rift at 05:00", "", now + 4 * 60)
            Notices.addUntil("spawn:abyss:1", "abyss", at(I18n.tr("notices.spawn.abyssBoss"), "21:30"),
                             "Executor Tamasa, Executor Argo, Executor Kaira (Chaotic Lower Reshanta)\nExecutioner Dramos, Turncoat Ducal, Ravager Marakha (Chaotic Middle Reshanta)",
                             now + 12 * 60)
            const field = I18n.tr("notices.spawn.fieldBoss")
            Notices.addUntil("fieldboss:101024", "commander", at(field, "05:05"), "High Commander Lagta (Verteron)", now + 15 * 60)
            Notices.addUntil("fieldboss:101021", "gartua", at(field, "05:04"), "Eternal Gartua (Verteron)", now + 29 * 60)
            makeNoticeTray()
            noticeTray.open = page === "tray"
            return noticeTray
        }
        else if (page === "card") {
            // The meter's first row, once a replay has ended.
            card = make("SkillCard.qml")
            const show = () => {
                card.player = meter.rowPlayer(0)
                card.visible = true
            }
            if (Meter.replaying && !Meter.replayDone)
                Meter.replayDoneChanged.connect(show)
            else
                show()
            return card
        }
        return meter
    }

    // The meter's ⋯ menu: each control once. Lock and suspend are here
    // unless their "Show ... button" setting puts them in the header. The
    // 1.x header's reload, meter screenshot and sort order live here too.
    // Like every menu of the app but the tray's, it is a UiMenu, drawn in
    // the app's theme.
    readonly property UiMenu meterMenu: UiMenu {
        // The ⋯ button is at the meter's right edge.
        alignEnd: true
        UiMenuItem {
            visible: !Settings.showLockButton && (Settings.locked || app.canLock)
            text: Settings.locked ? I18n.tr("meter.unlock") : I18n.tr("meter.lock")
            onTriggered: Settings.locked = !Settings.locked
        }
        UiMenuItem {
            visible: !Settings.showSuspendButton
            text: Capture.suspended ? I18n.tr("meter.resume") : I18n.tr("meter.suspend")
            onTriggered: Capture.suspended = !Capture.suspended
        }
        UiMenuItem {
            text: I18n.tr("settings.keybind.reload")
            onTriggered: Meter.saveAndClear()
        }
        // The meter alone, to the clipboard, as 1.x's header button.
        UiMenuItem {
            text: I18n.tr("details.screenshot.ariaLabel")
            onTriggered: app.meter.screenshot()
        }
        // The height follows the rows (kept across restarts); off, an edge
        // sets the height too. A double-click on an edge turns it on.
        UiMenuItem {
            checkable: true
            checked: Settings.fitRows
            text: I18n.tr("meter.fitToRows")
            onTriggered: Settings.fitRows ? Settings.fitRows = false : app.meter.fitToRows()
        }
        UiMenuItem {
            checkable: true
            checked: Meter.lowestFirst
            text: I18n.tr("meter.sortLowestFirst")
            onTriggered: Meter.lowestFirst = !Meter.lowestFirst
        }
        UiMenuSeparator {}
        UiMenuItem {
            text: I18n.tr("tray.history")
            onTriggered: app.openHistory()
        }
        UiMenuItem {
            text: I18n.tr("character.title")
            onTriggered: app.openCharacter()
        }
        UiMenuItem {
            text: I18n.tr("tray.settings")
            onTriggered: app.openSettings()
        }
        UiMenuSeparator {}
        UiMenuItem {
            text: I18n.tr("tray.quit")
            onTriggered: Qt.quit()
        }
    }

    // The footer's mode button: the five modes, the current one marked.
    readonly property UiMenu modeMenu: UiMenu {
        UiMenuItem {
            checkable: true
            checked: Meter.mode === "bossTargets"
            text: I18n.modeName("bossTargets")
            onTriggered: Meter.pickMode("bossTargets")
        }
        UiMenuItem {
            checkable: true
            checked: Meter.mode === "lastHitByMe"
            text: I18n.modeName("lastHitByMe")
            onTriggered: Meter.pickMode("lastHitByMe")
        }
        UiMenuItem {
            checkable: true
            checked: Meter.mode === "allTargets"
            text: I18n.modeName("allTargets")
            onTriggered: Meter.pickMode("allTargets")
        }
        UiMenuItem {
            checkable: true
            checked: Meter.mode === "trainTargets"
            text: I18n.modeName("trainTargets")
            onTriggered: Meter.pickMode("trainTargets")
        }
        UiMenuItem {
            checkable: true
            checked: Meter.mode === "encounter"
            text: I18n.modeName("encounter")
            onTriggered: Meter.pickMode("encounter")
        }
    }

    // The tray: show or hide (for desktops whose tray only opens the menu),
    // the lock as the emergency way back to a locked meter, Settings and
    // History. The system tray draws this menu, so it is the desktop's own.
    readonly property Platform.SystemTrayIcon tray: Platform.SystemTrayIcon {
        visible: !app.screenshotMode
        icon.source: System.devBuild ? "qrc:/daevalog/icons/app-dev.png" : "qrc:/daevalog/icons/app.png"
        tooltip: I18n.tr("header.title") + (System.devBuild ? " (dev)" : "")
        // Not shown until the tray opens it: KDE's tray shows a menu that
        // is visible, at the screen's corner, as soon as the meter starts.
        menu: Platform.Menu {
            visible: false
            Platform.MenuItem {
                text: I18n.tr("tray.toggle")
                onTriggered: app.toggleMeter()
            }
            Platform.MenuItem {
                text: Settings.locked ? I18n.tr("meter.unlock") : I18n.tr("meter.lock")
                onTriggered: Settings.locked = !Settings.locked
            }
            // As 1.x's tray: the windows open from here too, with the
            // meter hidden.
            Platform.MenuItem {
                text: I18n.tr("tray.settings")
                onTriggered: app.openSettings()
            }
            Platform.MenuItem {
                text: I18n.tr("tray.history")
                onTriggered: app.openHistory()
            }
            Platform.MenuSeparator {}
            Platform.MenuItem {
                text: I18n.tr("tray.quit")
                onTriggered: Qt.quit()
            }
        }
        onActivated: reason => {
            if (reason === Platform.SystemTrayIcon.Trigger)
                app.toggleMeter()
        }
    }

    // With --replay, the screenshot waits for the replay's end, Details for
    // its view of that end, History for its fights and the Character window
    // for its first reading. Live, Settings waits
    // for the capture's device list and, while the game runs, the port lock,
    // 10 s at most.
    readonly property Timer screenshot: Timer {
        interval: 400
        running: app.screenshotMode && app.shown !== null && (!Meter.replaying || Meter.replayDone)
            && (app.shown !== app.details || Details.ready) && (app.shown !== app.history || app.history.settled)
            && (app.shown !== app.character || app.character.settled)
            && (app.shown !== app.card || app.card.view !== null)
            && (app.shown !== app.settings || Meter.replaying || app.devicesWaited
                || (Capture.devices !== "[]" && (Capture.port > 0 || !Capture.gameRunning)))
        onTriggered: {
            const saved = Overlay.saveScreenshot(app.shown)
            if (!saved)
                console.error("Could not save the screenshot to", Overlay.screenshotPath)
            Qt.exit(saved ? 0 : 1)
        }
    }

    property bool devicesWaited: false
    readonly property Timer devicesWait: Timer {
        interval: 10000
        running: app.screenshotMode
        onTriggered: app.devicesWaited = true
    }

    Component.onCompleted: {
        Meter.start()
        createMeter()
        shown = Overlay.startView !== "" ? openStartView() : meter
        // --width, for a screenshot of a narrow window.
        if (Overlay.startWidth > 0)
            shown.width = Math.max(shown.minimumWidth, Overlay.startWidth)
        applyDetailsMonitor()
    }
}
