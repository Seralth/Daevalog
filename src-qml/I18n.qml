// SPDX-License-Identifier: AGPL-3.0-only
pragma Singleton
import QtQuick
import Daevalog

// The UI strings in the player's language, from the 1.x meter's translation
// files: I18n.tr("tray.quit"). Every function reads `language`, so a binding
// that calls one is redone when the language changes. Keys are written out
// whole, never built, so the key test (i18n.rs) finds every one.
QtObject {
    // The Language setting, or the system's language when it is unset.
    readonly property string language: Strings.resolve(Settings.language)

    function tr(key) {
        return Strings.text(language, key)
    }

    // The footer's name for one of the backend's modes.
    function modeName(mode) {
        switch (mode) {
        case "bossTargets":
            return tr("meter.modes.bossTargets")
        case "lastHitByMe":
            return tr("meter.modes.lastHitByMe")
        case "allTargets":
            return tr("meter.modes.allTargets")
        case "trainTargets":
            return tr("meter.modes.trainTargets")
        case "encounter":
            return tr("meter.modes.encounter")
        }
        return ""
    }

    // The header's target: the backend's, or for a mode that follows no one
    // target ("All Targets") the mode's own name.
    function targetName(mode, target) {
        return Strings.targetName(language, mode, target)
    }

    // A meter row's player: the name, "#id" until the game names them,
    // YOU when chosen, or class and number when other players' names are
    // hidden.
    function playerName(row) {
        return Strings.playerName(language, row.name, row.job, row.number, row.you, row.identifying,
            row.unattributed, Settings.hideOtherNames, Settings.showYou)
    }

    // A time of day as Display Time writes it, on a 24-hour clock or not.
    function clock(hour, minute, hour24) {
        return Strings.clock(language, hour, minute, hour24)
    }
}
