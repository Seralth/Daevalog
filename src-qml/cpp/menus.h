// SPDX-License-Identifier: AGPL-3.0-only
#pragma once

#include <QtCore/QPointF>
#include <QtCore/QRectF>
#include <QtQuick/QQuickWindow>

// The app's own menus (UiMenu.qml) are popup windows of their own, so they
// can reach past the small meter window.

// `local` (in the window's coordinates) on the screen.
QRectF mapRectToGlobal(QQuickWindow *window, const QRectF &local);
// The part of the screen at `point` that windows may use: the screen less
// its panels, as far as the desktop tells.
QRectF screenAreaAt(QQuickWindow *window, const QPointF &point);
// On Wayland the compositor places a popup: below `anchor` (the control
// that opened the menu, in its window's coordinates), flipped above it when
// it does not fit below, slid sideways to stay on the screen. Its left edge
// meets the anchor's left edge, or its right edge the anchor's right edge
// when `alignEnd`. Before Qt 6.6 Qt ignores this and asks for the menu's
// top-left corner at the position the window has.
void setPopupAnchor(QQuickWindow *menu, const QRectF &anchor, bool alignEnd);
// While `menu` is shown, every key press of the app goes to its QML
// function `takeKey(key)`, and a press outside it closes it without
// reaching anything else. On X11 the menu also takes the pointer and the
// keyboard, as a native menu does, so a press on another app closes it too.
// On Wayland the compositor closes it then.
void grabMenu(QQuickWindow *menu);
