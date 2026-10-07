//! Global hotkeys on Linux, through the desktop's GlobalShortcuts portal (KDE
//! Plasma, GNOME 48 and later, Hyprland). Only the click-through lock has one
//! here. The meter offers the shortcut with a description and the saved key as
//! the preferred one; the player binds or changes it in the desktop's own
//! shortcut settings. Without the portal (Sway, most X11 sessions) there is no
//! hotkey, and the tray menu locks and unlocks the meter instead.
//!
//! The meter holds no capability since capture moved to its own helper
//! (issue #11), so the portal serves it directly; it talks to the portal from
//! a thread of its own.

use std::sync::atomic::{AtomicBool, Ordering};

use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;

/// The name of the packaged .desktop file. The portal needs it to know the app.
const APP_ID: &str = "daevalog-dps-meter";
const LOCK_ID: &str = "toggle-lock";
const LOCK_DESCRIPTION: &str = "Lock or unlock the meter (clicks go through)";

pub struct HotkeyManager {
    started: AtomicBool,
}

impl HotkeyManager {
    pub fn new() -> Self {
        Self { started: AtomicBool::new(false) }
    }

    /// Offer the lock hotkey to the portal; `on_lock` runs when it is pressed.
    /// The reload and show/hide hotkeys are not offered on Linux.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &self,
        _: u32,
        _: u32,
        _: u32,
        _: u32,
        lock_mods: u32,
        lock_vk: u32,
        _: impl Fn() + Send + 'static,
        _: impl Fn() + Send + 'static,
        on_lock: impl Fn() + Send + 'static,
    ) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let preferred = portal_trigger(lock_mods, lock_vk);
        // The thread waits for key presses until the meter quits.
        let _ = std::thread::Builder::new().name("hotkey-portal".into()).spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())
                .and_then(|rt| rt.block_on(serve(preferred.as_deref(), on_lock)).map_err(|e| e.to_string()));
            if let Err(e) = result {
                tracing::info!("No lock hotkey (GlobalShortcuts portal: {e}); the tray menu locks the meter");
            }
        });
    }

    /// The portal session ends with the meter.
    pub fn stop(&self) {}
}

async fn serve(preferred: Option<&str>, on_lock: impl Fn()) -> ashpd::Result<()> {
    let connection = ashpd::zbus::Connection::session().await?;
    // Started from a terminal or by a launcher that gives it no app scope, the
    // portal cannot tell which app this is. Naming it must come before any
    // other portal call on this connection, and fails harmlessly where the
    // portal is too old.
    let _ = connection
        .call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.host.portal.Registry"),
            "Register",
            &(APP_ID, std::collections::HashMap::<&str, ashpd::zvariant::Value>::new()),
        )
        .await;
    let portal = GlobalShortcuts::with_connection(connection).await?;
    let session = portal.create_session(Default::default()).await?;
    let mut pressed = portal.receive_activated().await?;
    let shortcut = NewShortcut::new(LOCK_ID, LOCK_DESCRIPTION).preferred_trigger(preferred);
    let bound = portal
        .bind_shortcuts(&session, &[shortcut], None, Default::default())
        .await?
        .response()?;
    match bound.shortcuts().iter().find(|s| s.id() == LOCK_ID).map_or("", |s| s.trigger_description()) {
        "" => tracing::info!("Lock hotkey offered; it has no key until one is set in the desktop's shortcut settings"),
        keys => tracing::info!("Lock hotkey: {keys}"),
    }
    while let Some(event) = pressed.next().await {
        if event.shortcut_id() == LOCK_ID {
            on_lock();
        }
    }
    Ok(())
}

/// A saved hotkey (Win32 modifier flags and virtual-key code, as
/// `parse_hotkey_label` gives them) in the XDG shortcuts format the portal
/// takes as the preferred trigger, e.g. "CTRL+ALT+l".
fn portal_trigger(mods: u32, vk: u32) -> Option<String> {
    let key = match vk {
        0x08 => "BackSpace".to_string(),
        0x09 => "Tab".to_string(),
        0x0D => "Return".to_string(),
        0x1B => "Escape".to_string(),
        0x20 => "space".to_string(),
        0x21 => "Page_Up".to_string(),
        0x22 => "Page_Down".to_string(),
        0x23 => "End".to_string(),
        0x24 => "Home".to_string(),
        0x25 => "Left".to_string(),
        0x26 => "Up".to_string(),
        0x27 => "Right".to_string(),
        0x28 => "Down".to_string(),
        0x2D => "Insert".to_string(),
        0x2E => "Delete".to_string(),
        0x30..=0x39 => char::from(vk as u8).to_string(),
        0x41..=0x5A => char::from(vk as u8).to_ascii_lowercase().to_string(),
        0x70..=0x7B => format!("F{}", vk - 0x6F),
        _ => return None,
    };
    let mut parts: Vec<String> = [(0x0002, "CTRL"), (0x0001, "ALT"), (0x0004, "SHIFT"), (0x0008, "LOGO")]
        .into_iter()
        .filter(|&(flag, _)| mods & flag != 0)
        .map(|(_, name)| name.to_string())
        .collect();
    if parts.is_empty() {
        return None;
    }
    parts.push(key);
    Some(parts.join("+"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::hotkeys::parse_hotkey_label;

    fn from_label(label: &str) -> Option<String> {
        let (mods, vk) = parse_hotkey_label(label)?;
        portal_trigger(mods, vk)
    }

    #[test]
    fn saved_hotkeys_become_portal_triggers() {
        assert_eq!(from_label("Ctrl+Alt+L").as_deref(), Some("CTRL+ALT+l"));
        assert_eq!(from_label("Shift+Win+F5").as_deref(), Some("SHIFT+LOGO+F5"));
        assert_eq!(from_label("Alt+Ctrl+PageUp").as_deref(), Some("CTRL+ALT+Page_Up"));
        assert_eq!(from_label("Ctrl+7").as_deref(), Some("CTRL+7"));
        assert_eq!(from_label("Ctrl+Space").as_deref(), Some("CTRL+space"));
        assert_eq!(from_label("Ctrl+F12").as_deref(), Some("CTRL+F12"));
    }

    #[test]
    fn unknown_keys_and_bare_keys_have_no_trigger() {
        assert_eq!(portal_trigger(0x0002, 0xBA), None);
        assert_eq!(portal_trigger(0, 0x4C), None);
    }
}
