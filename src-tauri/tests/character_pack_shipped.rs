//! The game data pack the meter ships in `src/data`: it reads in every
//! language, it names everything its tables hold, and each language's own
//! file names the same things as the English one.

use std::collections::BTreeSet;
use std::path::PathBuf;

use a2tools_dps_meter_lib::character::pack::{CharacterPack, SlotGroup, FOLDER, LANGUAGES, PACK_VERSION};
use a2tools_dps_meter_lib::entity::job_class::JobClass;

fn data() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../src/data"))
}

#[test]
fn the_shipped_pack_reads_whole_in_every_language() {
    for language in LANGUAGES {
        let pack = CharacterPack::load(&data(), language).unwrap_or_else(|e| panic!("{language}: {e}"));
        assert_eq!(pack.problems(), Vec::<String>::new(), "{language}");
        assert_eq!(pack.slots.len(), 20, "{language}");
        let runes = pack.slots.iter().filter(|s| s.group == SlotGroup::Rune).count();
        assert_eq!(runes, 2, "{language}: 18 gear slots and 2 runes");
    }
    let pack = CharacterPack::load(&data(), "en").unwrap();
    let classes = [
        JobClass::Gladiator,
        JobClass::Templar,
        JobClass::Ranger,
        JobClass::Assassin,
        JobClass::Sorcerer,
        JobClass::Cleric,
        JobClass::Elementalist,
        JobClass::Chanter,
        JobClass::Fighter,
    ];
    for class in classes {
        assert!(pack.class_skills.get(&class).is_some_and(|l| !l.is_empty()), "{class:?} has no skills");
        assert!(pack.boards.values().any(|b| b.class == class), "{class:?} has no boards");
    }
    let mut nodes = pack.boards.values().flat_map(|b| b.nodes.values());
    assert!(nodes.any(|n| n.second_stat.is_some()), "no node gives a second stat");
}

/// Each part of a language's own text file, and the ids or keys it holds.
fn parts(language: &str) -> Vec<(String, BTreeSet<String>)> {
    let path = data().join("i18n").join(FOLDER).join(format!("{language}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let file: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(file["version"], PACK_VERSION, "{language}");
    file.as_object()
        .unwrap()
        .iter()
        .filter_map(|(part, v)| Some((part.clone(), v.as_object()?.keys().cloned().collect())))
        .collect()
}

#[test]
fn every_language_names_what_english_names() {
    let english = parts("en");
    let mut gaps = Vec::new();
    for language in LANGUAGES.into_iter().filter(|&l| l != "en") {
        let own = parts(language);
        for (part, ids) in &english {
            let theirs = own.iter().find(|(p, _)| p == part).map(|(_, ids)| ids.clone()).unwrap_or_default();
            let lacks: Vec<&String> = ids.difference(&theirs).collect();
            let more: Vec<&String> = theirs.difference(ids).collect();
            if !lacks.is_empty() || !more.is_empty() {
                gaps.push(format!("{language} {part}: lacks {lacks:?}, has more {more:?}"));
            }
        }
    }
    assert_eq!(gaps, Vec::<String>::new());
}
