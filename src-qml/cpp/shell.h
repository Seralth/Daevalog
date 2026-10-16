// SPDX-License-Identifier: AGPL-3.0-only
#pragma once

#include <cstdint>
#include <memory>

#include <QtCore/QPointF>
#include <QtCore/QRectF>
#include <QtCore/QString>
#include <QtGui/QColor>
#include <QtQuick/QQuickWindow>
#include <QtWidgets/QApplication>

#include "rust/cxx.h"

// A QApplication, not a QGuiApplication: the tray icon needs Qt Widgets on
// desktops whose platform theme has no tray of its own.
std::unique_ptr<QApplication> newApplication(rust::Slice<const rust::String> args);
int execApplication(QApplication &app);
void exitApplication(int code);

// The system's languages for the UI, most wanted first ("de-DE", "de").
rust::Vec<rust::String> systemLanguages();

// Renders the window as it is now and saves it as a PNG.
bool grabWindowToFile(QQuickWindow *window, const QString &path);

// Renders the window, with `beside` on its left when given, on `background`
// (the see-through meter reads as on a dark scene), puts the picture on the
// clipboard when `clipboard`, and returns it as PNG bytes; none when the
// window gave no picture.
rust::Vec<std::uint8_t> captureWindows(QQuickWindow *window, QQuickWindow *beside, bool clipboard, const QColor &background);

// Puts `text` on the clipboard.
void copyText(const QString &text);
// "Qt 6.11.2, wayland": the Qt the meter runs on and its platform, for a
// bug report.
QString qtLine();

// Starts noting the window's pointer event times and frame times, which a
// layer-shell drag needs. Once per window.
void watchWindow(QQuickWindow *window);
// CLOCK_MONOTONIC in microseconds. KWin stamps input events with the same
// clock, in milliseconds.
std::uint64_t monotonicMicros();
// The compositor's time stamp (ms) of the mouse event being delivered now,
// and whether a button was down in it.
std::uint64_t pointerEventTime();
bool pointerButtonDown();
// Where that event was on the screen. Only X11 knows; Wayland gives window
// coordinates alone.
QPointF pointerGlobalPosition();
// Ends the press that started a system resize, once that event is done.
// The compositor keeps the button's release, and the release Qt makes up for
// it reaches no window on KDE Wayland, so the grip would keep the pointer
// and take the next click.
void releasePointer(QQuickWindow *window);
// When the window handed its frames to the compositor (monotonicMicros),
// oldest first, since the last call; at most the last 64.
rust::Vec<std::uint64_t> takeFrameTimes();
// Asks for a frame. A frame carries new layer-shell margins and a new input
// region to the compositor; neither goes out on its own.
void requestFrame(QQuickWindow *window);
// Only `area` (window coordinates) takes mouse input; an empty area gives
// the whole window back. On Wayland this sets the surface's input region,
// on X11 the window's input shape.
void setInputArea(QQuickWindow *window, const QRectF &area);
// Whether setInputArea can leave part of the window taking clicks: on
// Wayland and X11.
bool canShapeInput();
// Keeps an X11 window out of the taskbar and the window list: each time
// before it maps, it gets _NET_WM_STATE_SKIP_TASKBAR, which Qt keeps (it
// adds its own states to those it finds). Install before the first show.
void keepOffTaskbar(QQuickWindow *window);
// Keeps an X11 window out of the taskbar while `off`, or lets it back: as
// keepOffTaskbar for each show, and a window on screen asks the window
// manager at once. Wayland lets no app ask, so there it does nothing.
void setOffTaskbar(QQuickWindow *window, bool off);
// Qt's platform plugin: "wayland", "xcb", "offscreen" ...
QString platformName();
