//! Pet collection records (`capture::decoder::pet_collection`), queued in
//! capture order for the meter's pet soul notices and taken once by it.

use crate::capture::decoder::PetCollectionEvent;

use super::DataStorage;

/// A meter that takes none (a replay) keeps only the newest this many.
const KEEP: usize = 256;

impl DataStorage {
    pub fn note_pet_collection(&self, event: PetCollectionEvent) {
        let mut queue = self.pet_collection.lock();
        if queue.len() == KEEP {
            queue.pop_front();
        }
        queue.push_back(event);
    }

    /// The records since the last call, oldest first.
    pub fn take_pet_collection(&self) -> Vec<PetCollectionEvent> {
        self.pet_collection.lock().drain(..).collect()
    }
}
