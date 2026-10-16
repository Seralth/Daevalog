// SPDX-License-Identifier: AGPL-3.0-only

//! `Hotkey`, the QML singleton for the global lock key: the click-through
//! lock, with the key from Settings > Keybinds.
//!
//! On Linux it goes through the desktop's GlobalShortcuts portal as 1.x
//! (KDE Plasma, GNOME 48 and later, Hyprland). The desktop asks the player
//! to approve the key the first time and may give another key. `trigger` is
//! the key the desktop gave. Without the portal (Sway, most X11 sessions)
//! there is no key, and the page says so; the tray and the ⋯ menu lock the
//! meter as before.
//!
//! On Windows the meter registers the key itself with RegisterHotKey. The
//! state is "bound", "taken" (another program holds the key) or "unbound"
//! (no key with a modifier was recorded).
//!
//! The key is served from a thread of its own; Qt hears from it through
//! the object's queue.

use core::pin::Pin;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use tokio::sync::mpsc;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        /// "off" (not started), "starting", "missing" (no portal),
        /// "waiting" (the desktop is asking the player), "bound", "unbound"
        /// (the desktop gave no key), "refused" (the player said no),
        /// "failed"; on Windows "taken" (another program has the key).
        #[qproperty(QString, state, READ, NOTIFY)]
        /// The key the desktop gave, in its words ("Ctrl+Alt+L"); on
        /// Windows the key registered.
        #[qproperty(QString, trigger, READ, NOTIFY)]
        /// The desktop can open its own settings for the key (portal 2).
        #[qproperty(bool, can_configure, READ, NOTIFY)]
        type Hotkey = super::HotkeyRust;

        /// Offers the lock key `label` ("Ctrl+Alt+L"), once.
        #[qinvokable]
        fn start(self: Pin<&mut Hotkey>, label: &QString);

        /// Offers `label` again, after the player recorded a new key.
        #[qinvokable]
        fn offer(self: Pin<&mut Hotkey>, label: &QString);

        /// Opens the desktop's settings for the key.
        #[qinvokable]
        fn configure(self: Pin<&mut Hotkey>);

        /// The lock key was pressed.
        #[qsignal]
        fn pressed(self: Pin<&mut Hotkey>);
    }

    impl cxx_qt::Threading for Hotkey {}
}

enum Command {
    /// A key label as Settings keeps it ("Ctrl+Alt+L").
    Offer(String),
    Configure,
}

pub struct HotkeyRust {
    state: QString,
    trigger: QString,
    can_configure: bool,
    commands: Option<mpsc::UnboundedSender<Command>>,
}

impl Default for HotkeyRust {
    fn default() -> Self {
        Self { state: QString::from("off"), trigger: QString::default(), can_configure: false, commands: None }
    }
}

/// What the key's thread tells the window.
#[derive(Debug, Clone, PartialEq)]
enum Event {
    State(&'static str),
    Trigger(String),
    CanConfigure(bool),
    Pressed,
}

/// The state after a bind: bound with a key, else unbound.
fn bound_state(trigger: &str) -> &'static str {
    if trigger.is_empty() { "unbound" } else { "bound" }
}

impl qobject::Hotkey {
    fn start(mut self: Pin<&mut Self>, label: &QString) {
        if self.commands.is_some() {
            return;
        }
        let (send, receive) = mpsc::unbounded_channel();
        self.as_mut().rust_mut().commands = Some(send);
        self.as_mut().show(Event::State("starting"));
        let qt = self.qt_thread();
        let label = label.to_string();
        let _ = std::thread::Builder::new().name("hotkey".into()).spawn(move || {
            let tell = |event: Event| {
                let _ = qt.queue(move |hotkey| hotkey.show(event));
            };
            serve_key(label, receive, &tell);
        });
    }

    fn offer(self: Pin<&mut Self>, label: &QString) {
        self.send(Command::Offer(label.to_string()));
    }

    fn configure(self: Pin<&mut Self>) {
        self.send(Command::Configure);
    }

    fn send(&self, command: Command) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(command);
            #[cfg(windows)]
            win32::wake();
        }
    }

    fn show(mut self: Pin<&mut Self>, event: Event) {
        match event {
            Event::State(state) => {
                self.as_mut().rust_mut().state = QString::from(state);
                self.as_mut().state_changed();
            }
            Event::Trigger(trigger) => {
                self.as_mut().rust_mut().trigger = QString::from(&trigger);
                self.as_mut().trigger_changed();
            }
            Event::CanConfigure(can) => {
                self.as_mut().rust_mut().can_configure = can;
                self.as_mut().can_configure_changed();
            }
            Event::Pressed => self.pressed(),
        }
    }
}

/// Runs the key's service on this thread until the program quits.
#[cfg(target_os = "linux")]
fn serve_key(label: String, commands: mpsc::UnboundedReceiver<Command>, tell: &dyn Fn(Event)) {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(_) => return tell(Event::State("failed")),
    };
    runtime.block_on(portal::serve(None, portal::preferred(&label), commands, tell));
}

#[cfg(windows)]
fn serve_key(label: String, commands: mpsc::UnboundedReceiver<Command>, tell: &dyn Fn(Event)) {
    win32::serve(label, commands, tell);
}

#[cfg(target_os = "linux")]
mod portal {
    use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
    use daevalog::platform::hotkeys::{parse_hotkey_label, portal_trigger};
    use futures_util::StreamExt;
    use tokio::sync::mpsc;

    use super::{bound_state, Command, Event};

    /// The program's .desktop name, which the portal knows the app by.
    const APP_ID: &str = env!("DAEVALOG_APP_ID");
    const LOCK_ID: &str = "toggle-lock";
    const LOCK_DESCRIPTION: &str = "Lock or unlock the meter (clicks go through)";

    /// The portal's trigger for a saved label, as the backend writes it.
    pub(super) fn preferred(label: &str) -> Option<String> {
        let (mods, vk) = parse_hotkey_label(label)?;
        portal_trigger(mods, vk)
    }

    /// An error from the portal as a state: the player closed the desktop's
    /// question, or anything else.
    fn error_state(error: &ashpd::Error) -> &'static str {
        match error {
            ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => "refused",
            _ => "failed",
        }
    }

    /// The portal session: offers the key, then passes on presses and changes
    /// and takes new offers until the program quits. Without the portal it
    /// says so and ends.
    /// `bus` is a D-Bus address in place of the session's (the tests).
    pub(super) async fn serve(bus: Option<&str>, wanted: Option<String>, mut commands: mpsc::UnboundedReceiver<Command>, tell: &dyn Fn(Event)) {
        let portal = match open(bus).await {
            Ok(portal) => portal,
            Err(error) => {
                eprintln!("daevalog-dps-meter: no lock hotkey (GlobalShortcuts portal: {error}); the tray menu locks the meter");
                return tell(Event::State("missing"));
            }
        };
        tell(Event::CanConfigure(portal.version() >= 2));
        let session = match portal.create_session(Default::default()).await {
            Ok(session) => session,
            Err(error) => {
                eprintln!("daevalog-dps-meter: no lock hotkey (GlobalShortcuts session: {error})");
                return tell(Event::State("missing"));
            }
        };
        let (Ok(mut pressed), Ok(mut changed)) = (portal.receive_activated().await, portal.receive_shortcuts_changed().await) else {
            return tell(Event::State("failed"));
        };
        let bind = |wanted: Option<String>| {
            let (portal, session) = (&portal, &session);
            async move {
                tell(Event::State("waiting"));
                let shortcut = NewShortcut::new(LOCK_ID, LOCK_DESCRIPTION).preferred_trigger(wanted.as_deref());
                match portal.bind_shortcuts(session, &[shortcut], None, Default::default()).await.and_then(|r| r.response()) {
                    Ok(bound) => {
                        let trigger = bound.shortcuts().iter().find(|s| s.id() == LOCK_ID).map_or("", |s| s.trigger_description());
                        tell(Event::Trigger(trigger.to_string()));
                        tell(Event::State(bound_state(trigger)));
                    }
                    Err(error) => {
                        eprintln!("daevalog-dps-meter: the lock hotkey was not bound ({error})");
                        tell(Event::State(error_state(&error)));
                    }
                }
            }
        };
        bind(wanted).await;
        loop {
            tokio::select! {
                Some(event) = pressed.next() => {
                    if event.shortcut_id() == LOCK_ID {
                        tell(Event::Pressed);
                    }
                }
                Some(event) = changed.next() => {
                    if let Some(shortcut) = event.shortcuts().iter().find(|s| s.id() == LOCK_ID) {
                        let trigger = shortcut.trigger_description();
                        tell(Event::Trigger(trigger.to_string()));
                        tell(Event::State(bound_state(trigger)));
                    }
                }
                command = commands.recv() => match command {
                    Some(Command::Offer(label)) => bind(preferred(&label)).await,
                    Some(Command::Configure) => {
                        if let Err(error) = portal.configure_shortcuts(&session, None, Default::default()).await {
                            eprintln!("daevalog-dps-meter: the desktop's shortcut settings did not open ({error})");
                        }
                    }
                    // The window is gone.
                    None => break,
                },
            }
        }
    }

    /// The portal, on a connection that names the app first: started from a
    /// terminal or a launcher that gives it no app scope, the portal cannot
    /// tell which app this is. The name must come before any other portal
    /// call, and fails harmlessly where the portal is too old.
    async fn open(bus: Option<&str>) -> ashpd::Result<GlobalShortcuts> {
        let connection = match bus {
            Some(address) => ashpd::zbus::connection::Builder::address(address)?.build().await?,
            None => ashpd::zbus::Connection::session().await?,
        };
        let _ = connection
            .call_method(
                Some("org.freedesktop.portal.Desktop"),
                "/org/freedesktop/portal/desktop",
                Some("org.freedesktop.host.portal.Registry"),
                "Register",
                &(APP_ID, std::collections::HashMap::<&str, ashpd::zvariant::Value>::new()),
            )
            .await;
        GlobalShortcuts::with_connection(connection).await
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_saved_key_is_offered_in_the_portals_words() {
            assert_eq!(preferred("Ctrl+Alt+L").as_deref(), Some("CTRL+ALT+l"));
            assert_eq!(preferred("Shift+F5").as_deref(), Some("SHIFT+F5"));
            assert_eq!(preferred("L"), None, "a key without a modifier is not offered");
            assert_eq!(preferred(""), None);
        }

        #[test]
        fn a_bind_without_a_key_is_unbound() {
            assert_eq!(bound_state("Ctrl+Alt+L"), "bound");
            assert_eq!(bound_state(""), "unbound");
        }

        #[test]
        fn a_closed_question_is_refused() {
            assert_eq!(error_state(&ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)), "refused");
            assert_eq!(error_state(&ashpd::Error::Response(ashpd::desktop::ResponseError::Other)), "failed");
        }

        /// No portal on the bus (a private bus with none): the state says so
        /// and nothing waits.
        #[test]
        fn without_a_portal_the_state_says_missing() {
            let bus = std::env::temp_dir().join(format!("daevalog-no-bus-{}", std::process::id()));
            let events = std::sync::Mutex::new(Vec::new());
            let tell = |e: Event| events.lock().unwrap().push(e);
            let (_send, receive) = mpsc::unbounded_channel();
            let address = format!("unix:path={}", bus.display());
            tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(serve(Some(&address), None, receive, &tell));
            assert_eq!(*events.lock().unwrap(), vec![Event::State("missing")]);
        }
    }
}

/// The lock key on Windows: RegisterHotKey on this thread, whose message
/// loop gets WM_HOTKEY. A new key from Settings wakes the loop with a
/// thread message.
#[cfg(windows)]
mod win32 {
    use std::sync::atomic::{AtomicU32, Ordering};

    use ::windows::Win32::Foundation::{LPARAM, WPARAM};
    use ::windows::Win32::System::Threading::GetCurrentThreadId;
    use ::windows::Win32::UI::Input::KeyboardAndMouse::{
        RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT,
    };
    use ::windows::Win32::UI::WindowsAndMessaging::{
        GetMessageW, PeekMessageW, PostThreadMessageW, MSG, PM_NOREMOVE, WM_APP, WM_HOTKEY,
    };
    use daevalog::platform::hotkeys::parse_hotkey_label;
    use tokio::sync::mpsc::{self, error::TryRecvError};

    use super::{Command, Event};

    /// The key's id for RegisterHotKey.
    const LOCK: i32 = 1;
    /// Tells the loop that a command is waiting.
    const WAKE: u32 = WM_APP + 1;

    /// The key thread's id; 0 before it runs.
    static THREAD: AtomicU32 = AtomicU32::new(0);

    pub(super) fn wake() {
        let thread = THREAD.load(Ordering::Acquire);
        if thread != 0 {
            // SAFETY: a plain message to a thread of this process.
            let _ = unsafe { PostThreadMessageW(thread, WAKE, WPARAM(0), LPARAM(0)) };
        }
    }

    /// What a registration came to.
    #[derive(Debug, PartialEq)]
    pub(super) enum Bound {
        /// The key is ours.
        Key,
        /// Another program holds it.
        Taken,
        /// The label has no key with a modifier.
        NoKey,
    }

    /// The state and trigger the page shows for a registration.
    pub(super) fn events(bound: &Bound, label: &str) -> [Event; 2] {
        match bound {
            Bound::Key => [Event::Trigger(label.trim().to_string()), Event::State("bound")],
            Bound::Taken => [Event::Trigger(String::new()), Event::State("taken")],
            Bound::NoKey => [Event::Trigger(String::new()), Event::State("unbound")],
        }
    }

    /// Registers `label` for this thread, in place of the key before.
    fn register(label: &str) -> Bound {
        // SAFETY: frees this thread's own key, if it has one.
        let _ = unsafe { UnregisterHotKey(None, LOCK) };
        let Some((mods, vk)) = parse_hotkey_label(label) else { return Bound::NoKey };
        // The label's modifier bits are RegisterHotKey's MOD_ values.
        let modifiers = HOT_KEY_MODIFIERS(mods) | MOD_NOREPEAT;
        // SAFETY: no window: WM_HOTKEY goes to this thread's queue.
        match unsafe { RegisterHotKey(None, LOCK, modifiers, vk) } {
            Ok(()) => Bound::Key,
            Err(e) => {
                eprintln!("daevalog-dps-meter: the lock key {label} was not registered ({e})");
                Bound::Taken
            }
        }
    }

    pub(super) fn serve(label: String, mut commands: mpsc::UnboundedReceiver<Command>, tell: &dyn Fn(Event)) {
        let mut msg = MSG::default();
        // SAFETY: makes this thread's message queue, so thread messages
        // posted from now on are kept.
        unsafe {
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
            THREAD.store(GetCurrentThreadId(), Ordering::Release);
        }
        let bind = |label: &str| events(&register(label), label).into_iter().for_each(tell);
        bind(&label);
        loop {
            // SAFETY: `msg` is written by the call; 0 is WM_QUIT, -1 an error.
            let got = unsafe { GetMessageW(&mut msg, None, 0, 0) }.0;
            if got <= 0 {
                break;
            }
            match msg.message {
                WM_HOTKEY if msg.wParam.0 == LOCK as usize => tell(Event::Pressed),
                WAKE => loop {
                    match commands.try_recv() {
                        Ok(Command::Offer(label)) => bind(&label),
                        // Windows has no settings page for the key.
                        Ok(Command::Configure) => {}
                        Err(TryRecvError::Empty) => break,
                        // The window is gone.
                        Err(TryRecvError::Disconnected) => return,
                    }
                },
                _ => {}
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_registration_shows_as_a_state() {
            assert_eq!(events(&Bound::Key, " Ctrl+Alt+L "), [Event::Trigger("Ctrl+Alt+L".into()), Event::State("bound")]);
            assert_eq!(events(&Bound::Taken, "Ctrl+Alt+L"), [Event::Trigger(String::new()), Event::State("taken")]);
            assert_eq!(events(&Bound::NoKey, "L"), [Event::Trigger(String::new()), Event::State("unbound")]);
        }

        #[test]
        fn a_key_without_a_modifier_is_not_registered() {
            assert_eq!(register("L"), Bound::NoKey);
            assert_eq!(register(""), Bound::NoKey);
        }
    }
}
