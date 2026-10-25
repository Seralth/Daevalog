// SPDX-License-Identifier: AGPL-3.0-only
pragma Singleton

import QtQuick
import Daevalog

// The palette, type and sizes. Nothing outside this file picks a colour.
QtObject {
    // The colour theme (Settings > Appearance):
    // "daevalog", the default and the supported one, from a desktop
    // scheme (a Nord variant from the KDE store); "classic", 2.0's first
    // look, close to 1.x's, the darkest; "nord", stock Nord (MIT, Arctic Ice
    // Studio), the lightest; "system", the desktop's palette, with no
    // promise. Each theme gives a few base colours; every role
    // below is made from them, so a control never takes a stray colour.
    readonly property string theme: Settings.colorTheme
    readonly property SystemPalette system: SystemPalette {
        colorGroup: SystemPalette.Active
    }
    readonly property var themes: ({
        "daevalog": {
            // KDE View BackgroundNormal; Window and Button BackgroundNormal.
            "window": "#161925",
            "surface": "#1E2233",
            // Lines, hovers and empty rows are this at a low alpha.
            "ink": "#ffffff",
            // KDE Window ForegroundNormal: 8.8:1 on the window, 7.9:1 on
            // the surface. Numbers stay white, the brightest.
            "name": "#66C2F2",
            "number": "#ffffff",
            // The meter's names and shares stay white: blue names on the
            // warm class bars (Chanter, Cleric) clash.
            "hud": "#ffffff",
            // Shares at 80%; heads and labels at 70%: 4.9:1 and 4.5:1.
            "muted": "#66C2F2",
            // 5.8:1 and 5.2:1; the 2.0 note colour #707C91 has 4.2:1 here.
            "note": "#8A95A8",
            "meterNote": "#8A95A8",
            // A line that carries values: white 38%, 3.5:1 on both.
            "valueLine": 0.38,
            // A scroll bar's thumb: white 42%, 4.3:1 on the window and
            // 3.8:1 on its track over the surface.
            "scrollThumb": 0.42,
            // The scheme has no red or green meaning (its Negative and
            // Positive are blue): status colours are the app's.
            "warning": "#F2C66D",
            "error": "#F07C7C",
            "negative": "#FF3B3B",
            // The notices' marks, on the window and on the panel over a mid
            // grey scene: an Abyss Boss red, 5.8:1 and 3.3:1; the Spacetime
            // Rift purple as its map icon, 6.8:1 and 3.9:1; the field boss
            // tiers: Gartua emerald, 9.1:1 and 5.2:1; the commanders yellow,
            // as Unique gear, 12.1:1 and 6.9:1.
            "abyssBoss": "#FF5C5C",
            "rift": "#B48CFF",
            "gartua": "#34D399",
            "commander": "#FFD23F",
            "ping": ["#7BD88F", "#F2C66D", "#F07C7C"],
            "accents": { "sky": "#5fcaff", "rose": "#ff6fae", "lemon": "#eef26b", "silver": "#c8d0dc" }
        },
        // The look the layer-1 screens were approved in, before themes:
        // white on near-black, as 1.x (#0A0E16). The darkest of the three,
        // and the most readable on the meter over a bright scene.
        "classic": {
            "window": "#090D18",
            // White 3% over the window, as the old translucent surface.
            "surface": "#10141F",
            "ink": "#ffffff",
            "name": Qt.rgba(1, 1, 1, 0.95),
            "number": "#ffffff",
            "hud": "#ffffff",
            "muted": "#ffffff",
            // Heads and labels at 45%, as before themes.
            "subAlpha": 0.45,
            // 4.6:1 on the window colour; 3.7:1 over a mid grey scene.
            "note": "#707C91",
            "meterNote": "#8A95A8",
            // 3.2:1 on the window colour.
            "valueLine": 0.35,
            // A scroll bar's thumb: 4.4:1 on the window, 4.0:1 on its track
            // over the surface.
            "scrollThumb": 0.42,
            "warning": "#f2b94f",
            "error": "#e06b6b",
            "negative": "#FF3B3B",
            // The notices' marks as the default theme's; on the window and
            // on the panel over a mid grey scene: 6.4:1 and 3.7:1, 7.5:1 and
            // 4.3:1, 10.1:1 and 5.8:1, 13.4:1 and 7.7:1.
            "abyssBoss": "#FF5C5C",
            "rift": "#B48CFF",
            "gartua": "#34D399",
            "commander": "#FFD23F",
            "ping": ["#7BD88F", "#F2C66D", "#F07C7C"],
            "accents": { "sky": "#5fcaff", "rose": "#ff6fae", "lemon": "#eef26b", "silver": "#c8d0dc" }
        },
        "nord": {
            // nord0 and nord1; text nord6; muted text is nord4 at 80% and
            // 70% (4.5:1 on nord1); nord3 stays for edges, never text.
            "window": "#2E3440",
            "surface": "#3B4252",
            "ink": "#ECEFF4",
            "name": "#ECEFF4",
            "number": "#ECEFF4",
            "hud": "#ECEFF4",
            "muted": "#D8DEE9",
            "note": Qt.rgba(0xD8 / 255, 0xDE / 255, 0xE9 / 255, 0.7),
            "meterNote": "#D8DEE9",
            // nord4 at 50%: 3.4:1 and 3.0:1.
            "valueLine": 0.5,
            // A scroll bar's thumb: nord6 at 55%, 4.6:1 on the window and
            // 3.7:1 on its track over nord1.
            "scrollThumb": 0.55,
            // nord13; nord11 red is 3.1:1 as text, so the softer #D08A91.
            "warning": "#EBCB8B",
            "error": "#D08A91",
            // nord11, Nord's own red; the hatch carries the meaning.
            "negative": "#BF616A",
            // The notices' marks, on the window and on the panel over a mid
            // grey scene. nord11 and nord15 are too dark for a mark on the
            // panel: a lighter red for an Abyss Boss, 4.9:1 and 3.1:1, and a
            // lighter purple for the rift, 5.8:1 and 3.6:1. The field boss
            // tiers in Nord's lighter tones: 7.0:1 and 4.4:1, 8.4:1 and 5.3:1.
            "abyssBoss": "#FF7A7A",
            "rift": "#C9A6C3",
            "gartua": "#8FD0A8",
            "commander": "#F0D264",
            "ping": ["#A3BE8C", "#EBCB8B", "#D08A91"],
            // Each preset as its Nord colour: frost, purple, yellow, snow.
            "accents": { "sky": "#88C0D0", "rose": "#B48EAD", "lemon": "#EBCB8B", "silver": "#D8DEE9" }
        }
    })
    readonly property var base: theme === "system" ? ({
        "window": system.window,
        "surface": system.base,
        "ink": system.windowText,
        "name": system.windowText,
        "number": system.windowText,
        "hud": system.windowText,
        "muted": system.windowText,
        "note": Qt.rgba(system.windowText.r, system.windowText.g, system.windowText.b, 0.6),
        "meterNote": system.windowText,
        "valueLine": 0.4,
        // A scroll bar's thumb: 3.6:1 or more on its track in Breeze and
        // Adwaita, light and dark.
        "scrollThumb": 0.6,
        "warning": themes.daevalog.warning,
        "error": themes.daevalog.error,
        "negative": themes.daevalog.negative,
        // The notices' marks as the default theme's; on Breeze Dark's
        // window (#202326) and on the panel over a mid grey scene: 5.2:1 and
        // 3.0:1, 6.1:1 and 3.6:1, 8.2:1 and 4.8:1, 10.9:1 and 6.4:1.
        "abyssBoss": themes.daevalog.abyssBoss,
        "rift": themes.daevalog.rift,
        "gartua": themes.daevalog.gartua,
        "commander": themes.daevalog.commander,
        "ping": themes.daevalog.ping,
        "accents": {}
    }) : (themes[theme] || themes.daevalog)

    function tint(c, alpha) {
        const k = Qt.lighter(c, 1.0)
        return Qt.rgba(k.r, k.g, k.b, alpha)
    }

    // Panel: the window colour at the window opacity setting (2.0 default
    // 72%), so the game shows through the meter.
    readonly property real panelOpacity: Settings.windowOpacity / 100
    readonly property color panel: tint(base.window, panelOpacity)
    readonly property color panelEdge: tint(base.ink, 0.08)
    readonly property color rowEmpty: tint(base.ink, 0.035)
    readonly property color hairline: tint(base.ink, 0.07)
    readonly property color bossTrack: tint(base.ink, 0.06)

    // The other windows (Details, History, Settings) are read off the game,
    // so the same panel colour is solid there.
    readonly property color window: base.window
    // A side column, a field: the theme's surface.
    readonly property color surface: base.surface
    readonly property color hover: tint(base.ink, 0.05)
    // A menu's entry under the pointer or the keys: a step brighter than a
    // hovered row, so the place the keys are on reads at a glance.
    readonly property color menuCurrent: tint(base.ink, 0.08)
    readonly property color fieldEdge: tint(base.ink, 0.12)

    readonly property color name: base.name
    readonly property color number: base.number
    readonly property color share: tint(base.muted, 0.8)
    readonly property color sub: tint(base.muted, base.subAlpha || 0.7)
    // Quiet text people still read: hints, footnotes, placeholders, a
    // paused state.
    readonly property color note: base.note
    // The note colour on the meter, whose see-through panel puts its text
    // on the game.
    readonly property color meterNote: base.meterNote
    // A line that carries values, such as the DPS chart's party line.
    readonly property color valueLine: tint(base.ink, base.valueLine)
    // Lines and marks that carry no values, never text.
    readonly property color faint: tint(base.ink, 0.3)
    // A scroll bar's thumb (UiScrollBar.qml): at least 3:1 on the window,
    // the surface and the bar's own track in every theme, so a page that
    // scrolls says so.
    readonly property color scrollThumb: tint(base.ink, base.scrollThumb)
    // A control that cannot act now (signed out, busy) is drawn this strong.
    readonly property real disabledOpacity: 0.4

    // The meter's own text, over the game: names and shares in the theme's
    // light ink, on an outline of the opposite brightness (dark under light
    // text, light under the dark text of a light System palette).
    readonly property color hudName: tint(base.hud, 0.95)
    readonly property color hudShare: tint(base.hud, 0.8)
    // The meter's small text and marks (column heads, footer tabs, header
    // icons) in the full light ink: the windows' muted colour fell to 2.2:1
    // to 2.7:1 over a snow scene through the see-through panel, and small
    // thin text needs it sharp.
    readonly property color hudSub: base.hud
    readonly property color outline: Qt.lighter(base.hud, 1.0).hslLightness > 0.5 ? Qt.rgba(0, 0, 0, 0.85) : Qt.rgba(1, 1, 1, 0.85)

    // One accent, from four presets, each in a hue gap between the class
    // colours; under System, the desktop's
    // highlight. Used only for the active tab, the state dot, your row's
    // rail, switches, sliders and selection.
    readonly property var accents: theme === "system" ? ({ "sky": system.highlight, "rose": system.highlight,
        "lemon": system.highlight, "silver": system.highlight }) : base.accents
    readonly property color accent: accents[Settings.accent] || accents.sky
    readonly property color accentTint: Qt.rgba(accent.r, accent.g, accent.b, 0.12)
    // Text on a filled accent (a switch's knob, a chosen swatch's ring).
    readonly property color onAccent: window
    // The ping by its band (Meter.pingLevel): none yet, up to 80 ms,
    // 81-150 ms, over 150 ms. Pale green, amber and coral, as the rule for
    // the see-through panels asks.
    readonly property var pingColors: [meterNote].concat(base.ping)
    // Only for real problems: a capture that cannot start, a number that
    // differs from the game's record.
    readonly property color warning: base.warning
    readonly property color error: base.error
    // Debuffs on the player: saturated red, drawn hatched (Timeline.qml).
    readonly property color negative: base.negative
    // The notices' marks (NoticeIcon.qml): an Abyss Boss in red; a
    // Spacetime Rift in purple, as its map icon; a field boss in its tier's
    // colour: Gartua and the commanders (the rest keep the meter's ink).
    readonly property color abyssBoss: base.abyssBoss
    readonly property color rift: base.rift
    readonly property color gartua: base.gartua
    readonly property color commander: base.commander

    // Bars: flat class colour, a thin solid edge at their end. The strength
    // is the "Bar strength" setting, 10-50%.
    readonly property real barStrength: Settings.barStrength / 100

    readonly property var classColors: ({
        "gladiator": "#4FD1C5",
        "templar": "#5F8CFF",
        "ranger": "#41D98A",
        "assassin": "#7BE35A",
        "sorcerer": "#9A6BFF",
        "spiritmaster": "#E06BFF",
        "cleric": "#F2C15A",
        "chanter": "#FF9A3D",
        // Muted so saturated red stays for negatives.
        "fighter": "#C47A7A"
    })

    // Item grades (the game's grade keys; Legend reads "Epic" in game and
    // Epic "Heroic"), on a worn item's name, glyph and edge in the Character
    // window: the hues of the game's own grade text styles (grey, green,
    // blue, gold, orange, red, cyan), lightened to read on the cards below:
    // 6.4:1 or more on Daevalog, 7.3:1 on Classic, 4.5:1 on Nord (Mythic;
    // on Nord's surface Legend is only 4.1:1, so the cards are a faint lift
    // off the window, not the surface). A light System palette takes the
    // same hues darker: 5.0:1 or more on its cards.
    readonly property var gradeColors: ({
        "Common": "#c8ccd4",
        "Rare": "#6fd08c",
        "Legend": "#6aa8ff",
        "Unique": "#f2c15a",
        "Epic": "#ff9a3d",
        "Mythic": "#ff7a85",
        "Special": "#3fe8d0"
    })
    readonly property var gradeColorsOnLight: ({
        "Common": "#4b5260",
        "Rare": "#216435",
        "Legend": "#003f96",
        "Unique": "#805a0a",
        "Epic": "#8a4300",
        "Mythic": "#a8001c",
        "Special": "#006b5e"
    })
    readonly property bool lightWindow: Qt.lighter(base.window, 1.0).hslLightness > 0.5
    function gradeColor(grade) {
        return (lightWindow ? gradeColorsOnLight : gradeColors)[grade] || note
    }
    // A card in the windows (the Character window's gear and boards): a
    // faint lift off the window, which keeps coloured text at the window's
    // contrast.
    readonly property color card: tint(base.ink, 0.035)

    // The windows' table bars (Details, History): the blue names sit on
    // them, so at most 12%, where the names keep 4.5:1 on every class colour
    // and the monsters' grey, a chosen row too; at the meter's 40% they fell
    // to 2.2:1. The meter's names are white and keep the setting.
    readonly property real tableBarStrength: Math.min(barStrength, 0.12)

    // A meter row whose share is low (Settings > Meter > Low share colour,
    // Row): the warning or error colour behind the whole row.
    readonly property real lowShareTint: 0.22

    function classColor(job) {
        return classColors[job] || valueLine
    }

    function barFill(c, strength) {
        return Qt.rgba(c.r, c.g, c.b, strength === undefined ? barStrength : strength)
    }

    // 60% class colour, 40% white: the edge stays visible on dark fills.
    function barEdge(c) {
        return Qt.rgba(c.r * 0.6 + 0.4, c.g * 0.6 + 0.4, c.b * 0.6 + 0.4, 1)
    }

    // A colour as Canvas takes it.
    function css(c) {
        const k = Qt.lighter(c, 1.0)
        return "rgba(" + Math.round(k.r * 255) + "," + Math.round(k.g * 255) + "," + Math.round(k.b * 255) + "," + k.a + ")"
    }

    function classIcon(job) {
        return job !== "" ? "qrc:/daevalog/classes/" + job + ".png" : ""
    }

    // One family: Pretendard, bundled.
    readonly property FontLoader pretendard: FontLoader {
        source: "qrc:/daevalog/fonts/PretendardVariable.ttf"
    }
    readonly property string font: pretendard.status === FontLoader.Ready ? pretendard.name : "sans-serif"

    // One type scale: 11 for heads, hints and the footer, 13 for rows and
    // body text, 15 for titles and headline numbers. Hierarchy comes from
    // weight and brightness, not more sizes.
    readonly property int sizeSmall: 11
    readonly property int sizeRow: 13
    readonly property int sizeTitle: 15

    // Slim mode: 22 px rows, no column heads, the boss strip 3 px without text.
    readonly property bool slim: Settings.slimMode
    // The meter window's type, rows, columns and bands, at the Meter size
    // setting (MeterSizes.qml).
    readonly property MeterSizes meter: MeterSizes {
        scale: Settings.meterSize / 100
        slim: Settings.slimMode
    }

    // The other windows: table rows, setting rows, the side column.
    readonly property int tableRow: 26
    readonly property int settingRow: 44
    readonly property int sideWidth: 200
    readonly property int gutter: 16

    // How long the pointer must rest on an item before its tooltip shows,
    // in ms; every move starts the wait again. The meter waits as long as
    // the windows: a longer wait felt slow.
    readonly property int tipRest: 700
    readonly property int meterTipRest: 700

    // Fixed-width digits so number columns do not jitter. font.features
    // exists from Qt 6.6; older Qt keeps the default digits.
    function tabularDigits(text) {
        try {
            text.font.features = { "tnum": 1 }
        } catch (e) {
        }
    }
}
