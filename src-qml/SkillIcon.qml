// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import Daevalog

// A skill row's icon from Daevalog's own set (SkillIcons), `size` px square.
// The SVG is drawn at that size, so it stays sharp at every size.
Image {
    id: icon

    property int code: 0
    // The skill's class (a Theme class key): a row with no icon of its own
    // gets the generic glyph in this class's colour.
    property string job: ""
    property int size: 20

    width: size
    height: size
    sourceSize: Qt.size(size, size)
    // Hidden (a table cell without an icon): nothing loaded.
    source: visible ? SkillIcons.source(code, job) : ""
    fillMode: Image.PreserveAspectFit
}
