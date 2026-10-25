//! The game data pack: what the Character window needs to name and lay out
//! the ids in a character file, version `PACK_VERSION`.
//!
//! Built along with the other game tables in `src/data`. Every table holds
//! `"version"` and, like the other tables, `"source"`; the keys below sit
//! beside them. Ids are JSON object keys, so they are strings in the files.
//! A stat amount is `[stat id, value]`, the value stored as the stat
//! records store it.
//!
//! ```text
//! character/slots.json        slots: [{ id, key, group }]   the slots the
//!                             paper doll shows, in its order; group: weapon,
//!                             armor, accessory or rune
//! character/items.json        items: { item: { grade, category, level,
//!                             sockets, base: [amount], fixed: [amount],
//!                             enhance: group or null } }
//!                             enhance: { group: [[amount]] }, indexed by
//!                             enhance level (Amplify levels after the last
//!                             enhance level), each the stats that level
//!                             adds in all; index 0 is empty
//! character/stat_window.json  groups: [{ key, stats: [stat] }]   the game's
//!                             stat window, its groups and stats in order
//!                             units: { stat: n }   stats stored n times the
//!                             value shown; any other stat: 1
//! character/skills.json       classes: { class: [{ skill, acquire, kind }] }
//!                             in the game's order; acquire: Mastery or
//!                             Stigma; kind: Active or Passive
//!                             specialties: { skill: [{ part, option,
//!                             unlock, slot }] }   option 1-5 as the game
//!                             numbers it; unlock: the skill level that opens
//!                             it; slot: the stigma tier's fixed slot, 0 for
//!                             any
//!                             mastery_slots: [level]   the skill level that
//!                             opens each Specialty slot, slot 1 first
//! character/boards.json       boards: { board: { class, order, nodes:
//!                             { node: { row, col, kind, grade, cost, title,
//!                             stat: amount or null, second_stat: amount or
//!                             null, skill: [skill, levels] or null } } } }
//!                             kind: Start, Stat or SkillLevel; title: a
//!                             node_titles key; second_stat: the other stat
//!                             of a Stat node that gives two, else null
//! character/titles.json       titles: { title: { category, owned: [amount],
//!                             equipped: [amount] } }
//! i18n/character/<lang>.json  the game's words, in one language: items,
//!                             boards, titles, specialties (the option text
//!                             as the game shows it), servers: { id: name };
//!                             node_titles: { title: text }; slots,
//!                             grades, title_categories, stat_groups:
//!                             { key: label }
//! ```
//!
//! Also read: `stats.json` (`percent`: the stats shown as a percent, stored
//! in hundredths), `i18n/skills/<lang>.json` and `i18n/stats/<lang>.json`
//! (skill and stat names). Text a language lacks comes from English.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::Deserialize;

use super::file::CharacterFile;
use crate::entity::job_class::JobClass;

/// The version this meter reads.
pub const PACK_VERSION: u32 = 1;

/// The tables' folder in the data folder; the text is in `i18n/<FOLDER>/`.
pub const FOLDER: &str = "character";

/// The languages the pack has text in.
pub const LANGUAGES: [&str; 8] = ["de", "en", "es", "fr", "ja", "ko", "pt", "ru"];

/// `[stat id, value]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct StatAmount(pub u16, pub i32);

/// `[skill id, levels]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct SkillBonus(pub u32, pub u32);

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Slot {
    /// The equip slot id, as the character file's gear has it.
    pub id: u8,
    /// The game's name for the slot (`MainHand`): the text key.
    pub key: String,
    pub group: SlotGroup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SlotGroup {
    Weapon,
    Armor,
    Accessory,
    Rune,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Item {
    /// The game's grade (`Legend`): the text key and the colour's.
    pub grade: String,
    /// The game's equip category (`Orb`, `Helmet`).
    pub category: String,
    /// Item Level before enhancing.
    pub level: u32,
    /// Manastone slots.
    pub sockets: u8,
    pub base: Vec<StatAmount>,
    /// Lines every copy of the item has.
    #[serde(default)]
    pub fixed: Vec<StatAmount>,
    /// Its `enhance` table; None when it cannot be enhanced.
    #[serde(default)]
    pub enhance: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StatGroup {
    /// The game's group (`MainStat`): the text key.
    pub key: String,
    pub stats: Vec<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct ClassSkill {
    pub skill: u32,
    pub acquire: Acquire,
    pub kind: SkillKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Acquire {
    Mastery,
    Stigma,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum SkillKind {
    Active,
    Passive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct SpecialtyOption {
    /// The id a character file's pick holds.
    pub part: u32,
    /// 1 to 5, the number the game shows.
    pub option: u8,
    /// The skill level that opens it.
    pub unlock: u32,
    /// A stigma tier's fixed slot; 0 for an option that goes in any slot.
    pub slot: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Board {
    pub class: JobClass,
    /// The board's place among its class's boards, from 1.
    pub order: u32,
    pub nodes: HashMap<u32, Node>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Node {
    pub row: i32,
    pub col: i32,
    pub kind: NodeKind,
    /// The node's grade (the game's item grades), for its colour.
    pub grade: String,
    /// Daevanion points to take it.
    pub cost: u32,
    /// The key of its text in `node_titles`.
    pub title: String,
    #[serde(default)]
    pub stat: Option<StatAmount>,
    /// The other stat of a Stat node that gives two.
    #[serde(default)]
    pub second_stat: Option<StatAmount>,
    #[serde(default)]
    pub skill: Option<SkillBonus>,
}

impl Node {
    /// `stat`, then `second_stat`.
    pub fn stats(&self) -> impl Iterator<Item = StatAmount> {
        self.stat.into_iter().chain(self.second_stat)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum NodeKind {
    Start,
    Stat,
    SkillLevel,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Title {
    /// `Attack`, `Defense` or `Etc`: the text key in `title_categories`;
    /// `None` for a title no slot takes, which has no text.
    pub category: String,
    /// What the title gives while owned, and while equipped.
    pub owned: Vec<StatAmount>,
    pub equipped: Vec<StatAmount>,
}

/// The game's words in one language, English where it has none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct PackText {
    pub items: HashMap<u32, String>,
    pub boards: HashMap<u32, String>,
    pub node_titles: HashMap<String, String>,
    pub titles: HashMap<u32, String>,
    pub title_categories: HashMap<String, String>,
    pub specialties: HashMap<u32, String>,
    pub servers: HashMap<u16, String>,
    pub slots: HashMap<String, String>,
    pub grades: HashMap<String, String>,
    pub stat_groups: HashMap<String, String>,
    /// From `i18n/skills/<lang>.json`.
    #[serde(skip)]
    pub skills: HashMap<u32, String>,
    /// From `i18n/stats/<lang>.json`.
    #[serde(skip)]
    pub stats: HashMap<u16, String>,
}

impl PackText {
    /// Takes every text `other` has.
    fn overlay(&mut self, other: PackText) {
        self.items.extend(other.items);
        self.boards.extend(other.boards);
        self.node_titles.extend(other.node_titles);
        self.titles.extend(other.titles);
        self.title_categories.extend(other.title_categories);
        self.specialties.extend(other.specialties);
        self.servers.extend(other.servers);
        self.slots.extend(other.slots);
        self.grades.extend(other.grades);
        self.stat_groups.extend(other.stat_groups);
        self.skills.extend(other.skills);
        self.stats.extend(other.stats);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharacterPack {
    pub slots: Vec<Slot>,
    pub items: HashMap<u32, Item>,
    pub enhance: HashMap<String, Vec<Vec<StatAmount>>>,
    pub stat_groups: Vec<StatGroup>,
    pub stat_units: HashMap<u16, i32>,
    /// Stats shown as a percent, stored in hundredths.
    pub percent: HashSet<u16>,
    pub class_skills: HashMap<JobClass, Vec<ClassSkill>>,
    pub specialties: HashMap<u32, Vec<SpecialtyOption>>,
    pub mastery_slots: Vec<u32>,
    pub boards: HashMap<u32, Board>,
    pub titles: HashMap<u32, Title>,
    pub text: PackText,
}

/// A pack file that did not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackError {
    pub file: String,
    pub why: String,
}

impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.file, self.why)
    }
}

/// An id a character file names that the pack cannot name or place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MissingId {
    pub kind: IdKind,
    pub id: u32,
}

/// What a missing id is. For `EnhanceLevel` the id is the item's; for
/// `NodeTitle` the node's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IdKind {
    Server,
    /// A class with no skill list.
    Class,
    Slot,
    Item,
    ItemName,
    EnhanceLevel,
    StatName,
    SkillName,
    SpecialtyPart,
    Board,
    BoardName,
    Node,
    NodeTitle,
    Title,
    TitleName,
}

// ----- the files -----

#[derive(Deserialize)]
struct Head {
    version: u32,
}
#[derive(Deserialize)]
struct SlotsFile {
    slots: Vec<Slot>,
}
#[derive(Deserialize)]
struct ItemsFile {
    items: HashMap<u32, Item>,
    enhance: HashMap<String, Vec<Vec<StatAmount>>>,
}
#[derive(Deserialize)]
struct StatWindowFile {
    groups: Vec<StatGroup>,
    #[serde(default)]
    units: HashMap<u16, i32>,
}
#[derive(Deserialize)]
struct SkillsFile {
    classes: HashMap<JobClass, Vec<ClassSkill>>,
    specialties: HashMap<u32, Vec<SpecialtyOption>>,
    mastery_slots: Vec<u32>,
}
#[derive(Deserialize)]
struct BoardsFile {
    boards: HashMap<u32, Board>,
}
#[derive(Deserialize)]
struct TitlesFile {
    titles: HashMap<u32, Title>,
}
#[derive(Deserialize)]
struct PercentFile {
    percent: Vec<u16>,
}

fn read_text(path: &Path) -> Result<String, PackError> {
    std::fs::read_to_string(path).map_err(|e| PackError { file: path.display().to_string(), why: e.to_string() })
}

fn parse<T: DeserializeOwned>(path: &Path, text: &str) -> Result<T, PackError> {
    serde_json::from_str(text).map_err(|e| PackError { file: path.display().to_string(), why: e.to_string() })
}

/// A pack file of this version.
fn versioned<T: DeserializeOwned>(path: &Path) -> Result<T, PackError> {
    let text = read_text(path)?;
    let head: Head = parse(path, &text)?;
    if head.version != PACK_VERSION {
        let why = format!("version {}; this meter reads version {PACK_VERSION}", head.version);
        return Err(PackError { file: path.display().to_string(), why });
    }
    parse(path, &text)
}

/// `{"<id>": "name"}`; keys that are not ids are left out.
fn names<K: std::str::FromStr + Eq + std::hash::Hash>(path: &Path) -> Result<HashMap<K, String>, PackError> {
    let map: HashMap<String, serde_json::Value> = parse(path, &read_text(path)?)?;
    Ok(map
        .into_iter()
        .filter_map(|(k, v)| Some((k.parse().ok()?, v.as_str()?.to_string())))
        .collect())
}

/// The English text: the pack's own, and the skill and stat names.
fn english_text(data_dir: &Path) -> Result<PackText, PackError> {
    let mut text: PackText = versioned(&data_dir.join("i18n").join(FOLDER).join("en.json"))?;
    text.skills = names(&data_dir.join("i18n/skills/en.json"))?;
    text.stats = names(&data_dir.join("i18n/stats/en.json"))?;
    Ok(text)
}

/// Text the game left as a placeholder: "???".
fn placeholder(text: &str) -> bool {
    !text.is_empty() && text.trim().chars().all(|c| c == '?')
}

/// The text of another language; each of its three files that does not
/// read leaves its part to English, and so does a skill name the game left
/// as "???" (it has no text for most Fighter skills in five languages).
fn language_text(data_dir: &Path, lang: &str) -> PackText {
    let file = format!("{lang}.json");
    let mut text: PackText = versioned(&data_dir.join("i18n").join(FOLDER).join(&file)).unwrap_or_default();
    text.skills = names(&data_dir.join("i18n/skills").join(&file)).unwrap_or_default();
    text.skills.retain(|_, name| !placeholder(name));
    text.stats = names(&data_dir.join("i18n/stats").join(&file)).unwrap_or_default();
    text
}

impl CharacterPack {
    /// The pack in the data folder `data_dir`, with text in `language` (a
    /// code of `LANGUAGES`; any other reads as English). Text a language's
    /// files lack, or a language file that does not read, falls back to
    /// English; a table or an English file that does not read is an error.
    pub fn load(data_dir: &Path, language: &str) -> Result<CharacterPack, PackError> {
        let tables = data_dir.join(FOLDER);
        let slots: SlotsFile = versioned(&tables.join("slots.json"))?;
        let items: ItemsFile = versioned(&tables.join("items.json"))?;
        let window: StatWindowFile = versioned(&tables.join("stat_window.json"))?;
        let skills: SkillsFile = versioned(&tables.join("skills.json"))?;
        let boards: BoardsFile = versioned(&tables.join("boards.json"))?;
        let titles: TitlesFile = versioned(&tables.join("titles.json"))?;
        let percent_path = data_dir.join("stats.json");
        let percent: PercentFile = parse(&percent_path, &read_text(&percent_path)?)?;

        let mut text = english_text(data_dir)?;
        if language != "en" && LANGUAGES.contains(&language) {
            text.overlay(language_text(data_dir, language));
        }
        Ok(CharacterPack {
            slots: slots.slots,
            items: items.items,
            enhance: items.enhance,
            stat_groups: window.groups,
            stat_units: window.units,
            percent: percent.percent.into_iter().collect(),
            class_skills: skills.classes,
            specialties: skills.specialties,
            mastery_slots: skills.mastery_slots,
            boards: boards.boards,
            titles: titles.titles,
            text,
        })
    }

    /// The stats `item` gains at enhance level `level`, in all; empty for
    /// level 0, an item that cannot be enhanced, or a level past its table.
    pub fn enhance_stats(&self, item: u32, level: u8) -> &[StatAmount] {
        self.items
            .get(&item)
            .and_then(|i| i.enhance.as_ref())
            .and_then(|group| self.enhance.get(group))
            .and_then(|levels| levels.get(level as usize))
            .map_or(&[], Vec::as_slice)
    }

    /// How many times larger than shown `stat` is stored: 100 for a percent
    /// stat (hundredths), the table's unit for the others.
    pub fn stat_unit(&self, stat: u16) -> i32 {
        if self.percent.contains(&stat) { 100 } else { self.stat_units.get(&stat).copied().unwrap_or(1) }
    }

    /// Every id the four screens would show from `file` that this pack
    /// cannot name or place, sorted. Skills the class's list does not hold
    /// (general skills, chain steps, event skills) are not shown, so they
    /// are not checked; the listed ones are the pack's own (`problems`).
    pub fn missing(&self, file: &CharacterFile) -> Vec<MissingId> {
        let mut out = HashSet::new();
        let mut miss = |kind: IdKind, id: u32, found: bool| {
            if !found {
                out.insert(MissingId { kind, id });
            }
        };
        let t = &self.text;
        miss(IdKind::Server, file.server.into(), t.servers.contains_key(&file.server));
        if let Some(class) = file.class {
            miss(IdKind::Class, class.job_code_head() as u32, self.class_skills.contains_key(&class));
        }
        for g in file.gear.iter().flat_map(|g| &g.items) {
            miss(IdKind::Slot, g.slot.into(), self.slots.iter().any(|s| s.id == g.slot));
            let item = self.items.get(&g.item);
            miss(IdKind::Item, g.item, item.is_some());
            miss(IdKind::ItemName, g.item, t.items.contains_key(&g.item));
            let Some(d) = &g.detail else { continue };
            if let Some(item) = item {
                let levels = item.enhance.as_ref().and_then(|e| self.enhance.get(e)).map_or(1, Vec::len);
                miss(IdKind::EnhanceLevel, g.item, (d.enhance as usize) < levels);
            }
            for line in &d.lines {
                miss(IdKind::StatName, line.stat.into(), t.stats.contains_key(&line.stat));
            }
            for &skill in &d.skill_lines {
                miss(IdKind::SkillName, skill, t.skills.contains_key(&skill));
            }
        }
        for s in file.specialties.iter().flat_map(|s| &s.list) {
            miss(IdKind::SkillName, s.skill, t.skills.contains_key(&s.skill));
            let options = self.specialties.get(&s.skill);
            for p in s.picks.iter().filter(|p| p.part != 0) {
                let known = options.is_some_and(|o| o.iter().any(|o| o.part == p.part));
                miss(IdKind::SpecialtyPart, p.part, known && t.specialties.contains_key(&p.part));
            }
        }
        for b in file.boards.iter().flat_map(|b| &b.list) {
            let board = self.boards.get(&b.board);
            miss(IdKind::Board, b.board, board.is_some());
            miss(IdKind::BoardName, b.board, t.boards.contains_key(&b.board));
            for &id in &b.nodes {
                let node = board.and_then(|board| board.nodes.get(&id));
                miss(IdKind::Node, id, node.is_some());
                let Some(node) = node else { continue };
                miss(IdKind::NodeTitle, id, t.node_titles.contains_key(&node.title));
                for StatAmount(stat, _) in node.stats() {
                    miss(IdKind::StatName, stat.into(), t.stats.contains_key(&stat));
                }
                if let Some(SkillBonus(skill, _)) = node.skill {
                    miss(IdKind::SkillName, skill, t.skills.contains_key(&skill));
                }
            }
        }
        if let Some(titles) = &file.titles {
            for &id in titles.owned.iter().chain(titles.equipped.iter().map(|e| &e.title)) {
                miss(IdKind::Title, id, self.titles.contains_key(&id));
                miss(IdKind::TitleName, id, t.titles.contains_key(&id));
            }
        }
        let mut out: Vec<MissingId> = out.into_iter().collect();
        out.sort();
        out
    }

    /// Where the pack fails itself: an id or key a table names that its
    /// text or another table lacks. Empty for a whole pack.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.slot_problems(&mut out);
        self.item_problems(&mut out);
        self.enhance_problems(&mut out);
        self.stat_group_problems(&mut out);
        self.skill_problems(&mut out);
        self.board_problems(&mut out);
        self.title_problems(&mut out);
        out.sort();
        out
    }

    fn stat_problem(&self, out: &mut Vec<String>, at: &str, s: u16) {
        if !self.text.stats.contains_key(&s) {
            out.push(format!("{at}: stat {s} has no name"));
        }
    }

    fn slot_problems(&self, out: &mut Vec<String>) {
        for s in &self.slots {
            if !self.text.slots.contains_key(&s.key) {
                out.push(format!("slot {}: no text for {}", s.id, s.key));
            }
        }
    }

    fn item_problems(&self, out: &mut Vec<String>) {
        let t = &self.text;
        for (id, item) in &self.items {
            if !t.items.contains_key(id) {
                out.push(format!("item {id}: no name"));
            }
            if !t.grades.contains_key(&item.grade) {
                out.push(format!("item {id}: no text for grade {}", item.grade));
            }
            if item.enhance.as_ref().is_some_and(|e| !self.enhance.contains_key(e)) {
                out.push(format!("item {id}: no enhance table {}", item.enhance.as_deref().unwrap_or("")));
            }
            for a in item.base.iter().chain(&item.fixed) {
                self.stat_problem(out, &format!("item {id}"), a.0);
            }
        }
    }

    fn enhance_problems(&self, out: &mut Vec<String>) {
        for (group, levels) in &self.enhance {
            if levels.first().is_some_and(|l| !l.is_empty()) {
                out.push(format!("enhance {group}: level 0 adds stats"));
            }
            for a in levels.iter().flatten() {
                self.stat_problem(out, &format!("enhance {group}"), a.0);
            }
        }
    }

    fn stat_group_problems(&self, out: &mut Vec<String>) {
        for g in &self.stat_groups {
            if !self.text.stat_groups.contains_key(&g.key) {
                out.push(format!("stat group {}: no text", g.key));
            }
            for &s in &g.stats {
                self.stat_problem(out, &format!("stat group {}", g.key), s);
            }
        }
    }

    fn skill_problems(&self, out: &mut Vec<String>) {
        let t = &self.text;
        for (class, list) in &self.class_skills {
            for s in list.iter().filter(|s| !t.skills.contains_key(&s.skill)) {
                out.push(format!("{class:?} skill {}: no name", s.skill));
            }
        }
        for (skill, options) in &self.specialties {
            for o in options.iter().filter(|o| !t.specialties.contains_key(&o.part)) {
                out.push(format!("skill {skill} option {}: no text", o.part));
            }
        }
    }

    fn board_problems(&self, out: &mut Vec<String>) {
        let t = &self.text;
        for (id, board) in &self.boards {
            if !t.boards.contains_key(id) {
                out.push(format!("board {id}: no name"));
            }
            for (n, node) in &board.nodes {
                if !t.node_titles.contains_key(&node.title) {
                    out.push(format!("node {n}: no text for {}", node.title));
                }
                match (node.kind, node.stat, node.second_stat, node.skill) {
                    (NodeKind::Stat, Some(_), _, None) => {
                        for a in node.stats() {
                            self.stat_problem(out, &format!("node {n}"), a.0);
                        }
                    }
                    (NodeKind::SkillLevel, None, None, Some(SkillBonus(skill, _))) => {
                        if !t.skills.contains_key(&skill) {
                            out.push(format!("node {n}: skill {skill} has no name"));
                        }
                    }
                    (NodeKind::Start, None, None, None) => {}
                    (kind, ..) => out.push(format!("node {n}: a {kind:?} node with the wrong values")),
                }
            }
        }
    }

    fn title_problems(&self, out: &mut Vec<String>) {
        let t = &self.text;
        for (id, title) in &self.titles {
            if !t.titles.contains_key(id) {
                out.push(format!("title {id}: no name"));
            }
            if title.category != "None" && !t.title_categories.contains_key(&title.category) {
                out.push(format!("title {id}: no text for category {}", title.category));
            }
            for a in title.owned.iter().chain(&title.equipped) {
                self.stat_problem(out, &format!("title {id}"), a.0);
            }
        }
    }
}
