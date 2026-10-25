//! Your own character from the records the game sends only about you: gear,
//! stats, skills, the Daevanion board, titles. The records are read one
//! packet at a time, in the order the game sent them. `LiveCharacter` takes
//! the live meter's packets and gives the character file
//! (`character::file`); `report` replays packet logs into one JSON object,
//! which the `character-replay` tool and the replay report
//! (`A2_REPLAY_CHARACTER`) print. The report's names come from the game's own
//! tables (`Names::load`); without them, or where a table lacks one, the id
//! stays. The character file holds ids only.
//!
//! Layouts (payload after the opcode):
//!
//! ```text
//! 33 36 self      <entity varint> <mask u32> <mask2 u8> [<len u8> <name>] <server u16>
//!                 <class u32> <u8> <level u32> <item level u32> <highest item level u32>
//! 56 36           <combat power u64> <highest combat power u64>
//! 1d 56           <item level u32> <highest item level u32>
//! 49 36 stats     <2 bytes> <count varint> (<EStat u16> <value i32>)* <8 bytes>   every zone load
//! 4a 36 stats     <entity varint> <count varint> (<EStat u16> <value i32>)* <8 bytes>   changes
//! 11 56 items     <2 bytes> <count varint> then items (below), at login
//! 1b 56 item      <01 01> then one item, when it moves or changes
//! 8c e2 / 8d e2   <01 01 01> <count u8> (<handle u64> <EEquipSlot u8>)*   equipped slots
//! 00 51 skills    <count u8> (<flag u8> <skill u32> <level> <base> <board bonus> <u8>
//!                 <gear bonus> <6 bytes>)*
//! 00 39 specialty <count u8> (<skill u32> <n u8> (<slot u8> <part u32>)*)*
//! 26 e2 board     <count u8> (<board u32> <n u8> <node u32>*)*
//! 79 56 titles    <n varint> <title u32>* <m varint> (<category u8> <title u32>)*
//! 5b 8d skins     <n u8> (<skin u32> <u32>)* then more, not decoded
//! ```
//!
//! A login sends `79 56`, `8c e2`, `26 e2` and `11 56` (only then), and a few
//! seconds later the self record. Those records come before the self record
//! that names who logged in, so when it names another character, what was
//! read before the login is dropped.
//!
//! An item is `<kind u8> <handle u64> <item id u32> <count u64> <container u8>
//! <slot u8>`; kind 1 is gear (always a count of 1), kind 0 anything else.
//! Gear then carries the maker's name (`<len> <name>`, empty unless
//! crafted), enchant, item level, equipped slot, socket count, the sockets
//! (`<manastone item u32> <stat u16> <u8>` each), the soul binding (8
//! bytes), the random lines `<n> (<stat u16> <value i32>)*` and the same
//! stats again, then the skill lines `<m> (<skill u32> <u8>)*` and the same
//! skills again. Those records pack their flags as bits into bytes placed
//! between the fields, so the gear fields drift by a byte from item to
//! item; they are found by what they must equal (the slot repeats; with the
//! item table, the socket count is the table's and the item level is at
//! least the table's plus the enchant: manastones raise it), not by a fixed
//! offset. A manastone's id in a socket reads like the head of another
//! item, so no item starts inside the sockets. Handles change on every
//! login, so gear is keyed by slot and item id.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Value};

use super::byte_queue::ByteQueue;
use super::framing::take_packets;
use super::names::clean_name_field;
use super::packet_log::LogLine;
use super::varint::{next_varint, varint_at, u16_at, u32_at, u64_at};
use crate::character::file::{self as cf, CharacterFile};
use crate::entity::job_class::JobClass;

/// The equipped container in an item record.
const EQUIPPED: u8 = 11;
/// The kind byte of a gear record.
const GEAR: u8 = 1;
/// No item level the game sends is higher.
const MAX_ITEM_LEVEL: u32 = 2_000;
/// No gear has more skill lines.
const MAX_SKILL_LINES: usize = 4;
/// Records sent only at login.
const LOGIN_RECORDS: [[u8; 2]; 4] = [[0x79, 0x56], [0x8c, 0xe2], [0x26, 0xe2], [0x11, 0x56]];

fn i32_at(b: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
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
pub struct Names {
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

/// Enum value names from the game's type mapping file (uncompressed,
/// version 3 or 4).
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
    /// The names in a game data export (its tables and text under
    /// `export/AION2/Content` below `root`); a table that does not read leaves
    /// its names out.
    pub fn load(root: &Path) -> Names {
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
            (1..=99_999_999).contains(&id)
        } else {
            self.skills.contains_key(&id)
        }
    }
}

// ----- the records -----

#[derive(Clone, Debug, PartialEq)]
struct Gear {
    enchant: u8,
    item_level: u32,
    sockets: u8,
    /// The socket bytes (`<manastone item u32> <stat u16> <u8>` each), kept
    /// only when any is set.
    socket_bytes: Vec<u8>,
    soul_bound: bool,
    lines: Vec<(u16, i32)>,
    skills: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq)]
struct Item {
    handle: [u8; 8],
    id: u32,
    count: u64,
    container: u8,
    slot: u8,
    gear: Option<Gear>,
}

/// One item starting at `at` (its kind byte) and ending before `end`.
/// `base_level` and `table_sockets` come from the item table when it is loaded.
fn decode_item(p: &[u8], at: usize, end: usize, base_level: Option<u32>, table_sockets: Option<u8>) -> Option<Item> {
    let handle: [u8; 8] = p.get(at + 1..at + 9)?.try_into().ok()?;
    let id = u32_at(p, at + 9)?;
    let count = u64_at(p, at + 13)?;
    let b = p.get(at + 21..end.min(p.len()))?;
    let (container, slot) = (*b.first()?, *b.get(1)?);
    let gear = if p[at] == GEAR { decode_gear(b, base_level, table_sockets) } else { None };
    Some(Item { handle, id, count, container, slot, gear })
}

/// Where a gear record's fields sit, from its `<container>`.
struct GearHead {
    enchant: u8,
    item_level: u32,
    sockets: u8,
    /// The first socket.
    sockets_at: usize,
}

impl GearHead {
    /// Past the sockets and the soul binding: no other item starts before.
    fn end(&self) -> usize {
        self.sockets_at + 7 * self.sockets as usize + 8
    }
}

/// The enchant, item level and sockets of the gear record `b` (from its
/// `<container>`).
fn gear_head(b: &[u8], container: u8, slot: u8, base_level: Option<u32>, table_sockets: Option<u8>) -> Option<GearHead> {
    let equipped_slot = if container == EQUIPPED { slot } else { 0 };
    // The enchant byte, then 13 bytes, then the item level (u32). A packed
    // flag byte before the enchant moves both by one.
    (12..=17).find_map(|j| {
        let enchant = *b.get(j)?;
        let item_level = u32_at(b, j + 14)?;
        let level_fits = match base_level {
            Some(base) => item_level >= base + enchant as u32,
            None => item_level >= 1,
        };
        if enchant > 30 || !level_fits || item_level > MAX_ITEM_LEVEL {
            return None;
        }
        // Then the equipped slot (0 in a bag), maybe after a flag byte, and
        // the socket count. In a bag both can read as a slot; the soul
        // binding is followed by a byte and then 3, so that tells them apart.
        let fits = |s: usize| {
            b.get(s) == Some(&equipped_slot) && b.get(s + 1).is_some_and(|&n| table_sockets.map_or(n <= 6, |t| n == t))
        };
        let then_3 = |s: usize| b.get(s + 2 + 7 * b[s + 1] as usize + 9) == Some(&3);
        let fitting: Vec<usize> = [j + 18, j + 19].into_iter().filter(|&s| fits(s)).collect();
        let s = fitting.iter().copied().find(|&s| then_3(s)).or(fitting.first().copied())?;
        Some(GearHead { enchant, item_level, sockets: b[s + 1], sockets_at: s + 2 })
    })
}

/// The gear record `b` (from its `<container>`) and where its fields sit,
/// and how many bytes a maker's name takes. A crafted item names its maker
/// at its sixth byte (`<len> <name>`; the length is 0 on other gear), so its
/// fields come that much later: they are read as if the name were empty.
/// The name is never kept.
fn find_gear(b: &[u8], base_level: Option<u32>, table_sockets: Option<u8>) -> Option<(Cow<'_, [u8]>, GearHead, usize)> {
    let (container, slot) = (*b.first()?, *b.get(1)?);
    if let Some(head) = gear_head(b, container, slot, base_level, table_sockets) {
        return Some((Cow::Borrowed(b), head, 0));
    }
    let name = *b.get(5)? as usize;
    clean_name_field(b.get(6..6 + name)?)?;
    let unnamed = [&b[..6], &b[6 + name..]].concat();
    let head = gear_head(&unnamed, container, slot, base_level, table_sockets)?;
    Some((Cow::Owned(unnamed), head, name))
}

/// The gear fields after `<container> <slot>`.
fn decode_gear(b: &[u8], base_level: Option<u32>, table_sockets: Option<u8>) -> Option<Gear> {
    let (b, head, _) = find_gear(b, base_level, table_sockets)?;
    let q = head.end() - 8;
    let socket_bytes =
        b.get(head.sockets_at..q).filter(|v| v.iter().any(|&x| x != 0)).map(|v| v.to_vec()).unwrap_or_default();
    // Who the item is bound to: zero for gear never soul bound.
    let soul_bound = b.get(q..q + 8).is_some_and(|v| v.iter().any(|&x| x != 0));
    // The random lines: <n> (<stat u16> <value i32>)*, then the same stats again
    // with the values they could still gain; then the skill lines.
    let mut lines = Vec::new();
    let mut skills = Vec::new();
    for t in q..b.len() {
        let c = b[t] as usize;
        if !(1..=6).contains(&c) || t + 2 + 12 * c > b.len() || b[t + 1 + 6 * c] != b[t] {
            continue;
        }
        let first: Vec<(u16, i32)> = (0..c).filter_map(|x| Some((u16_at(&b, t + 1 + 6 * x)?, i32_at(&b, t + 3 + 6 * x)?))).collect();
        let again: Vec<u16> = (0..c).filter_map(|x| u16_at(&b, t + 2 + 6 * c + 6 * x)).collect();
        if first.iter().all(|&(s, v)| (1..700).contains(&s) && v != 0) && first.iter().map(|l| l.0).eq(again) {
            lines = first;
            skills = skill_lines(&b, t + 2 + 12 * c).unwrap_or_default();
            break;
        }
    }
    let GearHead { enchant, item_level, sockets, .. } = head;
    Some(Gear { enchant, item_level, sockets, socket_bytes, soul_bound, lines, skills })
}

/// `<m> (<skill u32> <u8>)*` at `at`, then the same skills again.
fn skill_lines(b: &[u8], at: usize) -> Option<Vec<u32>> {
    let m = *b.get(at)? as usize;
    let read = |from: usize| (0..m).map(|x| u32_at(b, from + 1 + 5 * x)).collect::<Option<Vec<u32>>>();
    let again = at + 1 + 5 * m;
    let first = read(at)?;
    (m <= MAX_SKILL_LINES && b.get(again) == Some(&b[at]) && read(again)? == first).then_some(first)
}

#[derive(Clone, Debug, PartialEq)]
struct SkillLevel {
    id: u32,
    level: u8,
    base: u8,
    board: u8,
    unknown: u8,
    gear: u8,
}

/// `00 51`: every skill with its level and where the levels come from. An
/// entry is 16 bytes, a few longer (flag 5 by one byte, flag 3 by three), so
/// each is found within a few bytes of where the one before ends, by its
/// shape: flag 1, 3 or 5, a skill, and a level that is the sum of its parts.
fn decode_skills(p: &[u8], is_skill: impl Fn(u32) -> bool) -> Vec<SkillLevel> {
    let count = p.first().copied().unwrap_or(0) as usize;
    let entry = |o: usize| -> Option<SkillLevel> {
        let e = p.get(o..o + 16)?;
        let id = u32_at(e, 1)?;
        let &[level, base, board, unknown, gear] = &e[5..10] else { return None };
        let parts = [base, board, unknown, gear].iter().map(|&b| b as u32).sum::<u32>();
        (matches!(e[0], 1 | 3 | 5) && is_skill(id) && level >= 1 && level as u32 == parts)
            .then_some(SkillLevel { id, level, base, board, unknown, gear })
    };
    let mut out = Vec::new();
    let mut o = 1;
    while out.len() < count {
        let Some((at, skill)) = (o..o + 4).find_map(|at| Some((at, entry(at)?))) else { break };
        out.push(skill);
        o = at + 16;
    }
    out
}

/// A `00 51` at login lists every skill, but later ones can list only a
/// few (1 skill, or 6 at a zone load). A short
/// list updates those skills; a list at least half as long as the one held
/// replaces it.
fn merge_skills(held: &mut Vec<SkillLevel>, new: Vec<SkillLevel>) {
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
fn decode_specialties(p: &[u8]) -> Vec<(u32, Vec<(u8, u32)>)> {
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
fn decode_boards(p: &[u8]) -> Vec<(u32, Vec<u32>)> {
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

/// `79 56`: titles owned, then the equipped ones by category. Both counts
/// are varints: from 128 titles the owned count takes two bytes, and read as
/// one byte it puts every id a byte early.
fn decode_titles(p: &[u8]) -> Option<Titles> {
    let mut o = 0;
    let n = next_varint(p, &mut o)? as usize;
    let owned: Vec<u32> = (0..n).map_while(|i| u32_at(p, o + 4 * i)).collect();
    o += 4 * n;
    let m = next_varint(p, &mut o)? as usize;
    let equipped: Vec<(u8, u32)> = (0..m).map_while(|i| Some((*p.get(o + 5 * i)?, u32_at(p, o + 1 + 5 * i)?))).collect();
    (owned.len() == n && equipped.len() == m).then_some((owned, equipped))
}

/// `<count varint> (<EStat u16> <value i32>)*` from `at`. From 128 stats the
/// count takes two bytes; read as one byte it puts every stat id a byte
/// early.
fn decode_stat_list(p: &[u8], at: usize) -> Vec<(u16, i32)> {
    let mut from = at;
    let Some(n) = next_varint(p, &mut from) else { return Vec::new() };
    (0..n as usize).map_while(|i| Some((u16_at(p, from + 6 * i)?, i32_at(p, from + 2 + 6 * i)?))).collect()
}

struct Profile {
    entity: i32,
    name: Option<String>,
    server: Option<u16>,
    class: Option<JobClass>,
    level: Option<u32>,
}

impl Profile {
    /// The server and name that tell characters apart.
    fn who(&self) -> Option<(u16, &str)> {
        Some((self.server?, self.name.as_deref()?))
    }
}

/// `33 36`, the record about the character you play.
fn decode_profile(p: &[u8]) -> Option<(Profile, Option<(u32, u32)>)> {
    let id = varint_at(p, 0);
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
    let class = u32_at(p, after + 2).and_then(JobClass::job_from_party_list);
    let level = u32_at(p, after + 7).filter(|l| (1..=99).contains(l));
    let item_level = u32_at(p, after + 11).zip(u32_at(p, after + 15));
    Some((Profile { entity: id.value, name, server, class, level }, item_level))
}

// ----- the report -----

/// Your character's latest state from packet logs, oldest first: every
/// packet goes through in order, each stream's bytes framed on their own,
/// stamped as written. The names of the logs go into `source.files`.
pub fn report(logs: &[CaptureText], names: &Names) -> Value {
    let mut character = Character::default();
    // The timestamps as the logs wrote them, by `Stamp::line`.
    let mut written: Vec<&str> = Vec::new();
    for log in logs {
        let mut pending: HashMap<&str, ByteQueue> = HashMap::new();
        for line in log.text.lines().filter_map(LogLine::parse) {
            let at = Stamp { ms: line.at_ms, line: written.len() };
            let mut taken = false;
            for packet in take_packets(pending.entry(line.stream).or_default(), &line.bytes) {
                taken |= character.feed(names, at, &packet);
            }
            if taken {
                written.push(line.stamp);
            }
        }
    }
    let files: Vec<String> = logs.iter().map(|log| log.name.to_string()).collect();
    character.to_json(names, &files, &written)
}

/// One packet log for `report`: its name, which goes into `source.files`, and
/// its text.
pub struct CaptureText<'a> {
    pub name: &'a str,
    pub text: &'a str,
}

// ----- the state -----

/// When a record came: its capture time, and for `report` the index of the
/// timestamp its log wrote (0 live).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    ms: i64,
    line: usize,
}

#[derive(Default)]
struct Character {
    profile: Option<(Profile, Stamp)>,
    item_level: Option<(u32, u32, Stamp)>,
    combat_power: Option<(u64, u64, Stamp)>,
    stats: BTreeMap<u16, i32>,
    stats_as_of: Option<Stamp>,
    items: Vec<Item>,
    items_as_of: Option<Stamp>,
    skills: Vec<SkillLevel>,
    skills_as_of: Option<Stamp>,
    specialties: Vec<(u32, Vec<(u8, u32)>)>,
    specialties_as_of: Option<Stamp>,
    boards: Vec<(u32, Vec<u32>)>,
    boards_as_of: Option<Stamp>,
    titles: Option<(Titles, Stamp)>,
    skins: Option<(Vec<u32>, Stamp)>,
    quickslots: Option<(Vec<Value>, Stamp)>,
    collections: Option<(Vec<u32>, Stamp)>,
    last: Option<Stamp>,
    /// When the whole stat sheet (`49 36`), the login's item list and the
    /// login's skill list last came: changes alone are not the whole of it.
    whole_stats: Option<i64>,
    whole_items: Option<i64>,
    whole_skills: Option<i64>,
    /// When the records of a login began, until the self record that names
    /// who logged in.
    login: Option<i64>,
}

impl Character {
    /// One framed packet (length varint included), sent at `at`. True when
    /// it was one of your character's records.
    fn feed(&mut self, names: &Names, at: Stamp, packet: &[u8]) -> bool {
        let o = varint_at(packet, 0).length.max(0) as usize;
        let (Some(op), Some(p)) = (packet.get(o..o + 2), packet.get(o + 2..)) else { return false };
        let op = [op[0], op[1]];
        if LOGIN_RECORDS.contains(&op) && self.login.is_none() {
            self.login = Some(at.ms);
        }
        match op {
            [0x33, 0x36] => {
                if let Some((profile, levels)) = decode_profile(p) {
                    self.named(&profile, at.ms);
                    if let Some((now, best)) = levels.filter(|l| (1..=100_000).contains(&l.0)) {
                        self.item_level = Some((now, best, at));
                    }
                    self.profile = Some((profile, at));
                }
            }
            [0x56, 0x36] => {
                if let (Some(now), Some(best)) = (u64_at(p, 0), u64_at(p, 8)) {
                    self.combat_power = Some((now, best, at));
                }
            }
            [0x1d, 0x56] => {
                if let (Some(now), Some(best)) = (u32_at(p, 0), u32_at(p, 4)) {
                    self.item_level = Some((now, best, at));
                }
            }
            [0x49, 0x36] => {
                let list = decode_stat_list(p, 2);
                if !list.is_empty() {
                    self.stats = list.into_iter().collect();
                    self.stats_as_of = Some(at);
                    self.whole_stats = Some(at.ms);
                }
            }
            [0x4a, 0x36] => {
                // Past the entity varint, whatever its value.
                let mut o = 0;
                next_varint(p, &mut o);
                if o > 0 {
                    let list = decode_stat_list(p, o);
                    if !list.is_empty() {
                        self.stats.extend(list);
                        self.stats_as_of = Some(at);
                    }
                }
            }
            [0x11, 0x56] => {
                self.items = decode_items(names, p, 3);
                self.items_as_of = Some(at);
                self.whole_items = Some(at.ms);
            }
            [0x1b, 0x56] => {
                let row = u32_at(p, 11).and_then(|id| names.items.get(&id));
                let item = p
                    .starts_with(&[1, 1])
                    .then(|| decode_item(p, 2, p.len(), row.map(|r| r.item_level as u32), row.map(|r| r.sockets as u8)))
                    .flatten();
                if let Some(item) = item {
                    self.items.retain(|i| i.handle != item.handle);
                    self.items.push(item);
                    self.items_as_of = Some(at);
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
                    // The login's list is every skill; later ones can hold a few.
                    if self.login.is_some() {
                        self.whole_skills = Some(at.ms);
                    }
                    merge_skills(&mut self.skills, list);
                    self.skills_as_of = Some(at);
                }
            }
            [0x00, 0x39] => {
                self.specialties = decode_specialties(p);
                self.specialties_as_of = Some(at);
            }
            [0x26, 0xe2] => {
                self.boards = decode_boards(p);
                self.boards_as_of = Some(at);
            }
            [0x79, 0x56] => {
                if let Some(titles) = decode_titles(p) {
                    self.titles = Some((titles, at));
                }
            }
            [0x5b, 0x8d] => {
                let n = p.first().copied().unwrap_or(0) as usize;
                let skins = (0..n).filter_map(|i| u32_at(p, 1 + 8 * i)).collect();
                self.skins = Some((skins, at));
            }
            [0x00, 0x56] => self.quickslots = Some((quickslots(names, p), at)),
            [0x56, 0xe2] if !names.collections.is_empty() => {
                // Tab id, list id: progress and dates are not decoded.
                let lists = (0..p.len().saturating_sub(8))
                    .filter(|&i| u32_at(p, i).is_some_and(|t| names.collection_tabs.contains(&t)))
                    .filter_map(|i| u32_at(p, i + 4).filter(|l| names.collections.contains_key(l)))
                    .collect();
                self.collections = Some((lists, at));
            }
            _ => return false,
        }
        self.last = Some(at);
        true
    }

    /// A self record names `profile`, at `ms`. When it names another
    /// character than the one held, what was read before the login (or
    /// before this record, without a login) is dropped: it was the other's.
    fn named(&mut self, profile: &Profile, ms: i64) {
        let login = self.login.take().unwrap_or(ms);
        let held = self.profile.as_ref().and_then(|(p, _)| p.who());
        if held.is_none() || profile.who().is_none() || held == profile.who() {
            return;
        }
        let before = |at: Option<i64>| at.is_none_or(|at| at < login);
        let stale = |stamp: Option<Stamp>| stamp.is_some_and(|s| s.ms < login);
        if before(self.whole_stats) {
            self.stats.clear();
            self.stats_as_of = None;
            self.whole_stats = None;
        }
        if before(self.whole_items) {
            self.items.clear();
            self.items_as_of = None;
            self.whole_items = None;
        }
        if before(self.whole_skills) {
            self.skills.clear();
            self.skills_as_of = None;
            self.whole_skills = None;
        }
        if stale(self.specialties_as_of) {
            self.specialties.clear();
            self.specialties_as_of = None;
        }
        if stale(self.boards_as_of) {
            self.boards.clear();
            self.boards_as_of = None;
        }
        self.item_level = self.item_level.take().filter(|l| l.2.ms >= login);
        self.combat_power = self.combat_power.take().filter(|c| c.2.ms >= login);
        self.titles = self.titles.take().filter(|t| t.1.ms >= login);
        self.skins = self.skins.take().filter(|s| s.1.ms >= login);
        self.quickslots = self.quickslots.take().filter(|q| q.1.ms >= login);
        self.collections = self.collections.take().filter(|c| c.1.ms >= login);
    }
}

/// The items of a login's item list, from `from`.
fn decode_items(names: &Names, p: &[u8], from: usize) -> Vec<Item> {
    // The flag bytes make items variable in length, so each one is found by
    // its head: <kind 0|1> <handle> <real item id> <count> <container>.
    // Without the item table only gear is found, by its kind and count.
    let tables = !names.items.is_empty();
    let starts: Vec<usize> = (from..p.len().saturating_sub(22))
        .filter(|&i| {
            let id = u32_at(p, i + 9).unwrap_or(0);
            let count = u64_at(p, i + 13).unwrap_or(0);
            let head = if tables { p[i] <= 1 && names.is_item(id) } else { p[i] == GEAR && count == 1 && names.is_item(id) };
            head && (1..1_000_000_000_000).contains(&count) && p[i + 21] <= 40
        })
        .collect();
    let next = |from: usize| starts.iter().copied().find(|&s| s >= from).unwrap_or(p.len());
    let mut items = Vec::new();
    // Where the gear read last ends: a head before it is a manastone's id.
    let mut taken = 0;
    for &i in &starts {
        if i < taken {
            continue;
        }
        let row = u32_at(p, i + 9).and_then(|id| names.items.get(&id));
        let (level, sockets) = (row.map(|r| r.item_level as u32), row.map(|r| r.sockets as u8));
        let gear_end = (p[i] == GEAR)
            .then(|| p.get(i + 21..).and_then(|b| find_gear(b, level, sockets)))
            .flatten()
            .map(|(_, head, name)| i + 21 + name + head.end());
        // Without the table, a head is only taken for gear that reads.
        if !tables && gear_end.is_none() {
            continue;
        }
        if let Some(item) = decode_item(p, i, next(gear_end.unwrap_or(i + 1)), level, sockets) {
            items.push(item);
        }
        taken = gear_end.unwrap_or(taken);
    }
    items
}

impl Character {
    /// The worn items, by slot.
    fn worn(&self) -> Vec<&Item> {
        let mut worn: Vec<&Item> = self.items.iter().filter(|i| i.container == EQUIPPED).collect();
        worn.sort_by_key(|i| (i.slot, i.id));
        worn
    }

    /// The report: `files` its logs, `written` the timestamps they wrote.
    fn to_json(&self, names: &Names, files: &[String], written: &[&str]) -> Value {
        let text = |at: &Stamp| written.get(at.line).copied();
        let as_of = |at: Option<Stamp>| at.as_ref().and_then(text);
        let profile = self.profile.as_ref().map(|(p, when)| {
            let class = p.class.map(|c| format!("{c:?}"));
            let shown = class.as_ref().and_then(|c| names.text(&format!("String_STR_CLASS_{}_body", c.to_uppercase())));
            json!({
                "name": p.name, "server": p.server, "class": class, "class_name": shown,
                "level": p.level, "entity": p.entity, "as_of": text(when),
            })
        });
        let gear: Vec<Value> = self.worn().into_iter().map(|i| item_json(names, i)).collect();
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
            json!({ "owned": owned.iter().map(title).collect::<Vec<_>>(), "equipped": equipped, "as_of": text(when) })
        });
        let skins = self.skins.as_ref().map(|(ids, when)| {
            let list: Vec<Value> =
                ids.iter().map(|id| json!({ "id": id, "name": names.skins.get(id).and_then(|k| names.text(k)) })).collect();
            json!({ "owned": list, "as_of": text(when) })
        });
        let collections = self.collections.as_ref().map(|(ids, when)| {
            let list: Vec<Value> = ids
                .iter()
                .map(|id| json!({ "id": id, "name": names.collections.get(id).and_then(|k| names.text(&format!("String_{k}_body"))) }))
                .collect();
            json!({ "lists": list, "as_of": text(when) })
        });
        json!({
            "source": { "files": files, "last_record": as_of(self.last) },
            "character": profile,
            "item_level": self.item_level.as_ref().map(|(n, b, w)| json!({ "current": n, "highest": b, "as_of": text(w) })),
            "combat_power": self.combat_power.as_ref().map(|(n, b, w)| json!({ "current": n, "highest": b, "as_of": text(w) })),
            "gear": { "items": gear, "as_of": as_of(self.items_as_of) },
            "inventory": { "items": inventory, "as_of": as_of(self.items_as_of) },
            "stats": { "values": stats, "as_of": as_of(self.stats_as_of) },
            "skills": { "list": skills, "as_of": as_of(self.skills_as_of) },
            "specialties": { "list": specialties, "as_of": as_of(self.specialties_as_of) },
            "daevanion": { "boards": boards, "as_of": as_of(self.boards_as_of) },
            "titles": titles,
            "skins": skins,
            "quickslots": self.quickslots.as_ref().map(|(q, w)| json!({ "slots": q, "as_of": text(w) })),
            "collections": collections,
        })
    }

    /// The file of the character the game last named. Gear, stats and
    /// skills only once read whole: an item, a stat change or a few skills
    /// alone leave the section out. None until a self record gives the
    /// server and name, and while a login is under way.
    fn file(&self) -> Option<CharacterFile> {
        if self.login.is_some() {
            return None;
        }
        let (profile, _) = self.profile.as_ref()?;
        let (server, name) = profile.who()?;
        let mut file = CharacterFile::new(server, profile.class, profile.level);
        file.name = Some(name.to_string());
        let levels = |current: u64, highest: u64, at: Stamp| cf::Levels { as_of_ms: at.ms, current, highest };
        file.item_level = self.item_level.map(|(now, best, at)| levels(now.into(), best.into(), at));
        file.combat_power = self.combat_power.map(|(now, best, at)| levels(now, best, at));
        let whole = |at: Option<Stamp>, whole: Option<i64>| at.filter(|_| whole.is_some()).map(|at| at.ms);
        file.gear = whole(self.items_as_of, self.whole_items)
            .map(|as_of_ms| cf::Gear { as_of_ms, items: self.worn().into_iter().map(gear_item).collect() });
        file.stats = whole(self.stats_as_of, self.whole_stats).map(|as_of_ms| cf::Stats {
            as_of_ms,
            values: self.stats.iter().map(|(&stat, &value)| cf::StatValue { stat, value }).collect(),
        });
        file.skills = whole(self.skills_as_of, self.whole_skills).map(|as_of_ms| cf::Skills {
            as_of_ms,
            list: self
                .skills
                .iter()
                .map(|s| cf::SkillLevel { skill: s.id, level: s.level, base: s.base, board: s.board, gear: s.gear })
                .collect(),
        });
        file.specialties = self.specialties_as_of.map(|at| cf::Specialties {
            as_of_ms: at.ms,
            list: self
                .specialties
                .iter()
                .map(|(skill, slots)| cf::SpecialtySlots {
                    skill: *skill,
                    picks: slots.iter().map(|&(slot, part)| cf::Pick { slot, part }).collect(),
                })
                .collect(),
        });
        file.boards = self.boards_as_of.map(|at| cf::Boards {
            as_of_ms: at.ms,
            list: self.boards.iter().map(|(board, nodes)| cf::BoardNodes { board: *board, nodes: nodes.clone() }).collect(),
        });
        file.titles = self.titles.as_ref().map(|((owned, equipped), at)| cf::Titles {
            as_of_ms: at.ms,
            owned: owned.clone(),
            equipped: equipped.iter().map(|&(slot, title)| cf::EquippedTitle { slot, title }).collect(),
        });
        Some(file)
    }
}

fn gear_item(i: &Item) -> cf::GearItem {
    let detail = i.gear.as_ref().map(|g| cf::GearDetail {
        enhance: g.enchant,
        item_level: g.item_level,
        sockets: g.sockets,
        soul_bound: g.soul_bound,
        lines: g.lines.iter().map(|&(stat, value)| cf::StatValue { stat, value }).collect(),
        skill_lines: g.skills.clone(),
    });
    cf::GearItem { slot: i.slot, item: i.id, detail }
}

// ----- live -----

/// The character file's reader, live: every packet of the game's stream in
/// the order the decoder reads it. Without the game's tables, gear is found
/// by its record's shape.
#[derive(Default)]
pub struct LiveCharacter {
    character: Character,
    names: Names,
    records: u64,
}

impl LiveCharacter {
    /// One whole packet (length varint included), captured at `stamp_ms`.
    pub fn feed(&mut self, stamp_ms: i64, packet: &[u8]) {
        if self.character.feed(&self.names, Stamp { ms: stamp_ms, line: 0 }, packet) {
            self.records += 1;
        }
    }

    /// How many of your character's records were read: the file can only
    /// have changed when this moved.
    pub fn records(&self) -> u64 {
        self.records
    }

    /// The file of the character the game last named, its name (the file's
    /// key) in `name`. None until a self record gives the server and name,
    /// and while a login is under way: its records come before the self
    /// record that names who logged in.
    pub fn file(&self) -> Option<CharacterFile> {
        self.character.file()
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
    use crate::built::{from_hex, packet, self_record, stat_sheet, stats, F};

    fn at(ms: i64) -> Stamp {
        Stamp { ms, line: 0 }
    }

    // Two equipped items in the login record's layout (`11 56`), built with
    // made-up values. The orb has a packed flag byte after its item level;
    // the pauldrons carry three random lines and a skill.
    const ORB: &str = "01 0102030405060708 419f9706 0100000000000000 0b 01
        000000000000000000000000 0a 000000009001000000000000 00 37000000 00 01 03
        00000000000000000000000000000000000000000000000000000000
        000000000000000000 03 00000000000000000000000000000000000000";
    const PAULDRONS: &str = "01 1112131415161718 64738a0c 0100000000000000 0b 04
        0000000000000000000000000000 05 00000000000000000000000000 3b000000 04 04
        00000000000000000000000000000000000000000000000000000000
        a1a2a3a4a5a6a7a801 03 00000000000000000000000000
        03 c70032000000 380114000000 c8000a000000 03 c70000000000 380100000000 c80000000000
        01 8020ff00 01 01 8020ff00 0000000000";

    #[test]
    fn decodes_gear_whatever_byte_its_fields_drift_to() {
        let orb = from_hex(ORB);
        let item = decode_item(&orb, 0, orb.len(), None, None).expect("orb");
        assert_eq!((item.id, item.container, item.slot), (110_600_001, 11, 1));
        let g = item.gear.expect("gear");
        assert_eq!((g.enchant, g.item_level, g.sockets, g.soul_bound), (10, 55, 3, false));
        assert!(g.lines.is_empty());

        let pauldrons = from_hex(PAULDRONS);
        let item = decode_item(&pauldrons, 0, pauldrons.len(), Some(54), Some(4)).expect("pauldrons");
        let g = item.gear.expect("gear");
        assert_eq!((g.enchant, g.item_level, g.sockets, g.soul_bound), (5, 59, 4, true));
        assert_eq!(g.lines, vec![(199, 50), (312, 20), (200, 10)]);
        assert_eq!(g.skills, vec![16_720_000]);
    }

    #[test]
    fn keys_gear_by_slot_and_follows_it_off_and_on() {
        let names = Names::default();
        let mut c = Character::default();
        let mut inv = from_hex("1002 0200");
        inv.extend(from_hex(ORB));
        inv.extend(from_hex(PAULDRONS));
        c.feed(&names, at(1), &packet([0x11, 0x56], &[F::Raw(&inv)]));
        let gear = |c: &Character| -> Vec<String> {
            c.to_json(&names, &[], &[])["gear"]["items"].as_array().unwrap().iter().map(|v| v["key"].as_str().unwrap().to_string()).collect()
        };
        assert_eq!(gear(&c), ["1:110600001", "4:210400100"]);

        // The slot map without the orb: it is no longer worn.
        let mut map = from_hex("01010101");
        map.extend(from_hex("1112131415161718 04"));
        c.feed(&names, at(2), &packet([0x8d, 0xe2], &[F::Raw(&map)]));
        assert_eq!(gear(&c), ["4:210400100"]);
        // Its own record puts it in the bag, then the map puts it back on.
        let mut moved = from_hex("0101");
        moved.extend(from_hex(&ORB.replacen("0b 01", "01 4e", 1)));
        c.feed(&names, at(3), &packet([0x1b, 0x56], &[F::Raw(&moved)]));
        assert_eq!(gear(&c), ["4:210400100"]);
        let mut map = from_hex("01010102");
        map.extend(from_hex("0102030405060708 01 1112131415161718 04"));
        c.feed(&names, at(4), &packet([0x8d, 0xe2], &[F::Raw(&map)]));
        assert_eq!(gear(&c), ["1:110600001", "4:210400100"]);
    }

    #[test]
    fn stats_take_the_full_list_then_its_changes() {
        let names = Names::default();
        let mut c = Character::default();
        // 49 36: two bytes, the count, (stat, value)*, eight bytes.
        c.feed(&names, at(1), &packet([0x49, 0x36], &[F::Raw(&from_hex("0000 02 0100 b7020000 8000 c8010000 0010000000000000"))]));
        // 4a 36 from your entity, 1600: the weapon off.
        c.feed(&names, at(2), &stats(1600, &[(1, 0), (128, 256)]));
        assert_eq!(c.stats, BTreeMap::from([(1, 0), (128, 256)]));
        assert_eq!(c.stats_as_of, Some(at(2)));
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
        p.extend(skill(1, 16_140_000, [12, 9, 2, 0, 1]));
        p.extend(skill(5, 4_900_202, [1, 1, 0, 0, 0]));
        p.push(3); // the byte more seen after a flag-5 entry
        p.extend(skill(1, 16_150_000, [5, 5, 0, 0, 0]));
        let list = decode_skills(&p, |id| (1_000..=99_999_999).contains(&id));
        assert_eq!(list.len(), 3);
        assert_eq!((list[0].level, list[0].base, list[0].board, list[0].gear), (12, 9, 2, 1));
        assert_eq!((list[2].id, list[2].level), (16_150_000, 5));
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
        // Made-up node ids.
        let boards = decode_boards(&from_hex("02 33000000 02 e6c90700 dbc90700 36000000 01 46410800"));
        assert_eq!(boards, vec![(51, vec![510_438, 510_427]), (54, vec![540_998])]);
        let spec = decode_specialties(&from_hex("01 f06df600 02 01 fa6df600 02 00000000"));
        assert_eq!(spec, vec![(16_150_000, vec![(1, 16_150_010), (2, 0)])]);
        let (owned, equipped) = decode_titles(&from_hex("02 3290b700 5984c600 01 01 3290b700")).unwrap();
        assert_eq!(owned, vec![12_030_002, 13_010_009]);
        assert_eq!(equipped, vec![(1, 12_030_002)]);
    }

    /// `<owned varint> <title u32>* <equipped varint> (<category u8> <title
    /// u32>)*`, built from that layout: 130 owned titles take a two-byte
    /// count (`82 01`).
    #[test]
    fn titles_past_127_read_on_their_own_bytes() {
        let ids: Vec<u32> = (0..130).map(|i| 11_010_001 + i).collect();
        let mut p = vec![0x82, 0x01];
        for id in &ids {
            p.extend_from_slice(&id.to_le_bytes());
        }
        p.push(2);
        for (category, id) in [(1u8, 11_020_034u32), (3, 12_010_003)] {
            p.push(category);
            p.extend_from_slice(&id.to_le_bytes());
        }
        let (owned, equipped) = decode_titles(&p).unwrap();
        assert_eq!(owned, ids);
        assert_eq!(equipped, vec![(1, 11_020_034), (3, 12_010_003)]);
        assert_eq!(decode_titles(&p[..p.len() - 1]), None, "a cut record is not read");
    }

    #[test]
    fn stats_past_127_read_on_their_own_bytes() {
        let values: Vec<(u16, i32)> = (1..=130).map(|stat| (stat, 1_000 + stat as i32)).collect();
        let mut c = Character::default();
        assert!(c.feed(&Names::default(), at(1), &stat_sheet(&values)));
        assert_eq!(c.stats, values.iter().copied().collect::<BTreeMap<_, _>>());
    }

    /// A gear record in the layout of the module header, from made-up values.
    #[derive(Default)]
    struct GearRecord<'a> {
        handle: u8,
        item: u32,
        /// The slot worn in; None for the bag.
        worn: Option<u8>,
        maker: &'a str,
        enchant: u8,
        item_level: u32,
        sockets: u8,
        /// (manastone, stat, the stone's byte) in the first sockets.
        stones: &'a [(u32, u16, u8)],
        bound: bool,
        lines: &'a [(u16, i32)],
        skills: &'a [u32],
    }

    impl GearRecord<'_> {
        fn bytes(&self) -> Vec<u8> {
            let mut v = vec![GEAR];
            v.extend([self.handle; 8]);
            v.extend(self.item.to_le_bytes());
            v.extend(1u64.to_le_bytes());
            let (container, slot) = self.worn.map_or((1, 40), |slot| (EQUIPPED, slot));
            v.extend([container, slot, 0, 0, 0, self.maker.len() as u8]);
            v.extend(self.maker.as_bytes());
            v.extend([0; 8]);
            v.push(self.enchant);
            v.extend([0; 13]);
            v.extend(self.item_level.to_le_bytes());
            v.extend([self.worn.unwrap_or(0), self.sockets]);
            for n in 0..self.sockets as usize {
                match self.stones.get(n) {
                    Some(&(stone, stat, b)) => {
                        v.extend(stone.to_le_bytes());
                        v.extend(stat.to_le_bytes());
                        v.push(b);
                    }
                    None => v.extend([0; 7]),
                }
            }
            v.extend([if self.bound { 9 } else { 0 }; 8]);
            v.extend([0, 3]);
            v.extend([0; 13]);
            if !self.lines.is_empty() {
                for again in [false, true] {
                    v.push(self.lines.len() as u8);
                    for &(stat, value) in self.lines {
                        v.extend(stat.to_le_bytes());
                        v.extend(if again { 0 } else { value }.to_le_bytes());
                    }
                }
                for again in [false, true] {
                    v.push(self.skills.len() as u8);
                    for &skill in self.skills {
                        v.extend(skill.to_le_bytes());
                        v.push(u8::from(!again));
                    }
                }
            }
            v.extend([0; 3]);
            v
        }
    }

    /// A stack in the bag: kind 0, its count, 12 bytes after the slot.
    fn stack(handle: u8, item: u32, count: u64, slot: u8) -> Vec<u8> {
        let mut v = vec![0];
        v.extend([handle; 8]);
        v.extend(item.to_le_bytes());
        v.extend(count.to_le_bytes());
        v.extend([1, slot]);
        v.extend([0; 12]);
        v
    }

    fn item_list(records: &[Vec<u8>]) -> Vec<u8> {
        let mut p = vec![0x10, 0x02, records.len() as u8];
        for r in records {
            p.extend(r);
        }
        packet([0x11, 0x56], &[F::Raw(&p)])
    }

    const ORB_ID: u32 = 110_600_101;
    const STONE: u32 = 539_000_001;

    /// The item table for the made-up ids: an orb (level 40, two sockets), a
    /// manastone and a stack.
    fn item_table() -> Names {
        let row = |id: u32, level: i64, sockets: i64| ItemRow {
            id: Val { value: id.into() },
            desc: Key::default(),
            item_level: level,
            grade: String::new(),
            category: None,
            sockets,
        };
        let mut names = Names::default();
        for (id, level, sockets) in [(ORB_ID, 40, 2), (STONE, 0, 0), (930_100_023, 0, 0)] {
            names.items.insert(id, row(id, level, sockets));
        }
        names
    }

    #[test]
    fn a_manastone_in_a_socket_starts_no_item() {
        // The second stone's id reads like the head of an item with the
        // table; the stones lift the item level past the table's plus the
        // enchant.
        let orb = GearRecord {
            handle: 1,
            item: ORB_ID,
            worn: Some(1),
            enchant: 5,
            item_level: 48,
            sockets: 2,
            stones: &[(STONE, 128, 1), (STONE, 116, 2)],
            ..GearRecord::default()
        };
        let list = item_list(&[orb.bytes(), stack(2, 930_100_023, 5, 7)]);
        for names in [item_table(), Names::default()] {
            let mut c = Character::default();
            c.feed(&names, at(1), &list);
            let found: Vec<(u32, u8, u8)> = c.items.iter().map(|i| (i.id, i.container, i.slot)).collect();
            let want = if names.items.is_empty() { vec![(ORB_ID, 11, 1)] } else { vec![(ORB_ID, 11, 1), (930_100_023, 1, 7)] };
            assert_eq!(found, want, "only gear without the table");
            let g = c.items[0].gear.as_ref().expect("gear fields");
            assert_eq!((g.enchant, g.item_level, g.sockets), (5, 48, 2));
        }
    }

    #[test]
    fn skill_lines_take_five_bytes_each() {
        let pauldrons = GearRecord {
            handle: 3,
            item: 210_400_100,
            worn: Some(4),
            enchant: 2,
            item_level: 56,
            sockets: 4,
            bound: true,
            lines: &[(199, 50), (312, 20)],
            skills: &[16_740_000, 16_720_000],
            ..GearRecord::default()
        };
        let bytes = pauldrons.bytes();
        let g = decode_item(&bytes, 0, bytes.len(), None, None).and_then(|i| i.gear).expect("gear");
        assert_eq!((g.soul_bound, g.lines.as_slice()), (true, &[(199, 50), (312, 20)][..]));
        assert_eq!(g.skills, [16_740_000, 16_720_000]);
    }

    #[test]
    fn a_crafted_item_reads_past_its_makers_name() {
        let gloves = GearRecord {
            handle: 4,
            item: 210_500_100,
            maker: "Corvane",
            enchant: 1,
            item_level: 31,
            sockets: 1,
            ..GearRecord::default()
        };
        let list = item_list(&[gloves.bytes(), stack(5, 930_100_023, 9, 8)]);
        let mut c = Character::default();
        c.feed(&Names::default(), at(1), &list);
        let g = c.items[0].gear.as_ref().expect("gear fields");
        assert_eq!((c.items[0].container, g.enchant, g.item_level, g.sockets), (1, 1, 31, 1));
    }

    /// A login: the records the game sends before the self record, the self
    /// record, then the stat sheet, from `ms` on.
    fn login(c: &mut LiveCharacter, ms: i64, name: &str, gear: &[u8], titles: &[u32]) {
        let mut owned = vec![titles.len() as u8];
        titles.iter().for_each(|t| owned.extend(t.to_le_bytes()));
        owned.push(1);
        owned.push(1);
        owned.extend(titles[0].to_le_bytes());
        c.feed(ms, &packet([0x79, 0x56], &[F::Raw(&owned)]));
        c.feed(ms, &packet([0x26, 0xe2], &[F::Raw(&from_hex("01 33000000 02 e6c90700 dbc90700"))]));
        c.feed(ms, &packet([0x1d, 0x56], &[F::Raw(&from_hex("2c000000 2d000000"))]));
        c.feed(ms + 50, &item_list(&[gear.to_vec(), stack(9, 930_100_023, 5, 7)]));
        c.feed(ms + 50, &packet([0x56, 0x36], &[F::Raw(&from_hex("6015000000000000 7017000000000000"))]));
        let mut skills = vec![2u8];
        for (id, lv) in [(16_740_000u32, [6u8, 4, 1, 0, 1]), (16_720_000, [3, 3, 0, 0, 0])] {
            skills.push(1);
            skills.extend(id.to_le_bytes());
            skills.extend(lv);
            skills.extend([0; 6]);
        }
        c.feed(ms + 7_000, &packet([0x00, 0x51], &[F::Raw(&skills)]));
        c.feed(ms + 7_000, &packet([0x00, 0x39], &[F::Raw(&from_hex("01 a06eff00 02 01 aa6eff00 02 00000000"))]));
        c.feed(ms + 7_000, &self_record(9001, name, 1201, 30, 45));
        c.feed(ms + 7_100, &stat_sheet(&[(1, 32), (128, 650)]));
    }

    fn orb_on() -> Vec<u8> {
        GearRecord {
            handle: 1,
            item: ORB_ID,
            worn: Some(1),
            enchant: 5,
            item_level: 45,
            sockets: 2,
            bound: true,
            lines: &[(282, 1050)],
            skills: &[16_740_000],
            ..GearRecord::default()
        }
        .bytes()
    }

    #[test]
    fn a_login_becomes_a_character_file() {
        let mut c = LiveCharacter::default();
        assert_eq!(c.file(), None);
        login(&mut c, 1_000, "Tester", &orb_on(), &[12_030_002, 13_010_009]);
        let file = c.file().expect("a file once the self record named you");
        let mut want = CharacterFile::new(1201, Some(JobClass::Cleric), Some(45));
        want.name = Some("Tester".to_string());
        want.item_level = Some(cf::Levels { as_of_ms: 8_000, current: 500, highest: 500 });
        want.combat_power = Some(cf::Levels { as_of_ms: 1_050, current: 5_472, highest: 6_000 });
        want.gear = Some(cf::Gear {
            as_of_ms: 1_050,
            items: vec![cf::GearItem {
                slot: 1,
                item: ORB_ID,
                detail: Some(cf::GearDetail {
                    enhance: 5,
                    item_level: 45,
                    sockets: 2,
                    soul_bound: true,
                    lines: vec![cf::StatValue { stat: 282, value: 1050 }],
                    skill_lines: vec![16_740_000],
                }),
            }],
        });
        want.stats = Some(cf::Stats { as_of_ms: 8_100, values: vec![cf::StatValue { stat: 1, value: 32 }, cf::StatValue { stat: 128, value: 650 }] });
        want.skills = Some(cf::Skills {
            as_of_ms: 8_000,
            list: vec![
                cf::SkillLevel { skill: 16_740_000, level: 6, base: 4, board: 1, gear: 1 },
                cf::SkillLevel { skill: 16_720_000, level: 3, base: 3, board: 0, gear: 0 },
            ],
        });
        want.specialties = Some(cf::Specialties {
            as_of_ms: 8_000,
            list: vec![cf::SpecialtySlots { skill: 16_740_000, picks: vec![cf::Pick { slot: 1, part: 16_740_010 }, cf::Pick { slot: 2, part: 0 }] }],
        });
        want.boards = Some(cf::Boards { as_of_ms: 1_000, list: vec![cf::BoardNodes { board: 51, nodes: vec![510_438, 510_427] }] });
        want.titles = Some(cf::Titles {
            as_of_ms: 1_000,
            owned: vec![12_030_002, 13_010_009],
            equipped: vec![cf::EquippedTitle { slot: 1, title: 12_030_002 }],
        });
        assert_eq!(&file, &want);
        assert_eq!(CharacterFile::from_json(&file.to_json()), Ok(want));
    }

    #[test]
    fn no_file_while_a_login_is_under_way() {
        let mut c = LiveCharacter::default();
        login(&mut c, 1_000, "Tester", &orb_on(), &[12_030_002]);
        let records = c.records();
        // The next login's first records: no self record yet says whose.
        c.feed(60_000, &item_list(&[orb_on()]));
        assert_eq!(c.file(), None);
        assert_eq!(c.records(), records + 1);
        c.feed(67_000, &self_record(9001, "Tester", 1201, 30, 45));
        assert!(c.file().is_some());
    }

    #[test]
    fn another_character_starts_from_its_own_login() {
        let mut c = LiveCharacter::default();
        login(&mut c, 1_000, "Tester", &orb_on(), &[12_030_002]);
        // The same character again keeps what this login did not send.
        c.feed(30_000, &self_record(9001, "Tester", 1201, 30, 45));
        assert!(c.file().unwrap().boards.is_some());

        // Another character's login: its titles and items, then its self
        // record. What the first one had is gone, its stat sheet included.
        let bag = GearRecord { handle: 7, item: ORB_ID, enchant: 0, item_level: 40, sockets: 2, ..GearRecord::default() };
        c.feed(100_000, &packet([0x79, 0x56], &[F::Raw(&from_hex("01 5984c600 00"))]));
        c.feed(100_050, &item_list(&[bag.bytes()]));
        c.feed(107_000, &self_record(9002, "Other", 1201, 30, 12));
        let file = c.file().unwrap();
        assert_eq!((file.name.as_deref(), file.level), (Some("Other"), Some(12)));
        assert_eq!(file.titles.map(|t| t.owned), Some(vec![13_010_009]));
        assert_eq!(file.gear.map(|g| g.items), Some(vec![]), "nothing worn");
        assert_eq!((file.stats, file.skills, file.boards, file.specialties, file.combat_power), (None, None, None, None, None));
        assert_eq!(file.item_level.map(|l| l.as_of_ms), Some(107_000), "from its own self record");
    }

    #[test]
    fn sections_read_in_part_are_left_out() {
        // A meter started mid-session: a stat change, one item, a few
        // skills, then the self record of a zone load.
        let mut c = LiveCharacter::default();
        c.feed(1_000, &stats(1600, &[(1, 32)]));
        let mut one = vec![1, 1];
        one.extend(orb_on());
        c.feed(1_100, &packet([0x1b, 0x56], &[F::Raw(&one)]));
        let mut skills = vec![1u8, 1];
        skills.extend(16_740_000u32.to_le_bytes());
        skills.extend([6, 4, 1, 0, 1, 0, 0, 0, 0, 0, 0]);
        c.feed(1_200, &packet([0x00, 0x51], &[F::Raw(&skills)]));
        c.feed(1_300, &self_record(9001, "Tester", 1201, 30, 45));
        let file = c.file().unwrap();
        assert_eq!((file.gear, file.stats, file.skills), (None, None, None));
        assert!(file.item_level.is_some());
    }
}
