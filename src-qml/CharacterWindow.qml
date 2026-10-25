// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import Daevalog

// Your character as the meter last read it from the game, read only: gear,
// the stat sheet, skills and Daevanion boards. Everything shown is the
// Character object's (character_view.rs); this window only lays it out.
// With more than one character read, the head picks which.
AppWindow {
    id: window
    placeName: "character"

    // 0 Gear, 1 Character sheet, 2 Skills, 3 Daevanion.
    property int tab: 0

    title: I18n.tr("character.title")
    width: 1200
    height: 880
    minimumWidth: 940
    minimumHeight: 560

    readonly property var list: Character.list !== "" ? JSON.parse(Character.list) : null
    readonly property var view: Character.view !== "" ? JSON.parse(Character.view) : null
    readonly property var files: list ? list.files : []
    readonly property var head: view ? view.head : null
    // A reading has landed: a screenshot waits for it.
    readonly property bool settled: Character.ready

    // --open character/stats, character/skills, character/boards; and
    // character/base, the gear page with the first item's base stats shown.
    onPageChanged: tab = Math.max(0, ["gear", "stats", "skills", "boards"].indexOf(page))
    onVisibleChanged: {
        if (visible)
            Character.refresh()
    }
    // The meter writes a character as the game sends it: look again while
    // open. A look that finds nothing new changes nothing.
    Timer {
        interval: 3000
        repeat: true
        running: window.visible
        onTriggered: Character.refresh()
    }
    // The language or Display Time changed in Settings: every name and time
    // again.
    Connections {
        target: Settings
        function onLanguageChanged() {
            Character.redraw()
        }
        function onHour24Changed() {
            Character.redraw()
        }
    }

    // "Name  ·  Spiritmaster  ·  Lv. 45  ·  Siel": who a file is (the name
    // only when the meter kept one).
    function who(e) {
        return [e.name, e.classLabel, e.level !== null ? I18n.tr("character.level").replace("{n}", e.level) : "", e.server]
            .filter(s => s !== "").join("  ·  ")
    }
    function seen(e) {
        return e.seen !== "" ? I18n.tr("character.seen").replace("{date}", e.seen) : ""
    }

    // Who, when the meter last read them, and the two numbers the game
    // ranks a character by.
    Item {
        id: header
        visible: !!window.view
        x: Theme.gutter
        y: 14
        width: parent.width - 2 * Theme.gutter
        height: 40

        Image {
            id: classIcon
            visible: window.files.length < 2 && source !== ""
            anchors.verticalCenter: parent.verticalCenter
            width: 26
            height: 26
            source: window.head ? Theme.classIcon(window.head.class) : ""
            mipmap: true
        }
        Column {
            visible: window.files.length < 2
            x: classIcon.visible ? 36 : 0
            anchors.verticalCenter: parent.verticalCenter
            width: numbers.x - x - 24
            spacing: 2
            UiText {
                width: parent.width
                text: window.head ? window.head.classLabel : ""
                font.pixelSize: Theme.sizeTitle
                font.weight: Font.DemiBold
            }
            UiText {
                width: parent.width
                text: window.head ? [window.who(Object.assign({}, window.head, { "classLabel": "" })), window.seen(window.head)]
                    .filter(s => s !== "").join("  ·  ") : ""
                muted: true
                font.pixelSize: Theme.sizeSmall
            }
        }
        // Several characters: the head is the list to pick one from.
        UiDropdown {
            visible: window.files.length > 1
            anchors.verticalCenter: parent.verticalCenter
            width: Math.min(560, numbers.x - 24)
            options: window.files.map(e => ({ "label": [window.who(e), window.seen(e)].filter(s => s !== "").join("  ·  "),
                                               "value": e.key, "icon": Theme.classIcon(e.class) }))
            current: Character.chosen
            onPicked: value => Character.choose(value)
        }

        Row {
            id: numbers
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            spacing: 28
            Repeater {
                model: window.view ? [
                    [I18n.tr("character.itemLevel"), window.view.itemLevel],
                    [I18n.tr("character.combatPower"), window.view.combatPower]
                ] : []
                Column {
                    id: number
                    required property var modelData
                    spacing: 1
                    UiText {
                        text: number.modelData[0]
                        muted: true
                        font.pixelSize: Theme.sizeSmall
                    }
                    // The highest the game has recorded, right after the number.
                    Row {
                        spacing: 6
                        UiText {
                            id: value
                            text: number.modelData[1] ? Format.whole(number.modelData[1].current) : "-"
                            numeric: true
                            font.pixelSize: Theme.sizeTitle
                            font.weight: Font.DemiBold
                            color: Theme.number
                        }
                        UiText {
                            visible: !!number.modelData[1] && number.modelData[1].highest > number.modelData[1].current
                            anchors.baseline: value.baseline
                            text: number.modelData[1] ? I18n.tr("character.highest").replace("{n}", Format.whole(number.modelData[1].highest)) : ""
                            numeric: true
                            color: Theme.note
                            font.pixelSize: Theme.sizeSmall
                        }
                    }
                }
            }
        }
    }

    UiSegmented {
        id: tabs
        visible: !!window.view
        x: Theme.gutter - 4
        anchors.top: header.bottom
        anchors.topMargin: 10
        framed: false
        options: [
            { "label": I18n.tr("character.tabs.gear"), "value": 0 },
            { "label": I18n.tr("character.tabs.sheet"), "value": 1 },
            { "label": I18n.tr("character.tabs.skills"), "value": 2 },
            { "label": I18n.tr("character.tabs.boards"), "value": 3 }
        ]
        current: window.tab
        onPicked: value => window.tab = value
    }
    Rectangle {
        id: rule
        visible: !!window.view
        anchors.top: tabs.bottom
        anchors.topMargin: 8
        width: parent.width
        height: 1
        color: Theme.hairline
    }

    Loader {
        id: pages
        anchors.top: rule.bottom
        anchors.bottom: parent.bottom
        width: parent.width
        active: !!window.view
        sourceComponent: [gearPage, sheetPage, skillsPage, boardsPage][window.tab]
    }
    // Every page scrolls when it outgrows the window: a bar in the right
    // gutter says so, and where in the page you are.
    UiScrollBar {
        readonly property Flickable page: pages.item as Flickable
        vertical: true
        anchors.top: pages.top
        anchors.bottom: pages.bottom
        anchors.margins: 4
        anchors.right: parent.right
        anchors.rightMargin: 3
        position: page ? page.contentY : 0
        range: page ? Math.max(0, page.contentHeight - page.height) : 0
        view: page ? page.height : 1
        onMoved: p => page.contentY = p
    }
    Component {
        id: gearPage
        CharacterGear {
            view: window.view
            baseShown: window.page === "base"
        }
    }
    Component {
        id: sheetPage
        CharacterStats {
            view: window.view
        }
    }
    Component {
        id: skillsPage
        CharacterSkills {
            view: window.view
        }
    }
    Component {
        id: boardsPage
        CharacterBoards {
            view: window.view
        }
    }

    // No character read yet: what to do in game, in a line.
    Column {
        visible: Character.ready && !window.view
        anchors.centerIn: parent
        width: Math.min(520, parent.width - 2 * Theme.gutter)
        spacing: 8
        UiText {
            visible: !Character.packFailed
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            text: I18n.tr("character.empty")
            font.pixelSize: Theme.sizeTitle
            font.weight: Font.DemiBold
        }
        UiText {
            visible: !Character.packFailed
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            text: I18n.tr("character.emptyHint")
            muted: true
            wrapMode: Text.WordWrap
            maximumLineCount: 4
        }
        UiText {
            visible: Character.packFailed
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            text: I18n.tr("character.packFailed")
            color: Theme.warning
            wrapMode: Text.WordWrap
            maximumLineCount: 3
        }
        UiText {
            visible: !!window.list && window.list.unread > 0
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            text: window.list ? I18n.tr("character.unread").replace("{n}", window.list.unread) : ""
            color: Theme.note
            font.pixelSize: Theme.sizeSmall
        }
    }
}
