// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import QtTest
import ".."

// Auto-hide waits 20 ticks, then hides after three ticks with neither the
// game nor the meter in front, and shows again on the next tick with one.
Item {
    id: root
    property bool front: true

    AutoHide {
        id: hide
        tick: 5
        gameInFront: () => root.front
    }

    TestCase {
        name: "AutoHide"

        function init() {
            root.front = true
            hide.meterActive = false
            hide.enabled = false
            hide.ticks = 0
            hide.away = 0
            hide.hidden = false
        }

        function steps(n) {
            for (let i = 0; i < n; i++)
                hide.step()
        }

        function test_the_first_twenty_ticks_never_hide() {
            root.front = false
            steps(22)
            compare(hide.hidden, false)
            steps(1)
            compare(hide.hidden, true, "three ticks away after the first twenty")
        }

        function test_the_game_in_front_shows_the_meter_again() {
            root.front = false
            steps(23)
            compare(hide.hidden, true)
            root.front = true
            steps(1)
            compare(hide.hidden, false)
        }

        function test_the_meter_in_front_keeps_it() {
            root.front = false
            hide.meterActive = true
            steps(30)
            compare(hide.hidden, false)
        }

        function test_a_game_always_in_front_never_hides() {
            // The backend on Linux cannot tell, and says the game is in front.
            steps(100)
            compare(hide.hidden, false)
        }

        function test_turned_off_it_shows_the_meter() {
            hide.enabled = true
            root.front = false
            tryCompare(hide, "hidden", true, 2000)
            hide.enabled = false
            compare(hide.hidden, false)
            compare(hide.ticks, 0, "turned on again, it waits its twenty ticks again")
        }
    }
}
