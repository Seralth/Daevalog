// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Daevalog

// Every setting, in the sections of the 1.x panel: the list on the left,
// one section on the right. Settings the Settings object holds are saved,
// the Connection section is the Capture object's, the Account section the
// Account object's; the one the app does not use yet (Details on a monitor)
// is a stand-in until its object comes.
AppWindow {
    id: settings
    placeName: "settings"

    title: I18n.tr("settings.title")
    width: 860
    height: 640
    minimumWidth: 700
    minimumHeight: 420
    page: "appearance"

    // The report form is a page of Diagnostics. "-full" (--open only) grows
    // the window to the whole page, for a screenshot of a long section;
    // "meter-heal" (--open only) shows the column picker's Heal tab.
    readonly property string shownPage: page.replace("-full", "").replace("meter-heal", "meter")
    readonly property string section: shownPage === "report" ? "diagnostics" : shownPage
    readonly property bool fullPage: page.endsWith("-full")
    // "Show Details on monitor" works on X11 only: Wayland lets no window
    // pick its screen, so there the setting is hidden.
    readonly property bool monitorChoice: Qt.platform.pluginName !== "wayland"
    onFullPageChanged: {
        if (fullPage)
            height = Qt.binding(() => Math.max(640, pageColumn.implicitHeight + 40))
    }
    property string columnsTab: page.startsWith("meter-heal") ? "heal" : "dps"
    // Each meter tab's columns, as the Settings object keeps them.
    readonly property var meterColumns: JSON.parse(Settings.meterColumns)
    property string reportKind: "wrongNumbers"
    // Report a problem: the packet logs (System), the one picked to prepare,
    // and what the last Prepare and Copy did.
    readonly property var packetLogs: JSON.parse(System.packetLogs)
    property string chosenLog: ""
    property string prepareNote: ""
    property bool copied: false

    // A folder for the desktop to open. On Windows "C:\Users\..." becomes
    // file:///C:/Users/...
    function fileUrl(path) {
        if (Qt.platform.os === "windows")
            return "file:///" + path.split(/[\\/]/).map((part, i) => i === 0 ? part : encodeURIComponent(part)).join("/")
        return "file://" + path.split("/").map(encodeURIComponent).join("/")
    }
    // The issue form for a kind of report, with the report info filled in
    // (1.x's report.js `reportFormUrl`): GitHub fills the form's report-info
    // field from the query. A security report goes to a private advisory.
    function reportUrl(kind, info) {
        const repo = "https://github.com/Seralth/Daevalog"
        if (kind === "security")
            return repo + "/security/advisories/new"
        const form = { "wrongNumbers": "wrong-numbers.yml", "crash": "crash.yml", "other": "something-else.yml" }[kind]
        return repo + "/issues/new?template=" + form + "&report-info=" + encodeURIComponent(info || "")
    }
    onShownPageChanged: {
        if (shownPage === "report")
            System.refresh(Overlay.layerShell ? "layer shell" : "window")
    }
    Connections {
        target: System
        function onPreparedChanged() {
            const result = JSON.parse(System.prepared)
            if (result.error !== undefined) {
                settings.prepareNote = I18n.tr("settings.report.prepare.failed").replace("{error}", result.error)
                return
            }
            settings.prepareNote = I18n.tr("settings.report.prepare.done").replace("{name}", result.name)
            Qt.openUrlExternally(settings.fileUrl(System.dataFolder))
            System.refresh(Overlay.layerShell ? "layer shell" : "window")
        }
    }
    Timer {
        id: copiedTimer
        interval: 2000
        onTriggered: settings.copied = false
    }

    // "Only in BOSS." for a setting that acts in some modes only.
    function modesOnly(modes) {
        return I18n.tr("settings.modeOnly").replace("{mode}", modes.map(m => I18n.modeName(m)).join(", "))
    }

    // A page's label column: as wide as its longest setting name, at most
    // 45% of the page (longer names wrap), so each control sits right after
    // its name.
    function labelColumn(page) {
        let widest = 0
        // An index loop: Qt 6.4's list properties cannot be walked with for...of.
        for (let i = 0; i < page.children.length; i++) {
            const child = page.children[i]
            if (child.labelImplicit !== undefined)
                widest = Math.max(widest, child.labelImplicit)
        }
        return Math.min(Math.floor(page.width * 0.45) - 12, Math.max(120, Math.ceil(widest)))
    }

    readonly property var sections: [
        { "id": "connection", "label": I18n.tr("settings.connection") },
        { "id": "appearance", "label": I18n.tr("settings.appearance") },
        { "id": "meter", "label": I18n.tr("settings.sections.meter") },
        { "id": "account", "label": I18n.tr("settings.sections.account") },
        { "id": "keybinds", "label": I18n.tr("settings.keybinds") },
        { "id": "diagnostics", "label": I18n.tr("settings.diagnostics") },
        { "id": "advanced", "label": I18n.tr("settings.advanced") }
    ]

    // A group heading inside a section.
    component Group: UiText {
        width: parent ? parent.width : 400
        height: 34
        verticalAlignment: Text.AlignBottom
        bottomPadding: 6
        font.weight: Font.DemiBold
        muted: true
        font.pixelSize: Theme.sizeSmall
    }

    component Hint: UiText {
        width: parent ? parent.width : 400
        wrapMode: Text.WordWrap
        maximumLineCount: 6
        color: Theme.note
        font.pixelSize: Theme.sizeSmall
    }

    // An action: the button where a setting's name would be, what it does
    // beside it.
    component ActionRow: Item {
        id: actionRow
        property string text
        property string hint
        property bool last: false
        signal clicked()
        width: parent ? parent.width : 480
        implicitHeight: Math.max(Theme.settingRow, actionHint.implicitHeight + 18)
        UiButton {
            anchors.verticalCenter: parent.verticalCenter
            text: actionRow.text
            onClicked: actionRow.clicked()
        }
        UiText {
            id: actionHint
            anchors.verticalCenter: parent.verticalCenter
            x: (actionRow.parent && actionRow.parent.labelWidth > 0 ? actionRow.parent.labelWidth : Math.floor(parent.width * 0.45) - 12) + 16
            width: parent.width - x
            text: actionRow.hint
            color: Theme.note
            font.pixelSize: Theme.sizeSmall
            wrapMode: Text.WordWrap
            maximumLineCount: 4
        }
        Rectangle {
            visible: !actionRow.last
            anchors.bottom: parent.bottom
            width: parent.width
            height: 1
            color: Theme.hairline
        }
    }

    component SwitchRow: SettingRow {
        id: switchRow
        property bool checked
        signal toggled(bool on)
        UiSwitch {
            checked: switchRow.checked
            onToggled: on => switchRow.toggled(on)
        }
    }

    // A hotkey as the player set it. With `recordable`, a click waits for
    // new keys, as 1.x's keybind buttons: Escape or a click elsewhere keeps
    // the old ones.
    component KeyBox: Rectangle {
        id: keyBox
        property string keys
        property bool recordable: false
        property bool recording: false
        // The modifiers held so far, or the note that more are needed.
        property string held: ""
        signal recorded(string label)
        implicitWidth: 180
        implicitHeight: 28
        radius: 4
        color: Theme.surface
        border.width: 1
        border.color: recording ? Theme.accent : Theme.fieldEdge
        activeFocusOnTab: recordable
        onActiveFocusChanged: {
            if (!activeFocus)
                recording = false
        }
        onRecordingChanged: held = ""
        onRecordableChanged: {
            if (!recordable)
                recording = false
        }
        HotkeyText {
            id: keyText
        }
        Timer {
            id: heldNote
            interval: 1200
            onTriggered: keyBox.held = ""
        }
        UiText {
            anchors.centerIn: parent
            text: !keyBox.recording ? keyBox.keys : (keyBox.held !== "" ? keyBox.held : I18n.tr("settings.keybind.pressKeys"))
            color: keyBox.recording ? Theme.accent : Theme.name
            numeric: true
        }
        UiTip {
            text: keyBox.recordable && !keyBox.recording ? I18n.tr("settings.keybind.pressKeys") : ""
        }
        HoverHandler {
            enabled: keyBox.recordable
            cursorShape: Qt.PointingHandCursor
        }
        TapHandler {
            enabled: keyBox.recordable
            onTapped: {
                keyBox.recording = !keyBox.recording
                if (keyBox.recording)
                    keyBox.forceActiveFocus()
            }
        }
        Keys.onPressed: event => {
            if (!keyBox.recording)
                return
            event.accepted = true
            if (event.key === Qt.Key_Escape) {
                keyBox.recording = false
                return
            }
            if (keyText.isModifier(event.key)) {
                const mods = keyText.modifierText(event.modifiers)
                keyBox.held = mods.length > 0 ? mods.join("+") + "+..." : ""
                heldNote.stop()
                return
            }
            const label = keyText.label(event.key, event.modifiers)
            if (label === "" || !keyText.enoughModifiers(event.modifiers)) {
                keyBox.held = I18n.tr("settings.keybind.needModifiers")
                heldNote.restart()
                return
            }
            keyBox.recording = false
            keyBox.recorded(label)
        }
    }

    // What the desktop did with the lock key. On Windows the meter
    // registers the key itself (hotkey.rs).
    readonly property string lockKeyNote: Qt.platform.os === "windows" ? windowsKeyNote : portalKeyNote
    readonly property string windowsKeyNote: {
        switch (Hotkey.state) {
        case "bound": return I18n.tr("settings.keybind.windowsBound").replace("{key}", Hotkey.trigger)
        case "taken": return I18n.tr("settings.keybind.windowsTaken")
        case "unbound": return I18n.tr("settings.keybind.windowsUnbound")
        }
        return ""
    }
    readonly property string portalKeyNote: {
        switch (Hotkey.state) {
        case "missing": return I18n.tr("settings.keybind.portalMissing")
        case "waiting": return I18n.tr("settings.keybind.portalWaiting")
        case "bound": return I18n.tr("settings.keybind.portalBound").replace("{key}", Hotkey.trigger)
        case "unbound": return I18n.tr("settings.keybind.portalUnbound")
        case "refused": return I18n.tr("settings.keybind.portalRefused")
        case "failed": return I18n.tr("settings.keybind.portalFailed")
        }
        return ""
    }

    RowLayout {
        anchors.fill: parent
        spacing: 0

        // The sections and the version.
        Rectangle {
            Layout.preferredWidth: Theme.sideWidth
            Layout.fillHeight: true
            color: Theme.surface

            Column {
                y: 10
                width: parent.width
                Repeater {
                    model: settings.sections
                    Item {
                        id: entry
                        required property var modelData
                        readonly property bool active: settings.section === modelData.id
                        width: parent.width
                        height: 34
                        Rectangle {
                            anchors.fill: parent
                            color: entry.active ? Theme.accentTint : (entryHover.hovered ? Theme.hover : "transparent")
                        }
                        Rectangle {
                            visible: entry.active
                            width: 2
                            height: parent.height
                            color: Theme.accent
                        }
                        UiText {
                            x: 16
                            height: parent.height
                            text: entry.modelData.label
                            color: entry.active ? Theme.name : Theme.sub
                            font.weight: entry.active ? Font.DemiBold : Font.Normal
                        }
                        HoverHandler {
                            id: entryHover
                            cursorShape: Qt.PointingHandCursor
                        }
                        TapHandler {
                            onTapped: settings.page = entry.modelData.id
                        }
                    }
                }
            }

            UiText {
                anchors.bottom: parent.bottom
                anchors.bottomMargin: 14
                x: 16
                text: "Daevalog " + System.version
                muted: true
                font.pixelSize: Theme.sizeSmall
                numeric: true
            }
        }
        Rectangle {
            Layout.preferredWidth: 1
            Layout.fillHeight: true
            color: Theme.hairline
        }

        Flickable {
            id: scroller
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentHeight: pageColumn.implicitHeight + 40
            clip: true
            boundsBehavior: Flickable.StopAtBounds

            Column {
                id: pageColumn
                x: 24
                y: 16
                width: Math.min(640, scroller.width - 48)

                Row {
                    height: 34
                    spacing: 10
                    UiText {
                        visible: settings.shownPage === "report"
                        height: parent.height
                        text: I18n.tr("settings.diagnostics")
                        muted: true
                        font.pixelSize: Theme.sizeTitle
                        HoverHandler {
                            cursorShape: Qt.PointingHandCursor
                        }
                        TapHandler {
                            onTapped: settings.page = "diagnostics"
                        }
                    }
                    Chevron {
                        visible: settings.shownPage === "report"
                        anchors.verticalCenter: parent.verticalCenter
                        turn: -90
                    }
                    UiText {
                        height: parent.height
                        text: settings.shownPage === "report" ? I18n.tr("settings.report.title")
                            : (settings.sections.find(s => s.id === settings.section) || { "label": "" }).label
                        font.pixelSize: Theme.sizeTitle
                        font.weight: Font.DemiBold
                    }
                }

                // ---- Connection
                Column {
                    width: parent.width
                    visible: settings.shownPage === "connection"
                    readonly property real labelWidth: settings.labelColumn(this)
                    // What the meter found, as 1.x polls it, while the page shows.
                    Timer {
                        interval: 1000
                        repeat: true
                        triggeredOnStart: true
                        running: settings.visible && settings.shownPage === "connection"
                        onRunningChanged: {
                            if (running)
                                Capture.loadDevices()
                        }
                        onTriggered: Capture.refresh()
                    }
                    // The devices again now and then: a VPN or cable that
                    // comes or goes shows while the page is open.
                    Timer {
                        interval: 3000
                        repeat: true
                        running: settings.visible && settings.shownPage === "connection"
                        onTriggered: Capture.loadDevices()
                    }
                    // A typed name is saved after a pause in typing, on Enter
                    // or leaving the field, and when the window closes
                    // (1.x issue #13: a name typed just before closing was lost).
                    Timer {
                        id: nameSave
                        interval: 800
                        onTriggered: Capture.setTypedName(nameField.text)
                    }
                    Connections {
                        target: settings
                        function onVisibleChanged() {
                            if (!settings.visible && nameSave.running) {
                                nameSave.stop()
                                Capture.setTypedName(nameField.text)
                            }
                        }
                    }
                    // The game names you at each loading screen, over a
                    // typed name (live.rs), so the hint says so.
                    SettingRow {
                        label: I18n.tr("settings.characterName")
                        hint: I18n.tr("settings.characterNameHint")
                        UiField {
                            id: nameField
                            width: Math.min(260, parent.width)
                            placeholder: I18n.tr("settings.characterNamePlaceholder")
                            Component.onCompleted: text = Capture.name
                            onTyping: nameSave.restart()
                            onEdited: {
                                nameSave.stop()
                                Capture.setTypedName(text)
                            }
                            // Never under the player's cursor: they may be typing a new name.
                            Connections {
                                target: Capture
                                function onNameChanged() {
                                    if (!nameField.editing)
                                        nameField.text = Capture.name
                                }
                            }
                        }
                    }
                    // The pick is saved and kept while it is missing (a VPN
                    // that is off): the list shows it as not found, and the
                    // hint names the device the meter uses meanwhile.
                    SettingRow {
                        label: I18n.tr("settings.device.label")
                        hint: Capture.following !== ""
                            ? I18n.tr("settings.device.following").replace("{found}", Capture.following).replace("{device}", Capture.device)
                            : ""
                        UiDropdown {
                            width: Math.min(260, parent.width)
                            readonly property var listed: JSON.parse(Capture.devices)
                            options: [{ "label": I18n.tr("settings.device.autoDetect"), "value": "" }].concat(
                                listed.map(d => ({ "label": d, "value": d })),
                                Capture.device !== "" && !listed.includes(Capture.device)
                                    ? [{ "label": Capture.deviceMissing
                                            ? I18n.tr("settings.device.notFound").replace("{device}", Capture.device)
                                            : Capture.device, "value": Capture.device }]
                                    : [])
                            current: Capture.device
                            onPicked: value => Capture.device = value
                        }
                    }
                    SettingRow {
                        label: I18n.tr("settings.lockedPort")
                        last: true
                        Row {
                            spacing: 10
                            // 1.x's words: the device (no name is the loopback)
                            // and port once found; before, whether the game runs.
                            UiText {
                                height: 28
                                numeric: true
                                text: Capture.port > 0
                                    ? (Capture.lockedDevice || I18n.tr("connection.loopback")) + " · " + Capture.port
                                    : Capture.gameRunning ? I18n.tr("connection.detecting") : I18n.tr("connection.auto")
                            }
                            UiButton {
                                small: true
                                anchors.verticalCenter: parent.verticalCenter
                                text: I18n.tr("settings.resetDetection")
                                onClicked: Capture.resetDetection()
                            }
                        }
                    }
                }

                // ---- Appearance
                Column {
                    width: parent.width
                    visible: settings.shownPage === "appearance"
                    readonly property real labelWidth: settings.labelColumn(this)
                    SettingRow {
                        label: I18n.tr("settings.language.label")
                        UiDropdown {
                            width: Math.min(220, parent.width)
                            // Each language in its own name, as language lists do.
                            options: [
                                { "label": I18n.tr("settings.language.system"), "value": "" },
                                { "label": "English", "value": "en" },
                                { "label": "Deutsch", "value": "de" },
                                { "label": "Español", "value": "es" },
                                { "label": "Français", "value": "fr" },
                                { "label": "日本語", "value": "ja" },
                                { "label": "한국어", "value": "ko" },
                                { "label": "Português", "value": "pt" },
                                { "label": "Русский", "value": "ru" }
                            ]
                            current: Settings.language
                            onPicked: value => Settings.language = value
                        }
                    }
                    SettingRow {
                        label: I18n.tr("settings.timeFormat.label")
                        tip: I18n.tr("settings.timeFormat.hint")
                        UiDropdown {
                            width: Math.min(220, parent.width)
                            // Each choice shows a time in its format, as the
                            // game's own chat setting does. Until the player
                            // picks one, the system's clock.
                            options: [
                                { "label": I18n.clock(15, 30, false), "value": "12h" },
                                { "label": I18n.clock(15, 30, true), "value": "24h" }
                            ]
                            current: Settings.hour24 ? "24h" : "12h"
                            onPicked: value => Settings.timeFormat = value
                        }
                    }
                    SettingRow {
                        label: I18n.tr("settings.colorTheme.label")
                        hint: I18n.tr("settings.colorTheme.hint")
                        UiDropdown {
                            width: Math.min(220, parent.width)
                            // Daevalog and Nord are names, the same in every language.
                            options: [
                                { "label": "Daevalog", "value": "daevalog" },
                                { "label": I18n.tr("settings.colorTheme.classic"), "value": "classic" },
                                { "label": "Nord", "value": "nord" },
                                { "label": I18n.tr("settings.colorTheme.system"), "value": "system" }
                            ]
                            current: Settings.colorTheme
                            onPicked: value => Settings.colorTheme = value
                        }
                    }
                    // Under System the accent is the desktop's highlight.
                    SettingRow {
                        visible: Settings.colorTheme !== "system"
                        label: I18n.tr("settings.accent.label")
                        Row {
                            spacing: 10
                            Repeater {
                                model: ["sky", "rose", "lemon", "silver"]
                                Rectangle {
                                    id: swatch
                                    required property string modelData
                                    readonly property bool chosen: Settings.accent === modelData
                                    width: 24
                                    height: 24
                                    radius: 12
                                    color: "transparent"
                                    border.width: chosen ? 2 : 0
                                    border.color: Theme.name
                                    Rectangle {
                                        anchors.centerIn: parent
                                        width: 18
                                        height: 18
                                        radius: 9
                                        color: Theme.accents[swatch.modelData]
                                    }
                                    HoverHandler {
                                        cursorShape: Qt.PointingHandCursor
                                    }
                                    TapHandler {
                                        onTapped: Settings.accent = swatch.modelData
                                    }
                                }
                            }
                            UiText {
                                height: 24
                                muted: true
                                text: {
                                    switch (Settings.accent) {
                                    case "rose": return I18n.tr("settings.accent.rose")
                                    case "lemon": return I18n.tr("settings.accent.lemon")
                                    case "silver": return I18n.tr("settings.accent.silver")
                                    }
                                    return I18n.tr("settings.accent.sky")
                                }
                            }
                        }
                    }
                    SettingRow {
                        label: I18n.tr("settings.meterLayout.label")
                        UiSegmented {
                            options: [
                                { "label": I18n.tr("settings.meterLayout.normal"), "value": false },
                                { "label": I18n.tr("settings.meterLayout.slim"), "value": true }
                            ]
                            current: Settings.slimMode
                            onPicked: value => Settings.slimMode = value
                        }
                    }
                    SwitchRow {
                        label: I18n.tr("settings.splitView.label")
                        hint: I18n.tr("settings.splitView.hint")
                        checked: Settings.splitView
                        onToggled: on => Settings.splitView = on
                    }
                    SettingRow {
                        label: I18n.tr("settings.yourRow.label")
                        UiSegmented {
                            options: [
                                { "label": I18n.tr("settings.yourRow.name"), "value": false },
                                { "label": I18n.tr("meter.you"), "value": true }
                            ]
                            current: Settings.showYou
                            onPicked: value => Settings.showYou = value
                        }
                    }
                    SwitchRow {
                        label: I18n.tr("settings.hideOtherNames")
                        hint: I18n.tr("settings.hideOtherNamesHint")
                        checked: Settings.hideOtherNames
                        onToggled: on => Settings.hideOtherNames = on
                    }
                    SettingRow {
                        label: I18n.tr("settings.windowOpacity.label")
                        UiSlider {
                            width: Math.min(300, parent.width)
                            value: Settings.windowOpacity
                            onMoved: value => Settings.windowOpacity = value
                        }
                    }
                    SettingRow {
                        label: I18n.tr("settings.barStrength.label")
                        // The windows' tables cap it (Theme.tableBarStrength).
                        hint: I18n.tr("settings.barStrength.hint") + " " + I18n.tr("settings.barStrength.tableHint")
                        UiSlider {
                            width: Math.min(300, parent.width)
                            from: 10
                            to: 50
                            value: Settings.barStrength
                            onMoved: value => Settings.barStrength = value
                        }
                    }
                    SettingRow {
                        label: I18n.tr("settings.bossNameSize.label")
                        // Whole pixels: Qt draws font sizes in whole pixels
                        // (1.x went in half pixels).
                        UiSlider {
                            width: Math.min(300, parent.width)
                            from: 8
                            to: 20
                            stepSize: 1
                            value: Settings.targetNameSize
                            valueText: value + " px"
                            onMoved: value => Settings.targetNameSize = value
                        }
                    }
                    SettingRow {
                        label: I18n.tr("settings.bold.label")
                        last: !settings.monitorChoice
                        Row {
                            spacing: 10
                            UiSwitch {
                                anchors.verticalCenter: parent.verticalCenter
                                checked: Settings.boldNames
                                onToggled: on => Settings.boldNames = on
                            }
                            UiText {
                                text: I18n.tr("settings.bold.names")
                                height: 28
                            }
                            Item {
                                width: 10
                                height: 1
                            }
                            UiSwitch {
                                anchors.verticalCenter: parent.verticalCenter
                                checked: Settings.boldDps
                                onToggled: on => Settings.boldDps = on
                            }
                            UiText {
                                text: I18n.tr("settings.bold.dps")
                                height: 28
                            }
                        }
                    }
                    // X11 only: Wayland lets no window pick its screen.
                    SettingRow {
                        id: monitorRow
                        readonly property var screens: Array.from(Qt.application.screens)
                        visible: settings.monitorChoice
                        label: I18n.tr("settings.detailsMonitor.label")
                        // Closing Details turns it off, as in 1.x (Main.qml).
                        hint: Settings.detailsMonitor !== "" && !screens.some(s => s.name === Settings.detailsMonitor)
                            ? I18n.tr("settings.detailsMonitor.missing")
                            : screens.length < 2 ? I18n.tr("settings.detailsMonitor.single") : I18n.tr("settings.detailsMonitor.closeHint")
                        last: true
                        UiDropdown {
                            width: Math.min(260, parent.width)
                            options: [{ "label": I18n.tr("settings.detailsMonitor.off"), "value": "" }].concat(
                                monitorRow.screens.map(s => ({ "label": s.name + " · " + s.width + "×" + s.height, "value": s.name })))
                            current: Settings.detailsMonitor
                            onPicked: value => Settings.detailsMonitor = value
                        }
                    }
                }

                // ---- Meter
                Column {
                    width: parent.width
                    visible: settings.shownPage === "meter"
                    readonly property real labelWidth: settings.labelColumn(this)
                    // A change switches the running meter too (live.rs
                    // `follow_default_mode`), so the hint says so.
                    SettingRow {
                        label: I18n.tr("settings.defaultMeterMode.label")
                        hint: I18n.tr("settings.defaultMeterMode.hint")
                        UiDropdown {
                            width: Math.min(220, parent.width)
                            options: ["bossTargets", "lastHitByMe", "allTargets", "trainTargets", "encounter"].map(
                                m => ({ "label": I18n.modeName(m), "value": m }))
                            current: Settings.defaultMode
                            onPicked: value => Settings.defaultMode = value
                        }
                    }
                    // A party, a field event, a raid, and room (settings_store
                    // PLAYER_LIMITS); the meter grows a row at a time up to it.
                    SettingRow {
                        label: I18n.tr("settings.playerLimit.label")
                        UiDropdown {
                            implicitWidth: 80
                            options: [5, 8, 10, 12].map(n => ({ "label": String(n), "value": n }))
                            current: Settings.playerLimit
                            onPicked: value => Settings.playerLimit = value
                        }
                    }
                    SwitchRow {
                        label: I18n.tr("settings.pinMeToTop")
                        checked: Settings.pinYou
                        onToggled: on => Settings.pinYou = on
                    }
                    // Where the game's window cannot be seen in front (Linux),
                    // it would never hide.
                    SwitchRow {
                        visible: Overlay.autoHideSetting
                        label: I18n.tr("settings.autoHideMeter")
                        checked: Settings.autoHide
                        onToggled: on => Settings.autoHide = on
                    }
                    SwitchRow {
                        label: I18n.tr("settings.startInTray")
                        hint: I18n.tr("settings.startInTrayHint")
                        checked: Settings.startInTray
                        onToggled: on => Settings.startInTray = on
                    }
                    // X11 only: Wayland lets no app ask to skip the taskbar.
                    SwitchRow {
                        visible: Overlay.taskbarSetting
                        label: I18n.tr("settings.hideFromTaskbar")
                        hint: I18n.tr("settings.hideFromTaskbarHint")
                        checked: Settings.hideFromTaskbar
                        onToggled: on => Settings.hideFromTaskbar = on
                    }
                    SwitchRow {
                        label: I18n.tr("settings.showPing")
                        checked: Settings.showPing
                        onToggled: on => Settings.showPing = on
                    }
                    SwitchRow {
                        label: I18n.tr("settings.showTotalDps")
                        hint: I18n.tr("settings.showTotalHint")
                        checked: Settings.showTotal
                        onToggled: on => Settings.showTotal = on
                    }
                    // 1.x's Beta and Classic layouts each had one place for it.
                    // Greyed while Show total is off: nothing shows then.
                    SettingRow {
                        enabled: Settings.showTotal
                        label: I18n.tr("settings.groupDamage.label")
                        hint: I18n.tr("settings.groupDamage.hint")
                        UiSegmented {
                            options: [
                                { "label": I18n.tr("settings.groupDamage.footer"), "value": "footer" },
                                { "label": I18n.tr("settings.groupDamage.line"), "value": "line" }
                            ]
                            current: Settings.groupDamage
                            onPicked: value => Settings.groupDamage = value
                        }
                    }
                    // Where the health bar's text sits: 1.x's Beta and
                    // Classic layouts each had one.
                    // Slim has no health bar: greyed there.
                    SettingRow {
                        enabled: !Settings.slimMode
                        label: I18n.tr("settings.healthText.label")
                        hint: settings.modesOnly(["bossTargets", "lastHitByMe"]) + " " + I18n.tr("settings.healthText.hint")
                        UiSegmented {
                            options: [
                                { "label": I18n.tr("settings.healthText.centred"), "value": "centred" },
                                { "label": I18n.tr("settings.healthText.split"), "value": "split" }
                            ]
                            current: Settings.healthText
                            onPicked: value => Settings.healthText = value
                        }
                    }
                    // 1.x's rule: under 5% of the damage shown amber, under 3% red.
                    SettingRow {
                        label: I18n.tr("settings.lowShare.label")
                        hint: I18n.tr("settings.lowShare.hint")
                        UiSegmented {
                            options: [
                                { "label": I18n.tr("settings.lowShare.off"), "value": "off" },
                                { "label": I18n.tr("settings.lowShare.number"), "value": "number" },
                                { "label": I18n.tr("settings.lowShare.row"), "value": "row" }
                            ]
                            current: Settings.lowShare
                            onPicked: value => Settings.lowShare = value
                        }
                    }
                    SwitchRow {
                        label: I18n.tr("settings.roundDps")
                        checked: Settings.roundDps
                        onToggled: on => Settings.roundDps = on
                    }
                    SwitchRow {
                        label: I18n.tr("settings.showSuspendBtn")
                        hint: I18n.tr("settings.buttonPlaceHint")
                        checked: Settings.showSuspendButton
                        onToggled: on => Settings.showSuspendButton = on
                    }
                    SwitchRow {
                        label: I18n.tr("settings.showLockBtn")
                        hint: I18n.tr("settings.buttonPlaceHint")
                        checked: Settings.showLockButton
                        onToggled: on => Settings.showLockButton = on
                        last: !Overlay.layerSetting
                    }
                    // Saved now, used from the next start: a Wayland window
                    // cannot become a layer surface, or stop being one, once
                    // shown. --layer-shell turns it on for one run. Wayland
                    // only: on X11 there is no layer surface to be.
                    SwitchRow {
                        visible: Overlay.layerSetting
                        label: I18n.tr("settings.waylandLayer")
                        hint: I18n.tr("settings.layerHint")
                        checked: Settings.layerShell
                        onToggled: on => Settings.layerShell = on
                        last: true
                    }

                    Group {
                        text: I18n.tr("settings.columns.title")
                    }
                    Hint {
                        text: I18n.tr("settings.columns.hint")
                        bottomPadding: 8
                    }
                    UiSegmented {
                        options: [
                            { "label": I18n.tr("meter.tabs.dps"), "value": "dps" },
                            { "label": I18n.tr("meter.tabs.heal"), "value": "heal" },
                            { "label": I18n.tr("meter.tabs.tank"), "value": "tank" }
                        ]
                        current: settings.columnsTab
                        onPicked: value => settings.columnsTab = value
                    }
                    Item {
                        width: 1
                        height: 6
                    }
                    // Each column: its key, its name, and what it shows (the
                    // name's tooltip, as on the meter's heads).
                    Repeater {
                        model: ({
                            "dps": [
                                ["total", I18n.tr("settings.encColumns.total"), I18n.tr("meter.columnTips.total")],
                                ["encDps", I18n.tr("settings.encColumns.encDps"), I18n.tr("meter.columnTips.dps")],
                                ["dps", I18n.tr("settings.encColumns.dps"), I18n.tr("meter.columnTips.activeDps")],
                                ["pct", I18n.tr("settings.encColumns.pct"), I18n.tr("meter.columnTips.pct")],
                                ["crit", I18n.tr("settings.encColumns.crit"), I18n.tr("meter.columnTips.crit")],
                                ["last10", I18n.tr("settings.encColumns.last10"), I18n.tr("meter.columnTips.last").replace("{n}", "10")],
                                ["last30", I18n.tr("settings.encColumns.last30"), I18n.tr("meter.columnTips.last").replace("{n}", "30")],
                                ["last60", I18n.tr("settings.encColumns.last60"), I18n.tr("meter.columnTips.last").replace("{n}", "60")],
                                ["maxHit", I18n.tr("settings.encColumns.maxHit"), I18n.tr("meter.columnTips.maxHit")],
                                ["hits", I18n.tr("settings.encColumns.hits"), I18n.tr("meter.columnTips.hits")]
                            ],
                            "heal": [
                                ["healed", I18n.tr("settings.columns.healed"), I18n.tr("meter.columnTips.healed")],
                                ["hps", I18n.tr("settings.columns.hps"), I18n.tr("meter.columnTips.hps")],
                                ["pct", I18n.tr("settings.columns.healPct"), I18n.tr("meter.columnTips.healPct")],
                                ["taken", I18n.tr("settings.columns.taken"), I18n.tr("meter.columnTips.healTaken")]
                            ],
                            "tank": [
                                ["taken", I18n.tr("settings.columns.taken"), I18n.tr("meter.columnTips.taken")],
                                ["dtps", I18n.tr("settings.columns.dtps"), I18n.tr("meter.columnTips.dtps")],
                                ["pct", I18n.tr("settings.columns.takenPct"), I18n.tr("meter.columnTips.takenPct")],
                                ["block", I18n.tr("settings.columns.block"), I18n.tr("meter.columnTips.block")],
                                ["parry", I18n.tr("settings.columns.parry"), I18n.tr("meter.columnTips.parry")]
                            ]
                        })[settings.columnsTab]
                        Item {
                            id: columnLine
                            required property var modelData
                            readonly property var chosen: settings.meterColumns[settings.columnsTab] || []
                            readonly property bool on: chosen.indexOf(modelData[0]) >= 0
                            readonly property real labelWidth: parent && parent.labelWidth > 0 ? parent.labelWidth : Math.floor(width * 0.45) - 12
                            readonly property real labelImplicit: columnName.implicitWidth
                            width: parent.width
                            height: 28
                            UiText {
                                id: columnName
                                x: 0
                                width: Math.min(implicitWidth, columnLine.labelWidth)
                                height: parent.height
                                text: columnLine.modelData[1]
                                color: Theme.name
                                UiTip {
                                    text: columnLine.modelData[2]
                                }
                            }
                            UiSwitch {
                                x: columnLine.labelWidth + 16
                                anchors.verticalCenter: parent.verticalCenter
                                checked: columnLine.on
                                // Any number per tab; the meter widens to fit.
                                onToggled: on => {
                                    const columns = Object.assign({}, settings.meterColumns)
                                    let list = columnLine.chosen.filter(k => k !== columnLine.modelData[0])
                                    if (on)
                                        list.push(columnLine.modelData[0])
                                    columns[settings.columnsTab] = list
                                    Settings.meterColumns = JSON.stringify(columns)
                                }
                            }
                        }
                    }

                    Group {
                        text: I18n.tr("settings.encColumns.title")
                    }
                    SettingRow {
                        label: I18n.tr("settings.encounterTimeout.label")
                        hint: settings.modesOnly(["encounter"]) + " " + I18n.tr("settings.encounterTimeout.hint")
                        last: true
                        // Whole seconds, 5-300: a number out of range goes to
                        // the nearest end, as 1.x.
                        UiField {
                            id: timeoutField
                            width: 80
                            numeric: true
                            text: String(Settings.encounterTimeout)
                            onEdited: text => {
                                const n = Math.round(Number(text))
                                if (text.trim() !== "" && Number.isFinite(n))
                                    Settings.encounterTimeout = n
                                // Shows the kept value, also after a refused one.
                                timeoutField.text = Qt.binding(() => String(Settings.encounterTimeout))
                            }
                        }
                    }
                }

                // ---- Account
                Column {
                    width: parent.width
                    visible: settings.shownPage === "account"
                    readonly property real labelWidth: settings.labelColumn(this)
                    Group {
                        text: I18n.tr("settings.account.title")
                    }
                    // Who runs a2tools.app, what an upload sends there and
                    // where its rules and its help are: the site's own pages.
                    Hint {
                        bottomPadding: 10
                        maximumLineCount: 12
                        text: I18n.tr("settings.account.about")
                    }
                    Hint {
                        bottomPadding: 12
                        textFormat: Text.StyledText
                        linkColor: Theme.name
                        text: I18n.tr("settings.account.rules")
                            .replace("{privacy}", "<a href=\"https://a2tools.app/privacy\">" + I18n.tr("settings.account.privacyLink") + "</a>")
                            .replace("{help}", "<a href=\"https://a2tools.app/privacy#contact\">" + I18n.tr("settings.account.helpLink") + "</a>")
                        onLinkActivated: link => Qt.openUrlExternally(link)
                        HoverHandler {
                            cursorShape: parent.hoveredLink !== "" ? Qt.PointingHandCursor : Qt.ArrowCursor
                        }
                    }
                    // Who is signed in, asked each time the page shows; a
                    // sign-in opens the approval page on a2tools.app in the
                    // browser and shows its code, in case the page did not open.
                    readonly property bool asking: settings.visible && settings.shownPage === "account"
                    onAskingChanged: {
                        if (asking)
                            Account.refresh()
                    }
                    Component.onCompleted: {
                        if (asking)
                            Account.refresh()
                    }
                    Connections {
                        target: Account
                        function onLinkChanged() {
                            if (Account.link !== "")
                                Qt.openUrlExternally(Account.link)
                        }
                    }
                    SettingRow {
                        readonly property string state: Account.state
                        label: state === "signedIn"
                            ? I18n.tr("settings.account.signedIn").replace("{name}", Account.name)
                                + (Account.supporter ? " · " + I18n.tr("settings.account.supporter") : "")
                            : state === "waiting" ? I18n.tr("settings.account.working")
                            : state === "failed" ? I18n.tr("settings.account.failed").replace("{error}", Account.message)
                            : state === "unavailable" ? Account.message
                            : state === "signedOut" ? I18n.tr("settings.account.signedOut") : ""
                        hint: I18n.tr("settings.account.signInHint")
                        UiButton {
                            visible: Account.state !== "checking" && Account.state !== "waiting"
                            readonly property bool signedIn: Account.state === "signedIn" || Account.state === "unavailable"
                            text: signedIn ? I18n.tr("settings.account.signOut") : I18n.tr("settings.account.connect")
                            onClicked: signedIn ? Account.signOut() : Account.signIn()
                        }
                    }
                    // The code to approve, while the browser is asked.
                    SettingRow {
                        visible: Account.state === "waiting" && Account.code !== ""
                        label: I18n.tr("settings.account.codePrompt")
                        hint: I18n.tr("settings.account.codeHint")
                        UiText {
                            height: 28
                            text: Account.code
                            numeric: true
                            font.pixelSize: Theme.sizeTitle
                            font.weight: Font.DemiBold
                            color: Theme.number
                        }
                    }
                    // Signed out it could upload nothing: greyed, and why. A
                    // failed upload says so here, not only in the log.
                    SwitchRow {
                        readonly property bool signedIn: Account.state === "signedIn" || Account.state === "unavailable"
                        enabled: signedIn
                        label: I18n.tr("settings.account.autoUpload")
                        hint: [I18n.tr("settings.account.autoUploadHint"),
                            signedIn ? "" : I18n.tr("settings.account.autoUploadSignIn"),
                            Account.uploadFailure === "" ? "" : I18n.tr("settings.account.autoUploadFailed").replace("{error}", Account.uploadFailure)
                        ].filter(t => t !== "").join("\n")
                        checked: Account.autoUpload
                        onToggled: on => Account.autoUpload = on
                        last: true
                    }
                }

                // ---- Keybinds
                Column {
                    width: parent.width
                    visible: settings.shownPage === "keybinds"
                    readonly property real labelWidth: settings.labelColumn(this)
                    // On Linux only the lock has a key, as in 1.x: the
                    // desktop's GlobalShortcuts portal gives it. On Windows
                    // too: the meter registers that one key.
                    SettingRow {
                        visible: Qt.platform.os !== "linux" && Qt.platform.os !== "windows"
                        label: I18n.tr("settings.keybind.reload")
                        KeyBox {
                            keys: Settings.reloadHotkey
                        }
                    }
                    SettingRow {
                        visible: Qt.platform.os !== "linux" && Qt.platform.os !== "windows"
                        label: I18n.tr("settings.keybind.toggle")
                        KeyBox {
                            keys: Settings.toggleWindowHotkey
                        }
                    }
                    SettingRow {
                        label: I18n.tr("settings.keybind.lock")
                        last: true
                        hint: settings.lockKeyNote
                        Row {
                            spacing: 10
                            // The key the desktop gave, which can differ from
                            // the one asked for; without the portal no key
                            // can work, so none is recorded.
                            KeyBox {
                                keys: Hotkey.state === "bound" && Hotkey.trigger !== "" ? Hotkey.trigger : Settings.lockHotkey
                                recordable: Hotkey.state !== "missing"
                                onRecorded: label => {
                                    Settings.lockHotkey = label
                                    Hotkey.offer(label)
                                }
                            }
                            UiButton {
                                visible: Hotkey.canConfigure && (Hotkey.state === "bound" || Hotkey.state === "unbound")
                                anchors.verticalCenter: parent.verticalCenter
                                small: true
                                text: I18n.tr("settings.keybind.desktopSettings")
                                onClicked: Hotkey.configure()
                            }
                        }
                    }
                    Hint {
                        topPadding: 10
                        text: I18n.tr("settings.keybind.needModifiers")
                    }
                }

                // ---- Diagnostics
                Column {
                    width: parent.width
                    visible: settings.shownPage === "diagnostics"
                    readonly property real labelWidth: settings.labelColumn(this)
                    SwitchRow {
                        label: I18n.tr("settings.debugLogging")
                        checked: Settings.debugLogging
                        onToggled: on => Settings.debugLogging = on
                    }
                    SwitchRow {
                        label: I18n.tr("settings.saveRawPackets")
                        checked: Settings.packetLogging
                        onToggled: on => Settings.packetLogging = on
                    }
                    ActionRow {
                        text: I18n.tr("settings.report.button")
                        hint: I18n.tr("settings.report.intro")
                        onClicked: settings.page = "report"
                    }
                    // No Send logs to a2tools.app: the logs hold other players' names, and problems come here.
                    ActionRow {
                        text: I18n.tr("settings.sendLogs.openFolder")
                        hint: I18n.tr("settings.sendLogs.openFolderTip")
                        onClicked: Qt.openUrlExternally(settings.fileUrl(System.dataFolder))
                        last: true
                    }
                }

                // ---- Report a problem (a page of Diagnostics)
                Column {
                    width: parent.width
                    visible: settings.shownPage === "report"
                    readonly property real labelWidth: settings.labelColumn(this)
                    spacing: 10
                    Hint {
                        text: I18n.tr("settings.report.intro")
                        font.pixelSize: Theme.sizeRow
                    }
                    Flow {
                        width: parent.width
                        spacing: 8
                        Repeater {
                            model: [
                                { "label": I18n.tr("settings.report.kind.wrongNumbers"), "value": "wrongNumbers" },
                                { "label": I18n.tr("settings.report.kind.crash"), "value": "crash" },
                                { "label": I18n.tr("settings.report.kind.other"), "value": "other" },
                                { "label": I18n.tr("settings.report.kind.security"), "value": "security" }
                            ]
                            UiSegmented {
                                required property var modelData
                                options: [modelData]
                                current: settings.reportKind
                                onPicked: value => {
                                    settings.reportKind = value
                                    settings.prepareNote = ""
                                }
                            }
                        }
                    }
                    Hint {
                        text: {
                            switch (settings.reportKind) {
                            // 2.0's own text: 1.x has no Prepare debug log.
                            case "crash": return I18n.tr("settings.report.hint.crashCopy")
                            case "other": return I18n.tr("settings.report.hint.other")
                            case "security": return I18n.tr("settings.report.hint.security")
                            }
                            return I18n.tr("settings.report.hint.wrongNumbers")
                        }
                        font.pixelSize: Theme.sizeRow
                        color: Theme.name
                    }
                    Group {
                        visible: settings.reportKind === "wrongNumbers"
                        text: I18n.tr("settings.report.prepare.title")
                    }
                    Hint {
                        visible: settings.reportKind === "wrongNumbers"
                        text: I18n.tr("settings.report.prepare.hint")
                    }
                    Hint {
                        visible: settings.reportKind === "wrongNumbers" && settings.packetLogs.length === 0
                        text: I18n.tr("settings.report.prepare.none")
                    }
                    Row {
                        visible: settings.reportKind === "wrongNumbers" && settings.packetLogs.length > 0
                        spacing: 10
                        UiText {
                            height: 28
                            text: I18n.tr("settings.report.prepare.log")
                            muted: true
                        }
                        UiDropdown {
                            width: 280
                            options: settings.packetLogs.map(l => ({ "label": l.name, "value": l.name }))
                            current: settings.chosenLog !== "" ? settings.chosenLog : (settings.packetLogs[0] ? settings.packetLogs[0].name : "")
                            onPicked: value => settings.chosenLog = value
                        }
                        UiButton {
                            enabled: !System.preparing
                            text: System.preparing ? I18n.tr("settings.report.prepare.working") : I18n.tr("settings.report.prepare.button")
                            onClicked: System.prepareLog(settings.chosenLog)
                        }
                    }
                    // debug.log holds the names of players met and the home
                    // folder; issues are public, so the copy is what is attached.
                    UiButton {
                        visible: settings.reportKind === "crash"
                        enabled: !System.preparing
                        text: System.preparing ? I18n.tr("settings.report.prepare.working") : I18n.tr("settings.report.debugLog")
                        onClicked: System.prepareDebugLog()
                    }
                    Hint {
                        visible: (settings.reportKind === "wrongNumbers" || settings.reportKind === "crash") && settings.prepareNote !== ""
                        text: settings.prepareNote
                    }
                    UiButton {
                        visible: settings.reportKind === "wrongNumbers" && System.recordFolder !== ""
                        text: I18n.tr("settings.report.recordFolder")
                        onClicked: Qt.openUrlExternally(settings.fileUrl(System.recordFolder))
                        UiTip {
                            text: I18n.tr("settings.report.recordFolderTip")
                        }
                    }
                    Row {
                        width: parent.width
                        Group {
                            width: parent.width - copyButton.width
                            text: I18n.tr("settings.report.info")
                        }
                        UiButton {
                            id: copyButton
                            small: true
                            anchors.bottom: parent.bottom
                            anchors.bottomMargin: 4
                            text: settings.copied ? I18n.tr("settings.report.copied") : I18n.tr("settings.report.copy")
                            onClicked: {
                                System.copy(System.reportInfo)
                                settings.copied = true
                                copiedTimer.restart()
                            }
                        }
                    }
                    Rectangle {
                        width: parent.width
                        height: info.implicitHeight + 16
                        radius: 4
                        color: Theme.surface
                        border.width: 1
                        border.color: Theme.fieldEdge
                        UiText {
                            id: info
                            x: 10
                            y: 8
                            width: parent.width - 20
                            text: System.reportInfo
                            maximumLineCount: 10
                            wrapMode: Text.WordWrap
                            font.pixelSize: Theme.sizeSmall
                            numeric: true
                        }
                    }
                    Hint {
                        text: I18n.tr("settings.report.infoHint")
                    }
                    UiButton {
                        text: I18n.tr("settings.report.open")
                        onClicked: Qt.openUrlExternally(settings.reportUrl(settings.reportKind, System.reportInfo))
                    }
                }

                // ---- Advanced
                Column {
                    width: parent.width
                    visible: settings.shownPage === "advanced"
                    readonly property real labelWidth: settings.labelColumn(this)
                    // What Details and the Heal and Tank tabs count from a
                    // TRAIN row; the DPS rows do not change with it.
                    SettingRow {
                        label: I18n.tr("settings.trainingMode.label").replace("{mode}", I18n.modeName("trainTargets"))
                        hint: I18n.tr("settings.trainingMode.hint").replace("{mode}", I18n.modeName("trainTargets"))
                        UiDropdown {
                            width: Math.min(220, parent.width)
                            options: [
                                { "label": I18n.tr("settings.trainingMode.options.all"), "value": "all" },
                                { "label": I18n.tr("settings.trainingMode.options.highestDamage"), "value": "highestDamage" }
                            ]
                            current: Settings.trainingMode
                            onPicked: value => Settings.trainingMode = value
                        }
                    }
                    SettingRow {
                        label: I18n.tr("settings.allTargetsWindow.label")
                        hint: settings.modesOnly(["allTargets"])
                        UiDropdown {
                            width: Math.min(220, parent.width)
                            options: [
                                { "label": I18n.tr("settings.allTargetsWindow.options.off"), "value": 0 },
                                { "label": I18n.tr("settings.allTargetsWindow.options.30s"), "value": 30000 },
                                { "label": I18n.tr("settings.allTargetsWindow.options.1m"), "value": 60000 },
                                { "label": I18n.tr("settings.allTargetsWindow.options.2m"), "value": 120000 },
                                { "label": I18n.tr("settings.allTargetsWindow.options.3m"), "value": 180000 },
                                { "label": I18n.tr("settings.allTargetsWindow.options.5m"), "value": 300000 }
                            ]
                            current: Settings.allTargetsWindow
                            onPicked: value => Settings.allTargetsWindow = value
                        }
                    }
                    // Asks once more, as History's Delete does.
                    ConfirmStep {
                        id: resetStep
                        onConfirmed: {
                            Settings.resetAll()
                            Account.refresh()
                            // The name setting is gone; the meter keeps the
                            // session's name, and the game's goes back in.
                            Capture.refresh()
                        }
                    }
                    ActionRow {
                        text: resetStep.armed ? I18n.tr("settings.resetAllConfirm") : I18n.tr("settings.resetAllSettings")
                        hint: I18n.tr("settings.resetAllHint")
                        last: true
                        onClicked: resetStep.press()
                    }
                }
            }
        }
    }
}
