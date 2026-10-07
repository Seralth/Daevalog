use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex, RwLock};
use tracing::{info, warn};

/// How long exit waits for the last settings write. A stuck disk must not
/// keep the process alive.
const EXIT_WAIT: Duration = Duration::from_secs(5);

/// Application settings stored as key-value pairs, in settings.json in the
/// app data directory. Also migrated from the Kotlin app's settings.properties
/// on first run.
///
/// Settings are immediately visible in memory. A single sleeping writer
/// coalesces changes, with at most one pending snapshot and no idle polling.
pub struct Settings {
    shared: Arc<Shared>,
    writer: Option<JoinHandle<()>>,
    exit_wait: Duration,
}

struct Shared {
    values: RwLock<HashMap<String, String>>,
    file_path: PathBuf,
    pending: Mutex<Pending>,
    wake: Condvar,
    /// Tests: hold every write, as a stuck disk would.
    #[cfg(test)]
    stall: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct Pending {
    generation: u64,
    completed: u64,
    flush: bool,
    closing: bool,
    /// The writer has ended.
    stopped: bool,
    error: Option<String>,
    #[cfg(test)]
    writes: usize,
}

/// settings.json as the meter reads it: an object of strings. A file that is
/// missing or does not read that way counts as empty. Everything that reads
/// the file uses this, so no two readers can see different values.
pub fn read_file(file_path: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(file_path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

impl Settings {
    pub fn new(app_data_dir: PathBuf) -> Self {
        let file_path = app_data_dir.join("settings.json");
        let mut values = read_file(&file_path);
        if values.is_empty() {
            if let Some(migrated) = Self::try_migrate_from_kotlin() {
                values = migrated;
                info!("Migrated {} settings from Kotlin app", values.len());
            }
        }
        let missing = !file_path.exists();
        let shared = Arc::new(Shared {
            values: RwLock::new(values),
            file_path,
            pending: Mutex::new(Pending::default()),
            wake: Condvar::new(),
            #[cfg(test)]
            stall: std::sync::atomic::AtomicBool::new(false),
        });
        let worker = shared.clone();
        let writer = std::thread::Builder::new()
            .name("settings-writer".into())
            .spawn(move || worker.write_loop())
            .expect("start settings writer");
        let settings = Self {
            shared,
            writer: Some(writer),
            exit_wait: EXIT_WAIT,
        };
        if missing {
            settings.schedule_save();
        }
        settings
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.shared.values.read().get(key).cloned()
    }

    /// Only actual changes schedule a write or trigger a cross-window event.
    pub fn set(&self, key: &str, value: &str) -> bool {
        {
            let mut values = self.shared.values.write();
            if values.get(key).is_some_and(|old| old == value) {
                return false;
            }
            values.insert(key.to_string(), value.to_string());
        }
        self.schedule_save();
        true
    }

    /// Stores the values whose keys have none yet; a stored value stays.
    /// Returns the keys it stored, sorted.
    pub fn insert_missing(&self, values: impl IntoIterator<Item = (String, String)>) -> Vec<String> {
        let mut inserted = Vec::new();
        {
            let mut stored = self.shared.values.write();
            for (key, value) in values {
                if !stored.contains_key(&key) {
                    stored.insert(key.clone(), value);
                    inserted.push(key);
                }
            }
        }
        if !inserted.is_empty() {
            self.schedule_save();
        }
        inserted.sort();
        inserted
    }

    pub fn remove(&self, key: &str) {
        if self.shared.values.write().remove(key).is_some() {
            self.schedule_save();
        }
    }

    pub fn clear(&self) {
        self.shared.values.write().clear();
        self.schedule_save();
    }

    pub fn get_all(&self) -> HashMap<String, String> {
        self.shared.values.read().clone()
    }

    fn schedule_save(&self) {
        let mut pending = self.shared.pending.lock();
        pending.generation += 1;
        self.shared.wake.notify_all();
    }

    /// Exit waits for the last accepted value, including a slider's final edit,
    /// but a stuck disk cannot keep the process alive.
    pub fn flush(&self) -> Result<(), String> {
        self.flush_within(self.exit_wait)
    }

    fn flush_within(&self, timeout: Duration) -> Result<(), String> {
        let mut pending = self.shared.pending.lock();
        // An explicit flush retries a failed attempt once, without a background
        // retry loop or requiring another user edit after the disk recovers.
        if pending.completed == pending.generation && pending.error.is_some() {
            pending.generation += 1;
        }
        let wanted = pending.generation;
        if pending.completed >= wanted {
            return pending.error.clone().map_or(Ok(()), Err);
        }
        pending.flush = true;
        self.shared.wake.notify_all();
        let deadline = Instant::now() + timeout;
        while pending.completed < wanted {
            if self.shared.wake.wait_until(&mut pending, deadline).timed_out() {
                return Err("Timed out saving settings".to_string());
            }
        }
        pending.error.clone().map_or(Ok(()), Err)
    }

    /// Try to migrate settings from the Kotlin app's settings.properties file.
    fn try_migrate_from_kotlin() -> Option<HashMap<String, String>> {
        let appdata = std::env::var("APPDATA").ok()?;
        let kotlin_file = PathBuf::from(&appdata)
            .join("AionDPS")
            .join("settings.properties");
        if !kotlin_file.exists() {
            return None;
        }
        let text = std::fs::read_to_string(&kotlin_file).ok()?;
        let mut map = HashMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                map.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
        if map.is_empty() { None } else { Some(map) }
    }
}

impl Shared {
    fn save(&self) -> Result<(), String> {
        #[cfg(test)]
        while self.stall.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(1));
        }
        // Do not hold the values lock during serialization or disk I/O.
        let values = self.values.read().clone();
        let json = serde_json::to_vec_pretty(&values).map_err(|e| e.to_string())?;
        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        // A failed/interrupted write leaves the previous complete file intact.
        // The new file is on disk before it replaces the old one, so a crash or
        // power loss right after cannot leave an empty settings.json.
        let temporary = self.file_path.with_extension("json.tmp");
        let written = crate::platform::files::private_options()
            .write(true).create(true).truncate(true)
            .open(&temporary)
            .and_then(|mut file| {
                file.write_all(&json)?;
                file.sync_all()
            });
        if let Err(e) = written {
            let _ = std::fs::remove_file(&temporary);
            return Err(e.to_string());
        }
        std::fs::rename(&temporary, &self.file_path).map_err(|e| e.to_string())?;
        // The rename itself, where the system can sync a folder.
        if let Some(dir) = self.file_path.parent().and_then(|p| std::fs::File::open(p).ok()) {
            let _ = dir.sync_all();
        }
        Ok(())
    }

    fn write_loop(&self) {
        let mut pending = self.pending.lock();
        loop {
            while pending.generation == pending.completed && !pending.closing {
                self.wake.wait(&mut pending);
            }
            if pending.closing && pending.generation == pending.completed {
                pending.stopped = true;
                self.wake.notify_all();
                break;
            }
            // Fixed batching deadline: sustained slider motion cannot postpone
            // persistence forever. Notifications only shorten it for flush/exit.
            let deadline = Instant::now() + Duration::from_millis(250);
            while !pending.flush && !pending.closing {
                if self.wake.wait_until(&mut pending, deadline).timed_out() {
                    break;
                }
            }
            let generation = pending.generation;
            pending.flush = false;
            drop(pending);
            let result = self.save();
            if let Err(error) = &result {
                warn!("Could not save settings: {error}");
            }
            pending = self.pending.lock();
            pending.completed = generation;
            pending.error = result.err();
            #[cfg(test)]
            {
                pending.writes += 1;
            }
            self.wake.notify_all();
            // On failure wait for another change, rather than retrying in a loop.
        }
    }
}

impl Drop for Settings {
    /// Writes what is pending, but waits at most `exit_wait` for the writer:
    /// one stuck on the disk is left behind rather than joined.
    fn drop(&mut self) {
        let stopped = {
            let mut pending = self.shared.pending.lock();
            if pending.completed == pending.generation && pending.error.is_some() {
                pending.generation += 1;
            }
            pending.closing = true;
            self.shared.wake.notify_all();
            let deadline = Instant::now() + self.exit_wait;
            while !pending.stopped {
                if self.shared.wake.wait_until(&mut pending, deadline).timed_out() {
                    break;
                }
            }
            pending.stopped
        };
        if let Some(writer) = self.writer.take() {
            if stopped {
                let _ = writer.join();
            } else {
                warn!("Settings writer still busy at exit; not waiting for it");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "a2-settings-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn read(&self) -> HashMap<String, String> {
            serde_json::from_slice(&std::fs::read(self.0.join("settings.json")).unwrap()).unwrap()
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn slider_changes_are_coalesced_and_flush_persists_the_last_value() {
        let dir = Directory::new();
        let settings = Settings::new(dir.0.clone());
        settings.flush().unwrap();
        let before = settings.shared.pending.lock().writes;
        for value in 0..100 {
            assert!(settings.set("opacity", &value.to_string()));
        }
        assert_eq!(settings.get("opacity").as_deref(), Some("99"));
        for _ in 0..100 {
            assert!(!settings.set("opacity", "99"));
        }
        settings.flush().unwrap();
        assert_eq!(dir.read().get("opacity").map(String::as_str), Some("99"));
        let writes = settings.shared.pending.lock().writes - before;
        assert!(writes < 100, "every slider event wrote a file: {writes}");
        eprintln!("settings: 100 changes, {writes} file writes");
    }

    #[test]
    fn concurrent_changes_and_removal_survive_exit_and_reload() {
        let dir = Directory::new();
        {
            let settings = Settings::new(dir.0.clone());
            std::thread::scope(|scope| {
                for key in ["x", "y", "width", "height"] {
                    let settings = &settings;
                    scope.spawn(move || {
                        for i in 0..100 {
                            settings.set(key, &i.to_string());
                        }
                    });
                }
            });
            settings.set("removed", "value");
            settings.remove("removed");
            // Drop, including exit before the batching deadline, drains the writer.
        }
        let settings = Settings::new(dir.0.clone());
        for key in ["x", "y", "width", "height"] {
            assert_eq!(settings.get(key).as_deref(), Some("99"));
        }
        assert!(settings.get("removed").is_none());
        settings.clear();
        settings.flush().unwrap();
        assert!(dir.read().is_empty());
    }

    #[test]
    fn insert_missing_keeps_stored_values_and_saves_the_new_ones() {
        let dir = Directory::new();
        let settings = Settings::new(dir.0.clone());
        settings.set("kept", "file");
        let inserted = settings.insert_missing([
            ("kept".to_string(), "page".to_string()),
            ("new".to_string(), "page".to_string()),
        ]);
        assert_eq!(inserted, ["new"]);
        settings.flush().unwrap();
        let file = dir.read();
        assert_eq!(file.get("kept").map(String::as_str), Some("file"));
        assert_eq!(file.get("new").map(String::as_str), Some("page"));
        assert!(settings.insert_missing([("new".to_string(), "again".to_string())]).is_empty());
        assert_eq!(settings.get("new").as_deref(), Some("page"));
    }

    #[test]
    fn a_file_that_is_not_an_object_of_strings_reads_as_empty() {
        let dir = Directory::new();
        let path = dir.0.join("settings.json");
        std::fs::write(&path, r#"{"a": "1", "b": true}"#).unwrap();
        assert!(read_file(&path).is_empty());
        std::fs::write(&path, r#"{"a": "1"}"#).unwrap();
        assert_eq!(read_file(&path).get("a").map(String::as_str), Some("1"));
        assert!(read_file(&dir.0.join("missing.json")).is_empty());
    }

    #[test]
    fn flush_gives_up_at_its_deadline() {
        let dir = Directory::new();
        let settings = Settings::new(dir.0.clone());
        settings.set("value", "late");
        assert!(settings.flush_within(Duration::ZERO).is_err());
        settings.flush().unwrap();
        assert_eq!(dir.read().get("value").map(String::as_str), Some("late"));
    }

    #[test]
    fn a_stuck_writer_does_not_hold_up_exit() {
        let dir = Directory::new();
        let mut settings = Settings::new(dir.0.clone());
        settings.flush().unwrap();
        settings.exit_wait = Duration::from_millis(50);
        let shared = settings.shared.clone();
        shared.stall.store(true, Ordering::Relaxed);
        settings.set("value", "stuck");
        assert!(settings.flush().is_err(), "the flush gives up at its deadline");
        let started = Instant::now();
        drop(settings);
        assert!(started.elapsed() < Duration::from_secs(2), "exit waited for the writer");
        shared.stall.store(false, Ordering::Relaxed);
        // The writer left behind still finishes the write and ends.
        let mut pending = shared.pending.lock();
        while !pending.stopped {
            shared.wake.wait_for(&mut pending, Duration::from_secs(1));
        }
        drop(pending);
        assert_eq!(dir.read().get("value").map(String::as_str), Some("stuck"));
    }

    #[test]
    fn the_new_file_replaces_the_old_one_whole() {
        let dir = Directory::new();
        let settings = Settings::new(dir.0.clone());
        settings.set("value", "first");
        settings.flush().unwrap();
        settings.set("value", "second");
        settings.flush().unwrap();
        assert_eq!(dir.read().get("value").map(String::as_str), Some("second"));
        assert!(!dir.0.join("settings.json.tmp").exists(), "no temporary file left");
    }

    #[test]
    fn failed_write_preserves_previous_json_and_flush_can_retry_without_a_new_edit() {
        let dir = Directory::new();
        let settings = Settings::new(dir.0.clone());
        settings.set("value", "previous");
        settings.flush().unwrap();
        let temporary = dir.0.join("settings.json.tmp");
        std::fs::create_dir(&temporary).unwrap();
        settings.set("value", "failed");
        assert!(settings.flush().is_err());
        assert_eq!(
            dir.read().get("value").map(String::as_str),
            Some("previous")
        );
        std::fs::remove_dir(&temporary).unwrap();
        settings.flush().unwrap();
        assert_eq!(dir.read().get("value").map(String::as_str), Some("failed"));
    }
}
