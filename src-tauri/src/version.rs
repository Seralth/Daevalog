//! The version the meter shows, and the one it gives a2tools.app.

/// Shown in Settings and in reports: the release line and, when the build
/// knew it, the revision ("1.0 · r250"). Set by build.rs.
pub const DISPLAY: &str = env!("DAEVALOG_VERSION");

/// Sent to a2tools.app (app and parser version, user agent): the site treats
/// uploads by this version until it accepts a fork marker.
pub const UPLOAD_COMPAT_VERSION: &str = "2.0.44";

/// The HTTP user agent for every request to a2tools.app and its CDN.
pub fn user_agent() -> String {
    format!("A2Tools-DPS-Meter/{UPLOAD_COMPAT_VERSION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a2tools_app_sees_is_unchanged() {
        assert_eq!(UPLOAD_COMPAT_VERSION, "2.0.44");
        assert_eq!(user_agent(), "A2Tools-DPS-Meter/2.0.44");
    }

    #[test]
    fn the_shown_version_is_the_release_line() {
        let line = concat!(env!("CARGO_PKG_VERSION_MAJOR"), ".", env!("CARGO_PKG_VERSION_MINOR"));
        assert!(DISPLAY == line || DISPLAY.starts_with(&format!("{line} · r")), "{DISPLAY}");
    }
}
