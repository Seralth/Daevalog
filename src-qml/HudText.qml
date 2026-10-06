// SPDX-License-Identifier: MIT
pragma ComponentBehavior: Bound
import QtQuick
import Daevalog

// Text over the game: Pretendard on a dark outline and a soft drop, so it
// reads on snow. The outline is a copy behind the text, not Text.Outline,
// so muted (see-through) text shows the dark copy through it.
Text {
    id: label

    // Numbers get fixed-width digits so their columns do not jitter.
    property bool numeric: false

    font.family: Theme.font
    font.pixelSize: Theme.sizeRow
    color: Theme.name
    verticalAlignment: Text.AlignVCenter
    elide: Text.ElideRight
    maximumLineCount: 1

    component Shadow: Text {
        width: label.width
        height: label.height
        z: -1
        text: label.text
        font: label.font
        horizontalAlignment: label.horizontalAlignment
        verticalAlignment: label.verticalAlignment
        elide: label.elide
        maximumLineCount: 1
        color: Theme.outline
        style: Text.Outline
        styleColor: Theme.outline
    }

    Shadow {
        y: 2
        opacity: 0.45
    }
    Shadow {}

    Component.onCompleted: {
        if (numeric)
            Theme.tabularDigits(this)
    }
}
