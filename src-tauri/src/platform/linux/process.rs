//! Setup for the whole process, before the UI starts.
//!
//! WebKitGTK's DMA-BUF renderer hands its frames to the compositor as GPU
//! buffers, and some setups reject them: on NVIDIA under Wayland the window
//! never opens ("Error 71 (Protocol error) dispatching to Wayland display",
//! issue #7), and under XWayland GBM cannot allocate them ("Failed to create
//! GBM buffer"). 2.0.38 turned the renderer off for that. From WebKitGTK 2.54
//! a window drawn without it is mostly blank: only parts of the transparent
//! overlay paint (issue #8). Keeping the renderer and having it hand frames
//! over in shared memory avoids both. Tested on WebKitGTK 2.52.6 and 2.54.1,
//! AMD and NVIDIA, Wayland and XWayland. A value the player set themselves
//! for either variable is left alone.
//!
//! The display backend is chosen here too, before GTK opens the display:
//! a GDK_BACKEND the player set wins; an X11 session uses X11; GNOME uses
//! XWayland, where its keep-above works; KDE Plasma, Hyprland and Sway use
//! native Wayland with the overlay as a layer surface, the only way it stays
//! above a fullscreen game there. The layer setting is on by default on those
//! three. Turned off, KDE Plasma gets XWayland (KWin keeps an X11 window on
//! top); Hyprland and Sway keep a normal Wayland window. Other Wayland
//! desktops keep GTK's default, with the layer only when turned on.
//! COSMIC's session script exports GDK_BACKEND=wayland,x11 for every app, so
//! that value on COSMIC is the session's, not the player's.

use std::sync::OnceLock;

const DISABLE: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";
const FORCE_SHM: &str = "WEBKIT_DMABUF_RENDERER_FORCE_SHM";
/// The Settings switch for the layer overlay: "true", "false", or unset.
const LAYER_KEY: &str = "dpsMeter.waylandLayer";

/// Reads one environment variable; empty counts as unset.
type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

fn real_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[cfg(feature = "desktop")]
pub(crate) fn is_gnome() -> bool {
    desktop(&real_env) == Desktop::Gnome
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Session {
    X11,
    Wayland,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Desktop {
    Kde,
    Gnome,
    Hyprland,
    Sway,
    Cosmic,
    Other,
}

impl Desktop {
    fn name(self) -> &'static str {
        match self {
            Desktop::Kde => "KDE",
            Desktop::Gnome => "GNOME",
            Desktop::Hyprland => "Hyprland",
            Desktop::Sway => "Sway",
            Desktop::Cosmic => "COSMIC",
            Desktop::Other => "other desktop",
        }
    }
}

fn session(env: Env) -> Session {
    let kind = env("XDG_SESSION_TYPE").unwrap_or_default().to_ascii_lowercase();
    if kind == "x11" {
        Session::X11
    } else if kind == "wayland" || env("WAYLAND_DISPLAY").is_some() {
        Session::Wayland
    } else if env("DISPLAY").is_some() {
        Session::X11
    } else {
        Session::Unknown
    }
}

/// From XDG_CURRENT_DESKTOP; a compositor started from a console may leave it
/// unset, and then its own socket variable names it.
fn desktop(env: Env) -> Desktop {
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
            ("gnome", Desktop::Gnome),
            ("hyprland", Desktop::Hyprland),
            ("sway", Desktop::Sway),
            ("cosmic", Desktop::Cosmic),
        ] {
            if name.eq_ignore_ascii_case(known) {
                return desktop;
            }
        }
    }
    Desktop::Other
}

/// The GDK_BACKEND a desktop's session sets for every app. COSMIC's
/// start-cosmic exports `wayland,x11`.
fn session_backend(desktop: Desktop) -> Option<&'static str> {
    match desktop {
        Desktop::Cosmic => Some("wayland,x11"),
        _ => None,
    }
}

/// What the meter does with the display.
#[derive(Debug, PartialEq, Eq)]
struct Plan {
    /// Set GDK_BACKEND to this; `None` leaves it as it is.
    backend: Option<&'static str>,
    /// Make the overlay a layer surface.
    layer: bool,
    /// The layer switch's position when the player never set it.
    layer_by_default: bool,
    /// For the log: "display backend: ...".
    note: String,
}

/// The backend for this environment. `setting` is the saved layer switch;
/// `layer_ready`: gtk-layer-shell loads and the compositor offers
/// zwlr_layer_shell_v1.
fn plan(env: Env, setting: Option<&str>, layer_ready: bool) -> Plan {
    let session = session(env);
    let desktop = desktop(env);
    let x11_available = env("DISPLAY").is_some();
    let layer_by_default = session == Session::Wayland && matches!(desktop, Desktop::Kde | Desktop::Hyprland | Desktop::Sway);
    let layer_on = match setting {
        Some("true") => true,
        Some("false") => false,
        _ => layer_by_default,
    };
    let name = desktop.name();
    let make = |backend, layer, note: String| Plan { backend, layer, layer_by_default, note };
    let gdk_backend = env("GDK_BACKEND");
    let from_session = gdk_backend.as_deref().is_some_and(|b| Some(b) == session_backend(desktop));
    if let Some(backend) = gdk_backend.as_ref().filter(|_| !from_session) {
        // GTK tries the listed backends in order; X11 first gets no layer.
        let wayland_first = !backend.trim_start().starts_with("x11");
        return make(None, layer_on && wayland_first, format!("GDK_BACKEND={backend} (set by the player)"));
    }
    let mut plan = match (session, desktop) {
        (Session::X11, _) => make(Some("x11"), false, "x11 (X11 session)".into()),
        (Session::Unknown, _) => make(None, layer_on, "GTK's choice (no X11 or Wayland session found)".into()),
        (Session::Wayland, Desktop::Gnome) if x11_available => make(Some("x11,wayland"), false, "xwayland (GNOME)".into()),
        (Session::Wayland, Desktop::Gnome) => make(None, false, "wayland (GNOME, no XWayland)".into()),
        (Session::Wayland, Desktop::Kde | Desktop::Hyprland | Desktop::Sway) => {
            let why = if !layer_on { "layer overlay off" } else { "no layer-shell" };
            if layer_on && layer_ready {
                make(None, true, format!("wayland + layer overlay ({name})"))
            } else if desktop == Desktop::Kde && x11_available {
                make(Some("x11,wayland"), false, format!("xwayland ({name}, {why})"))
            } else {
                make(None, false, format!("wayland, normal window ({name}, {why})"))
            }
        }
        (Session::Wayland, Desktop::Cosmic | Desktop::Other) => {
            let note = if layer_on { format!("wayland + layer overlay if offered ({name})") } else { format!("wayland ({name})") };
            make(None, layer_on, note)
        }
    };
    if let (true, Some(backend)) = (from_session, gdk_backend) {
        plan.note = format!("{} [GDK_BACKEND={backend} is the {name} session's default, not the player's]", plan.note);
    }
    plan
}

/// The layer setting as saved, read before Tauri opens the settings. Read the
/// way the settings store reads the file, so the Settings switch shows what
/// this chose.
fn saved_layer_setting() -> Option<String> {
    let path = dirs::data_dir()?.join(crate::migrate::IDENTIFIER).join("settings.json");
    crate::config::settings::read_file(&path).remove(LAYER_KEY)
}

/// Whether the Wayland compositor offers zwlr_layer_shell_v1. Asks its
/// registry over the socket, before GTK connects; `false` when it cannot ask.
fn compositor_has_layer_shell() -> bool {
    use std::io::{Read, Write};
    let Some(display) = std::env::var_os("WAYLAND_DISPLAY").filter(|v| !v.is_empty()) else { return false };
    let path = std::path::PathBuf::from(&display);
    let path = if path.is_absolute() {
        path
    } else {
        match std::env::var_os("XDG_RUNTIME_DIR") {
            Some(dir) => std::path::PathBuf::from(dir).join(path),
            None => return false,
        }
    };
    let Ok(mut socket) = std::os::unix::net::UnixStream::connect(path) else { return false };
    let _ = socket.set_read_timeout(Some(std::time::Duration::from_secs(1)));
    // wl_display.get_registry(new id 2), then wl_display.sync(new id 3).
    let mut request = Vec::new();
    for (opcode, id) in [(1u32, 2u32), (0, 3)] {
        for word in [1, (12 << 16) | opcode, id] {
            request.extend_from_slice(&word.to_ne_bytes());
        }
    }
    if socket.write_all(&request).is_err() {
        return false;
    }
    let mut reply = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match socket.read(&mut chunk) {
            Ok(0) | Err(_) => return false,
            Ok(n) => reply.extend_from_slice(&chunk[..n]),
        }
        if let Some(found) = registry_has_layer_shell(&reply) {
            return found;
        }
    }
}

/// Reads registry events up to the sync's done event. `None` until it came.
fn registry_has_layer_shell(reply: &[u8]) -> Option<bool> {
    let word = |at: usize| reply.get(at..at + 4).map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]));
    let mut found = false;
    let mut at = 0;
    while let (Some(object), Some(size_opcode)) = (word(at), word(at + 4)) {
        let (size, opcode) = ((size_opcode >> 16) as usize, size_opcode & 0xffff);
        if size < 8 {
            return Some(false);
        }
        if at + size > reply.len() {
            return None;
        }
        match (object, opcode) {
            // wl_display.error
            (1, 0) => return Some(false),
            // wl_registry.global: name, interface, version
            (2, 0) => {
                let len = word(at + 12)? as usize;
                let name = reply.get(at + 16..at + 16 + len.saturating_sub(1))?;
                found |= name == b"zwlr_layer_shell_v1";
            }
            // wl_callback.done
            (3, 0) => return Some(found),
            _ => {}
        }
        at += size;
    }
    None
}

static PLAN: OnceLock<Plan> = OnceLock::new();
static LAYER_OFFERED: OnceLock<bool> = OnceLock::new();

/// Whether the overlay should become a layer surface.
pub fn overlay_layer() -> bool {
    PLAN.get().is_some_and(|plan| plan.layer)
}

/// Whether the layer switch is on when the player never set it.
pub fn overlay_layer_by_default() -> bool {
    PLAN.get().is_some_and(|plan| plan.layer_by_default)
}

/// Whether the session could have a layer overlay, whatever backend the meter
/// runs on now.
#[cfg(feature = "desktop")]
pub(crate) fn layer_offered() -> bool {
    LAYER_OFFERED.get().copied().unwrap_or(false)
}

/// What the meter chose for the display (the log's "display backend: ..."),
/// for a bug report.
pub fn display_backend() -> Option<String> {
    PLAN.get().map(|plan| plan.note.clone())
}

/// The distribution, from `/etc/os-release`: `NAME` and `VERSION_ID` only.
pub fn system_name() -> String {
    let text = std::fs::read_to_string("/etc/os-release")
        .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
        .unwrap_or_default();
    os_release_name(&text)
}

fn os_release_name(text: &str) -> String {
    let value = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .map(|v| v.trim().trim_matches('"').trim_matches('\'').to_string())
            .filter(|v| !v.is_empty())
    };
    match (value("NAME"), value("VERSION_ID")) {
        (Some(name), Some(version)) => format!("{name} {version}"),
        (Some(name), None) => name,
        (None, _) => "Linux".to_string(),
    }
}

/// Whether the system writes times of day on a 24-hour clock: the time format
/// of the locale for times (LC_ALL, else LC_TIME, else LANG, as the C library
/// picks it; KDE Plasma's Region settings set LC_TIME). `None` without a
/// locale (C or POSIX) or when that locale is not installed.
pub fn clock_24h() -> Option<bool> {
    let name = std::ffi::CString::new(time_locale(&real_env)?).ok()?;
    locale_clock_24h(&name)
}

/// Whether the installed locale `name` writes a 24-hour clock.
fn locale_clock_24h(name: &std::ffi::CStr) -> Option<bool> {
    // SAFETY: newlocale copies the name and returns a new locale or null;
    // nl_langinfo_l's text stays valid until freelocale, after the copy.
    unsafe {
        let locale = libc::newlocale(libc::LC_TIME_MASK, name.as_ptr(), std::ptr::null_mut());
        if locale.is_null() {
            return None;
        }
        let format = std::ffi::CStr::from_ptr(libc::nl_langinfo_l(libc::T_FMT, locale))
            .to_string_lossy()
            .into_owned();
        libc::freelocale(locale);
        Some(writes_24h(&format))
    }
}

/// The locale that sets the time format, unless it is C or POSIX: those are
/// no one's choice.
fn time_locale(env: Env) -> Option<String> {
    let name = ["LC_ALL", "LC_TIME", "LANG"].iter().find_map(|var| env(var))?;
    let language = name.split(['.', '@']).next().unwrap_or_default();
    (!matches!(language, "C" | "POSIX")).then_some(name)
}

/// A time format (`T_FMT`, e.g. `%r` in en_US, `%T` in de_DE) writes a
/// 24-hour clock unless it has a 12-hour field or an AM/PM mark.
fn writes_24h(format: &str) -> bool {
    !["%r", "%I", "%l", "%p", "%P"].iter().any(|field| format.contains(field))
}

/// Whether gtk-layer-shell loads. The layer is the Tauri window's, so without
/// that window there is none to offer.
#[cfg(feature = "desktop")]
fn layer_library_loads() -> bool {
    super::window::layer_library_loads()
}

#[cfg(not(feature = "desktop"))]
fn layer_library_loads() -> bool {
    false
}

/// Returns a note for the log.
pub fn prepare() -> Option<String> {
    let mut notes = Vec::new();
    let offered = session(&real_env) == Session::Wayland
        && compositor_has_layer_shell()
        && layer_library_loads();
    let _ = LAYER_OFFERED.set(offered);
    let plan = plan(&real_env, saved_layer_setting().as_deref(), offered);
    notes.push(format!("display backend: {}", plan.note));
    if let Some(backend) = plan.backend {
        // SAFETY: prepare runs before the UI or any worker thread starts.
        unsafe { std::env::set_var("GDK_BACKEND", backend) };
    }
    let _ = PLAN.set(plan);
    if std::env::var_os(DISABLE).is_none() && std::env::var_os(FORCE_SHM).is_none() {
        // SAFETY: prepare runs before the UI or any worker thread starts.
        unsafe { std::env::set_var(FORCE_SHM, "1") };
        notes.push(format!("{FORCE_SHM}=1 (set by the meter; set {FORCE_SHM}=0 to hand frames over as GPU buffers)"));
    }
    Some(notes.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_name_is_name_and_version_id_only() {
        let fedora = "NAME=\"Fedora Linux\"\nVERSION=\"40 (KDE Plasma)\"\nID=fedora\nVERSION_ID=40\nPRETTY_NAME=\"Fedora Linux 40 (KDE Plasma)\"\n";
        assert_eq!(os_release_name(fedora), "Fedora Linux 40");
        assert_eq!(os_release_name("NAME=\"CachyOS Linux\"\nID=cachyos\nBUILD_ID=rolling\n"), "CachyOS Linux");
        assert_eq!(os_release_name(""), "Linux");
    }

    #[test]
    fn the_time_format_tells_a_24_hour_clock() {
        // glibc's T_FMT: en_US, en_AU, de_DE and en_GB, ja_JP, ko_KR, C.
        for format in ["%r", "%I:%M:%S %p", "%l:%M:%S %P"] {
            assert!(!writes_24h(format), "{format}");
        }
        for format in ["%T", "%H:%M:%S", "%H時%M分%S秒", "%H시 %M분 %S초", ""] {
            assert!(writes_24h(format), "{format}");
        }
    }

    #[test]
    fn the_time_locale_is_lc_all_then_lc_time_then_lang() {
        let locale = |vars: &[(&str, &str)]| time_locale(&env_of(vars));
        assert_eq!(locale(&[("LANG", "en_US.UTF-8"), ("LC_TIME", "en_GB.UTF-8")]).as_deref(), Some("en_GB.UTF-8"));
        assert_eq!(locale(&[("LC_ALL", "de_DE.UTF-8"), ("LC_TIME", "en_US.UTF-8")]).as_deref(), Some("de_DE.UTF-8"));
        assert_eq!(locale(&[("LANG", "ko_KR.UTF-8"), ("LC_TIME", "")]).as_deref(), Some("ko_KR.UTF-8"));
        assert_eq!(locale(&[("LANG", "C.UTF-8")]), None, "C is no one's choice");
        assert_eq!(locale(&[("LC_TIME", "POSIX")]), None);
        assert_eq!(locale(&[]), None);
    }

    #[test]
    fn an_installed_locale_gives_its_clock() {
        // Only the locales the test machine has: C.UTF-8 is in every glibc.
        assert_eq!(locale_clock_24h(c"C.UTF-8"), Some(true));
        if let Some(us) = locale_clock_24h(c"en_US.UTF-8") {
            assert!(!us, "en_US writes 12-hour times");
        }
        assert_eq!(locale_clock_24h(c"xx_NOWHERE.UTF-8"), None);
    }

    fn env_of<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| vars.iter().find(|(k, v)| *k == name && !v.is_empty()).map(|(_, v)| v.to_string())
    }

    /// A Wayland session with XWayland, as a display manager starts it.
    fn wayland(desktop: &'static str) -> Vec<(&'static str, &'static str)> {
        vec![
            ("XDG_SESSION_TYPE", "wayland"),
            ("WAYLAND_DISPLAY", "wayland-0"),
            ("DISPLAY", ":0"),
            ("XDG_CURRENT_DESKTOP", desktop),
        ]
    }

    fn with(mut vars: Vec<(&'static str, &'static str)>, extra: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
        for (name, value) in extra {
            vars.retain(|(k, _)| k != name);
            vars.push((name, value));
        }
        vars
    }

    fn decide(vars: &[(&str, &str)], setting: Option<&str>, layer_ready: bool) -> (Option<&'static str>, bool, bool) {
        let p = plan(&env_of(vars), setting, layer_ready);
        (p.backend, p.layer, p.layer_by_default)
    }

    #[test]
    fn gnome_detection_matches_desktop_components_only() {
        for name in ["GNOME", "gnome", "ubuntu:GNOME", "GNOME-Classic:GNOME"] {
            assert_eq!(desktop(&env_of(&[("XDG_CURRENT_DESKTOP", name)])), Desktop::Gnome, "{name}");
        }
        for name in ["", "KDE", "Hyprland", "sway", "i3", "X-Cinnamon", "not-gnome", "COSMIC"] {
            assert_ne!(desktop(&env_of(&[("XDG_CURRENT_DESKTOP", name)])), Desktop::Gnome, "{name}");
        }
    }

    #[test]
    fn session_and_desktop_come_from_the_environment() {
        assert_eq!(session(&env_of(&[("XDG_SESSION_TYPE", "x11"), ("DISPLAY", ":0")])), Session::X11);
        assert_eq!(session(&env_of(&[("XDG_SESSION_TYPE", "wayland")])), Session::Wayland);
        // Sway or Hyprland started from a console: the session type is "tty".
        assert_eq!(session(&env_of(&[("XDG_SESSION_TYPE", "tty"), ("WAYLAND_DISPLAY", "wayland-1")])), Session::Wayland);
        assert_eq!(session(&env_of(&[("XDG_SESSION_TYPE", "tty"), ("DISPLAY", ":0")])), Session::X11);
        assert_eq!(session(&env_of(&[])), Session::Unknown);

        assert_eq!(desktop(&env_of(&[("XDG_CURRENT_DESKTOP", "KDE")])), Desktop::Kde);
        assert_eq!(desktop(&env_of(&[("XDG_CURRENT_DESKTOP", "Hyprland")])), Desktop::Hyprland);
        assert_eq!(desktop(&env_of(&[("XDG_CURRENT_DESKTOP", "sway")])), Desktop::Sway);
        assert_eq!(desktop(&env_of(&[("XDG_CURRENT_DESKTOP", "XFCE")])), Desktop::Other);
        assert_eq!(desktop(&env_of(&[("XDG_CURRENT_DESKTOP", "COSMIC")])), Desktop::Cosmic);
        assert_eq!(desktop(&env_of(&[("SWAYSOCK", "/run/user/1000/sway-ipc.sock")])), Desktop::Sway);
        assert_eq!(desktop(&env_of(&[("HYPRLAND_INSTANCE_SIGNATURE", "abc")])), Desktop::Hyprland);
        // A named desktop wins over a socket variable left over from elsewhere.
        assert_eq!(desktop(&env_of(&[("XDG_CURRENT_DESKTOP", "niri"), ("SWAYSOCK", "x")])), Desktop::Other);
    }

    #[test]
    fn kde_hyprland_and_sway_get_the_layer_overlay() {
        for name in ["KDE", "Hyprland", "sway"] {
            assert_eq!(decide(&wayland(name), None, true), (None, true, true), "{name}");
        }
        let p = plan(&env_of(&wayland("KDE")), None, true);
        assert_eq!(p.note, "wayland + layer overlay (KDE)");
        // Started from a console, with only the compositor's socket variable.
        let sway = [("XDG_SESSION_TYPE", "tty"), ("WAYLAND_DISPLAY", "wayland-1"), ("SWAYSOCK", "/run/sway.sock")];
        assert_eq!(decide(&sway, None, true), (None, true, true));
    }

    #[test]
    fn kde_without_the_layer_runs_through_xwayland() {
        // Switched off, or gtk-layer-shell / the protocol missing.
        for (setting, ready) in [(Some("false"), true), (None, false), (Some("true"), false)] {
            assert_eq!(decide(&wayland("KDE"), setting, ready), (Some("x11,wayland"), false, true), "{setting:?} {ready}");
        }
        assert_eq!(plan(&env_of(&wayland("KDE")), Some("false"), true).note, "xwayland (KDE, layer overlay off)");
        // No XWayland: a normal Wayland window.
        let no_x = with(wayland("KDE"), &[("DISPLAY", "")]);
        assert_eq!(decide(&no_x, Some("false"), true), (None, false, true));
    }

    #[test]
    fn hyprland_and_sway_without_the_layer_keep_a_wayland_window() {
        for name in ["Hyprland", "sway"] {
            assert_eq!(decide(&wayland(name), Some("false"), true), (None, false, true), "{name}");
            assert_eq!(decide(&wayland(name), Some("true"), false), (None, false, true), "{name}");
        }
    }

    #[test]
    fn gnome_runs_through_xwayland() {
        let gnome = wayland("ubuntu:GNOME");
        assert_eq!(decide(&gnome, None, false), (Some("x11,wayland"), false, false));
        // The switch does nothing on GNOME: it has no layer-shell.
        assert_eq!(decide(&gnome, Some("true"), false), (Some("x11,wayland"), false, false));
        let no_x = with(gnome, &[("DISPLAY", "")]);
        assert_eq!(decide(&no_x, None, false), (None, false, false));
    }

    #[test]
    fn cosmic_and_other_wayland_desktops_keep_gtk_default() {
        for name in ["COSMIC", "niri", "X-Cinnamon"] {
            // Layer off unless switched on; then on where the compositor offers it.
            assert_eq!(decide(&wayland(name), None, true), (None, false, false), "{name}");
            assert_eq!(decide(&wayland(name), Some("true"), true), (None, true, false), "{name}");
        }
    }

    #[test]
    fn cosmic_session_backend_is_not_the_players() {
        // start-cosmic exports GDK_BACKEND=wayland,x11 for every app.
        let cosmic = with(wayland("COSMIC"), &[("GDK_BACKEND", "wayland,x11")]);
        let p = plan(&env_of(&cosmic), None, true);
        assert_eq!((p.backend, p.layer, p.layer_by_default), (None, false, false));
        assert_eq!(p.note, "wayland (COSMIC) [GDK_BACKEND=wayland,x11 is the COSMIC session's default, not the player's]");
        let p = plan(&env_of(&cosmic), Some("true"), true);
        assert_eq!((p.backend, p.layer), (None, true));
        assert!(p.note.starts_with("wayland + layer overlay if offered (COSMIC) ["), "{}", p.note);
        // Any other value on COSMIC, or the same value elsewhere, is the player's.
        let x11 = with(wayland("COSMIC"), &[("GDK_BACKEND", "x11")]);
        assert_eq!(plan(&env_of(&x11), None, true).note, "GDK_BACKEND=x11 (set by the player)");
        let niri = with(wayland("niri"), &[("GDK_BACKEND", "wayland,x11")]);
        assert_eq!(plan(&env_of(&niri), None, true).note, "GDK_BACKEND=wayland,x11 (set by the player)");
    }

    #[test]
    fn x11_sessions_use_x11() {
        for name in ["XFCE", "X-Cinnamon", "KDE", "GNOME", "i3"] {
            let vars = [("XDG_SESSION_TYPE", "x11"), ("DISPLAY", ":0"), ("XDG_CURRENT_DESKTOP", name)];
            assert_eq!(decide(&vars, Some("true"), false), (Some("x11"), false, false), "{name}");
        }
        assert_eq!(plan(&env_of(&[("DISPLAY", ":0")]), None, false).backend, Some("x11"));
    }

    #[test]
    fn unknown_sessions_are_left_to_gtk() {
        assert_eq!(decide(&[], None, false), (None, false, false));
        assert_eq!(decide(&[("XDG_CURRENT_DESKTOP", "KDE")], None, false), (None, false, false));
        assert_eq!(decide(&[], Some("true"), false), (None, true, false));
    }

    #[test]
    fn the_player_backend_always_wins() {
        let sessions = [
            wayland("KDE"),
            wayland("GNOME"),
            wayland("sway"),
            wayland("COSMIC"),
            vec![("XDG_SESSION_TYPE", "x11"), ("DISPLAY", ":0"), ("XDG_CURRENT_DESKTOP", "XFCE")],
            vec![],
        ];
        for vars in sessions {
            let vars = with(vars, &[("GDK_BACKEND", "x11")]);
            let p = plan(&env_of(&vars), None, true);
            assert_eq!((p.backend, p.layer), (None, false), "{vars:?}");
            assert_eq!(p.note, "GDK_BACKEND=x11 (set by the player)");
        }
        // The layer switch still applies when the player chose Wayland.
        let kde = with(wayland("KDE"), &[("GDK_BACKEND", "wayland")]);
        assert_eq!(decide(&kde, None, true), (None, true, true));
        assert_eq!(decide(&kde, Some("false"), true), (None, false, true));
    }

    fn message(object: u32, opcode: u32, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&object.to_ne_bytes());
        out.extend_from_slice(&((((8 + body.len()) as u32) << 16) | opcode).to_ne_bytes());
        out.extend_from_slice(body);
        out
    }

    fn global(name: u32, interface: &str) -> Vec<u8> {
        let mut body = name.to_ne_bytes().to_vec();
        let len = interface.len() + 1;
        body.extend_from_slice(&(len as u32).to_ne_bytes());
        body.extend_from_slice(interface.as_bytes());
        body.resize(body.len() + (4 - interface.len() % 4), 0);
        body.extend_from_slice(&4u32.to_ne_bytes());
        message(2, 0, &body)
    }

    #[test]
    fn registry_reply_is_read_up_to_the_sync() {
        let mut reply = [global(1, "wl_compositor"), global(2, "zwlr_layer_shell_v1"), global(3, "xdg_wm_base")].concat();
        assert_eq!(registry_has_layer_shell(&reply), None);
        let done = message(3, 0, &0u32.to_ne_bytes());
        // Cut in the middle of the done event: not finished yet.
        reply.extend_from_slice(&done[..6]);
        assert_eq!(registry_has_layer_shell(&reply), None);
        reply.truncate(reply.len() - 6);
        reply.extend_from_slice(&done);
        assert_eq!(registry_has_layer_shell(&reply), Some(true));

        let reply = [global(1, "wl_compositor"), global(2, "zwlr_layer_shell"), message(3, 0, &[0; 4])].concat();
        assert_eq!(registry_has_layer_shell(&reply), Some(false));
        let reply = message(1, 0, &[0; 12]);
        assert_eq!(registry_has_layer_shell(&reply), Some(false));
    }
}
