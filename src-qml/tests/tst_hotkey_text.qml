// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import QtTest
import ".."

// A recorded key press becomes the label the settings keep, as 1.x writes
// it; a press without a modifier, or of a key no hotkey can use, makes none.
Item {
    HotkeyText {
        id: text
    }

    TestCase {
        name: "HotkeyText"

        function test_a_press_with_modifiers_makes_a_label() {
            compare(text.label(Qt.Key_L, Qt.ControlModifier | Qt.AltModifier), "Ctrl+Alt+L")
            compare(text.label(Qt.Key_F5, Qt.ShiftModifier | Qt.MetaModifier), "Shift+Win+F5")
            compare(text.label(Qt.Key_7, Qt.ControlModifier), "Ctrl+7")
            compare(text.label(Qt.Key_PageUp, Qt.AltModifier | Qt.ControlModifier), "Ctrl+Alt+PageUp")
            compare(text.label(Qt.Key_Space, Qt.ControlModifier), "Ctrl+Space")
        }

        function test_no_modifier_or_an_unknown_key_makes_none() {
            compare(text.label(Qt.Key_L, Qt.NoModifier), "")
            compare(text.label(Qt.Key_Semicolon, Qt.ControlModifier), "")
            compare(text.label(Qt.Key_Control, Qt.ControlModifier), "")
        }

        function test_modifiers_alone_are_not_a_key() {
            verify(text.isModifier(Qt.Key_Control))
            verify(text.isModifier(Qt.Key_Meta))
            verify(!text.isModifier(Qt.Key_L))
            compare(text.modifierText(Qt.ControlModifier | Qt.ShiftModifier).join("+"), "Ctrl+Shift")
        }

        function test_the_page_asks_for_two_modifiers() {
            verify(text.enoughModifiers(Qt.ControlModifier | Qt.AltModifier))
            verify(!text.enoughModifiers(Qt.ControlModifier))
            verify(!text.enoughModifiers(Qt.NoModifier))
        }
    }
}
