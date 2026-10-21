// SPDX-License-Identifier: AGPL-3.0-only

//! `Notices`, the QML singleton behind the notice tray above the meter
//! (NoticeTray.qml): a list of short notices, newest first, each of a kind
//! that picks its icon. Any part of the meter posts one with `post` (or
//! `post_until`, for one the tray counts down to) and takes one off with
//! `remove`, from any thread; the player dismisses them one by one or all at
//! once. The list lives as long as the meter runs.

use core::pin::Pin;
use std::sync::{Mutex, OnceLock};

use chrono::Timelike;
use cxx_qt::{CxxQtThread, CxxQtType, Threading};
use cxx_qt_lib::QString;
use serde_json::json;

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
        #[qproperty(QString, items, READ, NOTIFY)]
        #[qproperty(i32, count, READ, NOTIFY)]
        type Notices = super::NoticesRust;

        /// Adds a notice of `kind` (its icon) with a `title` and a line of
        /// `text`; one with the `key` of a notice on the list replaces it
        /// (empty: a new notice every time).
        #[qinvokable]
        fn add(self: Pin<&mut Notices>, key: QString, kind: QString, title: QString, text: QString);

        /// As `add`, for a notice the tray counts down to the unix second `until`.
        #[qinvokable]
        fn add_until(self: Pin<&mut Notices>, key: QString, kind: QString, title: QString, text: QString, until: i64);

        /// Takes the notice `id` off the list.
        #[qinvokable]
        fn dismiss(self: Pin<&mut Notices>, id: i32);

        /// Empties the list.
        #[qinvokable]
        fn clear(self: Pin<&mut Notices>);
    }

    impl cxx_qt::Threading for Notices {}
    impl cxx_qt::Initialize for Notices {}
}

/// How many notices the list keeps; the oldest go first.
pub const KEEP: usize = 50;

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub id: i32,
    /// Notices with the same key are one notice, updated in place: a pet's
    /// souls make one line however fast they come. Empty: none.
    pub key: String,
    pub kind: String,
    pub title: String,
    pub text: String,
    /// The local time it came, for its line in the tray.
    pub hour: u32,
    pub minute: u32,
    /// The unix second the tray counts down to instead (0: none).
    pub until: i64,
}

/// The list itself, newest first.
#[derive(Default)]
pub struct NoticeList {
    notices: Vec<Notice>,
    next_id: i32,
}

impl NoticeList {
    /// A notice with the key of one on the list takes its place and its id,
    /// and goes to the top.
    pub fn add(&mut self, key: &str, kind: &str, title: &str, text: &str, (hour, minute): (u32, u32), until: i64) -> i32 {
        let old = if key.is_empty() { None } else { self.notices.iter().position(|n| n.key == key) };
        let id = match old {
            Some(at) => self.notices.remove(at).id,
            None => {
                self.next_id += 1;
                self.next_id
            }
        };
        let notice = Notice { id, key: key.to_string(), kind: kind.to_string(), title: title.to_string(), text: text.to_string(), hour, minute, until };
        self.notices.insert(0, notice);
        self.notices.truncate(KEEP);
        id
    }

    /// Takes off the notice with `key`, if one is on the list.
    pub fn remove_key(&mut self, key: &str) -> bool {
        let before = self.notices.len();
        self.notices.retain(|n| key.is_empty() || n.key != key);
        self.notices.len() != before
    }

    pub fn dismiss(&mut self, id: i32) -> bool {
        let before = self.notices.len();
        self.notices.retain(|n| n.id != id);
        self.notices.len() != before
    }

    pub fn clear(&mut self) -> bool {
        let had = !self.notices.is_empty();
        self.notices.clear();
        had
    }

    pub fn notices(&self) -> &[Notice] {
        &self.notices
    }

    /// As QML reads it: [{"id", "kind", "title", "text", "hour", "minute", "until"}].
    pub fn json(&self) -> String {
        let items: Vec<_> = self
            .notices
            .iter()
            .map(|n| json!({ "id": n.id, "kind": n.kind, "title": n.title, "text": n.text, "hour": n.hour, "minute": n.minute, "until": n.until }))
            .collect();
        serde_json::Value::Array(items).to_string()
    }
}

pub struct NoticesRust {
    list: NoticeList,
    items: QString,
    count: i32,
}

impl Default for NoticesRust {
    fn default() -> Self {
        Self { list: NoticeList::default(), items: QString::from("[]"), count: 0 }
    }
}

/// The object, for posts from other threads.
static QT: OnceLock<CxxQtThread<qobject::Notices>> = OnceLock::new();

/// A post or a removal from another thread.
enum Change {
    Post { key: String, kind: String, title: String, text: String, until: i64 },
    Remove(String),
}

/// Changes made before the object existed, in order.
static WAITING: Mutex<Vec<Change>> = Mutex::new(Vec::new());

impl cxx_qt::Initialize for qobject::Notices {
    fn initialize(mut self: Pin<&mut Self>) {
        let _ = QT.set(self.qt_thread());
        let waiting = std::mem::take(&mut *WAITING.lock().unwrap_or_else(|e| e.into_inner()));
        for change in waiting {
            self.as_mut().apply(change);
        }
    }
}

/// Adds a notice from anywhere in the meter: `key` makes it replace the
/// notice with that key (empty: none), `kind` picks its icon
/// (NoticeIcon.qml), `title` and `text` are already in the player's
/// language.
pub fn post(key: &str, kind: &str, title: &str, text: &str) {
    post_until(key, kind, title, text, 0);
}

/// As `post`, for a notice the tray counts down to the unix second `until`.
pub fn post_until(key: &str, kind: &str, title: &str, text: &str, until: i64) {
    send(Change::Post { key: key.to_string(), kind: kind.to_string(), title: title.to_string(), text: text.to_string(), until });
}

/// Takes the notice with `key` off the list, from anywhere in the meter.
pub fn remove(key: &str) {
    send(Change::Remove(key.to_string()));
}

fn send(change: Change) {
    match QT.get() {
        Some(qt) => {
            let _ = qt.queue(move |notices| notices.apply(change));
        }
        None => WAITING.lock().unwrap_or_else(|e| e.into_inner()).push(change),
    }
}

fn now() -> (u32, u32) {
    let now = chrono::Local::now();
    (now.hour(), now.minute())
}

impl qobject::Notices {
    fn apply(mut self: Pin<&mut Self>, change: Change) {
        let changed = match change {
            Change::Post { key, kind, title, text, until } => {
                self.as_mut().rust_mut().list.add(&key, &kind, &title, &text, now(), until);
                true
            }
            Change::Remove(key) => self.as_mut().rust_mut().list.remove_key(&key),
        };
        if changed {
            self.publish();
        }
    }

    fn publish(mut self: Pin<&mut Self>) {
        let (items, count) = (QString::from(&self.list.json()), self.list.notices().len() as i32);
        self.as_mut().rust_mut().items = items;
        self.as_mut().items_changed();
        if self.count != count {
            self.as_mut().rust_mut().count = count;
            self.count_changed();
        }
    }

    fn add(self: Pin<&mut Self>, key: QString, kind: QString, title: QString, text: QString) {
        self.add_until(key, kind, title, text, 0);
    }

    fn add_until(self: Pin<&mut Self>, key: QString, kind: QString, title: QString, text: QString, until: i64) {
        self.apply(Change::Post { key: key.to_string(), kind: kind.to_string(), title: title.to_string(), text: text.to_string(), until });
    }

    fn dismiss(mut self: Pin<&mut Self>, id: i32) {
        if self.as_mut().rust_mut().list.dismiss(id) {
            self.publish();
        }
    }

    fn clear(mut self: Pin<&mut Self>) {
        if self.as_mut().rust_mut().list.clear() {
            self.publish();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_notice_comes_first() {
        let mut list = NoticeList::default();
        list.add("", "info", "First", "", (9, 5), 0);
        list.add("", "info", "Second", "", (9, 6), 0);
        let titles: Vec<_> = list.notices().iter().map(|n| n.title.as_str()).collect();
        assert_eq!(titles, ["Second", "First"]);
    }

    #[test]
    fn a_dismissed_notice_goes_and_the_others_stay() {
        let mut list = NoticeList::default();
        let a = list.add("", "info", "A", "", (0, 0), 0);
        let b = list.add("", "info", "B", "", (0, 0), 0);
        assert!(list.dismiss(a));
        assert!(!list.dismiss(a));
        assert_eq!(list.notices().iter().map(|n| n.id).collect::<Vec<_>>(), [b]);
    }

    #[test]
    fn clear_empties_the_list_and_ids_go_on_counting() {
        let mut list = NoticeList::default();
        let a = list.add("", "info", "A", "", (0, 0), 0);
        assert!(list.clear());
        assert!(!list.clear());
        let b = list.add("", "info", "B", "", (0, 0), 0);
        assert!(b > a);
    }

    #[test]
    fn a_keyed_notice_updates_in_place_and_goes_to_the_top() {
        let mut list = NoticeList::default();
        let pet = list.add("pet:1003", "pet", "Soul", "1/25", (9, 0), 0);
        list.add("", "info", "Other", "", (9, 1), 0);
        let again = list.add("pet:1003", "pet", "Soul", "2/25", (9, 2), 0);
        assert_eq!(again, pet, "the same notice keeps its id");
        assert_eq!(list.notices().len(), 2);
        assert_eq!(list.notices()[0].text, "2/25");
        assert_eq!((list.notices()[0].hour, list.notices()[0].minute), (9, 2));
        let other = list.add("pet:1035", "pet", "Soul", "1/25", (9, 3), 0);
        assert_ne!(other, pet, "another key is another notice");
        assert_eq!(list.notices().len(), 3);
    }

    #[test]
    fn the_list_keeps_only_the_newest() {
        let mut list = NoticeList::default();
        for i in 0..KEEP + 5 {
            list.add("", "info", &i.to_string(), "", (0, 0), 0);
        }
        assert_eq!(list.notices().len(), KEEP);
        assert_eq!(list.notices()[0].title, (KEEP + 4).to_string());
        assert_eq!(list.notices()[KEEP - 1].title, "5");
    }

    #[test]
    fn a_notice_comes_off_by_its_key() {
        let mut list = NoticeList::default();
        list.add("spawn:boss:1", "spawn", "Boss", "", (9, 0), 1);
        list.add("", "info", "Other", "", (9, 1), 0);
        assert!(list.remove_key("spawn:boss:1"));
        assert!(!list.remove_key("spawn:boss:1"));
        assert!(!list.remove_key(""), "an empty key takes nothing off");
        assert_eq!(list.notices().len(), 1);
    }

    #[test]
    fn qml_reads_every_field() {
        let mut list = NoticeList::default();
        list.add("", "info", "Title", "Line", (14, 7), 1_792_393_200);
        let items: serde_json::Value = serde_json::from_str(&list.json()).unwrap();
        assert_eq!(items[0]["id"], 1);
        assert_eq!(items[0]["kind"], "info");
        assert_eq!(items[0]["title"], "Title");
        assert_eq!(items[0]["text"], "Line");
        assert_eq!(items[0]["hour"], 14);
        assert_eq!(items[0]["minute"], 7);
        assert_eq!(items[0]["until"], 1_792_393_200i64);
    }
}
