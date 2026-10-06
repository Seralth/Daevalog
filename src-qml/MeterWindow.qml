// SPDX-License-Identifier: MIT
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Daevalog

// The meter overlay: frameless, see-through, kept on top; the lock makes
// it click-through.
Window {
    id: meter

    // Set by LayerMeterWindow on Wayland. A layer surface ignores system
    // move and resize: it moves by its margins and grows from its top-left.
    property bool layerSurface: false

    // Mock fight for step 1; the Meter model replaces it in step 3.
    property string fightTime: "02:14"
    property string target: "Krao Seal Guardian"
    property string zone: "Krao Cave"
    property real bossHealth: 0.62
    property string mode: "BOSS"
    property string groupTotal: "44.3K"
    property string ping: "18 ms"
    property int tab: 0

    readonly property int naturalHeight: 2 + Theme.headerHeight + Theme.bossHeight
        + Theme.headsHeight + rowList.implicitHeight + 10 + Theme.footerHeight

    signal menuRequested()
    signal closeRequested()

    width: 360
    minimumWidth: 260
    minimumHeight: 2 + Theme.headerHeight + Theme.footerHeight
    visible: true
    color: "transparent"
    title: "Daevalog"
    flags: Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint
           | (Overlay.locked ? Qt.WindowTransparentForInput : 0)

    // Not a binding: a resize by the user must stick.
    Component.onCompleted: height = naturalHeight

    ListModel {
        id: mockRows
        ListElement { job: "sorcerer"; name: "Morwen"; total: "2.61M"; rate: "19.5K"; share: "44%"; fill: 1.0; you: false }
        ListElement { job: "gladiator"; name: "Kaelith"; total: "1.94M"; rate: "14.5K"; share: "33%"; fill: 0.74; you: true }
        ListElement { job: "templar"; name: "Brannoc"; total: "0.98M"; rate: "7.3K"; share: "16%"; fill: 0.38; you: false }
        ListElement { job: "chanter"; name: "Ilsa"; total: "0.41M"; rate: "3.1K"; share: "7%"; fill: 0.16; you: false }
    }

    component HeaderButton: Item {
        id: button
        property bool hovered: hover.hovered
        signal clicked()
        implicitWidth: 16
        implicitHeight: 16
        HoverHandler { id: hover }
        TapHandler { onTapped: button.clicked() }
    }

    component SmallText: HudText {
        font.pixelSize: Theme.sizeSmall
        color: Theme.sub
    }

    // An edge or corner that resizes the window.
    component ResizeGrip: Item {
        id: grip
        required property int edges
        readonly property bool growsOnly: !(edges & (Qt.LeftEdge | Qt.TopEdge))
        property size startSize

        enabled: !Overlay.locked && (!meter.layerSurface || growsOnly)

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
            onActiveChanged: {
                if (!active)
                    return
                if (meter.layerSurface)
                    grip.startSize = Qt.size(meter.width, meter.height)
                else
                    meter.startSystemResize(grip.edges)
            }
            // A layer surface keeps its top-left corner, so the pointer's
            // travel is the size change.
            onTranslationChanged: {
                if (!active || !meter.layerSurface)
                    return
                if (grip.edges & Qt.RightEdge)
                    meter.width = Math.max(meter.minimumWidth, Math.round(grip.startSize.width + translation.x))
                if (grip.edges & Qt.BottomEdge)
                    meter.height = Math.max(meter.minimumHeight, Math.round(grip.startSize.height + translation.y))
            }
        }
    }

    Rectangle {
        id: panel
        anchors.fill: parent
        radius: 6
        color: Theme.panel
        border.color: Theme.panelEdge
        border.width: 1

        ColumnLayout {
            anchors.fill: parent
            anchors.margins: 1
            spacing: 0

            // Header: state dot, fight time, target; the zone quiet.
            Item {
                Layout.fillWidth: true
                Layout.preferredHeight: Theme.headerHeight

                DragHandler {
                    target: null
                    enabled: !Overlay.locked
                    onActiveChanged: {
                        if (active && !meter.layerSurface)
                            meter.startSystemMove()
                    }
                    // The window follows the pointer, so what is left of the
                    // translation is the next step.
                    onTranslationChanged: {
                        if (active && meter.layerSurface)
                            Overlay.moveLayerBy(Math.round(translation.x), Math.round(translation.y))
                    }
                }

                RowLayout {
                    anchors.fill: parent
                    anchors.leftMargin: 10
                    anchors.rightMargin: 10
                    spacing: 8

                    Rectangle {
                        implicitWidth: 7
                        implicitHeight: 7
                        radius: 3.5
                        color: Theme.accent
                    }
                    HudText {
                        numeric: true
                        text: meter.fightTime
                        color: Theme.number
                        font.weight: Font.DemiBold
                    }
                    HudText {
                        Layout.fillWidth: true
                        text: meter.target
                        font.weight: Font.DemiBold
                    }
                    SmallText {
                        text: meter.zone
                    }
                    // Drawn, not glyphs: Pretendard has no cross, and one font is the rule.
                    // Each mark sits on a dark copy, like the text's outline.
                    HeaderButton {
                        id: menuButton
                        onClicked: meter.menuRequested()
                        Rectangle {
                            anchors.centerIn: parent
                            width: 12
                            height: 4
                            radius: 2
                            color: Theme.outline
                        }
                        Row {
                            anchors.centerIn: parent
                            spacing: 2
                            Repeater {
                                model: 3
                                Rectangle {
                                    width: 2
                                    height: 2
                                    radius: 1
                                    color: menuButton.hovered ? Theme.name : Theme.sub
                                }
                            }
                        }
                    }
                    HeaderButton {
                        id: closeButton
                        onClicked: meter.closeRequested()
                        Repeater {
                            model: [45, -45]
                            Rectangle {
                                required property int modelData
                                anchors.centerIn: parent
                                width: 13
                                height: 3.5
                                radius: 1.75
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
                                width: 11
                                height: 1.5
                                rotation: modelData
                                antialiasing: true
                                color: closeButton.hovered ? Theme.name : Theme.sub
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

            // Boss health, when the target is a boss.
            Rectangle {
                Layout.fillWidth: true
                Layout.preferredHeight: Theme.bossHeight
                visible: meter.bossHealth >= 0
                color: Theme.bossTrack
                Rectangle {
                    width: Math.round(parent.width * meter.bossHealth)
                    height: parent.height
                    color: Theme.error
                    opacity: 0.75
                }
            }

            // Column heads, on the rows' columns.
            Item {
                Layout.fillWidth: true
                Layout.preferredHeight: Theme.headsHeight
                Item {
                    anchors.fill: parent
                    anchors.leftMargin: 6
                    anchors.rightMargin: 6
                    SmallText {
                        x: Theme.padLeft + Theme.colIcon + Theme.colGap
                        height: parent.height
                        text: "Name"
                    }
                    SmallText {
                        anchors.right: rateHead.left
                        anchors.rightMargin: Theme.colGap
                        width: Theme.colTotal
                        height: parent.height
                        horizontalAlignment: Text.AlignRight
                        text: "Total"
                    }
                    SmallText {
                        id: rateHead
                        anchors.right: shareHead.left
                        anchors.rightMargin: Theme.colGap
                        width: Theme.colRate
                        height: parent.height
                        horizontalAlignment: Text.AlignRight
                        text: "DPS"
                    }
                    SmallText {
                        id: shareHead
                        anchors.right: parent.right
                        anchors.rightMargin: Theme.padRight
                        width: Theme.colShare
                        height: parent.height
                        horizontalAlignment: Text.AlignRight
                        text: "%"
                    }
                }
            }

            // Players.
            Item {
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                Column {
                    id: rowList
                    x: 6
                    y: 4
                    width: parent.width - 12
                    spacing: Theme.rowGap
                    Repeater {
                        model: mockRows
                        delegate: MeterRow {
                            width: rowList.width
                        }
                    }
                }
            }

            // Footer: tabs and mode on the left, group total and ping right.
            Item {
                Layout.fillWidth: true
                Layout.preferredHeight: Theme.footerHeight

                Rectangle {
                    width: parent.width
                    height: 1
                    color: Theme.hairline
                }

                RowLayout {
                    anchors.fill: parent
                    anchors.leftMargin: 8
                    anchors.rightMargin: 8
                    spacing: 2

                    Repeater {
                        model: ["DPS", "Heal", "Tank"]
                        Rectangle {
                            id: tabButton
                            required property string modelData
                            required property int index
                            readonly property bool active: meter.tab === index
                            implicitWidth: tabLabel.implicitWidth + 14
                            implicitHeight: tabLabel.implicitHeight + 6
                            radius: 3
                            color: active ? Theme.accentTint : "transparent"
                            SmallText {
                                id: tabLabel
                                anchors.centerIn: parent
                                text: tabButton.modelData
                                color: tabButton.active ? Theme.accent : Theme.sub
                                font.weight: tabButton.active ? Font.DemiBold : Font.Normal
                            }
                            TapHandler {
                                onTapped: meter.tab = tabButton.index
                            }
                        }
                    }
                    Rectangle {
                        Layout.leftMargin: 8
                        implicitWidth: modeLabel.implicitWidth + 12
                        implicitHeight: modeLabel.implicitHeight + 6
                        radius: 3
                        color: "transparent"
                        border.color: Theme.hairline
                        border.width: 1
                        SmallText {
                            id: modeLabel
                            anchors.centerIn: parent
                            text: meter.mode
                        }
                    }
                    Item {
                        Layout.fillWidth: true
                    }
                    SmallText {
                        numeric: true
                        text: meter.groupTotal
                        color: Theme.number
                        font.weight: Font.DemiBold
                    }
                    SmallText {
                        Layout.leftMargin: 8
                        numeric: true
                        text: meter.ping
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
