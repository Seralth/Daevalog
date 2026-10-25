// SPDX-License-Identifier: AGPL-3.0-only

//! `GearGlyphs`, the QML singleton that names a gear slot's glyph: one flat
//! outline per kind of slot, from `icons/gear/`, compiled in as SVG path
//! data on a 24 px grid. `GearGlyph.qml` draws it in the item's grade colour.

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
        type GearGlyphs = super::GearGlyphsRust;

        /// The outline of slot `slot` (the game's slot name, such as
        /// "MainHand" or "Ring2") as SVG path data, empty for an unknown slot.
        #[qinvokable]
        fn outline(self: &GearGlyphs, slot: &QString) -> QString;
    }
}

#[derive(Default)]
pub struct GearGlyphsRust;

// GLYPHS: each glyph's name and path data (build.rs reads them from icons/gear/).
include!(concat!(env!("OUT_DIR"), "/gear_glyphs.rs"));

/// The game's gear slots, in paper doll order, each with its glyph. Two
/// slots of a kind (Earring1 and Earring2) share one.
pub const SLOTS: [(&str, &str); 20] = [
    ("MainHand", "main-hand"),
    ("SubHand", "off-hand"),
    ("Helmet", "helmet"),
    ("Shoulder", "shoulder"),
    ("Torso", "torso"),
    ("Pants", "pants"),
    ("Gloves", "gloves"),
    ("Boots", "boots"),
    ("Cape", "cape"),
    ("Belt", "belt"),
    ("Necklace", "necklace"),
    ("Earring1", "earring"),
    ("Earring2", "earring"),
    ("Ring1", "ring"),
    ("Ring2", "ring"),
    ("Bracelet1", "bracelet"),
    ("Bracelet2", "bracelet"),
    ("Amulet", "amulet"),
    ("Rune1", "rune"),
    ("Rune2", "rune"),
];

pub fn outline_of(slot: &str) -> &'static str {
    let Some((_, glyph)) = SLOTS.iter().find(|(s, _)| *s == slot) else { return "" };
    GLYPHS.iter().find(|(g, _)| g == glyph).map_or("", |(_, path)| path)
}

impl qobject::GearGlyphs {
    fn outline(&self, slot: &QString) -> QString {
        QString::from(outline_of(&slot.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_slot_has_a_glyph() {
        for (slot, _) in SLOTS {
            assert!(outline_of(slot).starts_with('M'), "{slot}");
        }
        assert_eq!(outline_of("Earring1"), outline_of("Earring2"));
        assert_eq!(outline_of("Pet"), "");
    }

    #[test]
    fn every_glyph_belongs_to_a_slot() {
        for (glyph, _) in GLYPHS {
            assert!(SLOTS.iter().any(|(_, g)| g == glyph), "icons/gear/{glyph}.svg");
        }
    }

    #[test]
    fn every_glyph_names_its_licence_and_lucides_are_listed() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let license = std::fs::read_to_string(root.join("../LICENSE")).unwrap();
        for (glyph, _) in GLYPHS {
            let svg = std::fs::read_to_string(root.join(format!("icons/gear/{glyph}.svg"))).unwrap();
            if svg.contains("<!-- SPDX-License-Identifier: ISC -->") {
                assert!(svg.contains("<!-- Lucide \""), "{glyph}");
                assert!(license.contains(&format!("icons/gear/{glyph}.svg")), "{glyph} is not in LICENSE");
            } else {
                assert!(svg.contains("<!-- SPDX-License-Identifier: AGPL-3.0-only -->"), "{glyph}");
            }
        }
    }
}
