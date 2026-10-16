//! Prints what History shows (`history/history_view.rs`) of a data folder of
//! saved fights, with every backend answer the view was built from.
//!
//! ```text
//! A2_HISTORY_DIR=<data folder>   its history/ and slices/ (required); the
//!                                record check writes one with A2_KEEP_APP
//! A2_RECORD_ROOTS=<folder>       a folder of account folders of the game's
//!                                record_*.dat files (optional)
//! A2_HISTORY_LANG=en             the language names are in
//! A2_HISTORY_CLOCK=12h           Display Time, 12h or 24h
//! cargo test --lib history_report -- --ignored --nocapture
//! ```
//!
//! "You" is the player who saved the newest fight with a slice. The first
//! line is `HLIST {"language", "clock", "you", "fights", "records", "share"}`:
//! the answers of `get_fight_history`, `game_record_status` and `share_status`.
//! Then one `HVIEW {"filters", "list"}` per layout and filter, and one
//! `HFIGHT {"record", "records", "view"}` per fight: the answers of
//! `load_fight` and `game_record_details`, and the fight as History shows it.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;

use crate::combat::details_view::{LookupNames, Words};
use crate::game_record::files::Checker;
use crate::history::fight_history::FightHistoryManager;
use crate::history::history_view::{self, Clock, Dungeons, Filters, Layout, ListInput, ListNames, ListWords, CLASS_KEYS};
use crate::i18n::lookup::{load_language, NpcLookup, SkillLookup};

use super::data_dir;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// A UI string, English when the language has none.
fn ui_text(language: &str, key: &str) -> String {
    let read = |lang: &str| -> Option<String> {
        let text = std::fs::read_to_string(data_dir().join(format!("i18n/ui/{lang}.json"))).ok()?;
        let mut v: serde_json::Value = serde_json::from_str(&text).ok()?;
        for part in key.split('.') {
            v = v.get(part)?.clone();
        }
        v.as_str().map(str::to_string)
    };
    read(language).or_else(|| read("en")).unwrap_or_else(|| key.to_string())
}


#[test]
#[ignore]
fn history_report() {
    let dir = PathBuf::from(env("A2_HISTORY_DIR").expect("A2_HISTORY_DIR"));
    let language = env("A2_HISTORY_LANG").unwrap_or_else(|| "en".into());
    let clock = env("A2_HISTORY_CLOCK").unwrap_or_else(|| "12h".into());
    assert!(clock == "12h" || clock == "24h", "A2_HISTORY_CLOCK is 12h or 24h");
    let skills = Arc::new(SkillLookup::new());
    let npcs = Arc::new(NpcLookup::new());
    load_language(&skills, &npcs, &data_dir(), &language);
    let dungeons = Dungeons::load(&data_dir(), &language);
    let words = ListWords {
        open_world: ui_text(&language, "history.openWorld"),
        classes: CLASS_KEYS.iter().map(|(job, key)| (job.to_string(), ui_text(&language, &format!("classes.{key}")))).collect(),
    };
    let names = ListNames { npcs: &npcs, dungeons: &dungeons, words: &words, zone: None, clock: Clock::new(clock == "24h", &language) };
    let details_words = Words {
        unnamed_skill: ui_text(&language, "skills.fallback"),
        dot: ui_text(&language, "skills.dot"),
    };
    let details_names = LookupNames { skills: &skills, npcs: &npcs, words: &details_words };

    let history = FightHistoryManager::new(dir.clone());
    let (fights, players) = history.listing();
    let checker = Checker {
        app_data_dir: dir.clone(),
        data_dir: Some(data_dir()),
        skills: skills.clone(),
        npcs: npcs.clone(),
        fights: fights.clone(),
        roots: env("A2_RECORD_ROOTS").map(PathBuf::from).into_iter().collect(),
        zone: None,
    };
    let records = checker.statuses();
    let share = crate::share::share_status(&dir);
    let uploaded: HashSet<String> = share.iter().filter(|(_, s)| s.url.is_some()).map(|(id, _)| id.clone()).collect();
    // You: the player who saved the newest fight with a slice.
    let you = fights
        .iter()
        .find_map(|f| {
            let id = crate::share::slice_uploader(&dir, &f.id)?;
            let record = history.load_fight(&f.id).ok()?;
            record.actors.iter().find(|a| a.actor_id == id).map(|a| a.nickname.clone())
        })
        .unwrap_or_default();

    println!("HLIST {}", json!({ "language": language, "clock": clock, "you": you, "fights": fights, "records": records, "share": share }));

    let input = ListInput { fights: &fights, players: &players, records: &records, uploaded: &uploaded, you: &you };
    let mut filters: Vec<Filters> = Vec::new();
    for layout in [Layout::Dungeon, Layout::Boss, Layout::List] {
        filters.push(Filters { layout, ..Filters::default() });
        filters.push(Filters { layout, show_training: false, ..Filters::default() });
        filters.push(Filters { layout, only_records: true, ..Filters::default() });
    }
    let all = history_view::list(&input, &Filters::default(), &names);
    for b in &all.bosses {
        filters.push(Filters { boss: b.value.clone(), ..Filters::default() });
    }
    for j in &all.jobs {
        filters.push(Filters { layout: Layout::List, job: j.value.clone(), ..Filters::default() });
    }
    for d in &all.dates {
        filters.push(Filters { layout: Layout::Boss, date: d.clone(), ..Filters::default() });
    }
    for f in &filters {
        let list = history_view::list(&input, f, &names);
        let shown = json!({
            "layout": f.layout.setting(), "boss": f.boss, "job": f.job, "date": f.date,
            "showTraining": f.show_training, "onlyRecords": f.only_records,
        });
        println!("HVIEW {}", json!({ "filters": shown, "list": list }));
    }

    for f in &fights {
        let record = history.load_fight(&f.id).expect("a saved fight");
        let views = checker.views(&f.id);
        let view = history_view::fight(&record, &you, uploaded.contains(&f.id), f.is_live, &views, &details_names, &names);
        println!("HFIGHT {}", json!({ "record": record, "records": views, "view": view }));
    }
}
