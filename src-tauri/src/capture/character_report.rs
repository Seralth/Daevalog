//! Your own character from the records the game sends only about you: gear,
//! stats, skills, the Daevanion board, titles. Replay only
//! (`checks::replay_character`); the character-file script runs it over the
//! packet logs.
//!
//! ```text
//! A2_REPLAY_CHARACTER=1        print the latest state as one JSON object
//!                              instead of the combat report
//! A2_REPLAY_FILE=a.txt:b.txt   the packet logs, oldest first
//! A2_GAME_DATA=dir             the game data export repo, for names
//!                              (default ~/Projects/aion2-data; ids stay either way)
//! ```
//!
//! Layouts, read from captures of 2026-10-06 (payload after the opcode):
//!
//! ```text
//! 33 36 self      <entity varint> <mask u32> <mask2 u8> [<len u8> <name>] <server u16>
//!                 <class u32> <u8> <level u32> <item level u32> <highest item level u32>
//! 56 36           <combat power u64> <highest combat power u64>
//! 1d 56           <item level u32> <highest item level u32>
//! 49 36 stats     <2 bytes> <count u8> (<EStat u16> <value i32>)* <8 bytes>   every zone load
//! 4a 36 stats     <entity varint> <count u8> (<EStat u16> <value i32>)* <8 bytes>   changes
//! 11 56 items     <2 bytes> <count u8> then items (below), at login
//! 1b 56 item      <01 01> then one item, when it moves or changes
//! 8c e2 / 8d e2   <01 01 01> <count u8> (<handle u64> <EEquipSlot u8>)*   equipped slots
//! 00 51 skills    <count u8> (<flag u8> <skill u32> <level> <base> <board bonus> <u8>
//!                 <gear bonus> <6 bytes>)*
//! 00 39 specialty <count u8> (<skill u32> <n u8> (<slot u8> <part u32>)*)*
//! 26 e2 board     <count u8> (<board u32> <n u8> <node u32>*)*
//! 79 56 titles    <n u8> <title u32>* <m u8> (<category u8> <title u32>)*
//! 5b 8d skins     <n u8> (<skin u32> <u32>)* then more, not decoded
//! ```
//!
//! An item is `<kind u8> <handle u64> <item id u32> <count u64> <container u8>
//! <slot u8>` and then, for gear, enchant, item level, equipped slot, socket
//! count, sockets, the soul binding and its random lines and skills. Those
//! records pack their flags as bits into bytes placed between the fields, so
//! the gear fields drift by a byte from item to item; they are found by what
//! they must equal (the item level is the table's plus the enchant, the slot
//! repeats), not by a fixed offset. Handles change on every login, so the
//! report keys gear by slot and item id.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Value};

use super::varint::read_varint;
use crate::entity::job_class::JobClass;

/// The equipped container in an item record.
const EQUIPPED: u8 = 11;

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn i32_at(b: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

// ----- names from the game data export -----

#[derive(Deserialize)]
struct Table<T> {
    #[serde(rename = "Properties")]
    properties: Rows<T>,
}
#[derive(Deserialize)]
struct Rows<T> {
    #[serde(rename = "Data")]
    data: Vec<T>,
}
#[derive(Deserialize, Default)]
struct Val {
    #[serde(rename = "Value", default)]
    value: i64,
}
#[derive(Deserialize, Default)]
struct Key {
    #[serde(rename = "Key", default)]
    key: String,
}

#[derive(Deserialize)]
struct ItemRow {
    #[serde(rename = "ID")]
    id: Val,
    #[serde(rename = "Desc", default)]
    desc: Key,
    #[serde(rename = "ItemType", default)]
    item_type: String,
    #[serde(rename = "ItemLevel", default)]
    item_level: i64,
    #[serde(rename = "ItemGrade", default)]
    grade: String,
    #[serde(rename = "EquipCategory", default)]
    category: Option<String>,
    #[serde(rename = "MagicStoneSlotCount", default)]
    sockets: i64,
}
#[derive(Deserialize)]
struct SkillRow {
    #[serde(rename = "ID")]
    id: Val,
    #[serde(rename = "SkillString_Key", default)]
    string_key: String,
    #[serde(rename = "SkillType", default)]
    skill_type: String,
}
#[derive(Deserialize)]
struct AcquireRow {
    #[serde(rename = "SkillId")]
    skill: Val,
    #[serde(rename = "AcquireType", default)]
    acquire: String,
}
#[derive(Deserialize)]
struct TitleRow {
    #[serde(rename = "ID")]
    id: Val,
    #[serde(rename = "Desc", default)]
    desc: Key,
    #[serde(rename = "EquipCategory", default)]
    category: String,
}
#[derive(Deserialize)]
struct BoardRow {
    #[serde(rename = "ID")]
    id: Val,
    #[serde(rename = "Title", default)]
    title: Key,
}
#[derive(Deserialize)]
struct NodeRow {
    #[serde(rename = "ID")]
    id: Val,
    #[serde(rename = "Type", default)]
    node_type: String,
    #[serde(rename = "Title", default)]
    title: Key,
    #[serde(rename = "Value01", default)]
    value1: String,
    #[serde(rename = "Value02", default)]
    value2: String,
    #[serde(rename = "CostDaevanionPoint", default)]
    cost: i64,
}
#[derive(Deserialize)]
struct SkinRow {
    #[serde(rename = "ID")]
    id: Val,
    #[serde(rename = "DescLight", default)]
    desc: Key,
}
#[derive(Deserialize)]
struct PartRow {
    #[serde(rename = "ID")]
    id: Val,
    #[serde(rename = "SpecializedSkillPartsDesc", default)]
    desc: String,
}
#[derive(Deserialize)]
struct CollectionRow {
    #[serde(rename = "ID")]
    id: Val,
    #[serde(rename = "Title", default)]
    title: Key,
}
#[derive(Deserialize)]
struct L10n {
    #[serde(rename = "Entries")]
    entries: HashMap<String, String>,
}

/// Item, skill, title and stat names. Empty when the export is missing: the
/// report then carries ids only.
#[derive(Default)]
pub(crate) struct Names {
    text: HashMap<String, String>,
    items: HashMap<u32, ItemRow>,
    skills: HashMap<u32, SkillRow>,
    acquire: HashMap<u32, String>,
    titles: HashMap<u32, TitleRow>,
    boards: HashMap<u32, String>,
    nodes: HashMap<u32, NodeRow>,
    skins: HashMap<u32, String>,
    parts: HashMap<u32, String>,
    collections: HashMap<u32, String>,
    collection_tabs: HashSet<u32>,
    stats: HashMap<u16, String>,
    slots: HashMap<u8, String>,
}

fn table<T: for<'de> Deserialize<'de>>(dir: &Path, name: &str) -> Vec<T> {
    let path = dir.join(format!("{name}.json"));
    let read = std::fs::File::open(&path)
        .map_err(|e| e.to_string())
        .and_then(|f| serde_json::from_reader::<_, Table<T>>(std::io::BufReader::new(f)).map_err(|e| e.to_string()));
    match read {
        Ok(t) => t.properties.data,
        Err(e) => {
            eprintln!("character: {name} not read: {e}");
            Vec::new()
        }
    }
}

/// Enum value names from a `.usmap` mapping file (uncompressed, version 3 or 4).
fn usmap_enums(path: &Path) -> HashMap<String, Vec<(u64, String)>> {
    let mut out = HashMap::new();
    let Ok(d) = std::fs::read(path) else { return out };
    let parse = || -> Option<HashMap<String, Vec<(u64, String)>>> {
        let version = *d.get(2)?;
        let mut o = 3;
        if version >= 1 {
            let versioned = i32_at(&d, o)?;
            o += 4;
            if versioned != 0 {
                return None;
            }
        }
        if *d.get(o)? != 0 {
            return None; // compressed
        }
        o += 9;
        let count = u32_at(&d, o)? as usize;
        o += 4;
        let mut names = Vec::with_capacity(count);
        for _ in 0..count {
            let len = if version >= 2 {
                o += 2;
                u16_at(&d, o - 2)? as usize
            } else {
                o += 1;
                *d.get(o - 1)? as usize
            };
            names.push(String::from_utf8_lossy(d.get(o..o + len)?).into_owned());
            o += len;
        }
        let enums = u32_at(&d, o)? as usize;
        o += 4;
        let mut map = HashMap::new();
        for _ in 0..enums {
            let name = names.get(u32_at(&d, o)? as usize)?.clone();
            o += 4;
            let n = if version >= 3 {
                o += 2;
                u16_at(&d, o - 2)? as usize
            } else {
                o += 1;
                *d.get(o - 1)? as usize
            };
            let mut values = Vec::with_capacity(n);
            for i in 0..n {
                if version >= 4 {
                    values.push((u64_at(&d, o)?, names.get(u32_at(&d, o + 8)? as usize)?.clone()));
                    o += 12;
                } else {
                    values.push((i as u64, names.get(u32_at(&d, o)? as usize)?.clone()));
                    o += 4;
                }
            }
            map.insert(name, values);
        }
        Some(map)
    };
    if let Some(m) = parse() {
        out = m;
    }
    out
}

impl Names {
    pub(crate) fn load(root: &Path) -> Names {
        let content = root.join("export/AION2/Content");
        let tables = content.join("Data/Table");
        let mut n = Names::default();
        let text = std::fs::File::open(content.join("L10N/Text/en-US/L10NString.json"))
            .ok()
            .and_then(|f| serde_json::from_reader::<_, L10n>(std::io::BufReader::new(f)).ok());
        match text {
            Some(t) => n.text = t.entries,
            None => eprintln!("character: no L10N strings under {}", content.display()),
        }
        for r in table::<ItemRow>(&tables, "Item") {
            n.items.insert(r.id.value as u32, r);
        }
        for r in table::<SkillRow>(&tables, "Skill") {
            n.skills.insert(r.id.value as u32, r);
        }
        for r in table::<AcquireRow>(&tables, "SkillAcquireData") {
            n.acquire.insert(r.skill.value as u32, r.acquire.rsplit("::").next().unwrap_or("").to_string());
        }
        for r in table::<TitleRow>(&tables, "Title") {
            n.titles.insert(r.id.value as u32, r);
        }
        for r in table::<BoardRow>(&tables, "DaevanionBoard") {
            n.boards.insert(r.id.value as u32, r.title.key);
        }
        for r in table::<NodeRow>(&tables, "DaevanionNode") {
            n.nodes.insert(r.id.value as u32, r);
        }
        for r in table::<SkinRow>(&tables, "Skin") {
            n.skins.insert(r.id.value as u32, r.desc.key);
        }
        for r in table::<PartRow>(&tables, "SpecializedSkillParts") {
            n.parts.insert(r.id.value as u32, r.desc);
        }
        for r in table::<CollectionRow>(&tables, "PeriodItemCollectionList") {
            n.collections.insert(r.id.value as u32, r.title.key);
        }
        for r in table::<CollectionRow>(&tables, "PeriodItemCollection") {
            n.collection_tabs.insert(r.id.value as u32);
        }
        // The newest mapping file, by name.
        let mut maps: Vec<PathBuf> = std::fs::read_dir(root.join("mappings"))
            .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "usmap")).collect())
            .unwrap_or_default();
        maps.sort();
        if let Some(path) = maps.last() {
            let enums = usmap_enums(path);
            for (v, name) in enums.get("EStat").into_iter().flatten() {
                n.stats.insert(*v as u16, name.rsplit("::").next().unwrap_or(name).to_string());
            }
            for (v, name) in enums.get("EEquipSlot").into_iter().flatten() {
                let s = name.rsplit("::").next().unwrap_or(name);
                n.slots.insert(*v as u8, s.strip_prefix('k').unwrap_or(s).to_string());
            }
        }
        n
    }

    fn text(&self, key: &str) -> Option<String> {
        self.text.get(key).cloned()
    }
    fn item_name(&self, id: u32) -> Option<String> {
        self.text(&format!("String_{}_body", self.items.get(&id)?.desc.key))
    }
    fn skill_name(&self, id: u32) -> Option<String> {
        self.text(&format!("SkillString_{}_skill_name", self.skills.get(&id)?.string_key))
    }
    fn stat(&self, id: u16) -> Value {
        let internal = self.stats.get(&id);
        let shown = internal.and_then(|s| self.text(&format!("String_StatName_{s}_body")));
        json!({ "id": id, "stat": internal, "name": shown })
    }
    fn skill(&self, id: u32) -> Value {
        json!({ "id": id, "name": self.skill_name(id) })
    }
    /// Is this a real item id? Without the table, any id in the game's range.
    fn is_item(&self, id: u32) -> bool {
        if self.items.is_empty() {
            (100_000_000..=999_999_999).contains(&id)
        } else {
            self.items.contains_key(&id)
        }
    }
    fn is_skill(&self, id: u32) -> bool {
        if self.skills.is_empty() {
            (1_000..=99_999_999).contains(&id)
        } else {
            self.skills.contains_key(&id)
        }
    }
}

// ----- the records -----

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Gear {
    pub enchant: u8,
    pub item_level: u32,
    pub sockets: u8,
    /// The socket bytes, kept only when any is set: their layout is unseen.
    pub socket_bytes: Vec<u8>,
    pub soul_bound: bool,
    pub lines: Vec<(u16, i32)>,
    pub skills: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Item {
    pub handle: [u8; 8],
    pub id: u32,
    pub count: u64,
    pub container: u8,
    pub slot: u8,
    pub gear: Option<Gear>,
}

/// One item starting at `at` (its kind byte) and ending before `end`.
/// `base_level` and `table_sockets` come from the item table when it is loaded.
pub(crate) fn decode_item(p: &[u8], at: usize, end: usize, base_level: Option<u32>, table_sockets: Option<u8>, equip: bool) -> Option<Item> {
    let handle: [u8; 8] = p.get(at + 1..at + 9)?.try_into().ok()?;
    let id = u32_at(p, at + 9)?;
    let count = u64_at(p, at + 13)?;
    let b = p.get(at + 21..end.min(p.len()))?;
    let (container, slot) = (*b.first()?, *b.get(1)?);
    let gear = if equip { decode_gear(b, container, slot, base_level, table_sockets) } else { None };
    Some(Item { handle, id, count, container, slot, gear })
}

/// The gear fields after `<container> <slot>`.
fn decode_gear(b: &[u8], container: u8, slot: u8, base_level: Option<u32>, table_sockets: Option<u8>) -> Option<Gear> {
    let equipped_slot = if container == EQUIPPED { slot } else { 0 };
    // The enchant byte, then 13 bytes, then the item level (u32). A packed
    // flag byte before the enchant moves both by one.
    let (j, enchant, item_level) = (12..=17).find_map(|j| {
        let enchant = *b.get(j)?;
        let level = u32_at(b, j + 14)?;
        let ok = match base_level {
            Some(base) => enchant <= 30 && level == base + enchant as u32,
            None => {
                enchant <= 30
                    && (1..=2000).contains(&level)
                    && (b.get(j + 18) == Some(&equipped_slot) || b.get(j + 19) == Some(&equipped_slot))
            }
        };
        ok.then_some((j, enchant, level))
    })?;
    // Then the equipped slot (0 in a bag), maybe after a flag byte, and the
    // socket count.
    let k = j + 18;
    let s = [k, k + 1].into_iter().find(|&s| {
        b.get(s) == Some(&equipped_slot)
            && b.get(s + 1).is_some_and(|&n| table_sockets.map_or(n <= 6, |t| n == t))
    })?;
    let sockets = b[s + 1];
    let q = s + 2 + 7 * sockets as usize;
    let socket_bytes = b.get(s + 2..q).filter(|v| v.iter().any(|&x| x != 0)).map(|v| v.to_vec()).unwrap_or_default();
    // Who the item is bound to: zero for gear never soul bound.
    let soul_bound = b.get(q..q + 8).is_some_and(|v| v.iter().any(|&x| x != 0));
    // The random lines: <n> (<stat u16> <value i32>)*, then the same stats again
    // with the values they could still gain; then <m> <skill u32>*.
    let mut lines = Vec::new();
    let mut skills = Vec::new();
    for t in q..b.len() {
        let c = b[t] as usize;
        if !(1..=6).contains(&c) || t + 2 + 12 * c > b.len() || b[t + 1 + 6 * c] != b[t] {
            continue;
        }
        let first: Vec<(u16, i32)> = (0..c).filter_map(|x| Some((u16_at(b, t + 1 + 6 * x)?, i32_at(b, t + 3 + 6 * x)?))).collect();
        let again: Vec<u16> = (0..c).filter_map(|x| u16_at(b, t + 2 + 6 * c + 6 * x)).collect();
        if first.iter().all(|&(s, v)| (1..700).contains(&s) && v != 0) && first.iter().map(|l| l.0).eq(again) {
            lines = first;
            let u = t + 2 + 12 * c;
            let m = b.get(u).copied().unwrap_or(0) as usize;
            if m <= 4 {
                skills = (0..m).filter_map(|x| u32_at(b, u + 1 + 4 * x)).collect();
            }
            break;
        }
    }
    Some(Gear { enchant, item_level, sockets, socket_bytes, soul_bound, lines, skills })
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SkillLevel {
    pub id: u32,
    pub level: u8,
    pub base: u8,
    pub board: u8,
    pub unknown: u8,
    pub gear: u8,
}

/// `00 51`: every skill with its level and where the levels come from.
pub(crate) fn decode_skills(p: &[u8], is_skill: impl Fn(u32) -> bool) -> Vec<SkillLevel> {
    let count = p.first().copied().unwrap_or(0) as usize;
    let looks = |o: usize| p.get(o).is_some_and(|&f| f != 0) && u32_at(p, o + 1).is_some_and(&is_skill);
    let mut out = Vec::new();
    let mut o = 1;
    while out.len() < count && o + 16 <= p.len() {
        // One entry in the 2026-10-06 login carried a byte more than the rest.
        if !looks(o) && looks(o + 1) {
            o += 1;
        }
        if !looks(o) {
            break;
        }
        out.push(SkillLevel {
            id: u32_at(p, o + 1).unwrap_or(0),
            level: p[o + 5],
            base: p[o + 6],
            board: p[o + 7],
            unknown: p[o + 8],
            gear: p[o + 9],
        });
        o += 16;
    }
    out
}

/// A `00 51` at login lists every skill (72 on 2026-10-06), but later ones
/// can list only a few: 1 skill at 20:37, 6 at a zone load at 21:40. A short
/// list updates those skills; a list at least half as long as the one held
/// replaces it.
pub(crate) fn merge_skills(held: &mut Vec<SkillLevel>, new: Vec<SkillLevel>) {
    if new.len() * 2 >= held.len() {
        *held = new;
        return;
    }
    for s in new {
        match held.iter_mut().find(|h| h.id == s.id) {
            Some(h) => *h = s,
            None => held.push(s),
        }
    }
}

/// `00 39`: per skill, the specialty part in each slot (0: none).
pub(crate) fn decode_specialties(p: &[u8]) -> Vec<(u32, Vec<(u8, u32)>)> {
    let mut out = Vec::new();
    let count = p.first().copied().unwrap_or(0);
    let mut o = 1;
    for _ in 0..count {
        let (Some(skill), Some(&n)) = (u32_at(p, o), p.get(o + 4)) else { break };
        o += 5;
        let mut slots = Vec::new();
        for _ in 0..n {
            let (Some(&slot), Some(part)) = (p.get(o), u32_at(p, o + 1)) else { break };
            slots.push((slot, part));
            o += 5;
        }
        out.push((skill, slots));
    }
    out
}

/// `26 e2`: the Daevanion boards and the nodes taken on each.
pub(crate) fn decode_boards(p: &[u8]) -> Vec<(u32, Vec<u32>)> {
    let mut out = Vec::new();
    let count = p.first().copied().unwrap_or(0);
    let mut o = 1;
    for _ in 0..count {
        let (Some(board), Some(&n)) = (u32_at(p, o), p.get(o + 4)) else { break };
        o += 5;
        let nodes: Vec<u32> = (0..n as usize).filter_map(|i| u32_at(p, o + 4 * i)).collect();
        o += 4 * n as usize;
        out.push((board, nodes));
    }
    out
}

/// Titles owned, and the equipped ones as (category, title).
type Titles = (Vec<u32>, Vec<(u8, u32)>);

/// `79 56`: titles owned, then the equipped ones by category.
pub(crate) fn decode_titles(p: &[u8]) -> Option<Titles> {
    let n = *p.first()? as usize;
    let owned: Vec<u32> = (0..n).filter_map(|i| u32_at(p, 1 + 4 * i)).collect();
    let o = 1 + 4 * n;
    let m = *p.get(o)? as usize;
    let equipped: Vec<(u8, u32)> = (0..m).filter_map(|i| Some((*p.get(o + 1 + 5 * i)?, u32_at(p, o + 2 + 5 * i)?))).collect();
    (owned.len() == n && equipped.len() == m).then_some((owned, equipped))
}

/// `<count u8> (<EStat u16> <value i32>)*` from `at`.
pub(crate) fn decode_stat_list(p: &[u8], at: usize) -> Vec<(u16, i32)> {
    let n = p.get(at).copied().unwrap_or(0) as usize;
    (0..n).map_while(|i| Some((u16_at(p, at + 1 + 6 * i)?, i32_at(p, at + 3 + 6 * i)?))).collect()
}

struct Profile {
    entity: i32,
    name: Option<String>,
    server: Option<u16>,
    class: Option<JobClass>,
    level: Option<u32>,
}

/// `33 36`, the record about the character you play.
fn decode_profile(p: &[u8]) -> Option<(Profile, Option<(u32, u32)>)> {
    let id = read_varint(p, 0);
    if id.length <= 0 {
        return None;
    }
    let mask2 = id.length as usize + 4;
    let mut after = mask2 + 1;
    let mut name = None;
    if p.get(mask2)? & 1 != 0 {
        let len = *p.get(after)? as usize;
        name = std::str::from_utf8(p.get(after + 1..after + 1 + len)?).ok().map(str::to_string);
        after += 1 + len;
    }
    let server = u16_at(p, after);
    let class = u32_at(p, after + 2).and_then(JobClass::from_roster_class);
    let level = u32_at(p, after + 7).filter(|l| (1..=99).contains(l));
    let item_level = u32_at(p, after + 11).zip(u32_at(p, after + 15));
    Some((Profile { entity: id.value, name, server, class, level }, item_level))
}

// ----- the state -----

#[derive(Default)]
pub(crate) struct Character {
    profile: Option<(Profile, String)>,
    item_level: Option<(u32, u32, String)>,
    combat_power: Option<(u64, u64, String)>,
    stats: BTreeMap<u16, i32>,
    stats_as_of: Option<String>,
    items: Vec<Item>,
    items_as_of: Option<String>,
    skills: Vec<SkillLevel>,
    skills_as_of: Option<String>,
    specialties: Vec<(u32, Vec<(u8, u32)>)>,
    specialties_as_of: Option<String>,
    boards: Vec<(u32, Vec<u32>)>,
    boards_as_of: Option<String>,
    titles: Option<(Titles, String)>,
    skins: Option<(Vec<u32>, String)>,
    quickslots: Option<(Vec<Value>, String)>,
    collections: Option<(Vec<u32>, String)>,
    last: Option<String>,
}

impl Character {
    /// One framed packet (length varint included), sent at `when`.
    pub(crate) fn feed(&mut self, names: &Names, when: &str, packet: &[u8]) {
        let o = read_varint(packet, 0).length.max(0) as usize;
        let (Some(op), Some(p)) = (packet.get(o..o + 2), packet.get(o + 2..)) else { return };
        let when = when.to_string();
        match [op[0], op[1]] {
            [0x33, 0x36] => {
                if let Some((profile, levels)) = decode_profile(p) {
                    if let Some((now, best)) = levels.filter(|l| (1..=100_000).contains(&l.0)) {
                        self.item_level = Some((now, best, when.clone()));
                    }
                    self.profile = Some((profile, when.clone()));
                }
            }
            [0x56, 0x36] => {
                if let (Some(now), Some(best)) = (u64_at(p, 0), u64_at(p, 8)) {
                    self.combat_power = Some((now, best, when.clone()));
                }
            }
            [0x1d, 0x56] => {
                if let (Some(now), Some(best)) = (u32_at(p, 0), u32_at(p, 4)) {
                    self.item_level = Some((now, best, when.clone()));
                }
            }
            [0x49, 0x36] => {
                let list = decode_stat_list(p, 2);
                if !list.is_empty() {
                    self.stats = list.into_iter().collect();
                    self.stats_as_of = Some(when.clone());
                }
            }
            [0x4a, 0x36] => {
                let id = read_varint(p, 0);
                if id.length > 0 {
                    let list = decode_stat_list(p, id.length as usize);
                    if !list.is_empty() {
                        self.stats.extend(list);
                        self.stats_as_of = Some(when.clone());
                    }
                }
            }
            [0x11, 0x56] => {
                self.items = self.decode_items(names, p, 3, p.len());
                self.items_as_of = Some(when.clone());
            }
            [0x1b, 0x56] => {
                let row = u32_at(p, 11).and_then(|id| names.items.get(&id));
                let item = p
                    .starts_with(&[1, 1])
                    .then(|| {
                        let level = row.map(|r| r.item_level as u32);
                        decode_item(p, 2, p.len(), level, row.map(|r| r.sockets as u8), true)
                    })
                    .flatten();
                if let Some(item) = item {
                    self.items.retain(|i| i.handle != item.handle);
                    self.items.push(item);
                    self.items_as_of = Some(when.clone());
                }
            }
            [0x8c, 0xe2] | [0x8d, 0xe2] => {
                let n = p.get(3).copied().unwrap_or(0) as usize;
                if p.len() >= 4 + 9 * n && p.starts_with(&[1, 1, 1]) {
                    let equipped: HashMap<[u8; 8], u8> = (0..n)
                        .filter_map(|i| Some((p.get(4 + 9 * i..12 + 9 * i)?.try_into().ok()?, p[12 + 9 * i])))
                        .collect();
                    for item in &mut self.items {
                        match equipped.get(&item.handle) {
                            Some(&slot) => {
                                item.container = EQUIPPED;
                                item.slot = slot;
                            }
                            // Taken off; the item's own record says where it went.
                            None if item.container == EQUIPPED => item.container = 0,
                            None => {}
                        }
                    }
                }
            }
            [0x00, 0x51] => {
                let list = decode_skills(p, |id| names.is_skill(id));
                if !list.is_empty() {
                    merge_skills(&mut self.skills, list);
                    self.skills_as_of = Some(when.clone());
                }
            }
            [0x00, 0x39] => {
                self.specialties = decode_specialties(p);
                self.specialties_as_of = Some(when.clone());
            }
            [0x26, 0xe2] => {
                self.boards = decode_boards(p);
                self.boards_as_of = Some(when.clone());
            }
            [0x79, 0x56] => {
                if let Some(titles) = decode_titles(p) {
                    self.titles = Some((titles, when.clone()));
                }
            }
            [0x5b, 0x8d] => {
                let n = p.first().copied().unwrap_or(0) as usize;
                let skins = (0..n).filter_map(|i| u32_at(p, 1 + 8 * i)).collect();
                self.skins = Some((skins, when.clone()));
            }
            [0x00, 0x56] => self.quickslots = Some((quickslots(names, p), when.clone())),
            [0x56, 0xe2] if !names.collections.is_empty() => {
                // Tab id, list id: progress and dates are not decoded.
                let lists = (0..p.len().saturating_sub(8))
                    .filter(|&i| u32_at(p, i).is_some_and(|t| names.collection_tabs.contains(&t)))
                    .filter_map(|i| u32_at(p, i + 4).filter(|l| names.collections.contains_key(l)))
                    .collect();
                self.collections = Some((lists, when.clone()));
            }
            _ => return,
        }
        self.last = Some(when);
    }

    fn decode_items(&self, names: &Names, p: &[u8], from: usize, end: usize) -> Vec<Item> {
        // The flag bytes make items variable in length, so each one is found by
        // its head: <kind 0|1> <handle> <real item id> <count> <container>.
        let starts: Vec<usize> = (from..p.len().saturating_sub(22))
            .filter(|&i| {
                p[i] <= 1
                    && u32_at(p, i + 9).is_some_and(|id| names.is_item(id))
                    && u64_at(p, i + 13).is_some_and(|c| (1..1_000_000_000_000).contains(&c))
                    && p[i + 21] <= 40
            })
            .collect();
        starts
            .iter()
            .enumerate()
            .filter_map(|(n, &i)| {
                let stop = starts.get(n + 1).copied().unwrap_or(end);
                let id = u32_at(p, i + 9)?;
                let row = names.items.get(&id);
                let equip = row.map_or(stop - i > 50, |r| r.item_type.ends_with("Equip"));
                decode_item(
                    p,
                    i,
                    stop,
                    row.map(|r| r.item_level as u32),
                    row.map(|r| r.sockets as u8),
                    equip,
                )
            })
            .collect()
    }

    pub(crate) fn to_json(&self, names: &Names, files: &[String]) -> Value {
        let profile = self.profile.as_ref().map(|(p, when)| {
            let class = p.class.map(|c| format!("{c:?}"));
            let shown = class.as_ref().and_then(|c| names.text(&format!("String_STR_CLASS_{}_body", c.to_uppercase())));
            json!({
                "name": p.name, "server": p.server, "class": class, "class_name": shown,
                "level": p.level, "entity": p.entity, "as_of": when,
            })
        });
        let mut gear: Vec<&Item> = self.items.iter().filter(|i| i.container == EQUIPPED).collect();
        gear.sort_by_key(|i| (i.slot, i.id));
        let gear: Vec<Value> = gear.iter().map(|i| item_json(names, i)).collect();
        let mut bag: Vec<&Item> = self.items.iter().filter(|i| i.container != EQUIPPED).collect();
        bag.sort_by_key(|i| (i.container, i.slot, i.id));
        let inventory: Vec<Value> = bag
            .iter()
            .map(|i| {
                let mut v = item_json(names, i);
                v["container"] = json!(i.container);
                v["count"] = json!(i.count);
                v
            })
            .collect();
        let stats: Vec<Value> = self
            .stats
            .iter()
            .map(|(&id, &value)| {
                let mut v = names.stat(id);
                v["value"] = json!(value);
                v
            })
            .collect();
        let skills: Vec<Value> = self
            .skills
            .iter()
            .map(|s| {
                let row = names.skills.get(&s.id);
                json!({
                    "id": s.id, "name": names.skill_name(s.id),
                    "type": row.map(|r| r.skill_type.rsplit("::").next().unwrap_or("").to_string()),
                    "acquire": names.acquire.get(&s.id),
                    "level": s.level, "base": s.base, "board_bonus": s.board, "gear_bonus": s.gear,
                    "unknown": s.unknown,
                })
            })
            .collect();
        let specialties: Vec<Value> = self
            .specialties
            .iter()
            .map(|(skill, slots)| {
                let slots: Vec<Value> = slots
                    .iter()
                    .map(|&(slot, part)| {
                        let desc = names
                            .parts
                            .get(&part)
                            .and_then(|k| names.text(&format!("SkillString_{k}_specialized_skill_desc")));
                        json!({ "slot": slot, "part": part, "desc": desc })
                    })
                    .collect();
                json!({ "skill": names.skill(*skill), "slots": slots })
            })
            .collect();
        let boards: Vec<Value> = self
            .boards
            .iter()
            .map(|(board, nodes)| {
                let nodes: Vec<Value> = nodes
                    .iter()
                    .map(|n| {
                        let row = names.nodes.get(n);
                        json!({
                            "id": n,
                            "type": row.map(|r| r.node_type.rsplit("::").next().unwrap_or("").to_string()),
                            "title": row.and_then(|r| names.text(&format!("String_{}_body", r.title.key))),
                            "value1": row.map(|r| r.value1.clone()),
                            "value2": row.map(|r| r.value2.clone()),
                            "cost": row.map(|r| r.cost),
                        })
                    })
                    .collect();
                let name = names.boards.get(board).and_then(|k| names.text(&format!("String_{k}_body")));
                json!({ "id": board, "name": name, "nodes": nodes })
            })
            .collect();
        let titles = self.titles.as_ref().map(|((owned, equipped), when)| {
            let title = |id: &u32| {
                let row = names.titles.get(id);
                json!({
                    "id": id,
                    "name": row.and_then(|r| names.text(&r.desc.key)),
                    "category": row.map(|r| r.category.rsplit("::").next().unwrap_or("").to_string()),
                })
            };
            let equipped: Vec<Value> = equipped
                .iter()
                .map(|(slot, id)| {
                    let mut v = title(id);
                    v["slot"] = json!(slot);
                    v
                })
                .collect();
            json!({ "owned": owned.iter().map(title).collect::<Vec<_>>(), "equipped": equipped, "as_of": when })
        });
        let skins = self.skins.as_ref().map(|(ids, when)| {
            let list: Vec<Value> =
                ids.iter().map(|id| json!({ "id": id, "name": names.skins.get(id).and_then(|k| names.text(k)) })).collect();
            json!({ "owned": list, "as_of": when })
        });
        let collections = self.collections.as_ref().map(|(ids, when)| {
            let list: Vec<Value> = ids
                .iter()
                .map(|id| json!({ "id": id, "name": names.collections.get(id).and_then(|k| names.text(&format!("String_{k}_body"))) }))
                .collect();
            json!({ "lists": list, "as_of": when })
        });
        json!({
            "source": { "files": files, "last_record": self.last },
            "character": profile,
            "item_level": self.item_level.as_ref().map(|(n, b, w)| json!({ "current": n, "highest": b, "as_of": w })),
            "combat_power": self.combat_power.as_ref().map(|(n, b, w)| json!({ "current": n, "highest": b, "as_of": w })),
            "gear": { "items": gear, "as_of": self.items_as_of },
            "inventory": { "items": inventory, "as_of": self.items_as_of },
            "stats": { "values": stats, "as_of": self.stats_as_of },
            "skills": { "list": skills, "as_of": self.skills_as_of },
            "specialties": { "list": specialties, "as_of": self.specialties_as_of },
            "daevanion": { "boards": boards, "as_of": self.boards_as_of },
            "titles": titles,
            "skins": skins,
            "quickslots": self.quickslots.as_ref().map(|(q, w)| json!({ "slots": q, "as_of": w })),
            "collections": collections,
        })
    }
}

fn item_json(names: &Names, i: &Item) -> Value {
    let row = names.items.get(&i.id);
    let mut v = json!({
        "key": format!("{}:{}", i.slot, i.id),
        "slot": i.slot,
        "slot_name": if i.container == EQUIPPED { names.slots.get(&i.slot).cloned() } else { None },
        "item_id": i.id,
        "name": names.item_name(i.id),
        "grade": row.map(|r| r.grade.rsplit("::").next().unwrap_or("").to_string()),
        "category": row.and_then(|r| r.category.as_ref()).map(|c| c.rsplit("::").next().unwrap_or("").to_string()),
    });
    if let Some(g) = &i.gear {
        let lines: Vec<Value> = g
            .lines
            .iter()
            .map(|&(s, value)| {
                let mut l = names.stat(s);
                l["value"] = json!(value);
                l
            })
            .collect();
        v["enchant"] = json!(g.enchant);
        v["item_level"] = json!(g.item_level);
        v["base_item_level"] = json!(row.map(|r| r.item_level));
        v["sockets"] = json!(g.sockets);
        v["socket_bytes"] = json!(g.socket_bytes.iter().map(|b| format!("{b:02x}")).collect::<String>());
        v["soul_bound"] = json!(g.soul_bound);
        v["random_lines"] = json!(lines);
        v["skills"] = json!(g.skills.iter().map(|s| names.skill(*s)).collect::<Vec<_>>());
    }
    v
}

/// `00 56`: the skills and items on the quick bars, in the order sent. A skill
/// slot holds the skill and four chained skills; slot numbers are not decoded.
fn quickslots(names: &Names, p: &[u8]) -> Vec<Value> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 4 <= p.len() {
        let Some(id) = u32_at(p, i) else { break };
        if names.is_skill(id) && id >= 1_000_000 && p.get(i + 4) == Some(&4) && u32_at(p, i + 5) == Some(id) {
            let chain: Vec<Value> =
                (1..4).filter_map(|k| u32_at(p, i + 5 + 4 * k)).filter(|&c| c != 0).map(|c| names.skill(c)).collect();
            out.push(json!({ "skill": names.skill(id), "chain": chain }));
            i += 21;
        } else if !names.items.is_empty() && names.items.contains_key(&id) {
            out.push(json!({ "item": { "id": id, "name": names.item_name(id) } }));
            i += 4;
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        let s: String = s.split_whitespace().collect();
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// `<len varint> <op> <payload>`.
    fn packet(op: [u8; 2], payload: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        let mut n = payload.len() as u32 + 2 + 4;
        loop {
            let b = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                v.push(b);
                break;
            }
            v.push(b | 0x80);
        }
        v.extend_from_slice(&op);
        v.extend_from_slice(payload);
        v
    }

    // Two equipped items from the 2026-10-06 login (`11 56`), the handles and the
    // owner bytes replaced. The orb has a packed flag byte after its item level;
    // the pauldrons carry three random lines and a skill.
    const ORB: &str = "01 0102030405060708 9f149806 0100000000000000 0b 01
        000000000000000000000000 0c 00000000f401000000000000 00 39000000 00 01 04
        00000000000000000000000000000000000000000000000000000000
        000000000000000000 03 00000000000000000000000000000000000000";
    const PAULDRONS: &str = "01 1112131415161718 56e88a0c 0100000000000000 0b 04
        0000000000000000000000000000 04 00000000000000000000000000 3a000000 04 04
        00000000000000000000000000000000000000000000000000000000
        a1a2a3a4a5a6a7a801 03 00000000000000000000000000
        03 c70037000000 380111000000 c8000f000000 03 c70000000000 380100000000 c80000000000
        01 70f9fe00 01 01 70f9fe00 0000000000";

    #[test]
    fn decodes_gear_whatever_byte_its_fields_drift_to() {
        let orb = hex(ORB);
        let item = decode_item(&orb, 0, orb.len(), None, None, true).expect("orb");
        assert_eq!((item.id, item.container, item.slot), (110_630_047, 11, 1));
        let g = item.gear.expect("gear");
        assert_eq!((g.enchant, g.item_level, g.sockets, g.soul_bound), (12, 57, 4, false));
        assert!(g.lines.is_empty());

        let pauldrons = hex(PAULDRONS);
        let item = decode_item(&pauldrons, 0, pauldrons.len(), Some(54), Some(4), true).expect("pauldrons");
        let g = item.gear.expect("gear");
        assert_eq!((g.enchant, g.item_level, g.sockets, g.soul_bound), (4, 58, 4, true));
        assert_eq!(g.lines, vec![(199, 55), (312, 17), (200, 15)]);
        assert_eq!(g.skills, vec![16_710_000]);
    }

    #[test]
    fn keys_gear_by_slot_and_follows_it_off_and_on() {
        let names = Names::default();
        let mut c = Character::default();
        let mut inv = hex("1002 0200");
        inv.extend(hex(ORB));
        inv.extend(hex(PAULDRONS));
        c.feed(&names, "t1", &packet([0x11, 0x56], &inv));
        let gear = |c: &Character| -> Vec<String> {
            c.to_json(&names, &[])["gear"]["items"].as_array().unwrap().iter().map(|v| v["key"].as_str().unwrap().to_string()).collect()
        };
        assert_eq!(gear(&c), ["1:110630047", "4:210430038"]);

        // The slot map without the orb: it is no longer worn.
        let mut map = hex("01010101");
        map.extend(hex("1112131415161718 04"));
        c.feed(&names, "t2", &packet([0x8d, 0xe2], &map));
        assert_eq!(gear(&c), ["4:210430038"]);
        // Its own record puts it in the bag, then the map puts it back on.
        let mut moved = hex("0101");
        moved.extend(hex(&ORB.replacen("0b 01", "01 4e", 1)));
        c.feed(&names, "t3", &packet([0x1b, 0x56], &moved));
        assert_eq!(gear(&c), ["4:210430038"]);
        let mut map = hex("01010102");
        map.extend(hex("0102030405060708 01 1112131415161718 04"));
        c.feed(&names, "t4", &packet([0x8d, 0xe2], &map));
        assert_eq!(gear(&c), ["1:110630047", "4:210430038"]);
    }

    #[test]
    fn stats_take_the_full_list_then_its_changes() {
        let names = Names::default();
        let mut c = Character::default();
        // 49 36: two bytes, the count, (stat, value)*, eight bytes.
        c.feed(&names, "t1", &packet([0x49, 0x36], &hex("0000 02 0100 0f000000 8000 ae010000 411e000000000000")));
        // 4a 36 from entity 1580: the weapon off.
        c.feed(&names, "t2", &packet([0x4a, 0x36], &hex("ac0c 02 0100 00000000 8000 08010000 0000000000000000")));
        assert_eq!(c.stats, BTreeMap::from([(1, 0), (128, 264)]));
        assert_eq!(c.stats_as_of.as_deref(), Some("t2"));
    }

    #[test]
    fn reads_skill_levels_past_an_odd_entry() {
        let skill = |flag: u8, id: u32, lv: [u8; 5]| {
            let mut v = vec![flag];
            v.extend(id.to_le_bytes());
            v.extend(lv);
            v.extend([0u8; 6]);
            v
        };
        let mut p = vec![3u8];
        p.extend(skill(1, 16_140_000, [13, 10, 2, 0, 1]));
        p.extend(skill(5, 4_900_202, [1, 1, 0, 0, 0]));
        p.push(3); // the byte more seen after a flag-5 entry
        p.extend(skill(1, 16_150_000, [6, 6, 0, 0, 0]));
        let list = decode_skills(&p, |id| (1_000..=99_999_999).contains(&id));
        assert_eq!(list.len(), 3);
        assert_eq!((list[0].level, list[0].base, list[0].board, list[0].gear), (13, 10, 2, 1));
        assert_eq!((list[2].id, list[2].level), (16_150_000, 6));
    }

    #[test]
    fn a_short_skill_list_updates_the_full_one() {
        let s = |id, level| SkillLevel { id, level, base: level, board: 0, unknown: 0, gear: 0 };
        let mut held = vec![s(1, 1), s(2, 2), s(3, 3), s(4, 4), s(5, 5)];
        merge_skills(&mut held, vec![s(2, 9)]);
        assert_eq!(held.len(), 5);
        assert_eq!(held[1].level, 9);
        merge_skills(&mut held, vec![s(1, 1), s(2, 2), s(6, 6)]);
        assert_eq!(held.iter().map(|h| h.id).collect::<Vec<_>>(), vec![1, 2, 6]);
    }

    #[test]
    fn reads_boards_specialties_and_titles() {
        let boards = decode_boards(&hex("02 33000000 02 a1c80700 51c80700 36000000 01 d13d0800"));
        assert_eq!(boards, vec![(51, vec![510_113, 510_033]), (54, vec![540_113])]);
        let spec = decode_specialties(&hex("01 f06df600 02 01 fa6df600 02 00000000"));
        assert_eq!(spec, vec![(16_150_000, vec![(1, 16_150_010), (2, 0)])]);
        let (owned, equipped) = decode_titles(&hex("02 3290b700 5984c600 01 01 3290b700")).unwrap();
        assert_eq!(owned, vec![12_030_002, 13_010_009]);
        assert_eq!(equipped, vec![(1, 12_030_002)]);
    }
}
