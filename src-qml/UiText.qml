// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import Daevalog

// Text in the meter's other windows, which are read off the game: the same
// family and sizes as the overlay, without its outline.
Text {
    // Numbers get fixed-width digits so their columns do not jitter.
    property bool numeric: false
    // Secondary text: column heads and labels next to a value. Hints and
    // notes take Theme.note.
    property bool muted: false

    font.family: Theme.font
    font.pixelSize: Theme.sizeRow
    color: muted ? Theme.sub : Theme.name
    verticalAlignment: Text.AlignVCenter
    elide: Text.ElideRight
    maximumLineCount: 1

    Component.onCompleted: {
        if (numeric)
            Theme.tabularDigits(this)
    }
}
