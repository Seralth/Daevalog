// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import Daevalog

// One line of text input, with a quiet placeholder.
Rectangle {
    id: field

    property alias text: input.text
    property string placeholder
    property bool numeric: false
    // The cursor is in the field: the player may be typing.
    readonly property alias editing: input.activeFocus
    // Enter, or the field left.
    signal edited(string text)
    // Each change the player makes.
    signal typing(string text)

    implicitWidth: 220
    implicitHeight: 28
    radius: 4
    color: Theme.surface
    border.width: 1
    border.color: input.activeFocus ? Theme.accent : Theme.fieldEdge

    TextInput {
        id: input
        anchors.fill: parent
        anchors.leftMargin: 10
        anchors.rightMargin: 10
        verticalAlignment: TextInput.AlignVCenter
        font.family: Theme.font
        font.pixelSize: Theme.sizeRow
        color: Theme.name
        selectionColor: Theme.accentTint
        selectedTextColor: Theme.name
        clip: true
        onEditingFinished: field.edited(text)
        onTextEdited: field.typing(text)
    }
    UiText {
        anchors.fill: input
        visible: input.text === "" && !input.activeFocus
        text: field.placeholder
        color: Theme.note
    }
}
