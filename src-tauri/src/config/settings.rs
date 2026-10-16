//! The settings file: a flat map of string keys to string values, kept in
//! memory and saved by a background thread a moment after it changes.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};

/// Edits that land this close together go to disk as one write.
const BATCH_WINDOW: Duration = Duration::from_millis(250);

/// The saved settings: a JSON object whose values are all strings. Anything
/// else, or no file at all, reads as no settings.
pub fn read_file(file_path: &Path) -> HashMap<String, String> {
    match std::fs::read(file_path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => HashMap::new(),
    }
}

const FILE_NAME: &str = "settings.json";
/// How long quitting (and a plain `flush`) waits for the disk.
const QUIT_LIMIT: Duration = Duration::from_secs(5);
const TEMP_NAME: &str = "settings.json.tmp";
const TIMED_OUT: &str = "Timed out saving settings";

pub struct Settings {
    inner: Arc<Inner>,
    writer: Mutex<Option<JoinHandle<()>>>,
}

struct Inner {
    file: PathBuf,
    values: Mutex<HashMap<String, String>>,
    progress: Mutex<Progress>,
    changed: Condvar,
    #[cfg(test)]
    hooks: TestHooks,
}

/// Bookkeeping between the editing threads and the writer. Edits are
/// numbered; a write covers every edit numbered up to the count it read.
#[derive(Default)]
struct Progress {
    edits: u64,
    tried: u64,
    stored: u64,
    attempts: u64,
    error: Option<String>,
    redo: bool,
    hurry: bool,
    closing: bool,
    alive: bool,
}

impl Progress {
    fn has_work(&self) -> bool {
        self.edits > self.tried || self.redo
    }

    /// A write failed and nothing changed since: the next flush tries again.
    fn failed_and_idle(&self) -> bool {
        self.error.is_some() && self.tried == self.edits
    }
}

impl Settings {
    pub fn new(app_data_dir: PathBuf) -> Self {
        Self::open(app_data_dir, kotlin_settings_path())
    }

    fn open(app_data_dir: PathBuf, kotlin: Option<PathBuf>) -> Self {
        let file = app_data_dir.join(FILE_NAME);
        let on_disk = file.exists();
        let mut values = read_file(&file);
        if values.is_empty() {
            let old = kotlin.map(|p| read_kotlin(&p)).unwrap_or_default();
            if !old.is_empty() {
                tracing::info!("Took {} settings over from the old app", old.len());
                values = old;
            }
        }
        let progress = Progress { edits: u64::from(!on_disk), alive: true, ..Progress::default() };
        let inner = Arc::new(Inner {
            file,
            values: Mutex::new(values),
            progress: Mutex::new(progress),
            changed: Condvar::new(),
            #[cfg(test)]
            hooks: TestHooks::default(),
        });
        let worker = Arc::clone(&inner);
        let handle = std::thread::Builder::new()
            .name("settings-writer".into())
            .spawn(move || worker.run())
            .expect("start the settings writer");
        Settings { inner, writer: Mutex::new(Some(handle)) }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.inner.values.lock().get(key).cloned()
    }

    /// False when the key already held this value; nothing is saved then.
    pub fn set(&self, key: &str, value: &str) -> bool {
        {
            let mut values = self.inner.values.lock();
            match values.get_mut(key) {
                Some(old) if old == value => return false,
                Some(old) => value.clone_into(old),
                None => drop(values.insert(key.to_owned(), value.to_owned())),
            }
        }
        self.inner.note_edit();
        true
    }

    pub fn remove(&self, key: &str) {
        if self.inner.values.lock().remove(key).is_some() {
            self.inner.note_edit();
        }
    }

    pub fn clear(&self) {
        self.inner.values.lock().clear();
        self.inner.note_edit();
    }

    #[cfg(test)]
    pub fn get_all(&self) -> HashMap<String, String> {
        self.inner.values.lock().clone()
    }

    /// Waits until every edit made before the call is on disk.
    pub fn flush(&self) -> Result<(), String> {
        self.wait_saved(self.quit_limit())
    }

    pub(crate) fn wait_saved(&self, limit: Duration) -> Result<(), String> {
        let give_up = Instant::now() + limit;
        let mut p = self.inner.progress.lock();
        let goal = p.edits;
        if p.stored >= goal && p.error.is_none() {
            return Ok(());
        }
        if p.failed_and_idle() {
            p.redo = true;
        }
        p.hurry = true;
        let seen = p.attempts;
        self.inner.changed.notify_all();
        loop {
            if p.stored >= goal {
                return Ok(());
            }
            // A write that ended after this call, and covered the goal, failed.
            if let Some(e) = p.error.as_ref().filter(|_| p.attempts > seen && p.tried >= goal) {
                return Err(e.clone());
            }
            if self.inner.changed.wait_until(&mut p, give_up).timed_out() {
                return Err(TIMED_OUT.to_string());
            }
        }
    }

    fn quit_limit(&self) -> Duration {
        #[cfg(test)]
        if let Some(limit) = *self.inner.hooks.quit_limit.lock() {
            return limit;
        }
        QUIT_LIMIT
    }
}

impl Drop for Settings {
    fn drop(&mut self) {
        let give_up = Instant::now() + self.quit_limit();
        let mut p = self.inner.progress.lock();
        p.closing = true;
        if p.failed_and_idle() {
            p.redo = true;
        }
        self.inner.changed.notify_all();
        while p.alive {
            if self.inner.changed.wait_until(&mut p, give_up).timed_out() {
                break;
            }
        }
        let finished = !p.alive;
        drop(p);
        let handle = self.writer.lock().take();
        if finished {
            if let Some(handle) = handle {
                let _ = handle.join();
            }
        } else {
            tracing::warn!("Settings are still being saved at exit; not waiting any longer");
        }
    }
}

impl Inner {
    fn note_edit(&self) {
        let mut p = self.progress.lock();
        p.edits += 1;
        self.changed.notify_all();
    }

    /// The writer thread: sleeps until something changes, lets edits pile up
    /// for the batch window, then saves them all at once.
    fn run(&self) {
        let mut p = self.progress.lock();
        loop {
            while !p.has_work() && !p.closing {
                self.changed.wait(&mut p);
            }
            if !p.has_work() {
                break;
            }
            let due = Instant::now() + BATCH_WINDOW;
            while !p.hurry && !p.closing && !p.redo {
                if self.changed.wait_until(&mut p, due).timed_out() {
                    break;
                }
            }
            p.hurry = false;
            p.redo = false;
            let covers = p.edits;
            drop(p);

            #[cfg(test)]
            self.hooks.wait_for_release();
            let snapshot: BTreeMap<String, String> =
                self.values.lock().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            let outcome = store(&self.file, &snapshot);

            p = self.progress.lock();
            p.attempts += 1;
            p.tried = covers;
            match outcome {
                Ok(()) => {
                    p.stored = covers;
                    p.error = None;
                }
                Err(e) => {
                    tracing::warn!("Could not save settings: {e}");
                    p.error = Some(e);
                }
            }
            self.changed.notify_all();
        }
        p.alive = false;
        self.changed.notify_all();
    }
}

/// Writes the whole map beside the settings file, then swaps it in, so a
/// crash leaves either the old file or the new one.
fn store(file: &Path, values: &BTreeMap<String, String>) -> Result<(), String> {
    let folder = file.parent().unwrap_or(Path::new("."));
    crate::platform::files::create_private_dir(folder).map_err(|e| e.to_string())?;
    let json = serde_json::to_vec_pretty(values).map_err(|e| e.to_string())?;
    let temp = folder.join(TEMP_NAME);
    let put = |bytes: &[u8]| -> std::io::Result<()> {
        let mut out = crate::platform::files::private_options()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temp)?;
        out.write_all(bytes)?;
        out.sync_all()
    };
    if let Err(e) = put(&json).and_then(|()| std::fs::rename(&temp, file)) {
        let _ = std::fs::remove_file(&temp);
        return Err(e.to_string());
    }
    sync_folder(folder);
    Ok(())
}

/// Makes the rename itself durable where the system lets a folder be synced.
fn sync_folder(folder: &Path) {
    #[cfg(unix)]
    if let Ok(dir) = std::fs::File::open(folder) {
        let _ = dir.sync_all();
    }
    #[cfg(not(unix))]
    let _ = folder;
}

/// The old Kotlin app kept its settings under `%APPDATA%\AionDPS`.
fn kotlin_settings_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(base).join("AionDPS").join("settings.properties"))
}

fn read_kotlin(path: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(path).map(|text| parse_properties(&text)).unwrap_or_default()
}

/// `key=value` lines; `#` and `!` start comments. No escapes: the old app
/// never wrote any.
fn parse_properties(text: &str) -> HashMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with(['#', '!']))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .collect()
}

/// Knobs only the tests turn: a shorter quit limit, and a gate that stalls
/// every write the way a hung disk would.
#[cfg(test)]
#[derive(Default)]
struct TestHooks {
    quit_limit: Mutex<Option<Duration>>,
    gate_closed: Mutex<bool>,
    gate: Condvar,
}

#[cfg(test)]
impl TestHooks {
    fn wait_for_release(&self) {
        let mut closed = self.gate_closed.lock();
        while *closed {
            self.gate.wait(&mut closed);
        }
    }

    fn set_gate(&self, closed: bool) {
        *self.gate_closed.lock() = closed;
        self.gate.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::built::Scratch;

    fn file(dir: &Path) -> PathBuf {
        dir.join(FILE_NAME)
    }

    fn bytes(dir: &Path) -> Vec<u8> {
        std::fs::read(file(dir)).unwrap_or_default()
    }

    /// The value the file on disk holds for `key`.
    fn saved(dir: &Path, key: &str) -> Option<String> {
        read_file(&file(dir)).remove(key)
    }

    fn open(dir: &Path) -> Settings {
        Settings::open(dir.to_path_buf(), None)
    }

    fn block_temp_file(dir: &Path) -> PathBuf {
        let blocker = dir.join(TEMP_NAME);
        std::fs::create_dir(&blocker).expect("block the temporary file");
        blocker
    }

    fn writes_so_far(s: &Settings) -> u64 {
        s.inner.progress.lock().attempts
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|&(k, v)| (k.to_owned(), v.to_owned())).collect()
    }

    fn some(text: &str) -> Option<String> {
        Some(text.to_owned())
    }

    #[test]
    fn only_an_object_of_strings_reads_as_settings() {
        let dir = Scratch::new("settings.read");
        let path = dir.0.join("x.json");
        let cases = [
            (r#"{"a": "1", "b": true}"#, None),
            (r#"{"a": "1"}"#, some("1")),
            ("not json", None),
            (r#"["a"]"#, None),
        ];
        for (text, a) in cases {
            std::fs::write(&path, text).unwrap();
            let read = read_file(&path);
            assert_eq!(read.get("a").cloned(), a, "{text}");
            assert!(read.len() <= 1);
        }
        assert!(read_file(&dir.0.join("absent.json")).is_empty());
    }

    #[test]
    fn a_new_folder_gets_an_empty_object() {
        let dir = Scratch::new("settings.fresh");
        open(&dir).flush().unwrap();
        assert_eq!(bytes(&dir), b"{}");
    }

    #[test]
    fn properties_lines_follow_the_old_apps_rules() {
        let parsed = parse_properties("# old app\nhotkey = Ctrl+F1\nopacity=0.8\n\n!skip\nlanguage=ko\nbroken line\n");
        let want: HashMap<String, String> = pairs(&[("hotkey", "Ctrl+F1"), ("opacity", "0.8"), ("language", "ko")]).into_iter().collect();
        assert_eq!(parsed, want);
        assert_eq!(parse_properties("a=b=c")["a"], "b=c");
        assert_eq!(parse_properties("k=1\nk=2")["k"], "2");
        assert!(parse_properties("# only\n!comments\nno equals\n").is_empty());
    }

    #[test]
    fn old_app_settings_fill_a_missing_file() {
        let dir = Scratch::new("settings.migrate");
        let old = dir.0.join("settings.properties");
        std::fs::write(&old, "hotkey = Ctrl+F1\nopacity=0.8\nlanguage=ko\n").unwrap();
        let app = dir.0.join("app");
        let s = Settings::open(app.clone(), Some(old));
        s.flush().unwrap();
        let size = std::fs::metadata(app.join(FILE_NAME)).map(|m| m.len());
        assert_eq!(size.ok(), Some(65));
        assert!(!s.set("opacity", "0.8"));
        assert!(s.set("opacity", "0.9"));
    }

    #[test]
    fn an_unreadable_file_takes_old_settings_but_waits_for_an_edit() {
        let dir = Scratch::new("settings.unreadable");
        let old = dir.0.join("settings.properties");
        std::fs::write(&old, "hotkey=Ctrl+F1\nopacity=0.8\nlanguage=ko\n").unwrap();
        std::fs::write(file(&dir), "not json").unwrap();
        let s = Settings::open(dir.0.clone(), Some(old));
        assert_eq!(s.get("hotkey"), some("Ctrl+F1"));
        s.flush().unwrap();
        assert_eq!(bytes(&dir), b"not json");
        s.set("hotkey", "Alt+Z");
        s.flush().unwrap();
        assert_eq!((bytes(&dir).len(), saved(&dir, "language")), (63, some("ko")));
    }

    #[test]
    fn no_old_settings_leaves_the_map_empty() {
        let dir = Scratch::new("settings.nomigrate");
        let empty = dir.0.join("empty.properties");
        std::fs::write(&empty, "# nothing\n").unwrap();
        let sources = [None, Some(dir.0.join("missing.properties")), Some(empty)];
        for (n, kotlin) in sources.into_iter().enumerate() {
            let s = Settings::open(dir.0.join(format!("app{n}")), kotlin);
            assert!(s.get_all().is_empty(), "source {n}");
        }
    }

    #[test]
    fn edits_show_at_once_and_unicode_survives_the_file() {
        let dir = Scratch::new("settings.edits");
        let s = open(&dir);
        let odd = "値 \"quoted\" \\ \n second line";
        assert!(s.set("名前", odd));
        assert!(s.set("gone", "1"));
        s.remove("gone");
        s.remove("never there");
        assert_eq!(s.get("名前"), some(odd));
        assert!(s.get("gone").is_none());
        assert_eq!(s.get_all().into_iter().collect::<Vec<_>>(), pairs(&[("名前", odd)]));
        s.flush().unwrap();
        assert_eq!(saved(&dir, "名前"), some(odd));
        drop(s);
        assert_eq!(open(&dir).get("名前"), some(odd));
    }

    #[test]
    fn removing_an_absent_key_writes_nothing() {
        let dir = Scratch::new("settings.absent");
        let s = open(&dir);
        s.flush().unwrap();
        let before = writes_so_far(&s);
        s.remove("nothing");
        s.flush().unwrap();
        assert_eq!(writes_so_far(&s), before);
        s.clear();
        s.flush().unwrap();
        assert_eq!(writes_so_far(&s), before + 1);
        assert_eq!(bytes(&dir), b"{}");
    }

    #[test]
    fn a_slider_drag_becomes_a_few_writes() {
        let dir = Scratch::new("settings.batch");
        let s = open(&dir);
        s.flush().unwrap();
        let before = writes_so_far(&s);
        let changed = (0..100).filter(|n| s.set("opacity", &n.to_string())).count();
        let repeats = (0..100).filter(|_| s.set("opacity", "99")).count();
        assert_eq!((changed, repeats), (100, 0));
        s.flush().unwrap();
        assert_eq!((s.get("opacity"), saved(&dir, "opacity")), (some("99"), some("99")));
        assert!(writes_so_far(&s) - before < 100);
    }

    #[test]
    fn an_edit_reaches_disk_without_a_flush() {
        let dir = Scratch::new("settings.deadline");
        let s = open(&dir);
        s.flush().unwrap();
        s.set("value", "soon");
        let start = Instant::now();
        while saved(&dir, "value").is_none() {
            assert!(start.elapsed() < Duration::from_secs(1), "not saved within a second");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(saved(&dir, "value"), some("soon"));
    }

    #[test]
    fn many_threads_then_a_quick_exit_lose_nothing() {
        const KEYS: [&str; 4] = ["x", "y", "width", "height"];
        let dir = Scratch::new("settings.threads");
        let s = open(&dir);
        std::thread::scope(|scope| {
            let workers: Vec<_> =
                KEYS.iter().map(|key| scope.spawn(|| (0..100).for_each(|n| {
                    s.set(key, &n.to_string());
                }))).collect();
            workers.into_iter().for_each(|w| w.join().unwrap());
        });
        s.set("removed", "value");
        s.remove("removed");
        drop(s);
        let s = open(&dir);
        let found: Vec<_> = KEYS.iter().map(|k| s.get(k)).collect();
        assert_eq!(found, vec![some("99"); 4]);
        assert!(s.get("removed").is_none());
        s.clear();
        s.flush().unwrap();
        assert_eq!(bytes(&dir), b"{}");
    }

    #[test]
    fn a_zero_limit_times_out_and_a_normal_flush_saves() {
        let dir = Scratch::new("settings.limit");
        let s = open(&dir);
        // Held at the gate, no write that covers the edit can finish first,
        // however late this thread gets there.
        s.inner.hooks.set_gate(true);
        s.set("value", "late");
        assert_eq!(s.wait_saved(Duration::ZERO).unwrap_err(), TIMED_OUT);
        s.inner.hooks.set_gate(false);
        assert!(s.flush().is_ok());
        assert_eq!(saved(&dir, "value"), some("late"));
    }

    #[test]
    fn the_file_is_replaced_whole() {
        let dir = Scratch::new("settings.swap");
        let s = open(&dir);
        for word in ["first", "second"] {
            s.set("value", word);
            s.flush().unwrap();
        }
        assert_eq!(saved(&dir, "value"), some("second"));
        assert!(!dir.0.join(TEMP_NAME).exists());
    }

    #[test]
    fn a_blocked_save_keeps_the_old_file_and_the_next_flush_tries_again() {
        let dir = Scratch::new("settings.retry");
        let s = open(&dir);
        s.set("value", "previous");
        s.flush().unwrap();
        let blocker = block_temp_file(&dir);
        s.set("value", "failed");
        let failed = s.flush();
        assert!(failed.is_err());
        assert_eq!(saved(&dir, "value"), some("previous"));
        // No new edit: the writer must not have looped on the failure.
        let count = writes_so_far(&s);
        std::thread::sleep(BATCH_WINDOW * 2);
        assert_eq!(count, writes_so_far(&s));
        std::fs::remove_dir(blocker).unwrap();
        assert!(s.flush().is_ok());
        assert_eq!(saved(&dir, "value"), some("failed"));
    }

    #[test]
    fn closing_retries_a_failed_save() {
        let dir = Scratch::new("settings.dropretry");
        let s = open(&dir);
        let blocker = block_temp_file(&dir);
        s.set("value", "kept");
        assert!(s.flush().is_err());
        std::fs::remove_dir(blocker).unwrap();
        drop(s);
        assert_eq!(saved(&dir, "value"), some("kept"));
    }

    #[test]
    fn a_folder_that_cannot_exist_fails_the_flush() {
        let dir = Scratch::new("settings.nofolder");
        let plain = dir.0.join("plain");
        std::fs::write(&plain, "x").unwrap();
        let s = Settings::open(plain.join("app"), None);
        assert!(s.flush().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_save_makes_the_folder_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = Scratch::new("settings.modes");
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let (fresh, open_before) = (dir.0.join("fresh"), dir.0.join("open"));
        std::fs::create_dir(&open_before).unwrap();
        std::fs::set_permissions(&open_before, std::fs::Permissions::from_mode(0o755)).unwrap();
        for folder in [fresh, open_before] {
            let s = Settings::open(folder.clone(), None);
            s.set("a", "1");
            s.flush().unwrap();
            assert_eq!((mode(&folder), mode(&file(&folder))), (0o700, 0o600), "{}", folder.display());
        }
    }

    #[test]
    fn exit_gives_up_on_a_hung_disk_and_the_writer_finishes_later() {
        let dir = Scratch::new("settings.stuck");
        let s = open(&dir);
        s.flush().unwrap();
        *s.inner.hooks.quit_limit.lock() = Some(Duration::from_millis(50));
        s.inner.hooks.set_gate(true);
        s.set("value", "stuck");
        assert!(s.flush().is_err());
        let inner = Arc::clone(&s.inner);
        let start = Instant::now();
        drop(s);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(inner.progress.lock().alive);
        inner.hooks.set_gate(false);
        for _ in 0..10 {
            if !inner.progress.lock().alive {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        assert!(!inner.progress.lock().alive, "writer never finished");
        assert_eq!(saved(&dir, "value"), some("stuck"));
    }
}
