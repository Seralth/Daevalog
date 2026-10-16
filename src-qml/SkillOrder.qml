// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick

// The Details skill table's order and its damage over time rows, as 1.x's
// details.js (getSkillSortValue, compareSkillSort, the DOT sub-rows): a
// click on a column head sorts by it, a second click turns the order round;
// a skill opened on its damage over time shows its direct hits, and the
// damage over time on a row of its own under it.
QtObject {
    // A column's number in a row of `table` ("dmg", "heal", "taken",
    // "mechanics"). Rates sort by their counts, not the rounded percent.
    function value(row, table, key) {
        if (key === "name")
            return String(row.name || "").toLowerCase()
        if (table === "taken" || table === "mechanics") {
            switch (key) {
            case "taken": return row.damage || 0
            case "takenpct": return row.share || 0
            case "hits": return row.attacks || 0
            case "players": return row.players || 0
            }
            const share = row.rates ? row.rates[key] : null
            return share === null || share === undefined ? -1 : share
        }
        const hits = row.hits || 0
        const per = n => hits > 0 ? (n || 0) / hits : 0
        switch (key) {
        case "dmg":
        case "dmgpct":
        case "heal":
        case "healpct": return row.amount || 0
        case "hit":
        case "ticks": return hits
        case "mhit": return per(row.multiHits)
        case "mdmg": return row.multiHitDamage || 0
        case "crit": return per(row.crit)
        case "parry": return per(row.parry)
        case "perfect": return per(row.perfect)
        case "double": return per(row.double)
        case "back": return per(row.back)
        case "frontal": return per(row.front)
        case "block": return per(row.shieldBlock)
        case "perfectblock": return per(row.perfectBlock)
        case "ironwall": return per(row.ironWall)
        case "regeneration": return per(row.regeneration)
        case "miss": return row.miss || 0
        case "resist": return row.resist || 0
        case "regen": return row.regen || 0
        case "mindmg": return row.min || 0
        case "avgdmg":
        case "avgheal": return row.avg || 0
        case "maxdmg": return row.max || 0
        }
        return row.amount || 0
    }

    // The amount a tie falls back to: the direct hits', as 1.x.
    function own(row, table) {
        if (table === "taken" || table === "mechanics")
            return row.damage || 0
        return (row.direct || row).amount || 0
    }

    // A first click: names A to Z, numbers biggest first.
    function firstAscending(key) {
        return key === "name"
    }

    // The sort after a click on `key`'s head, from `sort` ({key, ascending}).
    function clicked(sort, key) {
        if (sort && sort.key === key)
            return { "key": key, "ascending": !sort.ascending }
        return { "key": key, "ascending": firstAscending(key) }
    }

    // `rows` sorted by `sort` (null or a key not among `keys`: the
    // backend's order, most first), each skill opened on its damage over
    // time (its name in `open`) followed by that row. Rows are new objects:
    // `table` names the table they are for, `sub` marks a damage over time
    // row, `expandable` and `expanded` its skill.
    function arrange(rows, table, sort, keys, open) {
        const list = rows.slice()
        if (sort && keys.indexOf(sort.key) >= 0) {
            const dir = sort.ascending ? 1 : -1
            list.sort((a, b) => {
                const x = value(a, table, sort.key)
                const y = value(b, table, sort.key)
                if (sort.key === "name")
                    return x.localeCompare(y) * dir || own(b, table) - own(a, table)
                if (x === y)
                    return own(b, table) - own(a, table)
                return (x - y) * dir
            })
        }
        const out = []
        const mark = (row, more) => Object.assign({}, row, { "table": table }, more)
        for (const row of list) {
            if (!row.overTime || !row.direct) {
                out.push(mark(row, {}))
                continue
            }
            const opened = open[row.name] === true
            if (!opened) {
                out.push(mark(row, { "expandable": true, "expanded": false }))
                continue
            }
            out.push(mark(row.direct, { "expandable": true, "expanded": true, "job": row.job }))
            out.push(mark(row.overTime, { "sub": true, "job": row.job }))
        }
        return out
    }
}
