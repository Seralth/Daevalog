// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick

// A key press as a hotkey label the settings keep ("Ctrl+Alt+L"), as 1.x's
// keybind buttons write it: Ctrl, Alt, Shift, Win, then the key. Only keys
// the backend reads (parse_hotkey_label) are taken.
QtObject {
    readonly property var names: {
        const map = {}
        map[Qt.Key_Space] = "Space"
        map[Qt.Key_Tab] = "Tab"
        map[Qt.Key_Return] = "Enter"
        map[Qt.Key_Enter] = "Enter"
        map[Qt.Key_Backspace] = "Backspace"
        map[Qt.Key_PageUp] = "PageUp"
        map[Qt.Key_PageDown] = "PageDown"
        map[Qt.Key_End] = "End"
        map[Qt.Key_Home] = "Home"
        map[Qt.Key_Left] = "Left"
        map[Qt.Key_Up] = "Up"
        map[Qt.Key_Right] = "Right"
        map[Qt.Key_Down] = "Down"
        map[Qt.Key_Insert] = "Insert"
        map[Qt.Key_Delete] = "Delete"
        return map
    }

    function isModifier(key) {
        return key === Qt.Key_Control || key === Qt.Key_Alt || key === Qt.Key_Shift || key === Qt.Key_Meta
            || key === Qt.Key_Super_L || key === Qt.Key_Super_R || key === Qt.Key_AltGr
    }

    // The modifiers held, in the label's order.
    function modifierText(modifiers) {
        const parts = []
        if (modifiers & Qt.ControlModifier)
            parts.push("Ctrl")
        if (modifiers & Qt.AltModifier)
            parts.push("Alt")
        if (modifiers & Qt.ShiftModifier)
            parts.push("Shift")
        if (modifiers & Qt.MetaModifier)
            parts.push("Win")
        return parts
    }

    // The page asks for two modifiers or more ("Need 2+ modifiers" in
    // every language), so a hotkey stays clear of the game's own keys.
    function enoughModifiers(modifiers) {
        return modifierText(modifiers).length >= 2
    }

    // The key's name, or "" for a key no hotkey can use.
    function keyName(key) {
        if (key >= Qt.Key_A && key <= Qt.Key_Z)
            return String.fromCharCode(key)
        if (key >= Qt.Key_0 && key <= Qt.Key_9)
            return String.fromCharCode(key)
        if (key >= Qt.Key_F1 && key <= Qt.Key_F12)
            return "F" + (key - Qt.Key_F1 + 1)
        return names[key] || ""
    }

    // The label for a press, or "" when it makes no hotkey: a key no
    // hotkey can use, or no modifier held.
    function label(key, modifiers) {
        const mods = modifierText(modifiers)
        const name = keyName(key)
        if (mods.length === 0 || name === "")
            return ""
        return mods.concat([name]).join("+")
    }
}
