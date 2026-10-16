// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick

// A number with nothing behind it shows "-", never 0:
// a rate or count of hits when the game sent no hit (world bosses send only
// other players' damage over time ticks), a lowest, average or highest with
// no hit and no tick, a tab's numbers with no record of its kind. A true 0
// stays 0.
QtObject {
    // Skill table columns over the row's direct hits: their shares and the
    // multi-hit damage.
    readonly property var perHit: ["mhit", "mdmg", "crit", "parry", "perfect", "double", "back", "frontal", "block",
        "perfectblock", "ironwall", "regeneration"]
    // Skill table columns over the hits and the ticks.
    readonly property var perHitOrTick: ["mindmg", "avgdmg", "maxdmg", "avgheal"]
    // The DMG overview's numbers that count or share the direct hits.
    readonly property var damageByHit: ["hits", "critRate", "perfectRate", "doubleRate", "backRate", "parryRate", "multiHitHits"]
    // Overview numbers that do not come from the tab's records.
    readonly property var kept: ["combatTime", "maxHp", "deaths"]

    function hitsAndTicks(row) {
        return (row.hits || 0) + (row.overTime ? row.overTime.hits || 0 : 0)
    }
    // A count or share of hits: "-" with no hit.
    function byHits(hits, text) {
        return hits > 0 ? text : "-"
    }
    // A Details or History skill row's `text` in column `key`.
    function skillCell(row, key, text) {
        if (perHit.indexOf(key) >= 0)
            return byHits(row.hits, text)
        if (perHitOrTick.indexOf(key) >= 0)
            return hitsAndTicks(row) > 0 ? text : "-"
        return text
    }
    // The DMG overview: `records`, the tab has a skill row.
    function damageStat(key, stats, records, text) {
        if (kept.indexOf(key) >= 0)
            return text
        if (!records)
            return "-"
        if (damageByHit.indexOf(key) >= 0 || (key === "multiHitDamage" && !(stats.multiHitDamage > 0)))
            return byHits(stats.hits, text)
        return text
    }
    // The HEAL overview: "-" with no healing skill entry.
    function healStat(key, stats, text) {
        return kept.indexOf(key) >= 0 || stats.skills > 0 ? text : "-"
    }
    // The TAKEN overview: "-" with no attack received.
    function takenStat(received, text) {
        return received && (received.attacks > 0 || received.damage > 0) ? text : "-"
    }
    // A History row's damage received numbers: "-" with no attack received.
    // A row without the count (DPS, Heal) keeps its number.
    function tankRow(row, text) {
        return row.attacks === undefined ? text : takenStat({ "attacks": row.attacks, "damage": row.amount }, text)
    }
}
