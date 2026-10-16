use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use super::history_view::{fight_players, FightPlayers};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use parking_lot::Mutex;

use crate::entity::fight_record::{FightRecord, FightSummary};
use crate::platform::files;

/// Every fight's summary, and its players' damage for History, by id.
type Listing = (Vec<FightSummary>, Arc<HashMap<String, FightPlayers>>);
/// Whether `name` is a single file name: no path separator, no "..", no
/// drive prefix. Fight ids and icon keys come from the front end and become
/// file names under the app data directory.
pub fn is_plain_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\', ':', '\0']) && !name.contains("..")
}

fn check_id(id: &str) -> Result<(), String> {
    if is_plain_name(id) {
        Ok(())
    } else {
        Err(format!("Invalid fight id: {id:?}"))
    }
}

/// Fights kept on disk; past this the ones that started first go.
const KEEP_FIGHTS: usize = 500;

/// Numbers the temporary files of every manager in the process, so two
/// saves of one fight never share one.
static TEMP_SERIAL: AtomicU64 = AtomicU64::new(0);

/// What a look at the folder's file list tells without opening a file: an
/// outside change to the fights moves one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FolderLook {
    fight_files: usize,
    latest_change: Option<SystemTime>,
}

/// The last listing read from disk, with what the folder looked like then.
struct KeptListing {
    look: FolderLook,
    writes_seen: u64,
    fights: Vec<FightSummary>,
    players: Arc<HashMap<String, FightPlayers>>,
}

/// The saved fights in `<app data dir>/history`, one `<id>.json` each.
pub struct FightHistoryManager {
    folder: PathBuf,
    tickets: AtomicU64,
    /// Per fight, the highest snapshot ticket written. Held over each rename
    /// so a check and its rename are one step.
    ticket_by_fight: Mutex<HashMap<String, u64>>,
    live: Mutex<HashSet<String>>,
    /// Bumped on every change this manager makes to the folder.
    own_writes: AtomicU64,
    kept: Mutex<Option<KeptListing>>,
    /// Per fight file, the start it read as (`None`: not a fight), with the
    /// file's time and size then. The trim reads only files not seen so.
    starts: Mutex<HashMap<String, (FileStamp, Option<i64>)>>,
}

/// A file's time and size.
type FileStamp = (Option<SystemTime>, u64);

impl FightHistoryManager {
    pub fn new(app_data_dir: PathBuf) -> Self {
        let folder = app_data_dir.join("history");
        let _ = files::create_private_dir(&folder);
        sweep_leftovers(&folder);
        Self {
            folder,
            tickets: AtomicU64::new(0),
            ticket_by_fight: Mutex::new(HashMap::new()),
            live: Mutex::new(HashSet::new()),
            own_writes: AtomicU64::new(0),
            kept: Mutex::new(None),
            starts: Mutex::new(HashMap::new()),
        }
    }

    fn fight_path(&self, id: &str) -> PathBuf {
        self.folder.join(format!("{id}.json"))
    }

    /// Saves the fight whole, whatever was saved before.
    pub fn save_fight(&self, record: &FightRecord) -> Result<(), String> {
        self.write_fight(record, None)
    }

    /// A ticket for a snapshot about to be taken: a later ticket marks a
    /// later snapshot.
    pub fn snapshot_ticket(&self) -> u64 {
        self.tickets.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Saves the fight unless a snapshot with a later ticket is already on
    /// disk, so a slow old snapshot never overwrites a newer one.
    pub fn save_snapshot(&self, record: &FightRecord, ticket: u64) -> Result<(), String> {
        self.write_fight(record, Some(ticket))
    }

    fn write_fight(&self, record: &FightRecord, ticket: Option<u64>) -> Result<(), String> {
        let id = record.id.as_str();
        check_id(id)?;
        let body = serde_json::to_string_pretty(record).map_err(|e| format!("Could not encode fight {id}: {e}"))?;
        let _ = files::create_private_dir(&self.folder);
        let serial = TEMP_SERIAL.fetch_add(1, Ordering::SeqCst);
        let temp = self.folder.join(format!(".{id}.{serial}.tmp"));
        if let Err(e) = files::write_private(&temp, body) {
            let _ = fs::remove_file(&temp);
            return Err(format!("Could not write fight {id}: {e}"));
        }
        {
            let mut by_fight = self.ticket_by_fight.lock();
            if let Some(t) = ticket {
                if by_fight.get(id).is_some_and(|&seen| seen > t) {
                    drop(by_fight);
                    let _ = fs::remove_file(&temp);
                    return Ok(());
                }
            }
            if let Err(e) = fs::rename(&temp, self.fight_path(id)) {
                drop(by_fight);
                let _ = fs::remove_file(&temp);
                return Err(format!("Could not save fight {id}: {e}"));
            }
            if let Some(t) = ticket {
                by_fight.insert(id.to_string(), t);
            }
        }
        self.own_writes.fetch_add(1, Ordering::SeqCst);
        self.trim_to_limit(record);
        Ok(())
    }

    /// Past the limit, drops the fights that started first. Only files that
    /// read as a fight count and can go. `saved` was just written, so its
    /// file is not read back.
    fn trim_to_limit(&self, saved: &FightRecord) {
        let files = fight_files(&self.folder);
        let mut starts = self.starts.lock();
        let present: HashSet<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
        starts.retain(|name, _| present.contains(name.as_str()));
        if files.len() <= KEEP_FIGHTS {
            return;
        }
        let saved_name = format!("{}.json", saved.id);
        let mut by_start = Vec::with_capacity(files.len());
        for (name, stamp) in files {
            let start = match starts.get(&name) {
                Some((seen, start)) if *seen == stamp => *start,
                _ if name == saved_name => {
                    starts.insert(name.clone(), (stamp, Some(saved.start_time_ms)));
                    Some(saved.start_time_ms)
                }
                _ => match read_start(&self.folder.join(&name)) {
                    Some(start) => {
                        starts.insert(name.clone(), (stamp, start));
                        start
                    }
                    None => None,
                },
            };
            if let Some(start) = start {
                by_start.push((start, name));
            }
        }
        let Some(extra) = by_start.len().checked_sub(KEEP_FIGHTS).filter(|&n| n > 0) else { return };
        by_start.sort_unstable();
        by_start.truncate(extra);
        for (_, name) in by_start {
            match fs::remove_file(self.folder.join(&name)) {
                Ok(()) => {
                    tracing::info!("History full: removed the oldest fight {name}");
                    starts.remove(&name);
                    if let Some(id) = name.strip_suffix(".json") {
                        self.ticket_by_fight.lock().remove(id);
                    }
                }
                Err(e) => tracing::warn!("History full, but {name} could not be removed: {e}"),
            }
        }
        self.own_writes.fetch_add(1, Ordering::SeqCst);
    }

    pub fn load_fight(&self, id: &str) -> Result<FightRecord, String> {
        check_id(id)?;
        let text = fs::read_to_string(self.fight_path(id)).map_err(|e| format!("Could not read fight {id}: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("Fight {id} is damaged: {e}"))
    }

    pub fn delete_fight(&self, id: &str) -> Result<(), String> {
        check_id(id)?;
        // The fight's ticket stays: a slow snapshot taken before the delete must not write it back.
        let result = fs::remove_file(self.fight_path(id)).map_err(|e| format!("Could not delete fight {id}: {e}"));
        self.own_writes.fetch_add(1, Ordering::SeqCst);
        result
    }

    pub fn fight_summaries(&self) -> Vec<FightSummary> {
        self.listing().0
    }

    /// Every saved fight, newest first; fights that started at the same
    /// moment go by id the same way, so every machine lists them alike.
    /// Reads the files again only after a change to the folder.
    pub fn listing(&self) -> Listing {
        let writes = self.own_writes.load(Ordering::SeqCst);
        let look = look_at(&self.folder);
        let (mut fights, players) = {
            let mut kept = self.kept.lock();
            match kept.as_ref() {
                Some(k) if k.writes_seen == writes && k.look == look => (k.fights.clone(), k.players.clone()),
                _ => {
                    let (fights, players) = read_folder(&self.folder);
                    let players = Arc::new(players);
                    *kept = Some(KeptListing { look, writes_seen: writes, fights: fights.clone(), players: players.clone() });
                    (fights, players)
                }
            }
        };
        let live = self.live.lock();
        for fight in &mut fights {
            fight.is_live = live.contains(&fight.id);
        }
        (fights, players)
    }

    /// The fights the meter is still recording, replacing the last set.
    pub fn set_in_progress(&self, ids: HashSet<String>) {
        *self.live.lock() = ids;
    }

    pub fn is_in_progress(&self, id: &str) -> bool {
        self.live.lock().contains(id)
    }
}

/// Removes the temporary files a killed meter left behind.
fn sweep_leftovers(folder: &Path) {
    let Ok(entries) = fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') && name.ends_with(".tmp") {
            if fs::remove_file(entry.path()).is_ok() {
                tracing::info!("Removed an unfinished save: {name}");
            }
        }
    }
}

/// The names of the `*.json` files in the folder; folders are left out.
fn fight_file_names(folder: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(folder) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".json"))
        .collect()
}

/// The `*.json` files in the folder with each one's time and size.
fn fight_files(folder: &Path) -> Vec<(String, FileStamp)> {
    let Ok(entries) = fs::read_dir(folder) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| {
            let meta = e.metadata().ok();
            let stamp = (meta.as_ref().and_then(|m| m.modified().ok()), meta.map_or(0, |m| m.len()));
            (e.file_name().to_string_lossy().into_owned(), stamp)
        })
        .filter(|(name, _)| name.ends_with(".json"))
        .collect()
}

/// The start of the fight in `path`, `Some(None)` when the text is no
/// fight, `None` when the file could not be read.
fn read_start(path: &Path) -> Option<Option<i64>> {
    let text = fs::read_to_string(path).ok()?;
    Some(serde_json::from_str::<FightSummary>(&text).ok().map(|f| f.start_time_ms))
}

fn look_at(folder: &Path) -> FolderLook {
    let names = fight_file_names(folder);
    let latest_change = names
        .iter()
        .filter_map(|name| fs::metadata(folder.join(name)).and_then(|m| m.modified()).ok())
        .max();
    FolderLook { fight_files: names.len(), latest_change }
}

fn read_folder(folder: &Path) -> (Vec<FightSummary>, HashMap<String, FightPlayers>) {
    let mut fights = Vec::new();
    let mut players = HashMap::new();
    for name in fight_file_names(folder) {
        let Ok(text) = fs::read_to_string(folder.join(&name)) else { continue };
        let Ok(record) = serde_json::from_str::<FightRecord>(&text) else { continue };
        players.insert(record.id.clone(), fight_players(&record));
        fights.push(summary_of(&record));
    }
    fights.sort_by(|a, b| b.start_time_ms.cmp(&a.start_time_ms).then_with(|| b.id.cmp(&a.id)));
    (fights, players)
}

fn summary_of(record: &FightRecord) -> FightSummary {
    FightSummary {
        id: record.id.clone(),
        boss_name: record.boss_name.clone(),
        target_id: record.target_id,
        start_time_ms: record.start_time_ms,
        duration_ms: record.duration_ms,
        total_damage: record.total_damage,
        jobs: record.jobs.clone(),
        job_ids: record.job_ids.clone(),
        is_train: record.is_train,
        is_live: false,
        app_version: record.app_version.clone(),
        mob_code: record.mob_code,
        dungeon_id: record.dungeon_id,
        member_jobs: record.party_jobs(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::built::{self, Scratch};

    fn fights(scratch: &Scratch) -> PathBuf {
        scratch.join("history")
    }

    fn names(scratch: &Scratch) -> Vec<String> {
        let mut names: Vec<String> =
            fs::read_dir(fights(scratch)).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    /// A 10 ms fight of `damage`.
    fn record(id: &str, start: i64, damage: i64) -> FightRecord {
        let mut fight = built::fight(id, start, 10);
        fight.total_damage = damage;
        fight.details.total_target_damage = damage;
        fight
    }

    fn put(scratch: &Scratch, record: &FightRecord) {
        fs::write(fights(scratch).join(format!("{}.json", record.id)), serde_json::to_string_pretty(record).unwrap()).unwrap();
    }

    fn ids(fights: &[FightSummary]) -> Vec<&str> {
        fights.iter().map(|f| f.id.as_str()).collect()
    }

    #[test]
    fn ids_with_paths_never_touch_a_file() {
        for bad in ["", "..", "../settings", "a/b", "a\\b", "C:x", "auto..1"] {
            assert!(!is_plain_name(bad), "{bad:?}");
        }
        assert!(is_plain_name("auto_36734_1759578598645"));
        assert!(is_plain_name("Skill_Icon_01.png"));

        let scratch = Scratch::new("store.ids");
        let store = FightHistoryManager::new(scratch.0.clone());
        fs::write(scratch.0.join("keep.json"), "{}").unwrap();
        assert!(store.delete_fight("../keep").is_err());
        assert!(store.load_fight("../keep").is_err());
        assert!(store.save_fight(&record("../keep", 1, 1)).is_err());
        assert!(store.save_snapshot(&record("a/b", 1, 1), 1).is_err());
        assert!(scratch.0.join("keep.json").exists());
        assert_eq!(fs::read_to_string(scratch.0.join("keep.json")).unwrap(), "{}");
    }

    #[test]
    fn unfinished_saves_are_swept_on_start() {
        let scratch = Scratch::new("store.sweep");
        fs::create_dir_all(fights(&scratch)).unwrap();
        fs::write(fights(&scratch).join(".auto_1_1000.3.tmp"), "{").unwrap();
        fs::write(fights(&scratch).join("auto_1_1000.json"), "{}").unwrap();
        let store = FightHistoryManager::new(scratch.0.clone());
        assert_eq!(names(&scratch), ["auto_1_1000.json"]);
        assert!(!store.is_in_progress("auto_1_1000"));
        assert_eq!(store.snapshot_ticket(), 1, "tickets start at 1");
    }

    #[test]
    fn a_save_replaces_the_file_and_an_older_snapshot_loses() {
        let scratch = Scratch::new("store.save");
        let store = FightHistoryManager::new(scratch.0.clone());
        store.save_fight(&record("auto_1_1000", 1000, 1)).unwrap();
        store.save_fight(&record("auto_1_1000", 1000, 2)).unwrap();
        assert_eq!(store.load_fight("auto_1_1000").unwrap().total_damage, 2);
        assert_eq!(names(&scratch), ["auto_1_1000.json"]);
        assert_eq!(store.fight_summaries().len(), 1);

        let older = store.snapshot_ticket();
        let newer = store.snapshot_ticket();
        assert!(older < newer);
        store.save_snapshot(&record("auto_1_1000", 1000, 4), newer).unwrap();
        store.save_snapshot(&record("auto_1_1000", 1000, 3), older).unwrap();
        assert_eq!(store.load_fight("auto_1_1000").unwrap().total_damage, 4);
        store.save_snapshot(&record("auto_1_1000", 1000, 6), newer).unwrap();
        assert_eq!(store.load_fight("auto_1_1000").unwrap().total_damage, 6, "the same ticket writes again");
        let latest = store.snapshot_ticket();
        store.save_snapshot(&record("auto_1_1000", 1000, 5), latest).unwrap();
        assert_eq!(store.load_fight("auto_1_1000").unwrap().total_damage, 5);
        assert_eq!(names(&scratch), ["auto_1_1000.json"]);
    }

    #[test]
    fn the_saved_file_is_the_pretty_record() {
        let scratch = Scratch::new("store.body");
        let store = FightHistoryManager::new(scratch.0.clone());
        let fight = record("auto_7_7000", 7000, 70);
        store.save_fight(&fight).unwrap();
        let text = fs::read_to_string(fights(&scratch).join("auto_7_7000.json")).unwrap();
        assert_eq!(text, serde_json::to_string_pretty(&fight).unwrap());
    }

    #[test]
    fn tickets_count_up_from_any_thread() {
        let scratch = Scratch::new("store.tickets");
        let store = Arc::new(FightHistoryManager::new(scratch.0.clone()));
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let store = store.clone();
                std::thread::spawn(move || (0..25).map(|_| store.snapshot_ticket()).collect::<Vec<_>>())
            })
            .collect();
        let mut all: Vec<u64> = workers.into_iter().flat_map(|w| w.join().unwrap()).collect();
        all.sort();
        assert_eq!(all, (1..=100).collect::<Vec<u64>>());
    }

    #[test]
    fn deleting_a_fight_and_a_missing_one() {
        let scratch = Scratch::new("store.delete");
        let store = FightHistoryManager::new(scratch.0.clone());
        store.save_fight(&record("auto_1_1000", 1000, 1)).unwrap();
        store.save_fight(&record("auto_2_2000", 2000, 1)).unwrap();
        assert_eq!(store.fight_summaries().len(), 2);
        store.delete_fight("auto_1_1000").unwrap();
        assert_eq!(ids(&store.fight_summaries()), ["auto_2_2000"]);
        assert!(store.delete_fight("auto_1_1000").is_err());
        assert!(store.load_fight("auto_1_1000").is_err());
    }

    #[test]
    fn the_listing_is_newest_first_and_ties_go_by_id() {
        for order in [["auto_1_2000", "auto_2_2000"], ["auto_2_2000", "auto_1_2000"]] {
            let scratch = Scratch::new("store.order");
            let store = FightHistoryManager::new(scratch.0.clone());
            store.save_fight(&record("auto_9_1000", 1000, 1)).unwrap();
            store.save_fight(&record(order[0], 2000, 1)).unwrap();
            store.save_fight(&record("auto_5_3000", 3000, 1)).unwrap();
            store.save_fight(&record(order[1], 2000, 1)).unwrap();
            assert_eq!(ids(&store.fight_summaries()), ["auto_5_3000", "auto_2_2000", "auto_1_2000", "auto_9_1000"]);
        }
    }

    #[test]
    fn the_listing_copies_the_record_and_its_players() {
        let scratch = Scratch::new("store.copy");
        let store = FightHistoryManager::new(scratch.0.clone());
        let mut fight = record("auto_3_3000", 3000, 30);
        fight.jobs = vec!["검성".into()];
        fight.job_ids = vec![11];
        fight.is_train = true;
        fight.app_version = "2.0.0".into();
        fight.mob_code = 700;
        fight.dungeon_id = 600091;
        // A file named apart from its id still lists by the record's id.
        fs::write(fights(&scratch).join("renamed.json"), serde_json::to_string(&fight).unwrap()).unwrap();
        let (fights, players) = store.listing();
        assert_eq!(fights.len(), 1);
        let f = &fights[0];
        assert_eq!(f.id, "auto_3_3000");
        assert_eq!((f.boss_name.as_str(), f.target_id, f.start_time_ms, f.duration_ms, f.total_damage), ("B", 1, 3000, 10, 30));
        assert_eq!((f.jobs.clone(), f.job_ids.clone(), f.is_train, f.app_version.as_str()), (vec!["검성".to_string()], vec![11], true, "2.0.0"));
        assert_eq!((f.mob_code, f.dungeon_id, f.is_live), (700, 600091, false));
        assert_eq!(f.member_jobs, fight.party_jobs());
        assert_eq!(players.get("auto_3_3000"), Some(&fight_players(&fight)));
    }

    #[test]
    fn only_fight_files_are_listed() {
        let scratch = Scratch::new("store.others");
        let store = FightHistoryManager::new(scratch.0.clone());
        store.save_fight(&record("auto_1_1000", 1000, 1)).unwrap();
        fs::write(fights(&scratch).join("notes.txt"), "hello").unwrap();
        fs::write(fights(&scratch).join("broken.json"), "{\"id\":").unwrap();
        fs::write(fights(&scratch).join("summary.json"), "{}").unwrap();
        fs::write(fights(&scratch).join(".auto_1_1000.9.tmp"), serde_json::to_string(&record("auto_1_1000", 1000, 9)).unwrap()).unwrap();
        fs::create_dir_all(fights(&scratch).join("sub.json")).unwrap();
        fs::create_dir_all(fights(&scratch).join("folder")).unwrap();
        let (fights, players) = store.listing();
        assert_eq!(ids(&fights), ["auto_1_1000"]);
        assert_eq!(fights[0].total_damage, 1);
        assert_eq!(players.len(), 1);
    }

    #[test]
    fn a_missing_folder_lists_nothing() {
        let scratch = Scratch::new("store.gone");
        let store = FightHistoryManager::new(scratch.0.clone());
        fs::remove_dir_all(fights(&scratch)).unwrap();
        assert!(store.listing().0.is_empty());
        assert!(store.load_fight("auto_1_1000").is_err());
    }

    #[test]
    fn an_unchanged_folder_is_not_read_again() {
        let scratch = Scratch::new("store.keep");
        let store = FightHistoryManager::new(scratch.0.clone());
        store.save_fight(&record("auto_1_1000", 1000, 1)).unwrap();
        let first = store.fight_summaries();
        assert_eq!(first.len(), 1);

        // Spoil the file but leave its time as it was: a listing that read
        // the file again would drop the fight.
        let path = fights(&scratch).join("auto_1_1000.json");
        let when = fs::metadata(&path).unwrap().modified().unwrap();
        fs::write(&path, "spoiled").unwrap();
        fs::File::options().write(true).open(&path).unwrap().set_modified(when).unwrap();
        let second = store.fight_summaries();
        assert_eq!(ids(&first), ids(&second));
        assert_eq!(second[0].total_damage, 1);
    }

    #[test]
    fn outside_and_own_changes_show_in_the_next_listing() {
        let scratch = Scratch::new("store.change");
        let store = FightHistoryManager::new(scratch.0.clone());
        store.save_fight(&record("auto_1_1000", 1000, 1)).unwrap();
        assert_eq!(store.fight_summaries().len(), 1);

        put(&scratch, &record("auto_2_2000", 2000, 1));
        assert_eq!(ids(&store.fight_summaries()), ["auto_2_2000", "auto_1_1000"], "a file added by hand");

        // Rewritten from outside with a later time and the same count.
        let path = fights(&scratch).join("auto_2_2000.json");
        put(&scratch, &record("auto_2_2000", 2000, 22));
        let later = SystemTime::now() + Duration::from_secs(60);
        fs::File::options().write(true).open(&path).unwrap().set_modified(later).unwrap();
        assert_eq!(store.fight_summaries()[0].total_damage, 22, "a file rewritten by hand");

        fs::remove_file(&path).unwrap();
        assert_eq!(ids(&store.fight_summaries()), ["auto_1_1000"], "a file removed by hand");

        store.save_fight(&record("auto_1_1000", 1000, 11)).unwrap();
        assert_eq!(store.fight_summaries()[0].total_damage, 11, "this manager's own save");
    }

    #[test]
    fn in_progress_is_asked_fresh() {
        let scratch = Scratch::new("store.live");
        let store = FightHistoryManager::new(scratch.0.clone());
        store.save_fight(&record("auto_1_1000", 1000, 1)).unwrap();
        assert!(!store.fight_summaries()[0].is_live);
        store.set_in_progress(HashSet::from(["auto_1_1000".to_string()]));
        assert!(store.fight_summaries()[0].is_live);
        assert!(store.is_in_progress("auto_1_1000"));

        store.set_in_progress(HashSet::from(["a".to_string()]));
        assert!(store.is_in_progress("a") && !store.is_in_progress("auto_1_1000"));
        store.set_in_progress(HashSet::new());
        assert!(!store.is_in_progress("a"));
        assert!(!store.fight_summaries()[0].is_live);
    }

    #[test]
    fn a_full_history_drops_the_fight_that_started_first() {
        let scratch = Scratch::new("store.full");
        let store = FightHistoryManager::new(scratch.0.clone());
        for start in 1..=501 {
            put(&scratch, &record(&format!("auto_1_{start}"), start, 1));
        }
        // A fight saved again keeps its start: it is not the oldest.
        store.save_fight(&record("auto_1_501", 501, 2)).unwrap();
        let names = names(&scratch);
        assert_eq!(names.len(), KEEP_FIGHTS);
        assert!(!names.contains(&"auto_1_1.json".to_string()));
        assert!(names.contains(&"auto_1_2.json".to_string()));
        assert_eq!(store.fight_summaries().len(), KEEP_FIGHTS);
    }

    /// The files a trim that reads every file would remove.
    fn oldest_past_limit(scratch: &Scratch) -> Vec<String> {
        let mut by_start: Vec<(i64, String)> = fight_file_names(&fights(scratch))
            .into_iter()
            .filter_map(|name| read_start(&fights(scratch).join(&name)).flatten().map(|s| (s, name)))
            .collect();
        by_start.sort_unstable();
        by_start.truncate(by_start.len().saturating_sub(KEEP_FIGHTS));
        let mut names: Vec<String> = by_start.into_iter().map(|(_, n)| n).collect();
        names.sort();
        names
    }

    fn removed_by(scratch: &Scratch, save: impl FnOnce()) -> Vec<String> {
        let before = names(scratch);
        save();
        let after = names(scratch);
        before.into_iter().filter(|n| !after.contains(n)).collect()
    }

    #[test]
    fn a_trim_removes_the_same_fights_and_forgets_them() {
        let scratch = Scratch::new("store.trim");
        let store = FightHistoryManager::new(scratch.0.clone());
        store.save_snapshot(&record("auto_9_5", 5, 1), store.snapshot_ticket()).unwrap();
        for i in 0..500 {
            put(&scratch, &record(&format!("auto_1_{i}"), 10 + (i * 919) % 1000, 1));
        }
        put(&scratch, &record("auto_b_3", 3, 1));
        put(&scratch, &record("auto_a_3", 3, 1));
        fs::write(fights(&scratch).join("renamed.json"), serde_json::to_string(&record("auto_r_1", 1, 1)).unwrap()).unwrap();
        fs::write(fights(&scratch).join("empty.json"), "{}").unwrap();

        for (id, start) in [("auto_2_2000", 2000), ("auto_3_3000", 3000), ("auto_1_0", 10)] {
            let fight = record(id, start, 2);
            put(&scratch, &fight);
            let wanted = oldest_past_limit(&scratch);
            assert_eq!(removed_by(&scratch, || store.save_fight(&fight).unwrap()), wanted, "saving {id}");
        }
        let left = names(&scratch);
        assert_eq!(left.len(), KEEP_FIGHTS + 1);
        for gone in ["renamed.json", "auto_a_3.json", "auto_b_3.json", "auto_9_5.json", "auto_1_0.json"] {
            assert!(!left.contains(&gone.to_string()), "{gone}");
        }
        assert!(store.ticket_by_fight.lock().is_empty());
        let mut seen: Vec<String> = store.starts.lock().keys().cloned().collect();
        seen.sort();
        assert_eq!(seen, left);
    }

    #[test]
    fn a_slow_snapshot_after_a_delete_does_not_bring_the_fight_back() {
        let scratch = Scratch::new("store.forget");
        let store = FightHistoryManager::new(scratch.0.clone());
        let early = store.snapshot_ticket();
        store.save_snapshot(&record("auto_1_1000", 1000, 1), store.snapshot_ticket()).unwrap();
        store.delete_fight("auto_1_1000").unwrap();
        let _ = store.save_snapshot(&record("auto_1_1000", 1000, 1), early);
        assert!(!store.fight_path("auto_1_1000").exists());
    }

    #[test]
    fn files_that_are_no_fight_never_count_or_go() {
        let scratch = Scratch::new("store.notfull");
        let store = FightHistoryManager::new(scratch.0.clone());
        for start in 1..=500 {
            put(&scratch, &record(&format!("auto_1_{start}"), start, 1));
        }
        fs::write(fights(&scratch).join("empty.json"), "{}").unwrap();
        store.save_fight(&record("auto_1_500", 500, 2)).unwrap();
        assert_eq!(names(&scratch).len(), KEEP_FIGHTS + 1);
    }

    #[test]
    fn a_skipped_snapshot_does_not_trim() {
        let scratch = Scratch::new("store.skip");
        let store = FightHistoryManager::new(scratch.0.clone());
        let older = store.snapshot_ticket();
        let newer = store.snapshot_ticket();
        store.save_snapshot(&record("auto_1_9999", 9999, 2), newer).unwrap();
        for start in 1..=500 {
            put(&scratch, &record(&format!("auto_2_{start}"), start, 1));
        }
        store.save_snapshot(&record("auto_1_9999", 9999, 1), older).unwrap();
        assert_eq!(names(&scratch).len(), KEEP_FIGHTS + 1);
        assert_eq!(store.load_fight("auto_1_9999").unwrap().total_damage, 2);
    }
}
