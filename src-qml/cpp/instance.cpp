// SPDX-License-Identifier: AGPL-3.0-only
#include "daevalog-qml/cpp/instance.h"

#include <memory>

#include <QtCore/QCoreApplication>
#include <QtCore/QCryptographicHash>
#include <QtCore/QDir>
#include <QtCore/QLockFile>
#include <QtCore/QPointer>
#include <QtCore/QThread>
#include <QtNetwork/QLocalServer>
#include <QtNetwork/QLocalSocket>

namespace {

std::unique_ptr<QLockFile> lock;
std::unique_ptr<QLocalServer> server;
QPointer<QQmlApplicationEngine> shownBy;
QString socketPath;

const QByteArray showRequest = QByteArrayLiteral("show\n");

void showMeter()
{
    if (!shownBy)
        return;
    const auto roots = shownBy->rootObjects();
    if (!roots.isEmpty())
        QMetaObject::invokeMethod(roots.first(), "showMeter");
}

bool listen()
{
    if (server)
        return true;
    // The lock is ours, so a socket left behind is a crashed meter's.
    QLocalServer::removeServer(socketPath);
    server = std::make_unique<QLocalServer>();
    server->setSocketOptions(QLocalServer::UserAccessOption);
    if (server->listen(socketPath))
        return true;
    qWarning("daevalog-dps-meter: later starts cannot reach this meter: %s", qPrintable(server->errorString()));
    server.reset();
    return false;
}

} // namespace

bool claimInstance(const QString &folder)
{
    QDir().mkpath(folder);
#ifdef Q_OS_WIN
    // On Windows a local server is a named pipe, whose name holds no
    // backslash: one name per folder, from its hash.
    socketPath = QStringLiteral("daevalog-")
        + QString::fromLatin1(QCryptographicHash::hash(QDir::cleanPath(folder).toUtf8(), QCryptographicHash::Sha256).toHex().left(32));
#else
    socketPath = folder + QStringLiteral("/instance.socket");
#endif
    auto mine = std::make_unique<QLockFile>(folder + QStringLiteral("/instance.lock"));
    // Never old enough to take: only a lock whose meter is gone is stale.
    mine->setStaleLockTime(0);
    if (mine->tryLock(0)) {
        lock = std::move(mine);
        return true;
    }
    // A folder the lock cannot be made in should not stop the meter.
    if (mine->error() != QLockFile::LockFailedError)
        return true;
    // The other meter listens a moment after it takes the lock.
    for (int tries = 0; tries < 30; ++tries) {
        QLocalSocket socket;
        socket.connectToServer(socketPath);
        if (socket.waitForConnected(500)) {
            socket.write(showRequest);
            socket.waitForBytesWritten(1000);
            socket.disconnectFromServer();
            return false;
        }
        QThread::msleep(100);
    }
    qWarning("daevalog-dps-meter: another meter holds %s but did not answer", qPrintable(folder));
    return false;
}

void listenInstance(QQmlApplicationEngine &engine)
{
    if (!lock || !listen())
        return;
    shownBy = &engine;
    // The server goes with the application; the lock stays until the
    // settings are saved and the process ends.
    QObject::connect(QCoreApplication::instance(), &QCoreApplication::aboutToQuit, [] { server.reset(); });
    QObject::connect(server.get(), &QLocalServer::newConnection, server.get(), [] {
        while (QLocalSocket *socket = server->nextPendingConnection()) {
            QObject::connect(socket, &QLocalSocket::disconnected, socket, &QObject::deleteLater);
            QObject::connect(socket, &QLocalSocket::readyRead, socket, [socket] {
                if (socket->readAll().contains(showRequest))
                    showMeter();
            });
        }
    });
}

void releaseInstance()
{
    server.reset();
    lock.reset();
    shownBy = nullptr;
}

bool waitForShowRequest(int ms)
{
    if (!lock || !listen() || !server->waitForNewConnection(ms))
        return false;
    std::unique_ptr<QLocalSocket> socket(server->nextPendingConnection());
    QByteArray got;
    while (socket && !got.contains(showRequest) && socket->waitForReadyRead(ms))
        got += socket->readAll();
    return got.contains(showRequest);
}
