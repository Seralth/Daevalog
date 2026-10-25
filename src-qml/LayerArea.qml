// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import org.kde.layershell as LayerShell

// The part of the screen a layer surface's margins count in: the screen
// less the bars and panels that keep room for themselves. Two layer
// surfaces one pixel thick, which the compositor stretches across that
// area, one down and one across; see-through, on the background layer,
// taking no clicks. They stay mapped: COSMIC closes the connection when a
// layer surface goes.
QtObject {
    id: area

    // Empty until the compositor has sized both.
    readonly property size size: down.height > 1 && across.width > 1
        ? Qt.size(across.width, down.height) : Qt.size(0, 0)

    component Probe: Window {
        id: probe
        width: 1
        height: 1
        visible: true
        // Shown again after the compositor closed it (Reopen.qml).
        readonly property Reopen reopen: Reopen {
            window: probe
        }
        color: "transparent"
        flags: Qt.FramelessWindowHint | Qt.WindowTransparentForInput
        LayerShell.Window.scope: "daevalog-area"
        LayerShell.Window.layer: LayerShell.Window.LayerBackground
        LayerShell.Window.keyboardInteractivity: LayerShell.Window.KeyboardInteractivityNone
    }

    readonly property Window down: Probe {
        LayerShell.Window.anchors: LayerShell.Window.AnchorTop | LayerShell.Window.AnchorBottom
            | LayerShell.Window.AnchorLeft
    }
    readonly property Window across: Probe {
        LayerShell.Window.anchors: LayerShell.Window.AnchorLeft | LayerShell.Window.AnchorRight
            | LayerShell.Window.AnchorTop
    }
}
