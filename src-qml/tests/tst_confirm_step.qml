// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import QtTest
import ".."

// Deleting a saved fight asks once more: one press arms, the second does
// it; waiting or another fight disarms.
Item {
    ConfirmStep {
        id: step
        wait: 300
    }
    SignalSpy {
        id: done
        target: step
        signalName: "confirmed"
    }

    TestCase {
        name: "ConfirmStep"

        function init() {
            step.cancel()
            done.clear()
        }

        function test_the_second_press_does_it() {
            step.press()
            compare(step.armed, true)
            compare(done.count, 0)
            step.press()
            compare(done.count, 1)
            compare(step.armed, false)
        }

        function test_waiting_disarms() {
            step.press()
            tryCompare(step, "armed", false, 1000)
            step.press()
            compare(done.count, 0, "the press after the wait only arms again")
        }

        function test_cancel_disarms() {
            step.press()
            step.cancel()
            step.press()
            compare(done.count, 0)
        }
    }
}
