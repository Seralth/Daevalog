//! Your character's file, kept as you play. Every packet the live parsers
//! read goes to a `LiveCharacter` (`character_report`); `save` writes the
//! file of the character you play into `characters/` in the data folder
//! (`character::store`) when the game told the meter something new about
//! it. The front end saves every `EVERY` and once more when the meter
//! quits. Always on; the file stays on this computer.
//!
//! The game sends gear, skills, Specialty picks, the Daevanion board and
//! titles at login only, so a meter started mid-session has not read them:
//! a section not read this session is kept from the character's file as it
//! was.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

use super::character_report::LiveCharacter;
use super::decoder::PacketTap;
use crate::character::file::CharacterFile;
use crate::character::store;

/// The front end saves this often at most.
pub const EVERY: Duration = Duration::from_secs(10);

#[derive(Default)]
pub struct CharacterKeeper {
    reader: Mutex<LiveCharacter>,
    saved: Mutex<Saved>,
}

#[derive(Default)]
struct Saved {
    /// The reader's record count at the last save.
    records: u64,
    /// By file name: each file as last written, or as found on disk.
    files: HashMap<String, CharacterFile>,
}

impl CharacterKeeper {
    pub fn new() -> Arc<CharacterKeeper> {
        Arc::new(CharacterKeeper::default())
    }

    /// What the parsers feed.
    pub fn tap(self: &Arc<Self>) -> PacketTap {
        let keeper = self.clone();
        Arc::new(move |at_ms, packet| keeper.reader.lock().feed(at_ms, packet))
    }

    /// Writes the file of the character you play into `data_dir` when the
    /// records read since the last save changed it, and returns its path.
    pub fn save(&self, data_dir: &Path) -> std::io::Result<Option<PathBuf>> {
        let mut saved = self.saved.lock();
        let (records, read) = {
            let reader = self.reader.lock();
            (reader.records(), reader.file())
        };
        // While a login is under way there is no file: the next save tries.
        let Some(file) = read.filter(|_| records != saved.records) else { return Ok(None) };
        let Some(name) = file.name.clone() else { return Ok(None) };
        let key = store::file_name(file.server, &name);
        if !saved.files.contains_key(&key) {
            if let Ok(old) = store::load(&store::path(data_dir, file.server, &name)) {
                saved.files.insert(key.clone(), old);
            }
        }
        let file = match saved.files.get(&key) {
            Some(old) => with_old(file, old),
            None => file,
        };
        if saved.files.get(&key) == Some(&file) {
            saved.records = records;
            return Ok(None);
        }
        let path = store::save(data_dir, &name, &file)?;
        saved.records = records;
        saved.files.insert(key, file);
        Ok(Some(path))
    }
}

/// `new`, with each section it has not read taken from `old`. Every field is
/// named, so a section added to the file must be merged here too. `new`
/// always has its name.
fn with_old(new: CharacterFile, old: &CharacterFile) -> CharacterFile {
    let old = old.clone();
    let CharacterFile { version, server, name, class, level, item_level, combat_power, gear, stats, skills, specialties, boards, titles } = new;
    CharacterFile {
        version,
        server,
        name,
        class: class.or(old.class),
        level: level.or(old.level),
        item_level: item_level.or(old.item_level),
        combat_power: combat_power.or(old.combat_power),
        gear: gear.or(old.gear),
        stats: stats.or(old.stats),
        skills: skills.or(old.skills),
        specialties: specialties.or(old.specialties),
        boards: boards.or(old.boards),
        titles: titles.or(old.titles),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::built::{packet, self_record, stat_sheet, Scratch, F};
    use crate::character::file::{Gear, GearItem, StatValue};

    #[test]
    fn saves_what_changed_and_keeps_what_this_session_did_not_read() {
        let dir = Scratch::new("character-keeper");
        let mut old = CharacterFile::new(1201, None, Some(44));
        old.gear = Some(Gear { as_of_ms: 1_000, items: vec![GearItem { slot: 1, item: 110_600_101, detail: None }] });
        store::save(&dir.0, "Tester", &old).unwrap();

        let keeper = CharacterKeeper::new();
        let tap = keeper.tap();
        assert_eq!(keeper.save(&dir.0).unwrap(), None, "nothing read");
        tap(5_000, &self_record(9001, "Tester", 1201, 30, 45));
        tap(5_100, &stat_sheet(&[(1, 32)]));
        let path = keeper.save(&dir.0).unwrap().expect("written");
        let file = store::load(&path).unwrap();
        assert_eq!((file.level, file.gear), (Some(45), old.gear), "the gear this session did not read");
        assert_eq!(file.name.as_deref(), Some("Tester"), "the name of the character you play");
        assert_eq!(file.stats.map(|s| s.values), Some(vec![StatValue { stat: 1, value: 32 }]));
        assert_eq!(keeper.save(&dir.0).unwrap(), None, "nothing new");

        // A login under way: nothing is written until its self record.
        tap(9_000, &packet([0x11, 0x56], &[F::U16(0x0210), F::U8(0)]));
        assert_eq!(keeper.save(&dir.0).unwrap(), None);
        tap(16_000, &self_record(9001, "Tester", 1201, 30, 46));
        assert_eq!(keeper.save(&dir.0).unwrap(), Some(path.clone()));
        assert_eq!(store::load(&path).unwrap().level, Some(46));
        assert_eq!(store::list(&dir.0), vec![path]);
    }

    /// The live path over packet logs: `A2_REPLAY_FILE` (logs joined by
    /// ':', oldest first) through the parsers with the keeper's tap, saved
    /// as the live meter saves (every `EVERY` of capture time, and at the
    /// end) into the data folder `A2_KEEP_APP`. Prints each file written;
    /// with `A2_PACK` (a data folder holding the pack), every id in each
    /// that the pack cannot name or place.
    #[test]
    #[ignore]
    fn live_replay() {
        use crate::capture::decoder::PacketDecoder;
        use crate::capture::packet_log::{LogLine, RepeatFilter};
        use crate::capture::reassembly::Reassembler;
        use crate::combat::data_storage::DataStorage;
        use crate::i18n::lookup::{NpcLookup, SkillLookup};

        let (Some(logs), Some(dir)) = (std::env::var("A2_REPLAY_FILE").ok(), std::env::var_os("A2_KEEP_APP")) else {
            return;
        };
        let dir = PathBuf::from(dir);
        let keeper = CharacterKeeper::new();
        let tap = keeper.tap();
        let (storage, skills, npcs) = (Arc::new(DataStorage::new()), Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
        let pack = std::env::var_os("A2_PACK").map(|data| crate::character::pack::CharacterPack::load(Path::new(&data), "en").expect("the pack"));
        let save = |at: i64| {
            if let Some(path) = keeper.save(&dir).expect("saved") {
                println!("{at} {}", path.display());
                if let Some(pack) = &pack {
                    println!("  missing: {:?}", pack.missing(&store::load(&path).expect("written whole")));
                }
            }
        };
        let mut saved_at = None;
        for log in logs.split(':').filter(|p| !p.is_empty()) {
            let text = std::fs::read_to_string(log).expect("a packet log");
            let mut streams: HashMap<String, (Reassembler, PacketDecoder)> = HashMap::new();
            let mut repeats = RepeatFilter::default();
            for line in text.lines().filter_map(LogLine::parse) {
                if repeats.is_repeat(&line) {
                    continue;
                }
                let (joiner, parser) = streams.entry(line.stream.to_string()).or_insert_with(|| {
                    let mut parser = PacketDecoder::new(storage.clone(), skills.clone(), npcs.clone());
                    parser.tap_packets(tap.clone());
                    (Reassembler::new(), parser)
                });
                parser.pin_clock(Some(line.at_ms));
                joiner.push_bytes(&line.bytes, parser);
                let due = saved_at.is_none_or(|at| line.at_ms - at >= EVERY.as_millis() as i64);
                if due {
                    save(line.at_ms);
                    saved_at = Some(line.at_ms);
                }
            }
        }
        save(saved_at.unwrap_or(0));
        crate::clock::set_override(None);
    }
}
