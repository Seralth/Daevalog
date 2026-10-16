//! What the Linux system is: compositor, distribution, clock format.

/// Reads one environment variable; empty counts as unset.
type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

/// The process environment as an `Env`. A value that is not Unicode counts
/// as unset too.
fn real_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// The display backend, for a bug report. The Qt meter picks its platform
/// itself (`src-qml`), so the backend has none to report.
pub fn display_backend() -> Option<String> {
    None
}

/// Whether the Wayland compositor offers zwlr_layer_shell_v1. Asks its
/// registry over the socket, before GTK connects; `false` when it cannot ask.
/// The Qt meter asks too, before Qt connects.
pub fn compositor_has_layer_shell() -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An environment of fixed values; an empty value is unset, as for real.
    fn fake_env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| pairs.iter().find(|(k, v)| *k == name && !v.is_empty()).map(|(_, v)| v.to_string())
    }

    /// One Wayland event: object id, (size << 16) | opcode, then the body.
    fn event(object: u32, opcode: u32, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&object.to_ne_bytes());
        out.extend_from_slice(&((((body.len() + 8) as u32) << 16) | opcode).to_ne_bytes());
        out.extend_from_slice(body);
        out
    }

    /// wl_registry.global on object 2: name, interface string, version.
    fn global(name: u32, interface: &str) -> Vec<u8> {
        let mut body = name.to_ne_bytes().to_vec();
        body.extend_from_slice(&(interface.len() as u32 + 1).to_ne_bytes());
        body.extend_from_slice(interface.as_bytes());
        body.push(0);
        body.resize(body.len().next_multiple_of(4), 0);
        body.extend_from_slice(&1u32.to_ne_bytes());
        event(2, 0, &body)
    }

    #[test]
    fn distribution_name_from_os_release() {
        let fedora = "NAME=\"Fedora Linux\"\nVERSION=\"40 (KDE Plasma)\"\nID=fedora\nVERSION_ID=40\n\
                      PRETTY_NAME=\"Fedora Linux 40 (KDE Plasma)\"\n";
        assert_eq!(os_release_name(fedora), "Fedora Linux 40");
        assert_eq!(os_release_name("NAME=\"CachyOS Linux\"\nID=cachyos\nBUILD_ID=rolling\n"), "CachyOS Linux");
        assert_eq!(os_release_name(""), "Linux");
    }

    #[test]
    fn twelve_hour_fields_mark_the_format() {
        for format in ["%r", "%I:%M:%S %p", "%l:%M:%S %P"] {
            assert!(!writes_24h(format), "{format}");
        }
        for format in ["%T", "%H:%M:%S", "%H時%M分%S秒", "%H시 %M분 %S초", ""] {
            assert!(writes_24h(format), "{format}");
        }
    }

    #[test]
    fn locale_for_times_follows_libc_order() {
        let pick = |pairs| time_locale(&fake_env(pairs));
        assert_eq!(pick(&[("LANG", "en_US.UTF-8"), ("LC_TIME", "en_GB.UTF-8")]).as_deref(), Some("en_GB.UTF-8"));
        assert_eq!(pick(&[("LC_ALL", "de_DE.UTF-8"), ("LC_TIME", "en_US.UTF-8")]).as_deref(), Some("de_DE.UTF-8"));
        assert_eq!(pick(&[("LANG", "ko_KR.UTF-8"), ("LC_TIME", "")]).as_deref(), Some("ko_KR.UTF-8"));
        assert_eq!(pick(&[("LANG", "C.UTF-8")]), None);
        assert_eq!(pick(&[("LC_TIME", "POSIX")]), None);
        assert_eq!(pick(&[]), None);
    }

    #[test]
    fn unset_real_variable_gives_no_locale() {
        // A name nothing else sets; std::env::set_var would race other tests.
        let name = "DAEVALOG_SURELY_UNSET_VARIABLE";
        assert_eq!(real_env(name), None);
        let only_lc_time = |var: &str| if var == "LC_TIME" { real_env(name) } else { None };
        assert_eq!(time_locale(&only_lc_time), None);
    }

    #[test]
    fn display_backend_is_none() {
        assert_eq!(display_backend(), None);
    }

    #[test]
    fn clock_from_installed_locales() {
        assert_eq!(locale_clock_24h(c"C.UTF-8"), Some(true));
        if let Some(us) = locale_clock_24h(c"en_US.UTF-8") {
            assert!(!us);
        }
        assert_eq!(locale_clock_24h(c"xx_NOWHERE.UTF-8"), None);
    }

    #[test]
    fn layer_shell_known_once_the_sync_is_done() {
        let mut reply = [global(1, "wl_compositor"), global(2, "zwlr_layer_shell_v1"), global(3, "xdg_wm_base")].concat();
        assert_eq!(registry_has_layer_shell(&reply), None);
        let done = event(3, 0, &0u32.to_ne_bytes());
        reply.extend_from_slice(&done[..6]);
        assert_eq!(registry_has_layer_shell(&reply), None);
        reply.extend_from_slice(&done[6..]);
        assert_eq!(registry_has_layer_shell(&reply), Some(true));

        let other = [global(1, "wl_compositor"), global(2, "zwlr_layer_shell"), done].concat();
        assert_eq!(registry_has_layer_shell(&other), Some(false));

        assert_eq!(registry_has_layer_shell(&event(1, 0, &[0; 12])), Some(false));
    }
}
