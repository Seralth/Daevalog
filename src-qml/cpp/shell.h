// SPDX-License-Identifier: MIT
#pragma once

#include <memory>

#include <QtCore/QString>
#include <QtQuick/QQuickWindow>
#include <QtWidgets/QApplication>

#include "rust/cxx.h"

// A QApplication, not a QGuiApplication: the tray icon needs Qt Widgets on
// desktops whose platform theme has no tray of its own.
std::unique_ptr<QApplication> newApplication(rust::Slice<const rust::String> args);
int execApplication(QApplication &app);
void exitApplication(int code);

// Renders the window as it is now and saves it as a PNG.
bool grabWindowToFile(QQuickWindow *window, const QString &path);
