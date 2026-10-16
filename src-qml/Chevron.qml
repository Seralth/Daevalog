// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import Daevalog

// A small drawn "v": Pretendard has no arrow glyphs, and one font is the rule.
Item {
    id: chevron
    property color color: Theme.sub
    // Points down; 90 points left, -90 right.
    property real turn: 0

    implicitWidth: 10
    implicitHeight: 6
    rotation: turn

    Rectangle {
        x: 0
        y: 2
        width: 6
        height: 1.5
        rotation: 40
        antialiasing: true
        color: chevron.color
    }
    Rectangle {
        x: 4
        y: 2
        width: 6
        height: 1.5
        rotation: -40
        antialiasing: true
        color: chevron.color
    }
}
