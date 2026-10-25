// SPDX-License-Identifier: AGPL-3.0-only

//! The countdown notices in the live loop: the spawn notices from the game's
//! schedule (world_spawns.rs) and the field boss notices from the world
//! map's times (field_bosses.rs), under the one Spawn notices setting.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::Local;

use daevalog::combat::data_storage::DataStorage;

use crate::field_bosses::FieldBossNotices;
use crate::timed_notices::{local_offset, SpawnNames, Words};
use crate::world_spawns::{zone_offset, Schedule, SpawnNotices};

/// How often the live loop looks.
const EVERY: Duration = Duration::from_secs(15);

struct Live {
    schedule: Schedule,
    names: HashMap<String, SpawnNames>,
    notices: SpawnNotices,
    field_bosses: FieldBossNotices,
    looked: Option<Instant>,
}

/// From the live loop: posts the notices due, takes off the ones past, and
/// all of them when `enabled` goes off.
pub fn tick(storage: &DataStorage, language: &str, hour24: bool, enabled: bool, zone: &str) {
    static LIVE: OnceLock<Mutex<Live>> = OnceLock::new();
    let live = LIVE.get_or_init(|| {
        let schedule = crate::data_dir().and_then(|dir| crate::read_json(&dir.join("world_spawns.json"))).map(|j| Schedule::from_json(&j)).unwrap_or_default();
        Mutex::new(Live { schedule, names: HashMap::new(), notices: SpawnNotices::default(), field_bosses: FieldBossNotices::default(), looked: None })
    });
    let mut live = live.lock().unwrap_or_else(|e| e.into_inner());
    let live = &mut *live;
    if !enabled {
        for key in live.notices.take_all().into_iter().chain(live.field_bosses.take_all()) {
            crate::notices::remove(&key);
        }
        live.looked = None;
        return;
    }
    if live.looked.is_some_and(|at| at.elapsed() < EVERY) {
        return;
    }
    live.looked = Some(Instant::now());
    let now = Local::now().timestamp();
    let offset = zone_offset(zone);
    let names = live.names.entry(language.to_string()).or_insert_with(|| SpawnNames::in_language(language));
    let words = Words { language, names, hour24, local_offset: &local_offset };
    for change in live.notices.moved(&words, now, &offset) {
        change.apply();
    }
    for list in storage.take_world_map() {
        for change in live.field_bosses.learn(&list, &words, now) {
            change.apply();
        }
    }
    let due = live.notices.due(&live.schedule, &words, now, &offset);
    for notice in due.into_iter().chain(live.field_bosses.due(&words, now)) {
        crate::notices::post_until(&notice.key, notice.kind, &notice.title, &notice.text, notice.until);
    }
    for key in live.notices.expired(now).into_iter().chain(live.field_bosses.expired(now)) {
        crate::notices::remove(&key);
    }
}
