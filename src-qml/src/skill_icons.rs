// SPDX-License-Identifier: AGPL-3.0-only

//! `SkillIcons`, the QML singleton that names a skill row's icon: Daevalog's
//! own flat set in `skill-icons/`, one SVG per meter row id, compiled in.
//! A row with no icon of its own gets a generic glyph in its class colour.

use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        type SkillIcons = super::SkillIconsRust;

        /// The icon of row `code`, a skill of class `job` (a Theme class key,
        /// or empty): a qrc URL, never empty.
        #[qinvokable]
        fn source(self: &SkillIcons, code: i32, job: &QString) -> QString;
    }
}

#[derive(Default)]
pub struct SkillIconsRust;

// ICONS: the row ids with an SVG; GENERIC: the class keys with a generic one
// (build.rs reads both from skill-icons/).
include!(concat!(env!("OUT_DIR"), "/skill_icons.rs"));

const ROOT: &str = "qrc:/daevalog/skill-icons/";

/// The row's own icon; else its base skill's (the id rounded down to 10000,
/// the meter's own folding of a skill's ranks); else the generic glyph in
/// the class colour, grey for no class.
pub fn source_of(code: i32, job: &str) -> String {
    let base = code - code % 10_000;
    if let Some(id) = [code, base].into_iter().find(|id| ICONS.binary_search(id).is_ok()) {
        return format!("{ROOT}{id}.svg");
    }
    let job = if GENERIC.contains(&job) { job } else { "none" };
    format!("{ROOT}generic/{job}.svg")
}

impl qobject::SkillIcons {
    fn source(&self, code: i32, job: &QString) -> QString {
        QString::from(&source_of(code, &job.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_take_their_own_icon() {
        assert_eq!(source_of(16010000, "spiritmaster"), "qrc:/daevalog/skill-icons/16010000.svg");
        // A spirit's attack is a row of its own.
        assert_eq!(source_of(100011, "spiritmaster"), "qrc:/daevalog/skill-icons/100011.svg");
    }

    #[test]
    fn a_rank_takes_its_base_skills_icon() {
        assert_eq!(source_of(16010007, "spiritmaster"), "qrc:/daevalog/skill-icons/16010000.svg");
    }

    #[test]
    fn unknown_rows_get_the_generic_glyph_in_the_class_colour() {
        // A Theostone proc.
        assert_eq!(source_of(30000201, "cleric"), "qrc:/daevalog/skill-icons/generic/cleric.svg");
        assert_eq!(source_of(19010000, "fighter"), "qrc:/daevalog/skill-icons/generic/fighter.svg");
        assert_eq!(source_of(30000201, ""), "qrc:/daevalog/skill-icons/generic/none.svg");
        assert_eq!(source_of(30000201, "nobody"), "qrc:/daevalog/skill-icons/generic/none.svg");
    }

    #[test]
    fn every_class_of_the_theme_has_a_generic_glyph() {
        let theme = include_str!("../Theme.qml");
        let start = theme.find("classColors: ({").unwrap();
        let block = &theme[start..start + theme[start..].find("})").unwrap()];
        let keys: Vec<&str> = block.split('"').skip(1).step_by(2).filter(|k| !k.starts_with('#')).collect();
        assert_eq!(keys.len(), 9, "{keys:?}");
        for key in keys {
            assert!(GENERIC.contains(&key), "{key}");
        }
    }
}
