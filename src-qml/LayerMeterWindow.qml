// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import org.kde.layershell as LayerShell
import Daevalog

// The meter as a Wayland layer-shell surface: above other windows, even a
// full-screen game, placed by margins from the screen's top-left corner.
// Loaded only with --layer-shell; Main.qml falls back to MeterWindow when
// org.kde.layershell is missing.
MeterWindow {
    layerSurface: Qt.platform.pluginName.startsWith("wayland")

    // hyprland.rs finds the window by this name.
    LayerShell.Window.scope: "daevalog-meter"
    LayerShell.Window.layer: LayerShell.Window.LayerOverlay
    LayerShell.Window.anchors: LayerShell.Window.AnchorTop | LayerShell.Window.AnchorLeft
    LayerShell.Window.margins: Overlay.layerMargins
    // No keys, so the game keeps them. On Sway a menu of the meter gets its
    // keys through the meter's surface, so it asks for them while one is open.
    LayerShell.Window.keyboardInteractivity: menuOpen && Overlay.layerMenuKeys
        ? LayerShell.Window.KeyboardInteractivityExclusive : LayerShell.Window.KeyboardInteractivityNone
    // The change goes out with the next frame.
    onMenuOpenChanged: update()

    Component.onCompleted: Overlay.keepLayerOnScreen(Qt.size(width, height), placeArea)
    onPlaceAreaChanged: Overlay.keepLayerOnScreen(Qt.size(width, height), placeArea)
    // Wider from more columns: still on the screen.
    onWidthChanged: Overlay.keepLayerOnScreen(Qt.size(width, height), placeArea)
}
