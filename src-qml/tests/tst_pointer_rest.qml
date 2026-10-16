// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import QtTest
import ".."

// The tooltip wait: a pointer that keeps moving never rests, leaving ends a
// rest at once, and the next item waits its own full time.
Item {
    id: root
    width: 300
    height: 100

    PointerRest {
        id: first
        width: 100
        height: 100
        delay: 500
    }
    PointerRest {
        id: second
        x: 100
        width: 100
        height: 100
        delay: 500
    }

    TestCase {
        name: "PointerRest"
        when: windowShown

        // The pointer starts on neither item.
        function init() {
            mouseMove(root, 250, 50)
            wait(20)
            verify(!first.rested && !second.rested)
        }

        function test_a_moving_pointer_never_rests() {
            for (let i = 0; i < 15; i++) {
                mouseMove(root, 20 + 4 * i, 50)
                wait(100)
                verify(!first.rested, "rested while moving, after move " + i)
            }
            tryVerify(() => first.rested, 2000, "no rest once the pointer stopped")
        }

        function test_leaving_ends_the_rest_at_once() {
            mouseMove(root, 50, 50)
            tryVerify(() => first.rested, 2000)
            mouseMove(root, 250, 50)
            verify(!first.rested)
        }

        function test_moves_after_a_rest_keep_it() {
            mouseMove(root, 50, 50)
            tryVerify(() => first.rested, 2000)
            mouseMove(root, 60, 40)
            verify(first.rested)
        }

        function test_the_next_item_waits_its_own_time() {
            mouseMove(root, 50, 50)
            tryVerify(() => first.rested, 2000)
            mouseMove(root, 150, 50)
            verify(!first.rested)
            wait(250)
            verify(!second.rested, "rested before its own wait")
            tryVerify(() => second.rested, 2000)
        }

        function test_an_inactive_item_never_rests() {
            first.active = false
            mouseMove(root, 50, 50)
            wait(800)
            verify(!first.rested)
            first.active = true
        }
    }
}
