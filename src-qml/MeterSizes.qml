// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick

// The meter window's sizes (Theme.meter), at Settings > Appearance > Meter
// size. At 100% each is the meter's own; the others grow every one of them
// alike, so the meter keeps its proportions, and its text is set in the
// larger size, not drawn larger, so it stays sharp. The other windows keep
// Theme's sizes.
QtObject {
    // 1 is 100%.
    property real scale: 1
    // Slim mode: 22 px rows, no column heads, a 3 px boss strip without text.
    property bool slim: false

    // A length in whole pixels; a mark's part, a radius or an offset as it
    // comes, so a small drawn mark keeps its shape.
    function px(n) {
        return Math.round(n * scale)
    }
    function part(n) {
        return n * scale
    }

    // Theme's type scale: 11 for heads and the footer, 13 for rows.
    readonly property int sizeSmall: px(11)
    readonly property int sizeRow: px(13)

    readonly property int rowHeight: px(slim ? 22 : 30)
    readonly property int rowGap: px(slim ? 2 : 3)
    readonly property int headerHeight: px(30)
    readonly property int headsHeight: px(20)
    // The boss's health bar, tall enough for its text (sizeSmall); slim's
    // is a thin line without text.
    readonly property int bossBand: px(slim ? 3 : 16)
    readonly property int footerHeight: px(28)

    // Row columns: icon | name | numbers. Number columns are set per tab.
    readonly property int iconSize: px(slim ? 15 : 18)
    readonly property int colIcon: px(22)
    readonly property int colGap: px(6)
    readonly property int padLeft: px(8)
    readonly property int padRight: px(10)
}
