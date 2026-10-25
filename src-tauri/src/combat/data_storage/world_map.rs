//! The world map's lists (`capture::decoder::world_map`), queued in capture
//! order for the meter's field boss notices and taken once by it.

use crate::capture::decoder::WorldMapList;

use super::DataStorage;

/// A meter that takes none (a replay) keeps only the newest this many. The
/// lists come about once a second while the map is open; the meter takes
/// them every 15 s.
const KEEP: usize = 32;

impl DataStorage {
    pub fn note_world_map(&self, list: WorldMapList) {
        let mut queue = self.world_map.lock();
        if queue.len() == KEEP {
            queue.pop_front();
        }
        queue.push_back(list);
    }

    /// The lists since the last call, oldest first.
    pub fn take_world_map(&self) -> Vec<WorldMapList> {
        self.world_map.lock().drain(..).collect()
    }
}
