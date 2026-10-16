// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import Daevalog

// A field that shows the chosen option and opens the list as a menu, the
// same kind of menu as the meter's ⋯, as wide as the field.
Rectangle {
    id: field

    // [{ label, value }], and an `icon` beside the label when set.
    property var options: []
    property var current
    // The shown choice's text size (Details' Font size).
    property int textSize: Theme.sizeRow
    signal picked(var value)

    readonly property var currentOption: options.find(option => option.value === current)
    readonly property string currentLabel: currentOption ? currentOption.label : ""
    readonly property string currentIcon: currentOption && currentOption.icon ? currentOption.icon : ""

    implicitWidth: 220
    implicitHeight: Math.max(28, textSize + 15)
    radius: 4
    color: hover.hovered ? Theme.hover : Theme.surface
    border.width: 1
    border.color: Theme.fieldEdge

    Image {
        id: icon
        visible: field.currentIcon !== ""
        x: 10
        anchors.verticalCenter: parent.verticalCenter
        width: visible ? 16 : 0
        height: 16
        source: field.currentIcon
        fillMode: Image.PreserveAspectFit
        smooth: true
        mipmap: true
    }
    UiText {
        anchors.left: icon.right
        anchors.right: chevron.left
        anchors.leftMargin: icon.visible ? 6 : 0
        anchors.rightMargin: 6
        anchors.verticalCenter: parent.verticalCenter
        text: field.currentLabel
        font.pixelSize: field.textSize
    }
    Chevron {
        id: chevron
        anchors.right: parent.right
        anchors.rightMargin: 10
        anchors.verticalCenter: parent.verticalCenter
    }

    HoverHandler {
        id: hover
        cursorShape: Qt.PointingHandCursor
    }
    TapHandler {
        onTapped: menu.open(field)
    }

    UiMenu {
        id: menu
        minimumListWidth: field.width
        entries: field.options.map(option => ({ "text": option.label, "checkable": true,
                                                 "checked": option.value === field.current, "icon": option.icon || "" }))
        onActivated: index => field.picked(field.options[index].value)
    }
}
