//! The version the meter shows, and the one it gives a2tools.app.

/// Shown in Settings and in reports: the release line and, when the build
/// knew it, the revision ("1.0 · r250"). Set by build.rs.
pub const DISPLAY: &str = env!("DAEVALOG_VERSION");

static RELEASE: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

/// The release line a front end other than the 1.x app shows and sends
/// ("2.0"), set once at its start.
pub fn set_release(line: &'static str) {
    let _ = RELEASE.set(line);
}

/// The version as the player sees it: `DISPLAY`, or with the release line
/// a front end set ("2.0 · r250").
pub fn display() -> String {
    match (RELEASE.get(), DISPLAY.split_once(" · ")) {
        (Some(line), Some((_, revision))) => format!("{line} · {revision}"),
        (Some(line), None) => line.to_string(),
        (None, _) => DISPLAY.to_string(),
    }
}

/// The A2Tools release the parser started from. Kept as the parser version
/// in evidence slices and saved fights.
pub const UPLOAD_COMPAT_VERSION: &str = "2.0.44";

/// The meter's name on a2tools.app (approved for every version in upstream
/// issue #37). Sent with every upload.
pub const UPLOAD_CLIENT: &str = "daevalog";

/// The version a2tools.app shows for an upload: "1.1 r593".
pub fn upload_version() -> String {
    display().replace(" · ", " ")
}

/// The HTTP user agent for every request to a2tools.app.
pub fn user_agent() -> String {
    format!("Daevalog/{}", upload_version().replace(' ', "-"))
}

#[cfg(test)]
mod tests {
    // `set_release` is tested in tests/release_line.rs, in a process of its own.
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
