//! The pet collection (the pets a character has registered, not combat
//! summons): the whole collection, sent when the client joins a world
//! server (login, entering or leaving a party dungeon or the Abyss); souls
//! added to pets, one record per pickup and one for a pet chest; and a pet
//! registered or reaching a new level, with its genus Insight after it. Read
//! into the storage's queue for the meter's pet soul notices.

use std::collections::BTreeMap;

use super::world::body_after;
use super::{varint_at, PacketDecoder};
use crate::capture::opcodes::{PET_COLLECTION, PET_LEVEL, PET_REGISTERED, PET_SOULS};

/// Pet ids the game data uses.
const PET_IDS: std::ops::RangeInclusive<u32> = 1_000..=99_999;
/// Pet levels a record may give.
const LEVELS: std::ops::RangeInclusive<u32> = 1..=9;
/// A pet never holds more souls toward its next level than this.
const MAX_SOULS: u32 = 9_999;
/// One record never adds this many souls to a pet.
const MAX_ADDED: u32 = 999;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PetCount {
    /// 0: not registered yet.
    pub level: u32,
    /// Souls toward the next level (toward registering at level 0).
    pub souls: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PetCollectionEvent {
    /// Every pet with a level or souls toward one.
    Collection(BTreeMap<u32, PetCount>),
    /// Souls added: (pet, how many).
    Souls(Vec<(u32, u32)>),
    /// A pet registered (level 1) or reached `level`; the genus Insight
    /// level and its experience after it.
    Level { pet: u32, level: u32, insight_level: u32, insight_exp: u64 },
}

fn u32_at(body: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(body.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(body: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(body.get(at..at + 8)?.try_into().ok()?))
}

/// The collection: the active pet's id, then the registered pets (id, the
/// id again, level), then the pets holding souls (id, souls). Each list's
/// count is one byte up to 127 pets; past that it may be a varint, which
/// no capture has shown yet, so both are tried.
pub fn collection(body: &[u8]) -> Option<BTreeMap<u32, PetCount>> {
    [false, true].into_iter().find_map(|wide| collection_with(body, wide))
}

fn collection_with(body: &[u8], wide: bool) -> Option<BTreeMap<u32, PetCount>> {
    let count = |at: usize| -> Option<(usize, usize)> {
        if wide {
            let v = varint_at(body, at);
            (v.length > 0 && v.value >= 0).then_some((v.value as usize, v.length as usize))
        } else {
            Some((*body.get(at)? as usize, 1))
        }
    };
    let mut pets = BTreeMap::new();
    let mut at = 4;
    let (n, width) = count(at)?;
    at += width;
    for _ in 0..n {
        let (pet, again, level) = (u32_at(body, at)?, u32_at(body, at + 4)?, u32_at(body, at + 8)?);
        if pet != again || !LEVELS.contains(&level) {
            return None;
        }
        pets.insert(pet, PetCount { level, souls: 0 });
        at += 12;
    }
    let (n, width) = count(at)?;
    at += width;
    for _ in 0..n {
        let (pet, souls) = (u32_at(body, at)?, u32_at(body, at + 4)?);
        if !PET_IDS.contains(&pet) || souls > MAX_SOULS {
            return None;
        }
        pets.entry(pet).or_default().souls = souls;
        at += 8;
    }
    Some(pets)
}

/// Souls added: a count, then (pet, souls) per pet.
pub fn souls(body: &[u8]) -> Option<Vec<(u32, u32)>> {
    let n = *body.first()? as usize;
    if body.len() != 1 + 8 * n {
        return None;
    }
    let added: Vec<(u32, u32)> = (0..n)
        .filter_map(|i| Some((u32_at(body, 1 + 8 * i)?, u32_at(body, 5 + 8 * i)?)))
        .filter(|&(pet, n)| PET_IDS.contains(&pet) && (1..=MAX_ADDED).contains(&n))
        .collect();
    (!added.is_empty()).then_some(added)
}

/// A pet registered or levelled up: 2 bytes, pet, level, 1 byte, genus
/// Insight level, its experience.
pub fn level(body: &[u8]) -> Option<PetCollectionEvent> {
    if body.len() != 23 {
        return None;
    }
    let (pet, level) = (u32_at(body, 2)?, u32_at(body, 6)?);
    if !PET_IDS.contains(&pet) || !LEVELS.contains(&level) {
        return None;
    }
    Some(PetCollectionEvent::Level { pet, level, insight_level: u32_at(body, 11)?, insight_exp: u64_at(body, 15)? })
}

impl PacketDecoder {
    /// A pet collection record, queued for the pet soul notices.
    pub(super) fn read_pet_collection(&self, frame: &[u8]) {
        let event = if let Some(body) = body_after(frame, PET_COLLECTION) {
            collection(body).map(PetCollectionEvent::Collection)
        } else if let Some(body) = body_after(frame, PET_SOULS) {
            souls(body).map(PetCollectionEvent::Souls)
        } else if let Some(body) = body_after(frame, PET_REGISTERED).or_else(|| body_after(frame, PET_LEVEL)) {
            level(body)
        } else {
            None
        };
        if let Some(event) = event {
            self.data_storage.note_pet_collection(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::processor;
    use super::*;
    use crate::built;

    fn le(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// A collection with `registered` (pet, level) and `holding` (pet, souls).
    fn collection_body(registered: &[(u32, u32)], holding: &[(u32, u32)]) -> Vec<u8> {
        let mut body = le(&[1_003]);
        body.push(registered.len() as u8);
        for &(pet, level) in registered {
            body.extend(le(&[pet, pet, level]));
        }
        body.push(holding.len() as u8);
        for &(pet, souls) in holding {
            body.extend(le(&[pet, souls]));
        }
        body
    }

    fn level_body(pet: u32, level: u32, insight_level: u32, exp: u64) -> Vec<u8> {
        let mut body = vec![0, 0];
        body.extend(le(&[pet, level]));
        body.push(0);
        body.extend(le(&[insight_level]));
        body.extend(exp.to_le_bytes());
        body
    }

    #[test]
    fn a_collection_gives_each_pets_level_and_souls() {
        let pets = collection(&collection_body(&[(1_003, 2), (1_035, 1)], &[(1_003, 12), (1_208, 3)])).unwrap();
        assert_eq!(pets[&1_003], PetCount { level: 2, souls: 12 });
        assert_eq!(pets[&1_035], PetCount { level: 1, souls: 0 });
        assert_eq!(pets[&1_208], PetCount { level: 0, souls: 3 }, "souls toward registering");
        assert_eq!(pets.len(), 3);
    }

    #[test]
    fn a_collection_that_does_not_read_is_left_out() {
        let mut twisted = collection_body(&[(1_003, 2)], &[]);
        twisted[9..13].copy_from_slice(&1_004u32.to_le_bytes());
        assert_eq!(collection(&twisted), None, "the id twice must match");
        assert_eq!(collection(&collection_body(&[(1_003, 12)], &[])), None, "no such level");
    }

    #[test]
    fn souls_come_one_per_pickup_or_several_from_a_chest() {
        assert_eq!(souls(&[&[1u8][..], &le(&[1_003, 1])].concat()), Some(vec![(1_003, 1)]));
        let chest = [&[2u8][..], &le(&[1_003, 5, 1_035, 1])].concat();
        assert_eq!(souls(&chest), Some(vec![(1_003, 5), (1_035, 1)]));
        assert_eq!(souls(&chest[..chest.len() - 1]), None, "cut short");
        assert_eq!(souls(&[&[1u8][..], &le(&[7, 1])].concat()), None, "not a pet id");
    }

    #[test]
    fn a_level_record_carries_the_genus_insight() {
        let event = level(&level_body(1_003, 2, 6, 12_050)).unwrap();
        assert_eq!(event, PetCollectionEvent::Level { pet: 1_003, level: 2, insight_level: 6, insight_exp: 12_050 });
        assert_eq!(level(&level_body(1_003, 0, 6, 0)), None, "level 0 is no level");
    }

    #[test]
    fn the_decoder_queues_each_record_in_order() {
        let (storage, mut p) = processor();
        let frames = [
            built::frame(&[&PET_COLLECTION[..], &collection_body(&[(1_003, 1)], &[(1_003, 24)])].concat()),
            built::frame(&[&PET_SOULS[..], &[1u8][..], &le(&[1_003, 1])].concat()),
            built::frame(&[&PET_LEVEL[..], &level_body(1_003, 2, 6, 12_050)].concat()),
            built::frame(&[&PET_REGISTERED[..], &level_body(1_208, 1, 3, 800)].concat()),
        ]
        .concat();
        assert_eq!(p.decode_buffer(&frames), frames.len());
        let events = storage.take_pet_collection();
        assert_eq!(events.len(), 4);
        assert!(matches!(&events[0], PetCollectionEvent::Collection(pets) if pets[&1_003] == PetCount { level: 1, souls: 24 }));
        assert_eq!(events[1], PetCollectionEvent::Souls(vec![(1_003, 1)]));
        assert!(matches!(events[2], PetCollectionEvent::Level { pet: 1_003, level: 2, .. }));
        assert!(matches!(events[3], PetCollectionEvent::Level { pet: 1_208, level: 1, .. }));
        assert!(storage.take_pet_collection().is_empty(), "taken once");
    }
}
