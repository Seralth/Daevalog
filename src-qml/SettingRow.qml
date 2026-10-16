// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import Daevalog

// One setting: its name in the page's label column, the control right after
// it and the hint under the control, so no wide gap lies between a name and
// its control. A tip shows when the pointer rests
// on the name.
Item {
    id: row

    property string label
    property string hint
    property string tip
    property bool last: false
    default property alias control: slot.data

    // The page's label column (SettingsWindow `labelColumn`): as wide as the
    // page's longest name; 45% of the row where a page gives none.
    readonly property real labelWidth: parent && parent.labelWidth > 0 ? parent.labelWidth : Math.floor(width * 0.45) - 12
    // The name's width on one line, for the page's label column.
    readonly property real labelImplicit: name.implicitWidth

    width: parent ? parent.width : 480
    implicitHeight: Math.max(Theme.settingRow, controls.y + controls.height + 9, name.y + name.height + 9)

    UiText {
        id: name
        width: row.labelWidth
        y: controls.y + Math.max(0, (slot.height - height) / 2)
        text: row.label
        wrapMode: Text.WordWrap
        maximumLineCount: 2
        UiTip {
            text: row.tip
        }
    }
    Column {
        id: controls
        x: row.labelWidth + 16
        y: 9
        width: parent.width - x
        spacing: 4
        Item {
            id: slot
            width: parent.width
            height: childrenRect.height
        }
        UiText {
            width: parent.width
            visible: row.hint !== ""
            text: row.hint
            color: Theme.note
            font.pixelSize: Theme.sizeSmall
            wrapMode: Text.WordWrap
            maximumLineCount: 6
        }
    }
    Rectangle {
        visible: !row.last
        anchors.bottom: parent.bottom
        width: parent.width
        height: 1
        color: Theme.hairline
    }
}
