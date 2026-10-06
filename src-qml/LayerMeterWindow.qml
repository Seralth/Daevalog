// SPDX-License-Identifier: MIT
import QtQuick
import org.kde.layershell as LayerShell
import Daevalog

// The meter as a Wayland layer-shell surface: above other windows, even a
// full-screen game, placed by margins from the screen's top-left corner.
// Loaded only with --layer-shell; Main.qml falls back to MeterWindow when
// org.kde.layershell is missing.
MeterWindow {
    layerSurface: Qt.platform.pluginName.startsWith("wayland")

    LayerShell.Window.scope: "daevalog-meter"
    LayerShell.Window.layer: LayerShell.Window.LayerOverlay
    LayerShell.Window.anchors: LayerShell.Window.AnchorTop | LayerShell.Window.AnchorLeft
    LayerShell.Window.margins: Overlay.layerMargins
    LayerShell.Window.keyboardInteractivity: LayerShell.Window.KeyboardInteractivityNone
}
