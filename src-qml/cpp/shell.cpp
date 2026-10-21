// SPDX-License-Identifier: AGPL-3.0-only
#include "daevalog-qml/cpp/shell.h"
// Turns QML debugging on at start when the build defines QT_QML_DEBUG
// (build.rs, DAEVALOG_QML_DEBUG); adds nothing otherwise.
#include <QtQml/qqmldebug.h>

#include <algorithm>
#include <chrono>
#include <cstdlib>
#include <cstring>
#include <ctime>
#include <mutex>
#include <string>
#include <vector>

#include <QtCore/QBuffer>
#include <QtCore/QDir>
#include <QtCore/QFile>
#include <QtCore/QFileInfo>
#include <QtCore/QLibrary>
#include <QtCore/QLocale>
#include <QtCore/QPointer>
#include <QtCore/QSaveFile>
#include <QtCore/QStandardPaths>
#include <QtCore/QVersionNumber>
#include <QtGui/QClipboard>
#include <QtGui/QGuiApplication>
#include <QtGui/QIcon>
#include <QtGui/QImage>
#include <QtGui/QMouseEvent>
#include <QtGui/QPainter>
#include <QtGui/QRegion>
#include <QtGui/qguiapplication_platform.h>

namespace {

// One meter window, so plain globals. The pointer values are written and
// read on the Qt thread, while the event they come from is delivered.
std::uint64_t pointerTime = 0;
bool buttonDown = false;
QPointF pointerGlobal;
QPointF pointerLocal;
const QPointingDevice *pointerDevice = nullptr;
// Written on the render thread, taken on the Qt thread.
std::mutex frameLock;
std::vector<std::uint64_t> frameTimes;

// Sees each mouse event before Qt Quick delivers it.
class PointerTimes : public QObject
{
public:
    using QObject::QObject;

protected:
    bool eventFilter(QObject *watched, QEvent *event) override
    {
        switch (event->type()) {
        case QEvent::MouseButtonPress:
        case QEvent::MouseButtonRelease:
        case QEvent::MouseMove: {
            const auto *mouse = static_cast<QMouseEvent *>(event);
            pointerTime = mouse->timestamp();
            buttonDown = mouse->buttons() != Qt::NoButton;
            pointerGlobal = mouse->globalPosition();
            pointerLocal = mouse->position();
            pointerDevice = mouse->pointingDevice();
            break;
        }
        default:
            break;
        }
        return QObject::eventFilter(watched, event);
    }
};

// Before Qt 6.8 (seen on 6.4.2; 6.8.2 is fine), Pretendard's variable font
// loaded from the resources gives Qt only its default instance, so Medium
// and DemiBold draw as Regular. fontconfig lists every weight of a font
// file, and Qt reads fontconfig's fonts at its first font lookup. So on
// those versions a copy of the font in the cache goes to fontconfig first.
void addFontWeights()
{
    if (QVersionNumber::fromString(QString::fromLatin1(qVersion())) >= QVersionNumber(6, 8))
        return;
    using AddFile = int (*)(void *config, const unsigned char *file);
    const auto addFile = reinterpret_cast<AddFile>(
        QLibrary::resolve(QStringLiteral("fontconfig"), 1, "FcConfigAppFontAddFile"));
    if (!addFile)
        return;
    QFile font(QStringLiteral(":/daevalog/fonts/PretendardVariable.ttf"));
    const QString dir = QStandardPaths::writableLocation(QStandardPaths::GenericCacheLocation)
        + QStringLiteral("/" DAEVALOG_APP_ID);
    const QString path = dir + QStringLiteral("/PretendardVariable.ttf");
    if (QFileInfo(path).size() != font.size()) {
        QSaveFile copy(path);
        if (!QDir().mkpath(dir) || !font.open(QIODevice::ReadOnly) || !copy.open(QIODevice::WriteOnly))
            return;
        copy.write(font.readAll());
        if (!copy.commit())
            return;
    }
    addFile(nullptr, reinterpret_cast<const unsigned char *>(QFile::encodeName(path).constData()));
}

} // namespace

std::unique_ptr<QApplication> newApplication(rust::Slice<const rust::String> args)
{
    addFontWeights();
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
    // The .desktop file's name (build.rs).
    QApplication::setDesktopFileName(QStringLiteral(DAEVALOG_APP_ID));
    auto app = std::make_unique<QApplication>(argc, argv.data());
#ifdef DAEVALOG_DEV
    // Red, so the developer build is not taken for the release one.
    QApplication::setWindowIcon(QIcon(QStringLiteral(":/daevalog/icons/app-dev.png")));
#else
    QApplication::setWindowIcon(QIcon(QStringLiteral(":/daevalog/icons/app.png")));
#endif
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

rust::Vec<rust::String> systemLanguages()
{
    rust::Vec<rust::String> tags;
    for (const QString &tag : QLocale::system().uiLanguages())
        tags.push_back(rust::String(tag.toStdString()));
    return tags;
}

bool grabWindowToFile(QQuickWindow *window, const QString &path)
{
    if (!window)
        return false;
    const QImage image = window->grabWindow();
    return !image.isNull() && image.save(path, "PNG");
}

rust::Vec<std::uint8_t> captureWindows(QQuickWindow *window, QQuickWindow *beside, bool clipboard, const QColor &background)
{
    rust::Vec<std::uint8_t> out;
    if (!window)
        return out;
    QImage details = window->grabWindow();
    if (details.isNull())
        return out;
    // Placed by pixels: the two windows may be on screens of different scale.
    details.setDevicePixelRatio(1);
    QImage meter = beside ? beside->grabWindow() : QImage();
    meter.setDevicePixelRatio(1);
    // The meter on the left, as 1.x on Linux: Wayland does not say where
    // windows are, so they cannot be placed as on screen.
    QImage image(meter.width() + details.width(), std::max(meter.height(), details.height()), QImage::Format_RGB32);
    image.fill(background);
    QPainter painter(&image);
    if (!meter.isNull())
        painter.drawImage(0, 0, meter);
    painter.drawImage(meter.width(), 0, details);
    painter.end();
    if (clipboard)
        QGuiApplication::clipboard()->setImage(image);
    QByteArray png;
    QBuffer buffer(&png);
    if (!buffer.open(QIODevice::WriteOnly) || !image.save(&buffer, "PNG"))
        return out;
    out.reserve(static_cast<std::size_t>(png.size()));
    for (const char byte : png)
        out.push_back(static_cast<std::uint8_t>(byte));
    return out;
}

void copyText(const QString &text)
{
    QGuiApplication::clipboard()->setText(text);
}

QString qtLine()
{
    return QStringLiteral("Qt %1, %2").arg(QString::fromLatin1(qVersion()), QGuiApplication::platformName());
}

void watchWindow(QQuickWindow *window)
{
    if (!window)
        return;
    window->installEventFilter(new PointerTimes(window));
    // Emitted on the render thread, right after the frame went out.
    QObject::connect(
        window, &QQuickWindow::frameSwapped, window,
        [] {
            const std::uint64_t now = monotonicMicros();
            const std::lock_guard<std::mutex> lock(frameLock);
            if (frameTimes.size() == 64)
                frameTimes.erase(frameTimes.begin());
            frameTimes.push_back(now);
        },
        Qt::DirectConnection);
}

std::uint64_t monotonicMicros()
{
#ifdef Q_OS_WIN
    // No CLOCK_MONOTONIC on Windows; steady_clock is the performance counter.
    const auto since = std::chrono::steady_clock::now().time_since_epoch();
    return std::uint64_t(std::chrono::duration_cast<std::chrono::microseconds>(since).count());
#else
    timespec now{};
    clock_gettime(CLOCK_MONOTONIC, &now);
    return std::uint64_t(now.tv_sec) * 1000000u + std::uint64_t(now.tv_nsec) / 1000u;
#endif
}

std::uint64_t pointerEventTime()
{
    return pointerTime;
}

bool pointerButtonDown()
{
    return buttonDown;
}

QPointF pointerGlobalPosition()
{
    return pointerGlobal;
}

void releasePointer(QQuickWindow *window)
{
    if (!window || !pointerDevice)
        return;
    const QPointer<QQuickWindow> target(window);
    const QPointF local = pointerLocal;
    const QPointF global = pointerGlobal;
    const QPointingDevice *device = pointerDevice;
    // After the event that started the resize is done with.
    QMetaObject::invokeMethod(
        window,
        [target, local, global, device] {
            if (!target)
                return;
            QMouseEvent release(QEvent::MouseButtonRelease, local, local, global, Qt::LeftButton, Qt::NoButton,
                                Qt::NoModifier, device);
            QCoreApplication::sendEvent(target, &release);
        },
        Qt::QueuedConnection);
}

rust::Vec<std::uint64_t> takeFrameTimes()
{
    const std::lock_guard<std::mutex> lock(frameLock);
    rust::Vec<std::uint64_t> taken;
    for (const std::uint64_t time : frameTimes)
        taken.push_back(time);
    frameTimes.clear();
    return taken;
}

void requestFrame(QQuickWindow *window)
{
    if (window)
        window->update();
}

// Declared by Qt's X11 interface too, when Qt has one.
struct xcb_connection_t;

namespace {

// The X11 input shape, from libxcb-shape. QWindow::setMask sets the X11
// bounding shape, which also cuts what is drawn, so the input shape is set
// here. The few calls are declared here and resolved at run time: Qt's X11
// plugin links libxcb and libxcb-shape, so they are there whenever the
// meter runs on X11, and the build needs no X11 headers.
struct XcbRectangle {
    std::int16_t x, y;
    std::uint16_t width, height;
};
struct XcbCookie {
    unsigned int sequence;
};
constexpr std::uint8_t shapeSet = 0; // XCB_SHAPE_SO_SET
constexpr std::uint8_t shapeInput = 2; // XCB_SHAPE_SK_INPUT
constexpr std::uint8_t unsorted = 0; // XCB_CLIP_ORDERING_UNSORTED

struct XcbShape {
    XcbCookie (*rectangles)(xcb_connection_t *, std::uint8_t op, std::uint8_t kind, std::uint8_t ordering,
                            std::uint32_t window, std::int16_t x, std::int16_t y, std::uint32_t count,
                            const XcbRectangle *rectangles) = nullptr;
    XcbCookie (*mask)(xcb_connection_t *, std::uint8_t op, std::uint8_t kind, std::uint32_t window,
                      std::int16_t x, std::int16_t y, std::uint32_t bitmap) = nullptr;
    int (*flush)(xcb_connection_t *) = nullptr;
};

xcb_connection_t *x11Connection()
{
#if QT_CONFIG(xcb)
    if (QGuiApplication::platformName() != QStringLiteral("xcb"))
        return nullptr;
    const auto *x11 = qGuiApp->nativeInterface<QNativeInterface::QX11Application>();
    return x11 ? x11->connection() : nullptr;
#else
    return nullptr;
#endif
}

const XcbShape *xcbShape()
{
    static const XcbShape shape = [] {
        XcbShape found;
        found.rectangles = reinterpret_cast<decltype(found.rectangles)>(
            QLibrary::resolve(QStringLiteral("xcb-shape"), 0, "xcb_shape_rectangles"));
        found.mask = reinterpret_cast<decltype(found.mask)>(
            QLibrary::resolve(QStringLiteral("xcb-shape"), 0, "xcb_shape_mask"));
        found.flush = reinterpret_cast<decltype(found.flush)>(
            QLibrary::resolve(QStringLiteral("xcb"), 1, "xcb_flush"));
        return found;
    }();
    return shape.rectangles && shape.mask && shape.flush ? &shape : nullptr;
}

} // namespace

namespace {

// xcb_screen_iterator_t: an xcb_screen_t starts with its root window.
struct XcbScreenIterator {
    const std::uint32_t *data;
    int rem;
    int index;
};

// xcb_client_message_event_t with 32-bit data.
struct XcbClientMessage {
    std::uint8_t responseType;
    std::uint8_t format;
    std::uint16_t sequence;
    std::uint32_t window;
    std::uint32_t type;
    std::uint32_t data[5];
};
static_assert(sizeof(XcbClientMessage) == 32);

// The few libxcb calls that set a window property or ask the window
// manager to, resolved at run time like the shape calls above.
struct XcbProperty {
    XcbCookie (*internAtom)(xcb_connection_t *, std::uint8_t onlyIfExists, std::uint16_t length,
                            const char *name) = nullptr;
    void *(*internAtomReply)(xcb_connection_t *, XcbCookie, void **error) = nullptr;
    XcbCookie (*change)(xcb_connection_t *, std::uint8_t mode, std::uint32_t window, std::uint32_t property,
                        std::uint32_t type, std::uint8_t format, std::uint32_t length, const void *data) = nullptr;
    XcbCookie (*sendEvent)(xcb_connection_t *, std::uint8_t propagate, std::uint32_t destination, std::uint32_t mask,
                           const char *event) = nullptr;
    const void *(*getSetup)(xcb_connection_t *) = nullptr;
    XcbScreenIterator (*rootsIterator)(const void *setup) = nullptr;
    int (*flush)(xcb_connection_t *) = nullptr;
};
constexpr std::uint8_t propertyAppend = 2; // XCB_PROP_MODE_APPEND
constexpr std::uint32_t atomType = 4; // XCB_ATOM_ATOM
constexpr std::uint8_t clientMessage = 33; // XCB_CLIENT_MESSAGE
// XCB_EVENT_MASK_SUBSTRUCTURE_NOTIFY | XCB_EVENT_MASK_SUBSTRUCTURE_REDIRECT
constexpr std::uint32_t toWindowManager = (1u << 19) | (1u << 20);

const XcbProperty *xcbProperty()
{
    static const XcbProperty property = [] {
        XcbProperty found;
        const auto resolve = [](const char *name) { return QLibrary::resolve(QStringLiteral("xcb"), 1, name); };
        found.internAtom = reinterpret_cast<decltype(found.internAtom)>(resolve("xcb_intern_atom"));
        found.internAtomReply = reinterpret_cast<decltype(found.internAtomReply)>(resolve("xcb_intern_atom_reply"));
        found.change = reinterpret_cast<decltype(found.change)>(resolve("xcb_change_property"));
        found.sendEvent = reinterpret_cast<decltype(found.sendEvent)>(resolve("xcb_send_event"));
        found.getSetup = reinterpret_cast<decltype(found.getSetup)>(resolve("xcb_get_setup"));
        found.rootsIterator = reinterpret_cast<decltype(found.rootsIterator)>(resolve("xcb_setup_roots_iterator"));
        found.flush = reinterpret_cast<decltype(found.flush)>(resolve("xcb_flush"));
        return found;
    }();
    return property.internAtom && property.internAtomReply && property.change && property.sendEvent
                   && property.getSetup && property.rootsIterator && property.flush
               ? &property
               : nullptr;
}

std::uint32_t internAtom(xcb_connection_t *x11, const XcbProperty &xcb, const char *name)
{
    void *reply = xcb.internAtomReply(x11, xcb.internAtom(x11, 0, std::uint16_t(std::strlen(name)), name), nullptr);
    if (!reply)
        return 0;
    // xcb_intern_atom_reply_t: the atom follows 8 bytes of header.
    std::uint32_t atom = 0;
    std::memcpy(&atom, static_cast<const char *>(reply) + 8, sizeof atom);
    std::free(reply);
    return atom;
}

// Window properties: whether the window keeps out of the taskbar, and
// whether an OffTaskbar watches it.
constexpr const char *offTaskbarFlag = "daevalogOffTaskbar";
constexpr const char *offTaskbarWatched = "daevalogOffTaskbarWatched";

// Sees the window's show, which Qt sends before it maps the window.
class OffTaskbar : public QObject
{
public:
    using QObject::QObject;

    // Asks the window manager to add or take away the state of a window on
    // screen: a _NET_WM_STATE message to the root window (EWMH).
    static void changeShown(QWindow *window, bool off)
    {
        xcb_connection_t *x11 = x11Connection();
        const XcbProperty *xcb = xcbProperty();
        if (!x11 || !xcb)
            return;
        const auto [state, skip] = atoms(x11, *xcb);
        const XcbScreenIterator screens = xcb->rootsIterator(xcb->getSetup(x11));
        if (!state || !skip || screens.rem < 1 || !screens.data)
            return;
        XcbClientMessage message{};
        message.responseType = clientMessage;
        message.format = 32;
        message.window = static_cast<std::uint32_t>(window->winId());
        message.type = state;
        message.data[0] = off ? 1 : 0; // _NET_WM_STATE_ADD, _NET_WM_STATE_REMOVE
        message.data[1] = skip;
        message.data[3] = 1; // from a normal application
        xcb->sendEvent(x11, 0, screens.data[0], toWindowManager, reinterpret_cast<const char *>(&message));
        xcb->flush(x11);
    }

protected:
    // Before the map, the state goes on the window; once it is on screen,
    // the window manager is asked as well: KWin 6.7 (XWayland) mapped the
    // meter without the state it found on it.
    bool eventFilter(QObject *watched, QEvent *event) override
    {
        auto *window = static_cast<QWindow *>(watched);
        if (event->type() == QEvent::Show) {
            asked = false;
            if (window->property(offTaskbarFlag).toBool())
                skipTaskbar(window);
        } else if (event->type() == QEvent::Expose && !asked && window->isExposed()) {
            asked = true;
            if (window->property(offTaskbarFlag).toBool())
                changeShown(window, true);
        }
        return QObject::eventFilter(watched, event);
    }

private:
    // Whether the window manager was asked since the last show.
    bool asked = false;

    struct Atoms {
        std::uint32_t state;
        std::uint32_t skip;
    };

    static Atoms atoms(xcb_connection_t *x11, const XcbProperty &xcb)
    {
        static const Atoms found{internAtom(x11, xcb, "_NET_WM_STATE"),
                                 internAtom(x11, xcb, "_NET_WM_STATE_SKIP_TASKBAR")};
        return found;
    }

    static void skipTaskbar(QWindow *window)
    {
        xcb_connection_t *x11 = x11Connection();
        const XcbProperty *xcb = xcbProperty();
        if (!x11 || !xcb)
            return;
        const auto [state, skip] = atoms(x11, *xcb);
        if (!state || !skip)
            return;
        // The window manager removes the property when the window unmaps.
        xcb->change(x11, propertyAppend, static_cast<std::uint32_t>(window->winId()), state, atomType, 32, 1, &skip);
        xcb->flush(x11);
    }
};

} // namespace

void keepOffTaskbar(QQuickWindow *window)
{
    setOffTaskbar(window, true);
}

void setOffTaskbar(QQuickWindow *window, bool off)
{
    if (!window)
        return;
    if (!window->property(offTaskbarWatched).toBool()) {
        window->installEventFilter(new OffTaskbar(window));
        window->setProperty(offTaskbarWatched, true);
    }
    const bool was = window->property(offTaskbarFlag).toBool();
    window->setProperty(offTaskbarFlag, off);
    if (was != off && window->isVisible())
        OffTaskbar::changeShown(window, off);
}

bool canShapeInput()
{
    if (QGuiApplication::platformName().startsWith(QStringLiteral("wayland")))
        return true;
    return x11Connection() && xcbShape();
}

void setInputArea(QQuickWindow *window, const QRectF &area)
{
    if (!window)
        return;
    xcb_connection_t *x11 = x11Connection();
    const XcbShape *shape = xcbShape();
    if (!x11 || !shape) {
        window->setMask(area.isEmpty() ? QRegion() : QRegion(area.toAlignedRect()));
        window->update();
        return;
    }
    const auto id = static_cast<std::uint32_t>(window->winId());
    if (area.isEmpty()) {
        shape->mask(x11, shapeSet, shapeInput, id, 0, 0, 0);
    } else {
        const qreal ratio = window->devicePixelRatio();
        const QRect native = QRectF(area.topLeft() * ratio, area.size() * ratio).toAlignedRect();
        const XcbRectangle rectangle{
            static_cast<std::int16_t>(native.x()), static_cast<std::int16_t>(native.y()),
            static_cast<std::uint16_t>(native.width()), static_cast<std::uint16_t>(native.height())};
        shape->rectangles(x11, shapeSet, shapeInput, unsorted, id, 0, 0, 1, &rectangle);
    }
    shape->flush(x11);
}

void setPopupPlace(QQuickWindow *window, const QRectF &anchor, bool below, std::uint32_t adjust)
{
    if (!window)
        return;
    const Qt::Edges edge = below ? Qt::BottomEdge : Qt::TopEdge;
    window->setProperty("_q_waylandPopupAnchorRect", anchor.toAlignedRect());
    window->setProperty("_q_waylandPopupAnchor", QVariant::fromValue(edge | Qt::LeftEdge));
    window->setProperty("_q_waylandPopupGravity", QVariant::fromValue(edge | Qt::RightEdge));
    window->setProperty("_q_waylandPopupConstraintAdjustment", QVariant::fromValue(adjust));
}

QString platformName()
{
    return QGuiApplication::platformName();
}
