// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import QtQuick.Shapes
import Daevalog

// A gear slot's glyph (icons/gear/, through GearGlyphs), `size` px square,
// drawn as an outline in `color`: the worn item's grade colour, the faint
// line colour for an empty slot.
Item {
    id: glyph

    // The game's slot name, such as "MainHand" or "Ring2".
    property string slot: ""
    property color color: Theme.faint
    property int size: 20
    readonly property string outline: GearGlyphs.outline(slot)

    implicitWidth: size
    implicitHeight: size

    // Drawn on the glyphs' 24 px grid and scaled to the glyph's size.
    Shape {
        visible: glyph.outline !== ""
        width: 24
        height: 24
        scale: glyph.width / 24
        transformOrigin: Item.TopLeft
        preferredRendererType: Shape.CurveRenderer
        ShapePath {
            strokeColor: glyph.color
            strokeWidth: 2
            fillColor: "transparent"
            capStyle: ShapePath.RoundCap
            joinStyle: ShapePath.RoundJoin
            PathSvg { path: glyph.outline }
        }
    }
}
