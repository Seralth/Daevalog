// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick

// One entry of a UiMenu.
QtObject {
    property string text
    // Drawn with a tick when checked; choosing it sends `triggered`, which
    // changes what `checked` is bound to.
    property bool checkable: false
    property bool checked: false
    property bool visible: true
    // Drawn before the text when set: a class's icon.
    property url icon
    signal triggered()
}
