// SPDX-License-Identifier: MIT
pragma Singleton

import QtQuick

// The design bible's palette, type and sizes. Nothing outside this file
// picks a colour.
QtObject {
    // Panel: rgb(9, 13, 24) at the window opacity (2.0 default 60%).
    readonly property real panelOpacity: 0.6
    readonly property color panel: Qt.rgba(9 / 255, 13 / 255, 24 / 255, panelOpacity)
    readonly property color panelEdge: Qt.rgba(1, 1, 1, 0.08)
    readonly property color rowEmpty: Qt.rgba(1, 1, 1, 0.035)
    readonly property color hairline: Qt.rgba(1, 1, 1, 0.07)
    readonly property color bossTrack: Qt.rgba(1, 1, 1, 0.06)

    readonly property color name: Qt.rgba(1, 1, 1, 0.95)
    readonly property color number: "#ffffff"
    readonly property color share: Qt.rgba(1, 1, 1, 0.8)
    readonly property color sub: Qt.rgba(1, 1, 1, 0.45)
    readonly property color faint: Qt.rgba(1, 1, 1, 0.3)
    readonly property color outline: Qt.rgba(0, 0, 0, 0.85)

    readonly property color accent: "#5fcaff"
    readonly property color accentTint: Qt.rgba(accent.r, accent.g, accent.b, 0.1)
    readonly property color error: "#e06b6b"

    // Bars: flat class colour at 40%, a thin solid edge at their end.
    readonly property real barStrength: 0.4

    readonly property var classColors: ({
        "gladiator": "#4FD1C5",
        "templar": "#5F8CFF",
        "ranger": "#41D98A",
        "assassin": "#7BE35A",
        "sorcerer": "#9A6BFF",
        "spiritmaster": "#E06BFF",
        "cleric": "#F2C15A",
        "chanter": "#FF9A3D",
        "fighter": "#E85D5D"
    })

    function classColor(job) {
        return classColors[job] || faint
    }

    function barFill(c) {
        return Qt.rgba(c.r, c.g, c.b, barStrength)
    }

    // 60% class colour, 40% white: the edge stays visible on dark fills.
    function barEdge(c) {
        return Qt.rgba(c.r * 0.6 + 0.4, c.g * 0.6 + 0.4, c.b * 0.6 + 0.4, 1)
    }

    function classIcon(job) {
        return "qrc:/daevalog/classes/" + job + ".png"
    }

    // One family: Pretendard, bundled.
    readonly property FontLoader pretendard: FontLoader {
        source: "qrc:/daevalog/fonts/PretendardVariable.woff2"
    }
    readonly property string font: pretendard.status === FontLoader.Ready ? pretendard.name : "sans-serif"

    readonly property int sizeSmall: 11
    readonly property int sizeRow: 13

    readonly property int rowHeight: 30
    readonly property int rowGap: 3
    readonly property int headerHeight: 30
    readonly property int headsHeight: 20
    readonly property int bossHeight: 6
    readonly property int footerHeight: 28

    // Row columns: icon | name | total | per second | share.
    readonly property int iconSize: 18
    readonly property int colIcon: 22
    readonly property int colTotal: 64
    readonly property int colRate: 56
    readonly property int colShare: 40
    readonly property int colGap: 6
    readonly property int padLeft: 8
    readonly property int padRight: 10

    // Fixed-width digits so number columns do not jitter. font.features
    // exists from Qt 6.6; older Qt keeps the default digits.
    function tabularDigits(text) {
        try {
            text.font.features = { "tnum": 1 }
        } catch (e) {
        }
    }
}
