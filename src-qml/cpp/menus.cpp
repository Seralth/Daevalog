// SPDX-License-Identifier: AGPL-3.0-only
#include "daevalog-qml/cpp/menus.h"

#include <QtCore/QPointer>
#include <QtCore/QVariant>
#include <QtGui/QGuiApplication>
#include <QtGui/QKeyEvent>
#include <QtGui/QMouseEvent>
#include <QtGui/QScreen>

namespace {

// xdg_positioner's constraint adjustments.
constexpr unsigned slideX = 1;
constexpr unsigned slideY = 2;
constexpr unsigned flipY = 8;

bool onX11()
{
    return QGuiApplication::platformName() == QStringLiteral("xcb");
}

// Watches the whole app while a menu is open. One menu is open at a time.
class MenuGrab : public QObject
{
public:
    QPointer<QQuickWindow> menu;
    QMetaObject::Connection hidden;

    void release()
    {
        if (menu && onX11()) {
            menu->setKeyboardGrabEnabled(false);
            menu->setMouseGrabEnabled(false);
        }
        QObject::disconnect(hidden);
        menu = nullptr;
        qApp->removeEventFilter(this);
    }

protected:
    bool eventFilter(QObject *watched, QEvent *event) override
    {
        // Only what reaches a window: Qt Quick passes the same events on to
        // items afterwards.
        auto *window = qobject_cast<QWindow *>(watched);
        if (!window || !menu || !menu->isVisible())
            return false;
        switch (event->type()) {
        case QEvent::MouseButtonPress:
        case QEvent::MouseButtonDblClick: {
            const auto *mouse = static_cast<QMouseEvent *>(event);
            if (window == menu && QRectF(QPointF(0, 0), menu->size()).contains(mouse->position()))
                return false;
            // Outside: the press only closes the menu, as a native menu's
            // does. A press on the button that opened it must not open it
            // again.
            QPointer<QQuickWindow> closing = menu;
            release();
            closing->setVisible(false);
            return true;
        }
        case QEvent::KeyPress: {
            const auto *key = static_cast<QKeyEvent *>(event);
            QMetaObject::invokeMethod(menu, "takeKey", Q_ARG(QVariant, QVariant(key->key())));
            return true;
        }
        case QEvent::KeyRelease:
        case QEvent::ShortcutOverride:
            return true;
        default:
            return false;
        }
    }
};

MenuGrab &menuGrab()
{
    static MenuGrab *grab = new MenuGrab;
    return *grab;
}

} // namespace

QRectF mapRectToGlobal(QQuickWindow *window, const QRectF &local)
{
    if (!window)
        return local;
    return QRectF(window->mapToGlobal(local.topLeft()), local.size());
}

QRectF screenAreaAt(QQuickWindow *window, const QPointF &point)
{
    QScreen *screen = QGuiApplication::screenAt(point.toPoint());
    if (!screen && window)
        screen = window->screen();
    if (!screen)
        screen = QGuiApplication::primaryScreen();
    return screen ? QRectF(screen->availableGeometry()) : QRectF();
}

void setPopupAnchor(QQuickWindow *menu, const QRectF &anchor, bool alignEnd)
{
    if (!menu)
        return;
    menu->setProperty("_q_waylandPopupAnchorRect", anchor.toAlignedRect());
    menu->setProperty("_q_waylandPopupAnchor",
                      QVariant::fromValue(Qt::Edges(Qt::BottomEdge | (alignEnd ? Qt::RightEdge : Qt::LeftEdge))));
    menu->setProperty("_q_waylandPopupGravity",
                      QVariant::fromValue(Qt::Edges(Qt::BottomEdge | (alignEnd ? Qt::LeftEdge : Qt::RightEdge))));
    menu->setProperty("_q_waylandPopupConstraintAdjustment", slideX | slideY | flipY);
}

void grabMenu(QQuickWindow *menu)
{
    if (!menu)
        return;
    MenuGrab &grab = menuGrab();
    if (grab.menu && grab.menu != menu) {
        QPointer<QQuickWindow> other = grab.menu;
        grab.release();
        other->setVisible(false);
    }
    if (grab.menu == menu)
        return;
    grab.menu = menu;
    qApp->installEventFilter(&grab);
    // Hidden by any means: Escape, a pick, the compositor.
    grab.hidden = QObject::connect(menu, &QWindow::visibleChanged, &grab, [](bool visible) {
        if (!visible)
            menuGrab().release();
    });
    if (onX11()) {
        menu->setMouseGrabEnabled(true);
        menu->setKeyboardGrabEnabled(true);
    }
}
