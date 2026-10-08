//! The version the meter shows, and the one it gives a2tools.app.

/// Shown in Settings and in reports: the release line and, when the build
/// knew it, the revision ("1.0 · r250"). Set by build.rs.
pub const DISPLAY: &str = env!("DAEVALOG_VERSION");

/// The A2Tools release the parser started from. Kept as the parser version
/// in evidence slices and saved fights.
pub const UPLOAD_COMPAT_VERSION: &str = "2.0.44";

/// The meter's name on a2tools.app (approved for every version in upstream
/// issue #37). Sent with every upload and dev-log registration.
pub const UPLOAD_CLIENT: &str = "daevalog";

/// The version a2tools.app shows for an upload: "1.1 r593".
pub fn upload_version() -> String {
    DISPLAY.replace(" · ", " ")
}

/// The HTTP user agent for every request to a2tools.app and its CDN.
pub fn user_agent() -> String {
    format!("Daevalog/{}", upload_version().replace(' ', "-"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a2tools_app_sees() {
        assert_eq!(UPLOAD_COMPAT_VERSION, "2.0.44");
        assert_eq!(UPLOAD_CLIENT, "daevalog");
        assert!(!upload_version().contains('·'), "{}", upload_version());
        assert!(user_agent().starts_with("Daevalog/") && !user_agent().contains(' '), "{}", user_agent());
    }

    #[test]
    fn the_shown_version_is_the_release_line() {
        let line = concat!(env!("CARGO_PKG_VERSION_MAJOR"), ".", env!("CARGO_PKG_VERSION_MINOR"));
        assert!(DISPLAY == line || DISPLAY.starts_with(&format!("{line} · r")), "{DISPLAY}");
    }
}
