// SPDX-License-Identifier: AGPL-3.0-only

//! The Character window's screens as JSON: one character file, named and
//! laid out by the game data pack in one language. Every number is the
//! file's or the pack's; the only sums are an item's base stats at its
//! enhance level and the nodes taken on each board. An id the pack cannot
//! name shows as `#id`; a section the game has not sent yet is null.

use std::collections::{HashMap, HashSet};

use daevalog::character::file::{BoardNodes, CharacterFile, Gear, GearDetail, GearItem, Levels, Pick, Skills, Stats, Titles};
use daevalog::character::pack::{Acquire, Board, CharacterPack, Item, NodeKind, SkillBonus, SkillKind, StatAmount};
use daevalog::entity::job_class::JobClass;
use serde_json::{Value, json};

/// The stats the game's stat window shows a Fighter only (its
/// `ShowClassTypeList`): Rage, its regeneration in and out of combat, and
/// Rage Gain increase.
const FIGHTER_ONLY: [u16; 4] = [575, 578, 579, 585];

/// Light [Ariel] and Darkness [Azphel]: the game's tables list them with the
/// god stats, but its stat window's ring shows only the other ten, and their
/// own text says they have no effect.
const NOT_SHOWN: [u16; 2] = [12, 18];

/// What a view needs besides the file and the pack.
pub struct Words<'a> {
    /// The UI language (`crate::i18n::LANGUAGES`), for the class's name.
    pub language: &'a str,
    /// A time, in ms since the Unix epoch, as History writes one.
    pub date: &'a dyn Fn(i64) -> String,
}

fn unknown(id: impl std::fmt::Display) -> String {
    format!("#{id}")
}

/// A stat as stored, as the game shows it: divided by its unit (a percent
/// stat is stored in hundredths), up to two decimals, a percent stat with
/// "%"; `signed` puts "+" before a value that is not negative.
fn stat_value(pack: &CharacterPack, stat: u16, value: i32, signed: bool) -> String {
    let unit = u64::from(pack.stat_unit(stat).max(1).unsigned_abs());
    let hundredths = (i64::from(value).unsigned_abs() * 100 + unit / 2) / unit;
    // At most 2^31 * 100: exact as an f64.
    let mut text = crate::snapshot::whole((hundredths / 100) as f64);
    if hundredths % 100 > 0 {
        text.push_str(format!(".{:02}", hundredths % 100).trim_end_matches('0'));
    }
    let sign = if value < 0 { "-" } else if signed { "+" } else { "" };
    let percent = if pack.percent.contains(&stat) { "%" } else { "" };
    format!("{sign}{text}{percent}")
}

fn stat_name(pack: &CharacterPack, stat: u16) -> String {
    pack.text.stats.get(&stat).cloned().unwrap_or_else(|| unknown(stat))
}

fn skill_name(pack: &CharacterPack, skill: u32) -> String {
    match pack.text.skills.get(&skill) {
        Some(name) => name.clone(),
        None => unknown(skill),
    }
}

/// One stat line: its name and its value.
fn line(pack: &CharacterPack, stat: u16, value: i32, signed: bool) -> Value {
    json!({ "name": stat_name(pack, stat), "value": stat_value(pack, stat, value, signed) })
}

/// The file's class; without one, the class most of its skills are of,
/// else its boards' class.
fn class_of(file: &CharacterFile, pack: &CharacterPack) -> Option<JobClass> {
    let skills = file.skills.iter().flat_map(|s| &s.list).filter_map(|s| Some((i32::try_from(s.skill).ok()?, 1)));
    file.class.or_else(|| JobClass::by_hits(skills)).or_else(|| {
        file.boards.iter().flat_map(|b| &b.list).find_map(|b| pack.boards.get(&b.board).map(|board| board.class))
    })
}

fn server_name(pack: &CharacterPack, server: u16) -> String {
    pack.text.servers.get(&server).cloned().unwrap_or_else(|| unknown(server))
}

/// A character in the window's list: who it is, as far as the file says
/// (its name only when the meter kept one), and when the meter last read it.
pub fn entry(key: &str, file: &CharacterFile, pack: &CharacterPack, words: &Words) -> Value {
    let job = class_of(file, pack).map_or("", |c| crate::snapshot::class_key(c.class_name()));
    let seen = file.newest_ms();
    json!({
        "key": key,
        "name": file.name.clone().unwrap_or_default(),
        "class": job,
        "classLabel": crate::i18n::class_name(words.language, job).unwrap_or_default(),
        "level": file.level,
        "server": server_name(pack, file.server),
        "seen": seen.map(|ms| (words.date)(ms)).unwrap_or_default(),
    })
}

/// Everything the four screens show of `file`.
pub fn view(key: &str, file: &CharacterFile, pack: &CharacterPack, words: &Words) -> Value {
    let class = class_of(file, pack);
    let levels = |l: Option<Levels>| l.map(|l| json!({ "current": l.current, "highest": l.highest }));
    json!({
        "head": entry(key, file, pack, words),
        "itemLevel": levels(file.item_level),
        "combatPower": levels(file.combat_power),
        "gear": file.gear.as_ref().map(|g| gear(g, pack)),
        "titles": file.titles.as_ref().map(|t| titles(t, pack)),
        "stats": file.stats.as_ref().map(|s| stats(s, class, pack)),
        "skills": file.skills.as_ref().map(|s| skills(s, file, class, pack)),
        "boards": file.boards.as_ref().map(|b| boards(&b.list, class, pack)),
    })
}

// ----- Gear -----

/// Every slot the paper doll shows, in its order, then items in slots the
/// pack does not know.
fn gear(gear: &Gear, pack: &CharacterPack) -> Vec<Value> {
    let mut out: Vec<Value> = pack
        .slots
        .iter()
        .map(|slot| {
            let worn = gear.items.iter().find(|g| g.slot == slot.id);
            json!({
                "slot": slot.key,
                "label": pack.text.slots.get(&slot.key).cloned().unwrap_or_else(|| unknown(slot.id)),
                "group": format!("{:?}", slot.group).to_lowercase(),
                "item": worn.map(|w| item(w, pack)),
            })
        })
        .collect();
    for worn in gear.items.iter().filter(|g| !pack.slots.iter().any(|s| s.id == g.slot)) {
        out.push(json!({ "slot": "", "label": unknown(worn.slot), "group": "other", "item": item(worn, pack) }));
    }
    out
}

fn item(worn: &GearItem, pack: &CharacterPack) -> Value {
    let table = pack.items.get(&worn.item);
    let detail = worn.detail.as_ref();
    let grade = table.map_or("", |t| t.grade.as_str());
    json!({
        "name": pack.text.items.get(&worn.item).cloned().unwrap_or_else(|| unknown(worn.item)),
        "grade": grade,
        "gradeLabel": pack.text.grades.get(grade).cloned().unwrap_or_default(),
        // The record's gear fields were read.
        "read": detail.is_some(),
        "enhance": detail.map(|d| d.enhance),
        "itemLevel": detail.map(|d| d.item_level).or(table.map(|t| t.level)),
        "sockets": detail.map(|d| d.sockets).or(table.map(|t| t.sockets)),
        "lines": detail.map_or_else(Vec::new, |d| d.lines.iter().map(|l| line(pack, l.stat, l.value, true)).collect()),
        "skills": detail.map_or_else(Vec::new, |d| skill_lines(d, pack)),
        "base": match (table, detail) {
            (Some(t), Some(d)) => base_at(t, worn.item, d.enhance, pack),
            _ => Vec::new(),
        },
        "fixed": table.map_or_else(Vec::new, |t| t.fixed.iter().map(|a| line(pack, a.0, a.1, true)).collect()),
    })
}

/// The +1 skill lines, one per skill, with how many lines name it.
fn skill_lines(detail: &GearDetail, pack: &CharacterPack) -> Vec<Value> {
    let mut counted: Vec<(u32, u32)> = Vec::new();
    for &skill in &detail.skill_lines {
        match counted.iter_mut().find(|(s, _)| *s == skill) {
            Some((_, n)) => *n += 1,
            None => counted.push((skill, 1)),
        }
    }
    counted.into_iter().map(|(skill, n)| json!({ "code": skill, "name": skill_name(pack, skill), "levels": n })).collect()
}

/// The item's base stats with what its enhance level adds, in the table's
/// order, a stat only the enhance level gives after them.
fn base_at(table: &Item, item: u32, enhance: u8, pack: &CharacterPack) -> Vec<Value> {
    let mut sums: Vec<(u16, i32)> = table.base.iter().map(|a| (a.0, a.1)).collect();
    for &StatAmount(stat, value) in pack.enhance_stats(item, enhance) {
        match sums.iter_mut().find(|(s, _)| *s == stat) {
            Some((_, sum)) => *sum += value,
            None => sums.push((stat, value)),
        }
    }
    sums.into_iter().map(|(stat, value)| line(pack, stat, value, false)).collect()
}

// ----- Titles -----

fn titles(titles: &Titles, pack: &CharacterPack) -> Value {
    let mut equipped = titles.equipped.clone();
    equipped.sort_by_key(|e| e.slot);
    let equipped: Vec<Value> = equipped
        .iter()
        .map(|e| {
            let table = pack.titles.get(&e.title);
            json!({
                "name": pack.text.titles.get(&e.title).cloned().unwrap_or_else(|| unknown(e.title)),
                "category": table.and_then(|t| pack.text.title_categories.get(&t.category)).cloned().unwrap_or_default(),
                "effects": table.map_or_else(Vec::new, |t| t.equipped.iter().map(|a| line(pack, a.0, a.1, true)).collect()),
            })
        })
        .collect();
    json!({ "owned": titles.owned.len(), "equipped": equipped })
}

// ----- The stat sheet -----

/// The game's stat window: its groups and their stats in its order, each
/// stat the file holds; Fighter-only stats only for a Fighter, and never the
/// two the game's window leaves out.
fn stats(stats: &Stats, class: Option<JobClass>, pack: &CharacterPack) -> Vec<Value> {
    let values: HashMap<u16, i32> = stats.values.iter().map(|v| (v.stat, v.value)).collect();
    let shows = |stat: &u16| {
        !NOT_SHOWN.contains(stat) && (class.is_none_or(|c| c == JobClass::Fighter) || !FIGHTER_ONLY.contains(stat))
    };
    pack.stat_groups
        .iter()
        .filter_map(|group| {
            let rows: Vec<Value> = group
                .stats
                .iter()
                .filter(|s| shows(s))
                .filter_map(|&s| values.get(&s).map(|&v| line(pack, s, v, false)))
                .collect();
            (!rows.is_empty()).then(|| {
                json!({
                    "title": pack.text.stat_groups.get(&group.key).cloned().unwrap_or_default(),
                    "rows": rows,
                })
            })
        })
        .collect()
}

// ----- Skills -----

/// The class's skills in the game's order, by kind: every Mastery skill,
/// learned or not, and the stigmas learned. Each with its level, the levels
/// boards and gear add, and its Specialty slots or a stigma's tiers.
fn skills(skills: &Skills, file: &CharacterFile, class: Option<JobClass>, pack: &CharacterPack) -> Value {
    let levels: HashMap<u32, _> = skills.list.iter().map(|s| (s.skill, s)).collect();
    let picks: HashMap<u32, &[Pick]> =
        file.specialties.iter().flat_map(|s| &s.list).map(|s| (s.skill, s.picks.as_slice())).collect();
    let (mut active, mut passive, mut stigma) = (Vec::new(), Vec::new(), Vec::new());
    let list = class.and_then(|c| pack.class_skills.get(&c)).map_or(&[][..], Vec::as_slice);
    for cs in list {
        let learned = levels.get(&cs.skill);
        if cs.acquire == Acquire::Stigma && learned.is_none() {
            continue;
        }
        let level = learned.map_or(0, |l| u32::from(l.level));
        let options = pack.specialties.get(&cs.skill).map_or(&[][..], Vec::as_slice);
        let picked = picks.get(&cs.skill).copied().unwrap_or_default();
        let (slots, tiers) = match cs.acquire {
            Acquire::Mastery if !options.is_empty() || !picked.is_empty() => {
                (mastery_slots(cs.skill, level, picked, class, pack), Vec::new())
            }
            Acquire::Mastery => (Vec::new(), Vec::new()),
            Acquire::Stigma => (Vec::new(), stigma_tiers(cs.skill, level, pack)),
        };
        let row = json!({
            "code": cs.skill,
            "name": skill_name(pack, cs.skill),
            "learned": learned.is_some(),
            "level": learned.map(|l| l.level),
            "board": learned.map_or(0, |l| l.board),
            "gear": learned.map_or(0, |l| l.gear),
            "slots": slots,
            "tiers": tiers,
        });
        match (cs.acquire, cs.kind) {
            (Acquire::Stigma, _) => stigma.push(row),
            (Acquire::Mastery, SkillKind::Active) => active.push(row),
            (Acquire::Mastery, SkillKind::Passive) => passive.push(row),
        }
    }
    json!({ "active": active, "passive": passive, "stigma": stigma })
}

/// A Mastery skill's Specialty slots, each with the level that opens it and
/// the option picked. The option's number is the game's (1 to 5); a pick the
/// pack does not list takes it from its id, which is the skill's plus ten
/// times the number. Fighter options have no text in the pack.
fn mastery_slots(skill: u32, level: u32, picked: &[Pick], class: Option<JobClass>, pack: &CharacterPack) -> Vec<Value> {
    let options = pack.specialties.get(&skill).map_or(&[][..], Vec::as_slice);
    pack.mastery_slots
        .iter()
        .enumerate()
        .map(|(at, &unlock)| {
            let slot = u8::try_from(at + 1).unwrap_or(u8::MAX);
            let part = picked.iter().find(|p| p.slot == slot && p.part != 0).map(|p| p.part);
            let known = part.and_then(|part| options.iter().find(|o| o.part == part));
            let option = known.map(|o| u32::from(o.option)).or_else(|| {
                part.and_then(|part| part.checked_sub(skill)).filter(|d| d % 10 == 0 && (1..=5).contains(&(d / 10))).map(|d| d / 10)
            });
            let text = match (part, known) {
                (Some(part), Some(_)) => pack.text.specialties.get(&part).cloned().unwrap_or_default(),
                (Some(_), None) if class == Some(JobClass::Fighter) => String::new(),
                (Some(part), None) => unknown(part),
                (None, _) => String::new(),
            };
            json!({ "unlock": unlock, "open": level >= unlock, "option": option.unwrap_or(0), "text": text })
        })
        .collect()
}

/// A stigma's tiers, by the level that turns each on.
fn stigma_tiers(skill: u32, level: u32, pack: &CharacterPack) -> Vec<Value> {
    let mut tiers: Vec<_> = pack.specialties.get(&skill).into_iter().flatten().filter(|o| o.slot != 0).collect();
    tiers.sort_by_key(|o| (o.unlock, o.slot));
    tiers
        .into_iter()
        .map(|o| {
            json!({
                "unlock": o.unlock,
                "reached": level >= o.unlock,
                "text": pack.text.specialties.get(&o.part).cloned().unwrap_or_else(|| unknown(o.part)),
            })
        })
        .collect()
}

// ----- Daevanion boards -----

/// The class's boards in the game's order, then any other board the file
/// names; each with its nodes and the ones taken.
fn boards(list: &[BoardNodes], class: Option<JobClass>, pack: &CharacterPack) -> Vec<Value> {
    let taken: HashMap<u32, HashSet<u32>> = list.iter().map(|b| (b.board, b.nodes.iter().copied().collect())).collect();
    let mut ids: Vec<(u32, u32)> = pack
        .boards
        .iter()
        .filter(|(_, b)| Some(b.class) == class)
        .map(|(&id, b)| (b.order, id))
        .collect();
    ids.sort_unstable();
    let mut ids: Vec<u32> = ids.into_iter().map(|(_, id)| id).collect();
    let mut others: Vec<u32> = list.iter().map(|b| b.board).filter(|b| !ids.contains(b)).collect();
    others.sort_unstable();
    others.dedup();
    ids.extend(others);
    let none = HashSet::new();
    ids.into_iter().map(|id| board(id, taken.get(&id).unwrap_or(&none), pack)).collect()
}

fn board(id: u32, taken: &HashSet<u32>, pack: &CharacterPack) -> Value {
    let name = pack.text.boards.get(&id).cloned().unwrap_or_else(|| unknown(id));
    let Some(board) = pack.boards.get(&id) else {
        let mut nodes: Vec<u32> = taken.iter().copied().collect();
        nodes.sort_unstable();
        let summary: Vec<Value> = nodes.iter().map(|n| json!({ "title": unknown(n), "count": 1 })).collect();
        return json!({ "name": name, "taken": nodes.len(), "total": null, "rows": 0, "cols": 0, "cells": [], "summary": summary });
    };
    let title = |n: u32| -> String {
        let node = &board.nodes[&n];
        pack.text.node_titles.get(&node.title).cloned().unwrap_or_else(|| unknown(n))
    };
    let (rows, cols, cells) = cells(board, taken, &title, pack);
    // What is taken, by node title: the title names every stat a node gives.
    let mut summary: Vec<(String, usize)> = Vec::new();
    let mut count = 0;
    let mut chosen: Vec<u32> = taken.iter().copied().collect();
    chosen.sort_unstable();
    for n in chosen {
        let text = match board.nodes.get(&n) {
            Some(node) if node.kind == NodeKind::Start => continue,
            Some(_) => title(n),
            None => unknown(n),
        };
        count += 1;
        match summary.iter_mut().find(|(t, _)| *t == text) {
            Some((_, c)) => *c += 1,
            None => summary.push((text, 1)),
        }
    }
    summary.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let total = board.nodes.values().filter(|n| n.kind != NodeKind::Start).count();
    json!({
        "name": name,
        "taken": count,
        "total": total,
        "rows": rows,
        "cols": cols,
        "cells": cells,
        "summary": summary.into_iter().map(|(title, count)| json!({ "title": title, "count": count })).collect::<Vec<_>>(),
    })
}

/// The board's grid: its size and each node at its place from the top left,
/// with what a stat node gives (one stat or two).
fn cells(board: &Board, taken: &HashSet<u32>, title: &dyn Fn(u32) -> String, pack: &CharacterPack) -> (i32, i32, Vec<Value>) {
    let (mut top, mut left, mut bottom, mut right) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for node in board.nodes.values() {
        top = top.min(node.row);
        bottom = bottom.max(node.row);
        left = left.min(node.col);
        right = right.max(node.col);
    }
    if board.nodes.is_empty() {
        return (0, 0, Vec::new());
    }
    let mut ids: Vec<&u32> = board.nodes.keys().collect();
    ids.sort_by_key(|n| (board.nodes[n].row, board.nodes[n].col));
    let cells = ids
        .into_iter()
        .map(|&n| {
            let node = &board.nodes[&n];
            let kind = match node.kind {
                NodeKind::Start => "start",
                NodeKind::Stat => "stat",
                NodeKind::SkillLevel => "skill",
            };
            json!({
                "row": node.row - top,
                "col": node.col - left,
                "kind": kind,
                "grade": node.grade,
                "taken": taken.contains(&n),
                "title": title(n),
                "stats": node.stats().map(|StatAmount(s, v)| line(pack, s, v, true)).collect::<Vec<_>>(),
                "code": node.skill.map_or(0, |SkillBonus(skill, _)| skill),
            })
        })
        .collect();
    (bottom - top + 1, right - left + 1, cells)
}

#[cfg(test)]
mod tests {
    use super::*;
    use daevalog::character::file::{SkillLevel, SpecialtySlots, StatValue};
    use daevalog::character::pack::IdKind;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, OnceLock};

    fn data() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data")
    }

    /// The shipped pack, read once per language for all tests.
    fn pack(language: &str) -> Arc<CharacterPack> {
        static PACKS: OnceLock<Mutex<HashMap<String, Arc<CharacterPack>>>> = OnceLock::new();
        let mut packs = PACKS.get_or_init(Default::default).lock().unwrap();
        packs
            .entry(language.to_string())
            .or_insert_with(|| Arc::new(CharacterPack::load(&data(), language).unwrap()))
            .clone()
    }

    /// The character file at `path`, from this crate's folder.
    fn file_at(path: &str) -> CharacterFile {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
        CharacterFile::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// The contract's sample character (made up).
    fn sample() -> CharacterFile {
        file_at("../src-tauri/tests/fixtures/character/sample-character.json")
    }

    fn date(ms: i64) -> String {
        format!("at {ms}")
    }

    fn words<'a>(language: &'a str) -> Words<'a> {
        Words { language, date: &date }
    }

    fn view_of(file: &CharacterFile, language: &str) -> Value {
        view("k", file, &pack(language), &words(language))
    }

    fn names(list: &Value) -> Vec<&str> {
        list.as_array().unwrap().iter().map(|v| v["name"].as_str().unwrap()).collect()
    }

    #[test]
    fn the_head_names_class_level_server_and_when_it_was_read() {
        let v = view_of(&sample(), "en");
        let head = &v["head"];
        assert_eq!(head["class"], "spiritmaster");
        assert_eq!(head["classLabel"], "Spiritmaster");
        assert_eq!(head["name"], "Samplechar");
        assert_eq!(head["level"], 45);
        assert_eq!(head["server"], "Siel");
        assert_eq!(head["seen"], "at 1767225602000", "the newest section");
        assert_eq!(v["itemLevel"], json!({ "current": 512, "highest": 515 }));
        assert_eq!(v["combatPower"], json!({ "current": 25000, "highest": 25210 }));
        let ko = view_of(&sample(), "ko");
        assert_eq!(ko["head"]["classLabel"], "정령성");
        assert_ne!(ko["head"]["server"], "Siel");
    }

    /// A made-up character of the window's own tests, from real ids.
    fn fixture(name: &str) -> CharacterFile {
        file_at(&format!("tests/fixtures/character/{name}.json"))
    }

    #[test]
    fn every_slot_shows_in_the_paper_dolls_order_worn_or_empty() {
        let v = view_of(&fixture("spiritmaster"), "en");
        let gear = v["gear"].as_array().unwrap();
        assert_eq!(gear.len(), 20);
        assert_eq!((gear[0]["slot"].as_str(), gear[0]["label"].as_str(), gear[0]["group"].as_str()), (Some("MainHand"), Some("Weapons"), Some("weapon")));
        let main = &gear[0]["item"];
        assert_eq!((main["name"].as_str(), main["grade"].as_str(), main["gradeLabel"].as_str()), (Some("Unyielding Orb"), Some("Legend"), Some("Epic")));
        assert_eq!((main["read"].as_bool(), main["enhance"].as_u64(), main["itemLevel"].as_u64()), (Some(true), Some(4), Some(46)));
        assert_eq!(main["sockets"], 3);
        assert_eq!(main["lines"].as_array().unwrap().len(), 3);
        // Base stats at +4: the table's, with what four enhance levels add.
        assert_eq!(main["base"][0], json!({ "name": "Min Attack", "value": "145" }), "128 + 17");
        assert_eq!(main["base"][1], json!({ "name": "Max Attack", "value": "273" }));
        assert_eq!(gear[19]["item"], Value::Null, "the second rune slot is empty");
        assert_eq!(gear[18]["group"], "rune");
        // The contract's sample: an item with its gear fields unread, and
        // items this pack does not know.
        let v = view_of(&sample(), "en");
        let torso = v["gear"].as_array().unwrap().iter().find(|g| g["slot"] == "Torso").unwrap().clone();
        assert_eq!((torso["item"]["read"].as_bool(), &torso["item"]["enhance"]), (Some(false), &Value::Null));
        assert_eq!(torso["item"]["name"], "#111100102");
        let main = &v["gear"][0]["item"];
        assert_eq!(main["lines"][0]["value"], "+10.5%", "Combat Speed 1050 in hundredths");
        assert_eq!(main["skills"], json!([{ "code": 16010000, "name": "Cold Shock", "levels": 1 }]));
        assert_eq!(v["gear"].as_array().unwrap().iter().filter(|g| g["item"].is_null()).count(), 14);
    }

    #[test]
    fn a_full_doll_shows_twenty_items() {
        // One item of each slot's kind, the longest-named in Russian.
        let p = pack("ru");
        let kinds: [(u8, &[&str]); 20] = [
            (1, &["Greatsword", "Sword", "Dagger", "Bow", "Magicbook", "Orb", "Mace", "Staff"]),
            (2, &["Guarder"]),
            (3, &["Helmet"]),
            (4, &["Shoulder"]),
            (5, &["Torso"]),
            (6, &["Pants"]),
            (7, &["Gloves"]),
            (8, &["Boots"]),
            (19, &["Cape"]),
            (17, &["Belt"]),
            (10, &["Necklace"]),
            (11, &["Earring"]),
            (12, &["Earring"]),
            (13, &["Ring"]),
            (14, &["Ring"]),
            (15, &["Bracelet"]),
            (16, &["Bracelet"]),
            (22, &["Amulet"]),
            (23, &["Rune"]),
            (24, &["Rune"]),
        ];
        let items = kinds
            .iter()
            .map(|(slot, categories)| {
                let item = p
                    .items
                    .iter()
                    .filter(|(_, i)| categories.contains(&i.category.as_str()))
                    .max_by_key(|(id, _)| (p.text.items[id].chars().count(), **id))
                    .map(|(id, _)| *id)
                    .unwrap();
                let detail = GearDetail { enhance: 0, item_level: 1, sockets: 0, soul_bound: false, lines: Vec::new(), skill_lines: Vec::new() };
                GearItem { slot: *slot, item, detail: Some(detail) }
            })
            .collect();
        let mut file = sample();
        file.gear = Some(Gear { as_of_ms: 1, items });
        let v = view_of(&file, "ru");
        let gear = v["gear"].as_array().unwrap();
        assert_eq!(gear.len(), 20);
        assert!(gear.iter().all(|g| g["item"].is_object() && !g["item"]["name"].as_str().unwrap().starts_with('#')));
    }

    #[test]
    fn stat_values_read_as_the_game_shows_them() {
        let p = pack("en");
        assert_eq!(stat_value(&p, 282, 2150, false), "21.5%", "Combat Speed in hundredths");
        assert_eq!(stat_value(&p, 282, 2105, false), "21.05%");
        assert_eq!(stat_value(&p, 282, -40, true), "-0.4%");
        assert_eq!(stat_value(&p, 242, 115400, false), "1,154", "Stamina, stored 100 times larger");
        assert_eq!(stat_value(&p, 193, 31240, false), "31,240");
        assert_eq!(stat_value(&p, 193, 0, true), "+0");
        assert_eq!(stat_value(&p, 128, 60, true), "+60");
        assert_eq!(stat_value(&p, 193, -1200, false), "-1,200");
    }

    #[test]
    fn the_sheet_follows_the_games_window_and_hides_fighter_stats_from_others() {
        let mut file = sample();
        let stats = file.stats.as_mut().unwrap();
        stats.values.push(StatValue { stat: 575, value: 100 });
        stats.values.push(StatValue { stat: 9999, value: 1 });
        stats.values.push(StatValue { stat: 12, value: 5 });
        stats.values.push(StatValue { stat: 18, value: 5 });
        let v = view_of(&file, "en");
        let groups = v["stats"].as_array().unwrap();
        let titles: Vec<&str> = groups.iter().map(|g| g["title"].as_str().unwrap()).collect();
        assert_eq!(titles[..2], ["Main Ability", "Main Stats"], "{titles:?}");
        let all: Vec<&str> = groups.iter().flat_map(|g| names(&g["rows"])).collect();
        assert!(!all.contains(&"Rage"), "a Spiritmaster's sheet has no Rage");
        assert!(
            !all.iter().any(|n| n.contains("Ariel") || n.contains("Azphel") || *n == "#12" || *n == "#18"),
            "Light and Darkness are not on the game's window: {all:?}"
        );
        assert!(!all.iter().any(|n| n.contains("9999")), "a stat outside the window is not on the sheet");
        let main = groups.iter().find(|g| g["title"] == "Main Stats").unwrap();
        assert_eq!(main["rows"][0], json!({ "name": "Might", "value": "120" }));
        file.class = Some(JobClass::Fighter);
        let v = view_of(&file, "en");
        let all: Vec<String> = v["stats"].as_array().unwrap().iter().flat_map(|g| names(&g["rows"]).into_iter().map(String::from).collect::<Vec<_>>()).collect();
        assert!(all.contains(&"Rage".to_string()));
    }

    #[test]
    fn skills_list_the_class_with_levels_specialties_and_stigma_tiers() {
        let mut file = sample();
        // A stigma at level 12: its first two tiers are on.
        file.skills.as_mut().unwrap().list.push(SkillLevel { skill: 16240000, level: 12, base: 12, board: 0, gear: 0 });
        let v = view_of(&file, "en");
        let skills = &v["skills"];
        let active = skills["active"].as_array().unwrap();
        let cold = active.iter().find(|s| s["code"] == 16010000).unwrap();
        assert_eq!((cold["level"].as_u64(), cold["board"].as_u64(), cold["gear"].as_u64()), (Some(14), Some(2), Some(1)));
        let slots = cold["slots"].as_array().unwrap();
        assert_eq!(slots.len(), 3);
        // Slots in the game's order: slot 1 first.
        assert_eq!(slots[0], json!({ "unlock": 8, "open": true, "option": 1, "text": "+20% MP restored" }));
        assert_eq!(slots[1]["option"], 3);
        assert_eq!(slots[2], json!({ "unlock": 20, "open": false, "option": 0, "text": "" }));
        // Every Mastery skill shows, learned or not.
        assert!(active.iter().any(|s| s["learned"] == false && s["level"].is_null()));
        assert_eq!(active.len() + skills["passive"].as_array().unwrap().len(), 23);
        let stigma = skills["stigma"].as_array().unwrap();
        assert_eq!(stigma.len(), 2, "only the stigmas learned");
        let added = stigma.iter().find(|s| s["code"] == 16240000).unwrap();
        let tiers: Vec<(u64, bool)> = added["tiers"].as_array().unwrap().iter().map(|t| (t["unlock"].as_u64().unwrap(), t["reached"] == true)).collect();
        assert_eq!(tiers, [(5, true), (10, true), (15, false), (20, false)]);
    }

    #[test]
    fn a_skill_the_language_has_no_name_for_takes_the_english_one() {
        let v = view_of(&fixture("fighter-full"), "ru");
        let active = v["skills"]["active"].as_array().unwrap();
        let flurry = active.iter().find(|s| s["code"] == 19010000).unwrap();
        let russian: Value = serde_json::from_str(&std::fs::read_to_string(data().join("i18n/skills/ru.json")).unwrap()).unwrap();
        assert_eq!(russian["19010000"], "???", "the game's own Russian text");
        assert_eq!(flurry["name"], "Flurry");
        assert!(!v.to_string().contains("???"));
    }

    #[test]
    fn a_fighters_specialty_shows_its_number_without_text() {
        let mut file = sample();
        file.class = Some(JobClass::Fighter);
        file.skills = Some(Skills { as_of_ms: 1, list: vec![SkillLevel { skill: 19010000, level: 12, base: 12, board: 0, gear: 0 }] });
        let picks = vec![Pick { slot: 1, part: 19010030 }, Pick { slot: 2, part: 0 }];
        file.specialties.as_mut().unwrap().list = vec![SpecialtySlots { skill: 19010000, picks }];
        let v = view_of(&file, "en");
        let flurry = v["skills"]["active"].as_array().unwrap().iter().find(|s| s["code"] == 19010000).unwrap().clone();
        assert_eq!(flurry["slots"][0], json!({ "unlock": 8, "open": true, "option": 3, "text": "" }));
        assert_eq!(flurry["slots"][1]["option"], 0);
    }

    #[test]
    fn boards_show_every_board_of_the_class_with_what_is_taken() {
        let v = view_of(&sample(), "en");
        let boards = v["boards"].as_array().unwrap();
        assert_eq!(boards.len(), 6);
        assert_eq!(names(&v["boards"])[..2], ["Nezekan", "Zikel"]);
        let first = &boards[0];
        // The file lists the start node too; it costs nothing and is not counted.
        assert_eq!((first["taken"].as_u64(), first["total"].as_u64()), (Some(3), Some(88)));
        assert_eq!((first["rows"].as_u64(), first["cols"].as_u64()), (Some(11), Some(11)));
        let cells = first["cells"].as_array().unwrap();
        assert_eq!(cells.len(), 89);
        assert_eq!(cells.iter().filter(|c| c["taken"] == true).count(), 4);
        assert!(cells.iter().any(|c| c["kind"] == "start"));
        let speed = cells.iter().find(|c| c["title"] == "Combat Speed").unwrap();
        assert_eq!((speed["row"].as_i64(), speed["col"].as_i64(), speed["taken"].as_bool()), (Some(0), Some(0), Some(true)));
        assert_eq!(speed["stats"].as_array().unwrap().len(), 1);
        assert_eq!(speed["stats"][0]["name"], "Combat Speed");
        // A node that gives two stats lists both.
        let all_cells: Vec<&Value> = boards.iter().flat_map(|b| b["cells"].as_array().unwrap()).collect();
        let two = all_cells.iter().find(|c| c["stats"].as_array().unwrap().len() == 2).expect("a two-stat node on some board");
        assert_ne!(two["stats"][0]["name"], two["stats"][1]["name"]);
        assert!(two["stats"].as_array().unwrap().iter().all(|s| s["value"].as_str().unwrap().starts_with('+')));
        assert!(all_cells.iter().filter(|c| c["kind"] != "stat").all(|c| c["stats"] == json!([])));
        let skill = cells.iter().find(|c| c["kind"] == "skill" && c["taken"] == true).unwrap();
        assert_eq!(skill["code"], 16720000);
        let summary = first["summary"].as_array().unwrap();
        assert_eq!(summary.iter().map(|s| s["count"].as_u64().unwrap()).sum::<u64>(), 3);
        // Zikel: nothing taken.
        assert_eq!(boards[1]["taken"], 0);
        assert_eq!(boards[5]["taken"], 0, "Azphel: its start only");
    }

    #[test]
    fn titles_show_the_equipped_ones_with_their_kind_and_effects() {
        let v = view_of(&sample(), "en");
        let titles = &v["titles"];
        assert_eq!(titles["owned"], 4);
        let equipped = titles["equipped"].as_array().unwrap();
        assert_eq!(equipped.len(), 3);
        assert_eq!(equipped[0]["category"], "Offensive");
        assert!(!equipped[0]["name"].as_str().unwrap().starts_with('#'));
        assert!(equipped[0]["effects"][0]["value"].as_str().unwrap().starts_with('+'));
    }

    #[test]
    fn sections_not_read_yet_are_null() {
        let file = CharacterFile::new(1001, Some(JobClass::Cleric), None);
        let v = view_of(&file, "en");
        for section in ["itemLevel", "combatPower", "gear", "titles", "stats", "skills", "boards"] {
            assert!(v[section].is_null(), "{section}");
        }
        assert_eq!(v["head"]["classLabel"], "Cleric");
        assert_eq!(v["head"]["level"], Value::Null);
        assert_eq!(v["head"]["seen"], "");
    }

    #[test]
    fn ids_the_pack_does_not_know_show_as_ids() {
        let mut file = sample();
        file.server = 9;
        file.gear.as_mut().unwrap().items.push(GearItem { slot: 99, item: 123, detail: None });
        file.gear.as_mut().unwrap().items[0].detail.as_mut().unwrap().lines.push(StatValue { stat: 9999, value: 5 });
        file.boards.as_mut().unwrap().list.push(BoardNodes { board: 77, nodes: vec![770001] });
        file.boards.as_mut().unwrap().list[0].nodes.push(510);
        file.titles.as_mut().unwrap().equipped[0].title = 5;
        let p = pack("en");
        let missing = p.missing(&file);
        assert!(missing.len() >= 7, "{missing:?}");
        let text = view("k", &file, &p, &words("en")).to_string();
        // Titles owned show as a count; the equipped ones by name.
        let equipped: Vec<u32> = file.titles.as_ref().unwrap().equipped.iter().map(|e| e.title).collect();
        for id in missing.iter().filter(|m| !matches!(m.kind, IdKind::Title | IdKind::TitleName) || equipped.contains(&m.id)) {
            assert!(text.contains(&format!("\"#{}\"", id.id)), "{id:?} is not shown as an id");
        }
        let v = view_of(&file, "en");
        assert_eq!(v["head"]["server"], "#9");
        let gear = v["gear"].as_array().unwrap();
        assert_eq!(gear[20]["label"], "#99");
        assert_eq!(gear[20]["item"]["name"], "#123");
        let boards = v["boards"].as_array().unwrap();
        assert_eq!(boards[6]["name"], "#77");
        assert_eq!(boards[0]["taken"], 4, "an unknown node on a known board counts");
    }

    #[test]
    fn the_class_comes_from_the_skills_when_the_file_has_none() {
        let mut file = sample();
        file.class = None;
        assert_eq!(class_of(&file, &pack("en")), Some(JobClass::Elementalist));
        file.skills = None;
        assert_eq!(class_of(&file, &pack("en")), Some(JobClass::Elementalist), "from its boards");
    }
}
