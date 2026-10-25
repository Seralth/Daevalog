//! The Character window's contract (`character`): the character file the
//! meter writes and the game data pack it reads, through their public
//! surface only. The sample file and pack in `fixtures/character/` are
//! made up; the sample file holds every field the format has.

use std::path::{Path, PathBuf};

use a2tools_dps_meter_lib::character::file::{CharacterFile, FileError, GearDetail, GearItem, Pick, StatValue};
use a2tools_dps_meter_lib::character::pack::{CharacterPack, IdKind, MissingId, StatAmount};
use a2tools_dps_meter_lib::character::store;
use a2tools_dps_meter_lib::entity::job_class::JobClass;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/character");

fn sample_text() -> String {
    std::fs::read_to_string(Path::new(FIXTURES).join("sample-character.json")).unwrap()
}

fn sample() -> CharacterFile {
    CharacterFile::from_json(&sample_text()).unwrap()
}

fn pack(language: &str) -> CharacterPack {
    CharacterPack::load(&Path::new(FIXTURES).join("pack"), language).unwrap()
}

/// A folder of its own under the system's temporary folder, empty.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("daevalog-character-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_sample_file_reads_and_writes_back_byte_for_byte() {
    let file = sample();
    assert_eq!(file.to_json(), sample_text());
    assert_eq!(CharacterFile::from_json(&file.to_json()).unwrap(), file);

    assert_eq!((file.version, file.server, file.class, file.level), (1, 1001, Some(JobClass::Elementalist), Some(45)));
    assert_eq!(file.name.as_deref(), Some("Samplechar"));
    let gear = &file.gear.as_ref().unwrap().items;
    assert_eq!(gear.len(), 6);
    assert_eq!(
        gear[0],
        GearItem {
            slot: 1,
            item: 110_600_101,
            detail: Some(GearDetail {
                enhance: 5,
                item_level: 45,
                sockets: 3,
                soul_bound: true,
                lines: vec![StatValue { stat: 282, value: 1050 }, StatValue { stat: 128, value: 60 }],
                skill_lines: vec![16_010_000],
            }),
        }
    );
    assert_eq!(gear[3].detail, None, "gear fields not read");
    let picks = &file.specialties.as_ref().unwrap().list[0].picks;
    assert_eq!(picks[2], Pick { slot: 3, part: 0 });
    assert_eq!(file.newest_ms(), Some(1_767_225_602_000));
}

#[test]
fn only_the_header_is_required() {
    let file = CharacterFile::from_json(r#"{"version": 1, "server": 1001, "class": null, "level": null}"#).unwrap();
    assert_eq!(file, CharacterFile::new(1001, None, None));
    assert_eq!(file.name, None);
    assert_eq!(file.newest_ms(), None);
    // A field this meter does not know is left alone.
    let newer = r#"{"version": 1, "server": 1001, "class": "Cleric", "level": 45, "wings": [1, 2]}"#;
    assert_eq!(CharacterFile::from_json(newer).unwrap().class, Some(JobClass::Cleric));
}

#[test]
fn a_file_of_another_version_is_refused_for_its_version() {
    let v2 = sample_text().replacen("\"version\": 1", "\"version\": 2", 1);
    assert_eq!(CharacterFile::from_json(&v2), Err(FileError::Version(2)));
    assert!(matches!(CharacterFile::from_json("{\"version\": 1}"), Err(FileError::Json(_))));
    assert!(matches!(CharacterFile::from_json("not json"), Err(FileError::Json(_))));
}

#[test]
fn each_character_has_one_file_named_without_its_name() {
    assert_eq!(store::key(1001, "Tester"), "47e140e764c24813");
    assert_eq!(store::file_name(1001, "Tester"), "1001-47e140e764c24813.json");
    assert_eq!(store::file_name(1001, "Other"), "1001-039897980481faaf.json");

    let dir = scratch("store");
    let mut file = sample();
    let path = store::save(&dir, "Tester", &file).unwrap();
    assert_eq!(path, dir.join(store::FOLDER).join("1001-47e140e764c24813.json"));
    file.level = Some(46);
    assert_eq!(store::save(&dir, "Tester", &file).unwrap(), path, "the same character, the same file");
    assert_eq!(store::load(&path).unwrap(), file);
    store::save(&dir, "Other", &CharacterFile::new(1001, Some(JobClass::Cleric), Some(30))).unwrap();

    // What a reader lists: the character files, never a temporary file.
    std::fs::write(dir.join(store::FOLDER).join(".1001-47e140e764c24813.json.tmp"), "{").unwrap();
    std::fs::write(dir.join(store::FOLDER).join("notes.json"), "{}").unwrap();
    let mut listed: Vec<String> =
        store::list(&dir).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    listed.sort();
    assert_eq!(listed, ["1001-039897980481faaf.json", "1001-47e140e764c24813.json"]);
    assert_eq!(store::list(&dir.join("none")), Vec::<PathBuf>::new());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(dir.join(store::FOLDER)).unwrap().permissions().mode() & 0o777, 0o700);
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_sample_pack_names_and_places_every_id_in_the_sample_file() {
    let pack = pack("en");
    assert_eq!(pack.problems(), Vec::<String>::new());
    assert_eq!(pack.missing(&sample()), Vec::<MissingId>::new());
    assert_eq!(pack.slots.len(), 20);
    assert_eq!(pack.text.items[&110_600_101], "Sample Orb");
    // Not shown, so neither needs text: a general skill the class's list
    // does not hold, and the category of a title no slot takes.
    assert!(!pack.text.skills.contains_key(&1001));
    assert_eq!(pack.titles[&14_000_001].category, "None");
    assert!(!pack.text.title_categories.contains_key("None"));
    assert_eq!(pack.text.node_titles[&pack.boards[&51].nodes[&510_033].title], "Combat Speed");
}

#[test]
fn a_node_that_gives_two_stats_carries_both() {
    let mut pack = pack("en");
    let nodes = &pack.boards[&56].nodes;
    assert_eq!((nodes[&560_021].stat, nodes[&560_021].second_stat), (Some(StatAmount(104, 5)), Some(StatAmount(52, 30))));
    assert_eq!((nodes[&560_020].stat, nodes[&560_020].second_stat), (Some(StatAmount(128, 30)), None));

    // A second stat with no name is caught like a first one.
    let mut file = sample();
    file.boards.as_mut().unwrap().list[1].nodes.push(560_021);
    assert_eq!(pack.missing(&file), Vec::<MissingId>::new());
    pack.boards.get_mut(&56).unwrap().nodes.get_mut(&560_021).unwrap().second_stat = Some(StatAmount(999, 1));
    assert_eq!(pack.missing(&file), vec![MissingId { kind: IdKind::StatName, id: 999 }]);
    assert_eq!(pack.problems(), ["node 560021: stat 999 has no name"]);
    // Only a Stat node has one.
    let start = pack.boards.get_mut(&56).unwrap().nodes.get_mut(&560_113).unwrap();
    start.second_stat = Some(StatAmount(52, 30));
    assert!(pack.problems().contains(&"node 560113: a Start node with the wrong values".to_string()));
}

#[test]
fn ids_the_pack_lacks_are_named() {
    let mut file = sample();
    let gear = &mut file.gear.as_mut().unwrap().items;
    gear.push(GearItem { slot: 9, item: 999_000_001, detail: None });
    let orb = gear[0].detail.as_mut().unwrap();
    orb.enhance = 6;
    orb.skill_lines.push(16_999_000);
    file.server = 1999;
    file.class = Some(JobClass::Gladiator);
    file.boards.as_mut().unwrap().list.push(a2tools_dps_meter_lib::character::file::BoardNodes { board: 59, nodes: vec![590_001] });
    file.boards.as_mut().unwrap().list[0].nodes.push(510_999);
    file.specialties.as_mut().unwrap().list[0].picks[2].part = 16_010_090;
    file.titles.as_mut().unwrap().owned.push(19_999_999);

    let missing = |kind, id| MissingId { kind, id };
    assert_eq!(
        pack("en").missing(&file),
        vec![
            missing(IdKind::Server, 1999),
            missing(IdKind::Class, 11),
            missing(IdKind::Slot, 9),
            missing(IdKind::Item, 999_000_001),
            missing(IdKind::ItemName, 999_000_001),
            missing(IdKind::EnhanceLevel, 110_600_101),
            missing(IdKind::SkillName, 16_999_000),
            missing(IdKind::SpecialtyPart, 16_010_090),
            missing(IdKind::Board, 59),
            missing(IdKind::BoardName, 59),
            missing(IdKind::Node, 510_999),
            missing(IdKind::Node, 590_001),
            missing(IdKind::Title, 19_999_999),
            missing(IdKind::TitleName, 19_999_999),
        ]
    );
}

#[test]
fn text_a_language_lacks_comes_from_english() {
    let de = pack("de");
    assert_eq!(de.text.items[&110_600_101], "Musterkugel");
    assert_eq!(de.text.items[&110_900_101], "Sample Guard");
    assert_eq!(de.text.servers[&1001], "Musterserver");
    assert_eq!((de.text.skills[&16_010_000].as_str(), de.text.skills[&16_020_000].as_str()), ("Musterblitz", "Sample Wave"));
    // The game's "???" for a skill it has no German text for.
    assert_eq!(de.text.skills[&16_040_000], "Sample Ward");
    assert_eq!((de.text.stats[&1].as_str(), de.text.stats[&2].as_str()), ("Macht", "Dexterity"));
    assert_eq!(de.missing(&sample()), Vec::<MissingId>::new());
    // A language the pack has no files for, or one it does not ship, reads as English.
    assert_eq!(pack("ja").text, pack("en").text);
    assert_eq!(pack("xx").text, pack("en").text);
}

#[test]
fn enhance_levels_and_stat_units_read_as_the_tables_say() {
    let pack = pack("en");
    assert_eq!(pack.enhance_stats(110_600_101, 5), [StatAmount(33, 65), StatAmount(31, 65)]);
    assert_eq!(pack.enhance_stats(110_600_101, 0), []);
    assert_eq!(pack.enhance_stats(110_600_101, 6), [], "past the table");
    assert_eq!(pack.enhance_stats(112_100_101, 1), [], "cannot be enhanced");
    // Percent stats are stored in hundredths; Stamina a hundred times larger.
    assert_eq!((pack.stat_unit(282), pack.stat_unit(242), pack.stat_unit(1)), (100, 100, 1));
    assert_eq!(pack.stat_groups.iter().map(|g| g.key.as_str()).collect::<Vec<_>>(), ["MainStat", "Attack", "BookMark", "Recovery"]);
}

#[test]
fn a_pack_file_of_another_version_is_refused() {
    let dir = scratch("pack");
    let from = Path::new(FIXTURES).join("pack");
    for rel in [
        "character/slots.json",
        "character/items.json",
        "character/stat_window.json",
        "character/skills.json",
        "character/boards.json",
        "character/titles.json",
        "stats.json",
        "i18n/character/en.json",
        "i18n/skills/en.json",
        "i18n/stats/en.json",
    ] {
        std::fs::create_dir_all(dir.join(rel).parent().unwrap()).unwrap();
        std::fs::copy(from.join(rel), dir.join(rel)).unwrap();
    }
    assert!(CharacterPack::load(&dir, "en").is_ok());
    let titles = dir.join("character/titles.json");
    let text = std::fs::read_to_string(&titles).unwrap().replacen("\"version\": 1", "\"version\": 2", 1);
    std::fs::write(&titles, text).unwrap();
    let error = CharacterPack::load(&dir, "en").unwrap_err();
    assert!(error.file.ends_with("titles.json") && error.why.contains("version 2"), "{error}");
    std::fs::remove_dir_all(&dir).unwrap();
}
