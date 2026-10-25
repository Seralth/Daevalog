//! The character file: what the game sent about the character you play, as
//! ids and numbers, version `FILE_VERSION`.
//!
//! Every section holds `as_of_ms`, the capture time (ms since the Unix epoch)
//! of the record it was last read from; a section the game has not sent yet
//! is null. The numbers are the game's own: a stat is the value its stat
//! record holds (percent stats in hundredths, as `src/data/stats.json` lists
//! them), a rolled line is the value the item rolled. The file holds no
//! game names; the one name it may hold is the character's own, which stays
//! on this computer like the file.
//!
//! ```text
//! version       1
//! server        the home server id
//! name          the character's name, null when not known; may be absent
//! class         the class (`JobClass`), null when not known
//! level         the character level, null when not known
//! item_level    { as_of_ms, current, highest }
//! combat_power  { as_of_ms, current, highest }
//! gear          { as_of_ms, items: [{ slot, item, detail }] }
//!               slot: the equip slot id (the pack's slots.json); item: the
//!               item id; detail: null when the record's gear fields could
//!               not be read, else { enhance, item_level, sockets,
//!               soul_bound, lines: [{ stat, value }], skill_lines: [skill] }
//! stats         { as_of_ms, values: [{ stat, value }] }   the game's stat sheet
//! skills        { as_of_ms, list: [{ skill, level, base, board, gear }] }
//! specialties   { as_of_ms, list: [{ skill, picks: [{ slot, part }] }] }
//!               part 0: nothing in that slot
//! boards        { as_of_ms, list: [{ board, nodes: [node] }] }   nodes taken
//! titles        { as_of_ms, owned: [title], equipped: [{ slot, title }] }
//! ```
//!
//! A full example is `tests/fixtures/character/sample-character.json`.

use serde::{Deserialize, Serialize};

use crate::entity::job_class::JobClass;

/// The version this meter writes and reads.
pub const FILE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterFile {
    pub version: u32,
    pub server: u16,
    /// Tells apart characters of the same class, level and server.
    #[serde(default)]
    pub name: Option<String>,
    pub class: Option<JobClass>,
    pub level: Option<u32>,
    #[serde(default)]
    pub item_level: Option<Levels>,
    #[serde(default)]
    pub combat_power: Option<Levels>,
    #[serde(default)]
    pub gear: Option<Gear>,
    #[serde(default)]
    pub stats: Option<Stats>,
    #[serde(default)]
    pub skills: Option<Skills>,
    #[serde(default)]
    pub specialties: Option<Specialties>,
    #[serde(default)]
    pub boards: Option<Boards>,
    #[serde(default)]
    pub titles: Option<Titles>,
}

/// Item Level or Combat Power: now, and the highest the game has recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Levels {
    pub as_of_ms: i64,
    pub current: u64,
    pub highest: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gear {
    pub as_of_ms: i64,
    /// The worn items, by slot.
    pub items: Vec<GearItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GearItem {
    /// The equip slot id.
    pub slot: u8,
    pub item: u32,
    pub detail: Option<GearDetail>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GearDetail {
    /// The enhance level, Amplify levels included.
    pub enhance: u8,
    /// As the game sends it: the item's table level plus `enhance`, and more
    /// for its manastones.
    pub item_level: u32,
    /// Manastone slots.
    pub sockets: u8,
    pub soul_bound: bool,
    /// The rolled lines, in the game's order.
    pub lines: Vec<StatValue>,
    /// The skills of the +1 skill lines.
    pub skill_lines: Vec<u32>,
}

/// A stat id (the game's `EStat`) and its value as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatValue {
    pub stat: u16,
    pub value: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    pub as_of_ms: i64,
    /// The stat sheet, buffs included, by stat id.
    pub values: Vec<StatValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skills {
    pub as_of_ms: i64,
    pub list: Vec<SkillLevel>,
}

/// A skill's level and the levels it has from points, boards and gear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillLevel {
    pub skill: u32,
    pub level: u8,
    pub base: u8,
    pub board: u8,
    pub gear: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Specialties {
    pub as_of_ms: i64,
    pub list: Vec<SpecialtySlots>,
}

/// One skill's Specialty slots. A stigma lists every tier it has, whatever
/// its level: those are not picks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecialtySlots {
    pub skill: u32,
    pub picks: Vec<Pick>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pick {
    pub slot: u8,
    /// The option's id (the pack's specialties); 0 for an empty slot.
    pub part: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Boards {
    pub as_of_ms: i64,
    pub list: Vec<BoardNodes>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardNodes {
    pub board: u32,
    /// The nodes taken.
    pub nodes: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Titles {
    pub as_of_ms: i64,
    pub owned: Vec<u32>,
    pub equipped: Vec<EquippedTitle>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EquippedTitle {
    /// The game's title slot: 1, 2 and 3 hold the Attack, Defense and Etc
    /// titles.
    pub slot: u8,
    pub title: u32,
}

/// Why a character file was not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileError {
    Io(String),
    Json(String),
    /// Written in a version this meter does not read.
    Version(u32),
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileError::Io(why) => write!(f, "not read: {why}"),
            FileError::Json(why) => write!(f, "not a character file: {why}"),
            FileError::Version(v) => write!(f, "character file version {v}; this meter reads version {FILE_VERSION}"),
        }
    }
}

impl CharacterFile {
    /// A file with no section read yet.
    pub fn new(server: u16, class: Option<JobClass>, level: Option<u32>) -> CharacterFile {
        CharacterFile {
            version: FILE_VERSION,
            server,
            name: None,
            class,
            level,
            item_level: None,
            combat_power: None,
            gear: None,
            stats: None,
            skills: None,
            specialties: None,
            boards: None,
            titles: None,
        }
    }

    /// The file's text: pretty JSON, fields in the order above, a newline
    /// at the end.
    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("a character file serialises");
        text.push('\n');
        text
    }

    /// Reads a file's text. The version is checked before the rest, so a
    /// file of another version is refused for its version, not as broken.
    pub fn from_json(text: &str) -> Result<CharacterFile, FileError> {
        #[derive(Deserialize)]
        struct Head {
            version: u32,
        }
        let head: Head = serde_json::from_str(text).map_err(|e| FileError::Json(e.to_string()))?;
        if head.version != FILE_VERSION {
            return Err(FileError::Version(head.version));
        }
        serde_json::from_str(text).map_err(|e| FileError::Json(e.to_string()))
    }

    /// The newest `as_of_ms` of any section; None when no section is read.
    pub fn newest_ms(&self) -> Option<i64> {
        [
            self.item_level.map(|s| s.as_of_ms),
            self.combat_power.map(|s| s.as_of_ms),
            self.gear.as_ref().map(|s| s.as_of_ms),
            self.stats.as_ref().map(|s| s.as_of_ms),
            self.skills.as_ref().map(|s| s.as_of_ms),
            self.specialties.as_ref().map(|s| s.as_of_ms),
            self.boards.as_ref().map(|s| s.as_of_ms),
            self.titles.as_ref().map(|s| s.as_of_ms),
        ]
        .into_iter()
        .flatten()
        .max()
    }
}
