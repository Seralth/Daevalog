use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tracing::{info, warn};

/// Global hotkey manager using Win32 RegisterHotKey.
/// Runs its own message loop thread to receive WM_HOTKEY messages.
pub struct HotkeyManager {
    running: Arc<AtomicBool>,
}

impl HotkeyManager {
    const HOTKEY_RELOAD: i32 = 1;
    const HOTKEY_TOGGLE: i32 = 2;

    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Start the hotkey listener thread.
    /// `on_reload` is called when the reload hotkey fires.
    /// `on_toggle` is called when the toggle window hotkey fires.
    pub fn start(
        &self,
        reload_mods: u32,
        reload_vk: u32,
        toggle_mods: u32,
        toggle_vk: u32,
        on_reload: impl Fn() + Send + 'static,
        on_toggle: impl Fn() + Send + 'static,
    ) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }

        let running = self.running.clone();

        std::thread::spawn(move || {
            use windows::Win32::UI::Input::KeyboardAndMouse::*;
            use windows::Win32::UI::WindowsAndMessaging::*;

            unsafe {
                // Register hotkeys
                let norepeat = 0x4000u32;
                let mut reload_ok = false;
                let mut toggle_ok = false;

                if reload_vk > 0 {
                    if RegisterHotKey(None, Self::HOTKEY_RELOAD,
                        HOT_KEY_MODIFIERS(reload_mods | norepeat), reload_vk).is_ok() {
                        reload_ok = true;
                        info!("Reload hotkey registered: mods={:#x} vk={:#x}", reload_mods, reload_vk);
                    } else {
                        warn!("Reload hotkey unavailable (mods={:#x} vk={:#x}) — already in use by another app",
                            reload_mods, reload_vk);
                    }
                }

                if toggle_vk > 0 {
                    if RegisterHotKey(None, Self::HOTKEY_TOGGLE,
                        HOT_KEY_MODIFIERS(toggle_mods | norepeat), toggle_vk).is_ok() {
                        toggle_ok = true;
                        info!("Toggle hotkey registered: mods={:#x} vk={:#x}", toggle_mods, toggle_vk);
                    } else {
                        warn!("Toggle hotkey unavailable (mods={:#x} vk={:#x}) — already in use by another app",
                            toggle_mods, toggle_vk);
                    }
                }

                info!("Global hotkeys registered (reload={:#x}+{:#x}, toggle={:#x}+{:#x})",
                    reload_mods, reload_vk, toggle_mods, toggle_vk);

                // Message loop
                let mut msg = MSG::default();
                while running.load(Ordering::SeqCst) {
                    let ret = PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE);
                    if ret.as_bool() {
                        if msg.message == WM_HOTKEY {
                            match msg.wParam.0 as i32 {
                                Self::HOTKEY_RELOAD => on_reload(),
                                Self::HOTKEY_TOGGLE => on_toggle(),
                                _ => {}
                            }
                        }
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                }

                if reload_ok { let _ = UnregisterHotKey(None, Self::HOTKEY_RELOAD); }
                if toggle_ok { let _ = UnregisterHotKey(None, Self::HOTKEY_TOGGLE); }
            }
        });
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}
