//! A front end's own release line. `set_release` is once per process, so this
//! runs in its own: in the lib tests it would change the version every other
//! test reads.

use a2tools_dps_meter_lib::version::{display, set_release, upload_version, user_agent, DISPLAY};

#[test]
fn a_front_end_shows_and_sends_its_own_release_line() {
    // Unset, as in the 1.x app: the build's line.
    assert_eq!(display(), DISPLAY);
    set_release("2.0");
    let revision = DISPLAY.split_once(" · ").map(|(_, r)| r);
    assert_eq!(display(), revision.map_or("2.0".to_string(), |r| format!("2.0 · {r}")));
    assert!(upload_version().starts_with("2.0") && !upload_version().contains('·'));
    assert!(user_agent().starts_with("Daevalog/2.0"));
}
