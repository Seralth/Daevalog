// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick

// What the history list shows of its groups: `size` fights at first and
// `size` more on each `more`, with the groups whose heading folded them
// shown by their heading alone. Folded groups take none of the fights.
QtObject {
    id: pages

    property int size: 30
    property int limit: size
    // Folded groups, by title.
    property var folded: ({})

    function more() {
        limit += size
    }

    function toggle(title) {
        const next = Object.assign({}, folded)
        if (next[title] === true)
            delete next[title]
        else
            next[title] = true
        folded = next
    }

    function reset() {
        limit = size
        folded = {}
    }

    // { groups: [{ title, count, folded, fights }], more }: `more` when some
    // fights are left out.
    function cut(groups) {
        const shown = []
        let left = limit
        let rest = false
        for (const group of groups) {
            if (left <= 0) {
                rest = true
                break
            }
            const fold = group.title !== "" && folded[group.title] === true
            const fights = fold ? [] : group.fights.slice(0, left)
            left -= fights.length
            if (!fold && fights.length < group.fights.length)
                rest = true
            shown.push({ "title": group.title, "count": group.fights.length, "folded": fold, "fights": fights })
        }
        return { "groups": shown, "more": rest }
    }
}
