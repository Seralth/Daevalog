// SPDX-License-Identifier: MIT
#include "daevalog-qml/cpp/shell.h"

#include <string>
#include <vector>

#include <QtGui/QIcon>
#include <QtGui/QImage>

std::unique_ptr<QApplication> newApplication(rust::Slice<const rust::String> args)
{
    // QApplication keeps argc and argv for its whole life.
    static std::vector<std::string> storage;
    static std::vector<char *> argv;
    static int argc = 0;
    for (const auto &arg : args)
        storage.emplace_back(std::string(arg));
    for (auto &arg : storage)
        argv.push_back(arg.data());
    argv.push_back(nullptr);
    argc = static_cast<int>(storage.size());

    QApplication::setApplicationName(QStringLiteral("Daevalog"));
    QApplication::setDesktopFileName(QStringLiteral("daevalog-dps-meter"));
    auto app = std::make_unique<QApplication>(argc, argv.data());
    QApplication::setWindowIcon(QIcon(QStringLiteral(":/daevalog/icons/app.png")));
    return app;
}

int execApplication(QApplication &app)
{
    return app.exec();
}

void exitApplication(int code)
{
    QCoreApplication::exit(code);
}

bool grabWindowToFile(QQuickWindow *window, const QString &path)
{
    if (!window)
        return false;
    const QImage image = window->grabWindow();
    return !image.isNull() && image.save(path, "PNG");
}
