// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import QtTest
import ".."

// The Details skill table: a column head sorts, a second click turns the
// order round; a skill opened on its damage over time shows its direct hits
// with the damage over time on a row under it.
Item {
    SkillOrder {
        id: order
    }

    // Curse: 600 direct in 3 hits (1 crit) plus 400 over time in 4 ticks.
    readonly property var curse: ({
        "code": 1, "name": "Curse", "amount": 1000, "share": 50, "hits": 3, "crit": 1, "min": 100, "avg": 143, "max": 300,
        "overTime": { "code": 1, "name": "Curse - DOT", "amount": 400, "share": 20, "hits": 4, "crit": 0, "avg": 100 },
        "direct": { "code": 1, "name": "Curse", "amount": 600, "share": 30, "hits": 3, "crit": 1, "avg": 200 }
    })
    readonly property var fire: ({ "code": 2, "name": "fire", "amount": 700, "share": 35, "hits": 7, "crit": 7, "avg": 100 })
    readonly property var bolt: ({ "code": 3, "name": "Bolt", "amount": 300, "share": 15, "hits": 1, "crit": 0, "avg": 300 })
    readonly property var keys: ["name", "dmg", "hit", "crit", "avgdmg"]

    function names(rows) {
        return rows.map(r => r.name).join(",")
    }

    TestCase {
        name: "SkillOrder"

        function test_a_head_sorts_and_a_second_click_turns_it_round() {
            const rows = [curse, fire, bolt]
            let sort = order.clicked(null, "hit")
            compare(sort.ascending, false, "numbers: biggest first")
            compare(names(order.arrange(rows, "dmg", sort, keys, {})), "fire,Curse,Bolt")
            sort = order.clicked(sort, "hit")
            compare(names(order.arrange(rows, "dmg", sort, keys, {})), "Bolt,Curse,fire")
            sort = order.clicked(sort, "name")
            compare(sort.ascending, true, "names: A to Z first")
            compare(names(order.arrange(rows, "dmg", sort, keys, {})), "Bolt,Curse,fire", "case left aside")
        }

        function test_rates_sort_by_their_counts() {
            const sort = { "key": "crit", "ascending": false }
            compare(names(order.arrange([curse, fire, bolt], "dmg", sort, keys, {})), "fire,Curse,Bolt")
        }

        function test_damage_sorts_with_the_damage_over_time() {
            const sort = { "key": "dmg", "ascending": false }
            compare(names(order.arrange([bolt, fire, curse], "dmg", sort, keys, {})), "Curse,fire,Bolt")
        }

        function test_a_column_not_shown_keeps_the_backends_order() {
            const sort = { "key": "maxdmg", "ascending": true }
            compare(names(order.arrange([curse, fire, bolt], "dmg", sort, keys, {})), "Curse,fire,Bolt")
        }

        function test_an_opened_skill_shows_its_damage_over_time_under_it() {
            const sort = { "key": "dmg", "ascending": false }
            const closed = order.arrange([curse, fire], "dmg", sort, keys, {})
            compare(names(closed), "Curse,fire")
            compare(closed[0].expandable, true)
            compare(closed[0].expanded, false)
            compare(closed[0].amount, 1000, "closed: with its damage over time")
            compare(closed[1].expandable, undefined, "nothing to open")

            const opened = order.arrange([curse, fire], "dmg", sort, keys, { "Curse": true })
            compare(names(opened), "Curse,Curse - DOT,fire")
            compare(opened[0].expanded, true)
            compare(opened[0].amount, 600, "opened: the direct hits alone")
            compare(opened[0].avg, 200)
            compare(opened[1].sub, true)
            compare(opened[1].amount, 400)
        }

        function test_the_damage_over_time_row_stays_under_its_skill_in_any_order() {
            const sort = { "key": "hit", "ascending": true }
            compare(names(order.arrange([fire, curse, bolt], "dmg", sort, keys, { "Curse": true })), "Bolt,Curse,Curse - DOT,fire")
        }

        function test_taken_rows_sort_by_their_own_numbers() {
            const rows = [{ "name": "a", "damage": 10, "attacks": 5, "rates": { "parry": 20 } },
                          { "name": "b", "damage": 30, "attacks": 1, "rates": {} }]
            compare(names(order.arrange(rows, "taken", { "key": "hits", "ascending": false }, ["hits", "parry"], {})), "a,b")
            compare(names(order.arrange(rows, "taken", { "key": "parry", "ascending": true }, ["hits", "parry"], {})), "b,a",
                "no share sorts below 0%")
        }

        function test_each_row_says_which_table_it_is_for() {
            const opened = order.arrange([curse, fire], "dmg", null, keys, { "Curse": true })
            compare(opened.map(r => r.table).join(","), "dmg,dmg,dmg", "the damage over time row too")
            const mechanic = { "name": "Slam", "damage": 800, "attacks": 3, "players": 2, "hit": [{ "id": 101, "attacks": 2, "damage": 500 }] }
            const rows = order.arrange([mechanic], "mechanics", null, ["taken"], {})
            compare(rows[0].table, "mechanics")
            compare(rows[0].hit.length, 1)
            compare(mechanic.table, undefined, "the view's row stays as it was")
        }
    }
}
