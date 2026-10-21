// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import QtQuick.Shapes
import Daevalog

// A notice's kind as a small mark: "pet" a soul wisp, "boss" crossed swords
// (a Field Boss), "rift" an orbit in red (a Spacetime Rift), "warning" an
// exclamation mark in the warning colour in a ring, anything else an "i".
// The swords and the orbit are Lucide's outline icons (ISC licence, see
// LICENSE), drawn in the theme's colours.
Item {
    id: icon
    property string kind: "info"
    readonly property bool pet: kind === "pet"
    readonly property bool warning: kind === "warning"
    // Lucide's paths on its 24 px grid, as SVG path data.
    readonly property var outlines: ({
        "boss": "M14.5 17.5 L3 6 L3 3 L6 3 L17.5 14.5 M13 19 L19 13 M16 16 L20 20 M19 21 L21 19 "
                + "M14.5 6.5 L18 3 L21 3 L21 6 L17.5 9.5 M5 14 L9 18 M7 17 L4 20 M3 19 L5 21",
        "rift": "M15 12 A3 3 0 1 1 9 12 A3 3 0 1 1 15 12 M21 5 A2 2 0 1 1 17 5 A2 2 0 1 1 21 5 "
                + "M7 19 A2 2 0 1 1 3 19 A2 2 0 1 1 7 19 M10.4 21.9 a10 10 0 0 0 9.941 -15.416 "
                + "M13.5 2.1 a10 10 0 0 0 -9.841 15.416"
    })
    readonly property bool outlined: outlines[kind] !== undefined
    readonly property color ink: warning ? Theme.warning : kind === "rift" ? Theme.rift : Theme.hudSub

    implicitWidth: 14
    implicitHeight: 14

    Image {
        visible: icon.pet
        anchors.fill: parent
        source: icon.pet ? "qrc:/daevalog/icons/pet-soul.svg" : ""
        sourceSize: Qt.size(width * 2, height * 2)
        fillMode: Image.PreserveAspectFit
        smooth: true
    }

    // Drawn on Lucide's 24 px grid and scaled to the icon's size.
    Shape {
        visible: icon.outlined
        width: 24
        height: 24
        scale: icon.width / 24
        transformOrigin: Item.TopLeft
        preferredRendererType: Shape.CurveRenderer
        ShapePath {
            strokeColor: icon.ink
            strokeWidth: 2
            fillColor: "transparent"
            capStyle: ShapePath.RoundCap
            joinStyle: ShapePath.RoundJoin
            PathSvg { path: icon.outlined ? icon.outlines[icon.kind] : "" }
        }
    }

    // Drawn at 14 px and scaled to the icon's size.
    Item {
        visible: !icon.pet && !icon.outlined
        width: 14
        height: 14
        scale: icon.width / 14
        transformOrigin: Item.TopLeft
        Rectangle {
            anchors.fill: parent
            radius: width / 2
            color: Theme.outline
            border.width: 1.5
            border.color: icon.ink
            antialiasing: true
        }
        // The stroke, then the dot: below it for "i", above it for "!".
        Rectangle {
            x: (14 - width) / 2
            y: icon.warning ? 3 : 6
            width: 1.75
            height: 5
            radius: 0.75
            color: icon.ink
        }
        Rectangle {
            x: (14 - width) / 2
            y: icon.warning ? 9 : 3.25
            width: 2
            height: 2
            radius: 1
            color: icon.ink
        }
    }
}
