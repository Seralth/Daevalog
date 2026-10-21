//! Power Shard records (`capture::decoder::power_shards`), queued in capture
//! order for the meter's shard notice and taken once by it.

use crate::capture::decoder::ShardEvent;

use super::DataStorage;

/// A meter that takes none (a replay) keeps only the newest this many.
const KEEP: usize = 256;

impl DataStorage {
    pub fn note_shard(&self, event: ShardEvent) {
        let mut queue = self.shards.lock();
        if queue.len() == KEEP {
            queue.pop_front();
        }
        queue.push_back(event);
    }

    /// The records since the last call, oldest first.
    pub fn take_shards(&self) -> Vec<ShardEvent> {
        self.shards.lock().drain(..).collect()
    }
}
