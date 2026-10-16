// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import QtTest
import ".."

// Details and History: a number with nothing behind it shows "-", a true 0
// stays 0.
Item {
    NoData {
        id: noData
    }

    // A damage over time row alone: 4 ticks, no direct hit.
    readonly property var dot: ({ "name": "Curse - DOT", "amount": 400, "hits": 4, "crit": 0, "min": 90, "avg": 100, "max": 110 })
    // A skill that only missed: no hit, no tick.
    readonly property var missed: ({ "name": "Bolt", "amount": 0, "hits": 0, "crit": 0, "min": 0, "avg": 0, "max": 0, "miss": 3 })
    // A direct row with its ticks folded in.
    readonly property var folded: ({ "name": "Curse", "amount": 1000, "hits": 0, "crit": 0, "avg": 100,
        "overTime": { "name": "Curse - DOT", "amount": 400, "hits": 4 } })

    TestCase {
        name: "NoData"

        function test_skill_rows() {
            compare(noData.skillCell(missed, "crit", "0%"), "-", "no hit, no crit rate")
            compare(noData.skillCell(missed, "avgdmg", "0"), "-", "no hit or tick, no average")
            compare(noData.skillCell(missed, "miss", "3"), "3", "a count stays")
            compare(noData.skillCell(missed, "hit", "0"), "0", "the hit count of a skill that only missed is a true 0")
            compare(noData.skillCell(dot, "avgdmg", "100"), "100", "ticks have an average")
            compare(noData.skillCell(folded, "maxdmg", "110"), "110", "the folded ticks count")
            compare(noData.skillCell(folded, "crit", "0%"), "-", "no direct hit")
            compare(noData.skillCell(missed, "mdmg", "0"), "-", "no hit, no multi-hit damage")
            compare(noData.skillCell({ "hits": 5 }, "mdmg", "0"), "0", "5 hits, none multi: a true 0")
            compare(noData.skillCell({ "hits": 5 }, "crit", "0%"), "0%", "5 hits, no crit: a true 0")
        }

        function test_damage_overview() {
            const ticks = { "hits": 0, "damage": 5000, "multiHitDamage": 0 }
            for (const key of ["hits", "critRate", "backRate", "multiHitHits", "multiHitDamage"])
                compare(noData.damageStat(key, ticks, true, "0"), "-", key)
            compare(noData.damageStat("totalDamage", ticks, true, "5,000"), "5,000")
            compare(noData.damageStat("regen", ticks, true, "0"), "0", "regen has records behind it")
            compare(noData.damageStat("totalDamage", { "hits": 0, "damage": 0 }, false, "0"), "-", "no damage record")
            compare(noData.damageStat("combatTime", { "hits": 0 }, false, "01:00"), "01:00")
            const hit = { "hits": 3, "damage": 900, "multiHitDamage": 0 }
            compare(noData.damageStat("critRate", hit, true, "0.0%"), "0.0%")
            compare(noData.damageStat("multiHitDamage", hit, true, "0"), "0")
        }

        function test_heal_and_taken_overview() {
            compare(noData.healStat("totalHealing", { "skills": 0 }, "0"), "-")
            compare(noData.healStat("combatTime", { "skills": 0 }, "01:00"), "01:00")
            compare(noData.healStat("hotTicks", { "skills": 2 }, "0"), "0", "heals without a HoT: a true 0")
            compare(noData.takenStat({ "damage": 0, "attacks": 0 }, "0"), "-")
            compare(noData.takenStat(null, "0"), "-")
            compare(noData.takenStat({ "damage": 0, "attacks": 4 }, "0"), "0", "4 attacks fully blocked")
        }

        function test_history_rows() {
            compare(noData.byHits(0, "0"), "-")
            compare(noData.byHits(12, "12"), "12")
        }

        function test_history_tank_rows() {
            compare(noData.tankRow({ "amount": 0, "attacks": 0 }, "0"), "-", "a death with no attack received")
            compare(noData.tankRow({ "amount": 0, "attacks": 6 }, "0"), "0", "6 attacks, every one blocked")
            compare(noData.tankRow({ "amount": 900, "attacks": 0 }, "900"), "900", "damage over time ticks only")
            compare(noData.tankRow({ "amount": 0 }, "0"), "0", "a DPS or Heal row has no count")
        }
    }
}
