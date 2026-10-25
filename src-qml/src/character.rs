// SPDX-License-Identifier: AGPL-3.0-only

//! `Character`, the QML singleton behind the Character window: the
//! character files the meter keeps in the data folder (or the ones
//! --character names), and the one shown, laid out by `character_view` in
//! the window's language. Read on a thread of their own and queued onto the
//! Qt thread as JSON; a reading that finds the same files, language and
//! clock as the last one makes nothing new, so the window can ask often.

use core::pin::Pin;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use daevalog::character::file::CharacterFile;
use daevalog::character::pack::CharacterPack;
use daevalog::character::store;
use daevalog::history::history_view::Clock;
use serde_json::json;

use crate::character_view::{self, Words};

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QString, list, READ, NOTIFY)]
        #[qproperty(QString, view, READ, NOTIFY)]
        #[qproperty(QString, chosen, READ, NOTIFY)]
        #[qproperty(bool, ready, READ, NOTIFY)]
        #[qproperty(bool, pack_failed, READ, NOTIFY)]
        type Character = super::CharacterRust;

        /// Reads the character files again; the list and the one shown
        /// change only when a file did.
        #[qinvokable]
        fn refresh(self: Pin<&mut Character>);

        /// Shows the character file `key` (a list entry's).
        #[qinvokable]
        fn choose(self: Pin<&mut Character>, key: &QString);

        /// Writes everything again, for a change of language or Display Time.
        #[qinvokable]
        fn redraw(self: Pin<&mut Character>);
    }

    impl cxx_qt::Threading for Character {}
}

/// What a reading is made from: the files with their times, the language,
/// the clock and the file shown.
#[derive(Clone, PartialEq)]
struct Stamp {
    files: Vec<(PathBuf, Option<SystemTime>)>,
    language: String,
    hour24: bool,
    chosen: String,
}

/// A reading as the window takes it.
struct Reading {
    /// {"files": [entry], "unread": files that did not read}.
    list: String,
    chosen: String,
    view: String,
    pack_failed: bool,
}

#[derive(Default)]
pub struct CharacterRust {
    /// The characters as JSON; empty until the first reading.
    list: QString,
    /// The one shown as JSON; empty with none.
    view: QString,
    chosen: QString,
    /// A reading has landed.
    ready: bool,
    /// The game data pack did not load: nothing can be named.
    pack_failed: bool,
    stamp: Option<Stamp>,
    /// Bumped by every reading; an older answer is dropped.
    reading: u64,
}

/// The files to show: --character's, else the data folder's.
fn sources() -> Vec<PathBuf> {
    let named = &crate::options().characters;
    if !named.is_empty() {
        return named.clone();
    }
    crate::settings_store::folder().map(|dir| store::list(&dir)).unwrap_or_default()
}

/// The pack in one language, read once per language.
fn pack(language: &str) -> Result<Arc<CharacterPack>, String> {
    static PACK: Mutex<Option<(String, Arc<CharacterPack>)>> = Mutex::new(None);
    let mut held = PACK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((held_language, pack)) = held.as_ref()
        && held_language == language
    {
        return Ok(pack.clone());
    }
    let dir = crate::data_dir().ok_or_else(|| "no game data folder".to_string())?;
    let fresh = Arc::new(CharacterPack::load(&dir, language).map_err(|e| e.to_string())?);
    *held = Some((language.to_string(), fresh.clone()));
    Ok(fresh)
}

/// A time as History writes one: "YYYY-MM-DD h:mm PM" or "YYYY-MM-DD HH:mm".
fn date_text(ms: i64, clock: &Clock) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|t| format!("{} {}", t.format("%Y-%m-%d"), clock.text(&t, false)))
        .unwrap_or_default()
}

fn read(stamp: &Stamp) -> Reading {
    let pack = match pack(&stamp.language) {
        Ok(pack) => pack,
        Err(why) => {
            eprintln!("daevalog-dps-meter: the character data did not load: {why}");
            let list = json!({ "files": [], "unread": 0 }).to_string();
            return Reading { list, chosen: String::new(), view: String::new(), pack_failed: true };
        }
    };
    let clock = Clock::new(stamp.hour24, &stamp.language);
    let date = |ms: i64| date_text(ms, &clock);
    let words = Words { language: &stamp.language, date: &date };
    let mut files: Vec<(String, Option<SystemTime>, CharacterFile)> = Vec::new();
    let mut unread = 0;
    for (path, time) in &stamp.files {
        match store::load(path) {
            Ok(file) => files.push((path.display().to_string(), *time, file)),
            Err(why) => {
                eprintln!("daevalog-dps-meter: {}: {why}", path.display());
                unread += 1;
            }
        }
    }
    // The character read last first.
    files.sort_by(|a, b| b.2.newest_ms().cmp(&a.2.newest_ms()).then_with(|| b.1.cmp(&a.1)).then_with(|| a.0.cmp(&b.0)));
    let entries: Vec<_> = files.iter().map(|(key, _, file)| character_view::entry(key, file, &pack, &words)).collect();
    let shown = files.iter().find(|(key, ..)| *key == stamp.chosen).or(files.first());
    Reading {
        list: json!({ "files": entries, "unread": unread }).to_string(),
        chosen: shown.map(|(key, ..)| key.clone()).unwrap_or_default(),
        view: shown.map(|(key, _, file)| character_view::view(key, file, &pack, &words).to_string()).unwrap_or_default(),
        pack_failed: false,
    }
}

/// Sets a property to `$value` and signals it, if it changed.
macro_rules! show {
    ($this:ident, $field:ident, $signal:ident, $value:expr) => {{
        let value = $value;
        if $this.$field != value {
            $this.as_mut().rust_mut().$field = value;
            $this.as_mut().$signal();
        }
    }};
}

impl qobject::Character {
    fn refresh(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().reading += 1;
        let reading = self.reading;
        let language = crate::i18n::resolve(crate::settings_store::store().values().language.as_deref()).to_string();
        let hour24 = crate::settings_store::clock_24h();
        let chosen = self.chosen.to_string();
        let last = self.stamp.clone();
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let files = sources()
                .into_iter()
                .map(|path| {
                    let time = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                    (path, time)
                })
                .collect();
            let stamp = Stamp { files, language, hour24, chosen };
            if last.as_ref() == Some(&stamp) {
                let _ = qt.queue(move |mut character| {
                    if reading == character.reading {
                        show!(character, ready, ready_changed, true);
                    }
                });
                return;
            }
            let out = read(&stamp);
            let _ = qt.queue(move |character| character.land(reading, stamp, out));
        });
    }

    fn land(mut self: Pin<&mut Self>, reading: u64, mut stamp: Stamp, out: Reading) {
        if reading != self.reading {
            return;
        }
        stamp.chosen = out.chosen.clone();
        self.as_mut().rust_mut().stamp = Some(stamp);
        show!(self, pack_failed, pack_failed_changed, out.pack_failed);
        show!(self, list, list_changed, QString::from(&out.list));
        show!(self, chosen, chosen_changed, QString::from(&out.chosen));
        show!(self, view, view_changed, QString::from(&out.view));
        show!(self, ready, ready_changed, true);
    }

    fn choose(mut self: Pin<&mut Self>, key: &QString) {
        if self.chosen == *key {
            return;
        }
        show!(self, chosen, chosen_changed, key.clone());
        self.refresh();
    }

    fn redraw(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().stamp = None;
        self.refresh();
    }
}
