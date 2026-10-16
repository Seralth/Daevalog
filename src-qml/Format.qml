// SPDX-License-Identifier: AGPL-3.0-only
pragma Singleton
import QtQuick

// Numbers as the meter writes them (snapshot.rs): every window formats
// through these.
QtObject {
    function amount(v) {
        if (v >= 100000)
            return (v / 1000000).toFixed(2) + "M"
        if (v >= 1000)
            return (v / 1000).toFixed(1) + "K"
        return v.toFixed(0)
    }
    function rate(v) {
        if (v >= 1000000)
            return (v / 1000000).toFixed(2) + "M"
        if (v >= 1000)
            return (v / 1000).toFixed(1) + "K"
        return v.toFixed(0)
    }
    function pct(v) {
        return v.toFixed(0) + "%"
    }
    // A share, as snapshot.rs `share`: whole percent from 10% up, one
    // decimal below, as 1.x writes every share.
    function share(v) {
        const tenths = Math.round(v * 10) / 10
        return tenths < 10 ? tenths.toFixed(1) + "%" : v.toFixed(0) + "%"
    }
    function pct1(v) {
        return v.toFixed(1) + "%"
    }
    // Each comma follows a digit: Qt 6.4 writes two commas for a match
    // that takes no characters.
    function whole(v) {
        return Math.round(v).toString().replace(/(\d)(?=(\d{3})+$)/g, "$1,")
    }
    function clock(seconds) {
        const s = Math.max(0, Math.floor(seconds))
        return String(Math.floor(s / 60)).padStart(2, "0") + ":" + String(s % 60).padStart(2, "0")
    }
}
