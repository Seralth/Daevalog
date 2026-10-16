//! What the Windows system is: its version and clock format.

use windows::core::PCWSTR;
use windows::Win32::Globalization::{GetLocaleInfoEx, LOCALE_STIMEFORMAT};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

use super::{from_wide, wide};

/// The display backend, for a bug report. The Qt meter reports its own
/// platform (`src-qml`).
pub fn display_backend() -> Option<String> {
    None
}

/// Windows has no Wayland compositor.
pub fn compositor_has_layer_shell() -> bool {
    false
}

const VERSION_KEY: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";

/// A text value under HKLM\`VERSION_KEY`.
fn version_value(name: &str) -> Option<String> {
    let (key, value) = (wide(VERSION_KEY), wide(name));
    let mut buffer = [0u16; 128];
    let mut bytes = (buffer.len() * 2) as u32;
    // SAFETY: the buffer is writable for `bytes` bytes; the names are
    // NUL-terminated and live for the call.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    status.is_ok().then(|| from_wide(&buffer)).filter(|text| !text.trim().is_empty())
}

/// "Windows 11 23H2 (build 22631)", from the registry.
pub fn system_name() -> String {
    let build = version_value("CurrentBuildNumber").and_then(|b| b.trim().parse().ok());
    windows_name(build, version_value("DisplayVersion").as_deref())
}

/// Windows 11 still says "Windows 10" in ProductName; its builds start at 22000.
fn windows_name(build: Option<u32>, release: Option<&str>) -> String {
    let Some(build) = build else { return "Windows".to_string() };
    let name = if build >= 22_000 { "Windows 11" } else { "Windows 10" };
    match release.map(str::trim).filter(|r| !r.is_empty()) {
        Some(release) => format!("{name} {release} (build {build})"),
        None => format!("{name} (build {build})"),
    }
}

/// Whether the user's time format is a 24-hour clock, from Region settings.
pub fn clock_24h() -> Option<bool> {
    let mut buffer = [0u16; 80];
    // SAFETY: the buffer is writable and its length is passed with it; a
    // null locale name is the user's default.
    let len = unsafe { GetLocaleInfoEx(PCWSTR::null(), LOCALE_STIMEFORMAT, Some(&mut buffer)) };
    (len > 0).then(|| writes_24h(&from_wide(&buffer)))
}

/// A Windows time format ("HH:mm:ss", "h:mm:ss tt") is a 24-hour clock
/// unless it has an `h` (12-hour) or a `t` (AM/PM) outside quoted text.
fn writes_24h(format: &str) -> bool {
    let mut quoted = false;
    for c in format.chars() {
        match c {
            '\'' => quoted = !quoted,
            'h' | 't' if !quoted => return false,
            _ => {}
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_the_build() {
        assert_eq!(windows_name(Some(22631), Some("23H2")), "Windows 11 23H2 (build 22631)");
        assert_eq!(windows_name(Some(19045), Some("22H2")), "Windows 10 22H2 (build 19045)");
        assert_eq!(windows_name(Some(26100), None), "Windows 11 (build 26100)");
        assert_eq!(windows_name(None, Some("23H2")), "Windows");
    }

    #[test]
    fn this_system_has_a_name() {
        assert!(system_name().starts_with("Windows"));
    }

    #[test]
    fn twelve_hour_fields_mark_the_format() {
        for format in ["h:mm:ss tt", "tt h:mm:ss", "hh:mm"] {
            assert!(!writes_24h(format), "{format}");
        }
        for format in ["HH:mm:ss", "H:mm", "HH'h'mm", "H' t 'mm", ""] {
            assert!(writes_24h(format), "{format}");
        }
    }

    #[test]
    fn display_and_layer_shell_are_none() {
        assert_eq!(display_backend(), None);
        assert!(!compositor_has_layer_shell());
        let _ = clock_24h();
    }
}
