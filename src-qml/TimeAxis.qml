// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import Daevalog

// Fight time under a chart or the timeline: m:ss every 10 s, or every 15 s,
// 30 s, 1 min, 2 min ... where the labels would be closer than 64 px.
Item {
    id: axis

    property real seconds: 0
    property int textSize: Theme.sizeSmall
    readonly property int step: {
        const perSecond = width / Math.max(1, seconds)
        for (const s of [10, 15, 30, 60, 120, 300, 600]) {
            if (s * perSecond >= Math.max(64, textSize * 6))
                return s
        }
        return 600
    }

    height: textSize + 5

    Repeater {
        model: axis.seconds > 0 ? Math.floor(axis.seconds / axis.step) + 1 : 0
        UiText {
            required property int index
            readonly property int at: index * axis.step
            x: Math.min(axis.width - width, Math.max(0, at / axis.seconds * axis.width - width / 2))
            height: axis.height
            numeric: true
            muted: true
            font.pixelSize: axis.textSize
            text: Math.floor(at / 60) + ":" + String(at % 60).padStart(2, "0")
        }
    }
}
