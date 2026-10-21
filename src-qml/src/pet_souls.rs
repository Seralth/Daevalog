// SPDX-License-Identifier: AGPL-3.0-only

//! Pet soul notices: on each soul a pet takes, how many it holds toward its
//! next level and how many it needs to the top, in one tray notice per pet
//! that each new soul updates. The counts start from the pet collection the
//! game sends when it joins a world server (login, entering or leaving a
//! party dungeon or the Abyss) and follow every soul, registration and level
//! after it. They are kept in the data folder; counts kept from an earlier
//! run say they may be behind until the next collection, since souls taken
//! while the meter was off were never seen.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{Datelike, Local, TimeZone, Timelike};
use daevalog::capture::decoder::{PetCollectionEvent, PetCount};
use serde_json::{json, Value};

use crate::i18n::{fill, text};

pub const SOUL: &str = "notices.pet.soul";
pub const SOULS: &str = "notices.pet.souls";
pub const REGISTERED: &str = "notices.pet.registered";
pub const LEVEL: &str = "notices.pet.level";
pub const REACHED: &str = "notices.pet.reached";
pub const TO_REGISTER: &str = "notices.pet.toRegister";
pub const TO_LEVEL: &str = "notices.pet.toLevel";
pub const MAX: &str = "notices.pet.max";
pub const UNKNOWN: &str = "notices.pet.unknown";
pub const BEHIND: &str = "notices.pet.behind";
pub const INSIGHT: &str = "notices.pet.insight";
pub const INSIGHT_GAIN: &str = "notices.pet.insightGain";
pub const INSIGHT_MAX: &str = "notices.pet.insightMax";
pub const INSIGHT_LEVEL: &str = "notices.pet.insightLevel";
pub const UNNAMED: &str = "notices.pet.unnamed";

/// The keys this file looks up, for the strings check.
#[cfg(test)]
pub const KEYS: [&str; 15] = [
    SOUL, SOULS, REGISTERED, LEVEL, REACHED, TO_REGISTER, TO_LEVEL, MAX, UNKNOWN, BEHIND, INSIGHT, INSIGHT_GAIN, INSIGHT_MAX,
    INSIGHT_LEVEL, UNNAMED,
];

/// The game data's numbers for pets (src/data/pets.json).
#[derive(Default)]
pub struct PetTable {
    /// Genus and the souls to register, by pet.
    pets: HashMap<u32, (String, u32)>,
    /// By level from 1: the souls to the next level (0 at the top), and the
    /// genus Insight that reaching it gives.
    levels: Vec<(u32, u64)>,
    /// By genus, then Insight level: the experience to the next (0 at the top).
    insight: HashMap<String, HashMap<u32, u64>>,
}

impl PetTable {
    pub fn from_json(json: &Value) -> PetTable {
        let pets = json["pets"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(id, pet)| {
                Some((id.parse().ok()?, (pet["genus"].as_str()?.to_string(), pet["register"].as_u64()? as u32)))
            })
            .collect();
        let mut levels: Vec<(u64, u32, u64)> = json["levels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| Some((l["level"].as_u64()?, l["souls"].as_u64()? as u32, l["insight"].as_u64()?)))
            .collect();
        levels.sort();
        let insight = json["insight"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(genus, by_level)| {
                let needs = by_level
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter_map(|(level, need)| Some((level.parse().ok()?, need.as_u64()?)))
                    .collect();
                (genus.clone(), needs)
            })
            .collect();
        PetTable { pets, levels: levels.into_iter().map(|(_, souls, insight)| (souls, insight)).collect(), insight }
    }

    /// Souls to register `pet`, then to each next level: 5, 25, 75. A pet
    /// newer than the game data counts as the others do.
    fn steps(&self, pet: u32) -> Vec<u32> {
        let register = self.pets.get(&pet).map(|p| p.1).or_else(|| self.pets.values().map(|p| p.1).min()).unwrap_or(5);
        std::iter::once(register).chain(self.levels.iter().map(|l| l.0).filter(|&souls| souls > 0)).collect()
    }

    fn genus(&self, pet: u32) -> Option<&str> {
        self.pets.get(&pet).map(|p| p.0.as_str())
    }
}

/// Pet and genus names in one language (src/data/i18n/pets/<lang>.json).
#[derive(Default)]
pub struct PetNames {
    pets: HashMap<u32, String>,
    genus: HashMap<String, String>,
}

impl PetNames {
    pub fn from_json(json: &Value) -> PetNames {
        let map = |key: &str| json[key].as_object().into_iter().flatten().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())));
        PetNames { pets: map("pets").filter_map(|(k, v)| Some((k.parse().ok()?, v))).collect(), genus: map("genus").collect() }
    }
}

/// A notice for the tray: its key (one per pet), title and text.
pub type Notice = (String, String, String);

/// The words one language gives, with the clock's format.
pub struct Words<'a> {
    pub language: &'a str,
    pub names: &'a PetNames,
    pub hour24: bool,
}

impl Words<'_> {
    fn say(&self, key: &str, values: &[(&str, &str)]) -> String {
        fill(&text(self.language, key), values)
    }

    fn pet(&self, pet: u32) -> String {
        self.names.pets.get(&pet).cloned().unwrap_or_else(|| self.say(UNNAMED, &[("id", &pet.to_string())]))
    }

    /// A time of day, with its date when it is not today.
    fn when(&self, at: i64, today: chrono::NaiveDate) -> String {
        let Some(at) = Local.timestamp_opt(at, 0).single() else { return String::new() };
        let clock = crate::i18n::clock(self.language, at.hour(), at.minute(), self.hour24);
        if at.date_naive() == today {
            clock
        } else {
            format!("{:02}-{:02} {clock}", at.month(), at.day())
        }
    }
}

/// The counts, and what the notices say about them.
#[derive(Default)]
pub struct PetSouls {
    /// None until a collection came (or counts were kept from a run before).
    pets: Option<BTreeMap<u32, PetCount>>,
    /// When the collection the counts start from came, in Unix seconds.
    as_of: i64,
    /// Kept from an earlier run: souls taken meanwhile were never seen.
    behind: bool,
    /// Changed since the last save.
    dirty: bool,
}

impl PetSouls {
    /// One record; the notices it makes. `now` is Unix seconds.
    pub fn apply(&mut self, table: &PetTable, words: &Words, event: PetCollectionEvent, now: i64) -> Vec<Notice> {
        let today = Local.timestamp_opt(now, 0).single().map(|t| t.date_naive()).unwrap_or_default();
        match event {
            PetCollectionEvent::Collection(pets) => {
                *self = PetSouls { pets: Some(pets), as_of: now, behind: false, dirty: true };
                Vec::new()
            }
            PetCollectionEvent::Souls(added) => added
                .into_iter()
                .map(|(pet, n)| {
                    let name = words.pet(pet);
                    let title = if n == 1 {
                        words.say(SOUL, &[("pet", &name)])
                    } else {
                        words.say(SOULS, &[("pet", &name), ("count", &n.to_string())])
                    };
                    let reached = self.add(table, pet, n);
                    let status = self.status(table, words, pet, today);
                    let text = match reached {
                        Some(level) => words.say(REACHED, &[("level", &level.to_string()), ("status", &status)]),
                        None => status,
                    };
                    (key(pet), title, text)
                })
                .collect(),
            PetCollectionEvent::Level { pet, level, insight_level, insight_exp } => {
                if let Some(pets) = self.pets.as_mut() {
                    pets.entry(pet).or_default().level = level;
                    self.dirty = true;
                }
                let name = words.pet(pet);
                let title = if level == 1 {
                    words.say(REGISTERED, &[("pet", &name)])
                } else {
                    words.say(LEVEL, &[("pet", &name), ("level", &level.to_string())])
                };
                let insight = insight_text(table, words, pet, level, insight_level, insight_exp);
                let status = self.status(table, words, pet, today);
                vec![(key(pet), title, format!("{insight}. {status}"))]
            }
        }
    }

    /// Souls in; the game spends them on levels at once. The level reached,
    /// if any.
    fn add(&mut self, table: &PetTable, pet: u32, n: u32) -> Option<u32> {
        let pets = self.pets.as_mut()?;
        let steps = table.steps(pet);
        let count = pets.entry(pet).or_default();
        count.souls += n;
        let mut reached = None;
        while let Some(&need) = steps.get(count.level as usize) {
            if count.souls < need {
                break;
            }
            count.souls -= need;
            count.level += 1;
            reached = Some(count.level);
        }
        self.dirty = true;
        reached
    }

    /// "9/25 (16 to level 2), 91 to max".
    fn status(&self, table: &PetTable, words: &Words, pet: u32, today: chrono::NaiveDate) -> String {
        let Some(pets) = &self.pets else { return words.say(UNKNOWN, &[]) };
        let count = pets.get(&pet).copied().unwrap_or_default();
        let steps = table.steps(pet);
        let status = match steps.get(count.level as usize) {
            None => words.say(MAX, &[("level", &count.level.to_string())]),
            Some(&need) => {
                let left = need.saturating_sub(count.souls);
                let to_max = left + steps[count.level as usize + 1..].iter().sum::<u32>();
                let values = [
                    ("souls", count.souls.to_string()),
                    ("need", need.to_string()),
                    ("left", left.to_string()),
                    ("next", (count.level + 1).to_string()),
                    ("toMax", to_max.to_string()),
                ];
                let values: Vec<(&str, &str)> = values.iter().map(|(k, v)| (*k, v.as_str())).collect();
                words.say(if count.level == 0 { TO_REGISTER } else { TO_LEVEL }, &values)
            }
        };
        if self.behind {
            words.say(BEHIND, &[("status", &status), ("time", &words.when(self.as_of, today))])
        } else {
            status
        }
    }

    /// As kept in the data folder.
    pub fn to_json(&self) -> Value {
        let pets: Option<BTreeMap<String, [u32; 2]>> =
            self.pets.as_ref().map(|pets| pets.iter().map(|(id, c)| (id.to_string(), [c.level, c.souls])).collect());
        json!({ "asOf": self.as_of, "pets": pets })
    }

    /// Counts kept by an earlier run: maybe behind.
    pub fn from_json(json: &Value) -> PetSouls {
        let pets = json["pets"].as_object().map(|pets| {
            pets.iter()
                .filter_map(|(id, c)| Some((id.parse().ok()?, PetCount { level: c[0].as_u64()? as u32, souls: c[1].as_u64()? as u32 })))
                .collect()
        });
        let behind = pets.is_some();
        PetSouls { pets, as_of: json["asOf"].as_i64().unwrap_or(0), behind, dirty: false }
    }
}

fn key(pet: u32) -> String {
    format!("pet:{pet}")
}

/// "Fera Insight 6: 12050/20000 (+250)": the genus Insight after a pet
/// registered or levelled; Special pets give none.
fn insight_text(table: &PetTable, words: &Words, pet: u32, level: u32, insight_level: u32, exp: u64) -> String {
    let genus_id = table.genus(pet).unwrap_or_default();
    let genus = words.names.genus.get(genus_id).cloned().unwrap_or_else(|| genus_id.to_string());
    let level_text = insight_level.to_string();
    let need = table.insight.get(genus_id).and_then(|needs| needs.get(&insight_level)).copied();
    let gain = if genus_id == "Special" { 0 } else { table.levels.get(level as usize - 1).map_or(0, |l| l.1) };
    match need {
        Some(0) => words.say(INSIGHT_MAX, &[("genus", &genus), ("level", &level_text)]),
        Some(need) => {
            let values = [("genus", genus.as_str()), ("level", &level_text), ("exp", &exp.to_string()), ("need", &need.to_string())];
            let line = words.say(INSIGHT, &values);
            if gain > 0 {
                words.say(INSIGHT_GAIN, &[("insight", &line), ("gain", &gain.to_string())])
            } else {
                line
            }
        }
        None => words.say(INSIGHT_LEVEL, &[("genus", &genus), ("level", &level_text)]),
    }
}

// ----- the live meter -----

/// How often changed counts are written at most.
const SAVE_EVERY: Duration = Duration::from_secs(10);

struct Live {
    table: PetTable,
    names: HashMap<String, PetNames>,
    souls: PetSouls,
    file: Option<PathBuf>,
    saved: Instant,
}

fn names_in(language: &str) -> PetNames {
    let data = crate::data_dir();
    let read = |lang: &str| data.as_ref().and_then(|dir| crate::read_json(&dir.join("i18n/pets").join(format!("{lang}.json"))));
    read(language).or_else(|| read("en")).map(|json| PetNames::from_json(&json)).unwrap_or_default()
}

fn save(file: &Path, souls: &PetSouls) {
    let tmp = file.with_extension("json.tmp");
    let written = daevalog::platform::files::write_private(&tmp, souls.to_json().to_string()).and_then(|_| std::fs::rename(&tmp, file));
    if let Err(error) = written {
        eprintln!("daevalog-dps-meter: the pet soul counts were not saved: {error}");
    }
}

/// The live loop's call, once a tick: the pet records since the last call
/// become tray notices, and changed counts are saved now and then.
pub fn tick(storage: &daevalog::combat::data_storage::DataStorage, language: &str, hour24: bool) {
    static LIVE: OnceLock<Mutex<Live>> = OnceLock::new();
    let events = storage.take_pet_collection();
    let live = LIVE.get_or_init(|| {
        let table = crate::data_dir().and_then(|dir| crate::read_json(&dir.join("pets.json"))).map(|j| PetTable::from_json(&j)).unwrap_or_default();
        let file = crate::settings_store::folder().map(|dir| dir.join("pet-souls.json"));
        let souls = file.as_deref().and_then(crate::read_json).map(|j| PetSouls::from_json(&j)).unwrap_or_default();
        Mutex::new(Live { table, names: HashMap::new(), souls, file, saved: Instant::now() })
    });
    let mut live = live.lock().unwrap_or_else(|e| e.into_inner());
    let live = &mut *live;
    if !events.is_empty() {
        let names = live.names.entry(language.to_string()).or_insert_with(|| names_in(language));
        let words = Words { language, names, hour24 };
        let now = Local::now().timestamp();
        for event in events {
            for (key, title, text) in live.souls.apply(&live.table, &words, event, now) {
                crate::notices::post(&key, "pet", &title, &text);
            }
        }
    }
    if live.souls.dirty && live.saved.elapsed() >= SAVE_EVERY {
        if let Some(file) = &live.file {
            save(file, &live.souls);
        }
        live.souls.dirty = false;
        live.saved = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> PetTable {
        PetTable::from_json(&json!({
            "pets": { "1003": { "genus": "Feral", "register": 5 }, "1001": { "genus": "Special", "register": 5 } },
            "levels": [
                { "level": 1, "souls": 25, "insight": 500 },
                { "level": 2, "souls": 75, "insight": 250 },
                { "level": 3, "souls": 0, "insight": 250 }
            ],
            "insight": { "Feral": { "5": 10000, "6": 20000, "10": 0 } }
        }))
    }

    fn names() -> PetNames {
        PetNames::from_json(&json!({ "pets": { "1003": "Forest Specter" }, "genus": { "Feral": "Fera" } }))
    }

    fn words(names: &PetNames) -> Words<'_> {
        Words { language: "en", names, hour24: true }
    }

    fn collection(pets: &[(u32, u32, u32)]) -> PetCollectionEvent {
        PetCollectionEvent::Collection(pets.iter().map(|&(id, level, souls)| (id, PetCount { level, souls })).collect())
    }

    const NOW: i64 = 1_791_250_000;

    #[test]
    fn a_soul_says_how_many_to_the_next_level_and_to_max() {
        let (table, names) = (table(), names());
        let mut souls = PetSouls::default();
        assert!(souls.apply(&table, &words(&names), collection(&[(1003, 1, 21)]), NOW).is_empty());
        let notices = souls.apply(&table, &words(&names), PetCollectionEvent::Souls(vec![(1003, 1)]), NOW);
        assert_eq!(notices, vec![("pet:1003".into(), "Forest Specter soul".into(), "22/25 (3 to level 2), 78 to max".into())]);
    }

    #[test]
    fn souls_that_fill_a_level_spend_at_once() {
        let (table, names) = (table(), names());
        let mut souls = PetSouls::default();
        souls.apply(&table, &words(&names), collection(&[(1003, 1, 24)]), NOW);
        let notices = souls.apply(&table, &words(&names), PetCollectionEvent::Souls(vec![(1003, 3)]), NOW);
        assert_eq!(notices[0].1, "Forest Specter +3 souls");
        assert_eq!(notices[0].2, "Level 2 reached. 2/75 (73 to level 3), 73 to max");
    }

    #[test]
    fn an_unregistered_pet_counts_toward_registering_and_a_full_one_says_max() {
        let (table, names) = (table(), names());
        let mut souls = PetSouls::default();
        souls.apply(&table, &words(&names), collection(&[(1003, 3, 0)]), NOW);
        let new = souls.apply(&table, &words(&names), PetCollectionEvent::Souls(vec![(1208, 1)]), NOW);
        assert_eq!(new[0].1, "Pet 1208 soul", "a pet the game data does not name");
        assert_eq!(new[0].2, "1/5 (4 to register), 104 to max");
        let full = souls.apply(&table, &words(&names), PetCollectionEvent::Souls(vec![(1003, 1)]), NOW);
        assert_eq!(full[0].2, "Level 3, max");
    }

    #[test]
    fn before_any_collection_the_count_is_unknown() {
        let (table, names) = (table(), names());
        let mut souls = PetSouls::default();
        let notices = souls.apply(&table, &words(&names), PetCollectionEvent::Souls(vec![(1003, 1)]), NOW);
        assert_eq!(notices[0].2, "Count unknown: go to Select Character and back, or enter a party dungeon once");
    }

    #[test]
    fn a_level_up_shows_the_genus_insight() {
        let (table, names) = (table(), names());
        let mut souls = PetSouls::default();
        souls.apply(&table, &words(&names), collection(&[(1003, 1, 0)]), NOW);
        let event = PetCollectionEvent::Level { pet: 1003, level: 2, insight_level: 6, insight_exp: 12_050 };
        let notices = souls.apply(&table, &words(&names), event, NOW);
        assert_eq!(notices[0].1, "Forest Specter reached level 2");
        assert_eq!(notices[0].2, "Fera Insight 6: 12050/20000 (+250). 0/75 (75 to level 3), 75 to max");
        let top = PetCollectionEvent::Level { pet: 1003, level: 3, insight_level: 10, insight_exp: 0 };
        assert!(souls.apply(&table, &words(&names), top, NOW)[0].2.starts_with("Fera Insight 10 (max). Level 3, max"));
    }

    #[test]
    fn kept_counts_say_they_may_be_behind_until_the_next_collection() {
        let (table, names) = (table(), names());
        let mut first = PetSouls::default();
        first.apply(&table, &words(&names), collection(&[(1003, 1, 10)]), NOW);
        let mut kept = PetSouls::from_json(&first.to_json());
        let notices = kept.apply(&table, &words(&names), PetCollectionEvent::Souls(vec![(1003, 1)]), NOW + 60);
        assert!(notices[0].2.starts_with("11/25 (14 to level 2), 89 to max (pet list from "), "{}", notices[0].2);
        kept.apply(&table, &words(&names), collection(&[(1003, 1, 12)]), NOW + 120);
        let fresh = kept.apply(&table, &words(&names), PetCollectionEvent::Souls(vec![(1003, 1)]), NOW + 180);
        assert_eq!(fresh[0].2, "13/25 (12 to level 2), 87 to max");
    }

    /// The notices a packet log makes, one line each, for a compare by hand:
    /// `A2_PET_REPLAY=<packets_*.txt> cargo test --lib pet_souls -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn a_packet_log_prints_its_notices() {
        use daevalog::capture::packet_log::{LogLine, PacketLogReplay};
        use daevalog::combat::data_storage::DataStorage;
        use daevalog::i18n::lookup::{NpcLookup, SkillLookup};
        use std::sync::Arc;
        let Ok(file) = std::env::var("A2_PET_REPLAY") else { return };
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data");
        let table = PetTable::from_json(&crate::read_json(&dir.join("pets.json")).unwrap());
        let names = PetNames::from_json(&crate::read_json(&dir.join("i18n/pets/en.json")).unwrap());
        let storage = Arc::new(DataStorage::new());
        let mut feed = PacketLogReplay::new(storage.clone(), Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()), Default::default());
        let mut souls = PetSouls::default();
        for line in std::fs::read_to_string(file).unwrap().lines() {
            let Some(line) = LogLine::parse(line) else { continue };
            feed.feed(&line);
            for event in storage.take_pet_collection() {
                for (_, title, text) in souls.apply(&table, &words(&names), event, line.at_ms / 1000) {
                    println!("PET {} {title}: {text}", line.time_of_day().get(..8).unwrap_or(""));
                }
            }
        }
    }

    #[test]
    fn the_shipped_pet_table_reads() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data");
        let table = PetTable::from_json(&crate::read_json(&dir.join("pets.json")).unwrap());
        assert!(table.pets.len() > 100);
        assert_eq!(table.steps(1003), vec![5, 25, 75]);
        let names = PetNames::from_json(&crate::read_json(&dir.join("i18n/pets/en.json")).unwrap());
        assert!(names.pets.len() > 100 && names.genus.len() >= 4);
        for language in crate::i18n::LANGUAGES {
            assert!(crate::read_json(&dir.join("i18n/pets").join(format!("{language}.json"))).is_some(), "{language}");
        }
    }
}
