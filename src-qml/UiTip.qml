// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import Daevalog

// A note shown under an item once the pointer rests on it (UiTipCard with
// one text). Put it inside the item it explains. The meter's notes wait
// longer (Theme.meterTipRest): the pointer crosses the meter on its way to
// other windows.
UiTipCard {
    id: tip

    property string text

    active: text !== ""
    cardWidth: note.width
    padX: 8
    padY: 5

    UiText {
        id: note
        // No wider than the window: the meter can be 260 px.
        width: Math.min(300, implicitWidth, Math.max(120, (tip.Window.contentItem ? tip.Window.contentItem.width : 324) - 24))
        text: tip.text
        wrapMode: Text.WordWrap
        maximumLineCount: 8
        // The rows' size: at 11 px the text blurred.
        font.pixelSize: Theme.sizeRow
    }
}
