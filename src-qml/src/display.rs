// SPDX-License-Identifier: AGPL-3.0-only

//! Which desktop the meter runs on, for the few places where compositors
//! differ in ways Qt does not hide, and the Qt platform it runs on.

use std::sync::OnceLock;

/// The desktops the meter treats differently. The rest are `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desktop {
    Kde,
    Hyprland,
    Sway,
    Cosmic,
    Gnome,
    Other,
}

/// Reads one environment variable; empty counts as unset.
type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

fn real_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// This session's desktop.
pub fn desktop() -> Desktop {
    detect(&real_env)
}

/// From XDG_CURRENT_DESKTOP; a compositor started from a console may leave it
/// unset, and then its own socket variable names it. The same rule as the
/// 1.x meter's.
fn detect(env: Env) -> Desktop {
    let Some(names) = env("XDG_CURRENT_DESKTOP") else {
        return if env("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            Desktop::Hyprland
        } else if env("SWAYSOCK").is_some() {
            Desktop::Sway
        } else if env("KDE_FULL_SESSION").is_some() {
            Desktop::Kde
        } else {
            Desktop::Other
        };
    };
    for name in names.split(':') {
        for (known, desktop) in [
            ("kde", Desktop::Kde),
            ("hyprland", Desktop::Hyprland),
            ("sway", Desktop::Sway),
            ("cosmic", Desktop::Cosmic),
            ("gnome", Desktop::Gnome),
        ] {
            if name.eq_ignore_ascii_case(known) {
                return desktop;
            }
        }
    }
    Desktop::Other
}

static LAYER_SHELL: OnceLock<bool> = OnceLock::new();

/// Picks Qt's platform; first thing in `main`, before Qt starts. A Wayland
/// compositor without layer shell (GNOME) keeps no normal window above a
/// fullscreen one, while an X11 window with stay-on-top stays above, so
/// there the meter runs under XWayland. A platform the player chose
/// (QT_QPA_PLATFORM or -platform) wins.
#[cfg(not(windows))]
pub fn choose_platform(args: &[String]) {
    use daevalog::platform::process;
    let offered = real_env("WAYLAND_DISPLAY").is_some() && process::compositor_has_layer_shell();
    let _ = LAYER_SHELL.set(offered);
    if let Some(platform) = platform_for(&real_env, args, offered) {
        // SAFETY: called before Qt or any other thread starts.
        unsafe { std::env::set_var("QT_QPA_PLATFORM", platform) };
    }
}

/// Windows has one platform, Qt's own "windows", and no layer shell.
#[cfg(windows)]
pub fn choose_platform(_args: &[String]) {
    let _ = LAYER_SHELL.set(false);
}

#[cfg_attr(windows, allow(dead_code))]
fn platform_for(env: Env, args: &[String], layer_shell: bool) -> Option<&'static str> {
    let chosen = env("QT_QPA_PLATFORM").is_some() || args.iter().any(|arg| arg == "-platform");
    let xwayland = env("WAYLAND_DISPLAY").is_some() && env("DISPLAY").is_some();
    (!chosen && xwayland && !layer_shell).then_some("xcb")
}

/// Whether the Wayland compositor offers layer shell, as `choose_platform`
/// found; `false` without Wayland.
pub fn compositor_has_layer_shell() -> bool {
    LAYER_SHELL.get().copied().unwrap_or(false)
}

/// Whether the meter is a layer surface while the player has not chosen:
/// on Hyprland and Sway, which tile a normal window, and on KDE, where
/// the layer works over the game (clicks, lock and click-through). Only
/// where the compositor offers layer shell, so KDE on X11 keeps its window.
pub fn layer_shell_by_default() -> bool {
    matches!(desktop(), Desktop::Hyprland | Desktop::Sway | Desktop::Kde) && compositor_has_layer_shell()
}

/// Whether the meter is a window the X11 window manager does not manage
/// (override-redirect). KWin keeps an active fullscreen window above every
/// window it manages, "keep above" ones too, but stacks all of them below
/// the windows it does not manage, so on KDE X11 only those stay above a
/// fullscreen game. Such a window moves and resizes itself.
pub fn x11_unmanaged(platform: &str) -> bool {
    platform == "xcb" && desktop() == Desktop::Kde
}

/// Whether the compositor reports the pointer, while a button is held,
/// from where the window was at the press rather than from where it is now.
/// Sway does (its seatop_down adds the pointer's travel to the press
/// position), so a layer drag there is measured from its start.
pub fn pointer_from_press() -> bool {
    desktop() == Desktop::Sway
}

/// Whether a layer drag takes the pointer from the compositor's socket:
/// on Hyprland, which measures it from where its layer animation is while a
/// button is held (see `hyprland.rs`).
pub fn pointer_from_socket() -> bool {
    desktop() == Desktop::Hyprland
}

/// Whether a popup of the layer-shell meter is left behind when the meter
/// moves: Hyprland moves a layer surface with its layer animation and its
/// popups only to where the surface was at their last commit, so a still
/// popup stops part of the way. Shown again, it goes to its place.
pub fn layer_popups_lag() -> bool {
    desktop() == Desktop::Hyprland
}

/// Whether the layer-shell meter takes the keys while one of its menus is
/// open. Sway gives a popup's keys only to its parent's surface, and the
/// meter's layer surface takes none (it must not take them from the game),
/// so there the meter asks for them until the menu closes. KWin, Hyprland
/// and COSMIC give them to the popup itself.
pub fn layer_menu_takes_keys() -> bool {
    desktop() == Desktop::Sway
}

/// Whether a hidden normal Wayland window is also minimized: on KDE and
/// COSMIC, whose trays hand the meter an activation token, and activation
/// brings a minimized window back. Minimized, nothing of it is drawn
/// (COSMIC outlines every window, drawn or not) and it leaves the focus
/// chain.
pub fn minimize_when_hidden() -> bool {
    matches!(desktop(), Desktop::Kde | Desktop::Cosmic)
}

/// Whether the meter keeps out of the taskbar and the window list: on GNOME
/// under XWayland. There each show maps a window GNOME has not seen; it
/// does not focus it, and when it overlaps the focused window it marks it
/// as wanting attention and posts '"…" is ready', unless the window skips
/// the taskbar. Asking for no focus (user time 0) does not stop that.
pub fn off_taskbar(platform: &str) -> bool {
    platform == "xcb" && desktop() == Desktop::Gnome
}

/// Whether "Keep out of the taskbar" can act: on X11 only, where a window
/// asks the window manager to skip the taskbar. Wayland lets no app ask, so
/// there Settings hides it.
pub fn taskbar_setting(platform: &str) -> bool {
    platform == "xcb"
}

/// Whether "Overlay: Wayland layer" can act: where Qt runs on Wayland. On
/// X11 (KDE X11, GNOME's XWayland meter) there is no layer surface to be.
pub fn layer_setting(platform: &str) -> bool {
    platform.starts_with("wayland")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        |name| vars.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
    }

    #[test]
    fn desktops_by_name_or_socket() {
        assert_eq!(detect(&env_of(&[("XDG_CURRENT_DESKTOP", "KDE")])), Desktop::Kde);
        assert_eq!(detect(&env_of(&[("XDG_CURRENT_DESKTOP", "sway")])), Desktop::Sway);
        assert_eq!(detect(&env_of(&[("XDG_CURRENT_DESKTOP", "Hyprland")])), Desktop::Hyprland);
        assert_eq!(detect(&env_of(&[("XDG_CURRENT_DESKTOP", "ubuntu:GNOME")])), Desktop::Gnome);
        assert_eq!(detect(&env_of(&[("XDG_CURRENT_DESKTOP", "XFCE")])), Desktop::Other);
        assert_eq!(detect(&env_of(&[("XDG_CURRENT_DESKTOP", "COSMIC")])), Desktop::Cosmic);
        assert_eq!(detect(&env_of(&[("SWAYSOCK", "/run/user/1000/sway-ipc.sock")])), Desktop::Sway);
        assert_eq!(detect(&env_of(&[("HYPRLAND_INSTANCE_SIGNATURE", "abc")])), Desktop::Hyprland);
        assert_eq!(detect(&env_of(&[])), Desktop::Other);
    }

    #[test]
    fn keep_out_of_the_taskbar_shows_on_x11_only() {
        assert!(taskbar_setting("xcb"), "X11, and XWayland on GNOME");
        for platform in ["wayland", "wayland-egl", "offscreen", "windows"] {
            assert!(!taskbar_setting(platform), "{platform}");
        }
    }

    #[test]
    fn the_wayland_layer_switch_shows_on_wayland_only() {
        assert!(layer_setting("wayland"));
        assert!(layer_setting("wayland-egl"));
        for platform in ["xcb", "offscreen", "windows"] {
            assert!(!layer_setting(platform), "{platform}");
        }
    }

    #[test]
    fn xwayland_without_layer_shell_unless_the_player_chose() {
        let gnome = [("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")];
        assert_eq!(platform_for(&env_of(&gnome), &[], false), Some("xcb"));
        // Layer shell offered (KDE, Hyprland, Sway, COSMIC): Qt's own choice.
        assert_eq!(platform_for(&env_of(&gnome), &[], true), None);
        // No XWayland to go to.
        assert_eq!(platform_for(&env_of(&[("WAYLAND_DISPLAY", "wayland-0")]), &[], false), None);
        // An X11 session.
        assert_eq!(platform_for(&env_of(&[("DISPLAY", ":0")]), &[], false), None);
        let chosen = [("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0"), ("QT_QPA_PLATFORM", "wayland")];
        assert_eq!(platform_for(&env_of(&chosen), &[], false), None);
        let args = ["daevalog-qml".to_string(), "-platform".to_string(), "wayland".to_string()];
        assert_eq!(platform_for(&env_of(&gnome), &args, false), None);
    }
}
