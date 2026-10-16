//! A pet or summon spawn (`41 36`, kind 0x5F) names its owner inline. The
//! kind is followed by a mask that was four bytes wide in older logs and is
//! two bytes now; the parser tries the name gate after either, and must not
//! link a summon to anyone when the name read there matches no player.

use std::collections::HashMap;
use std::sync::Arc;

use a2tools_dps_meter_lib::{
    capture::decoder::PacketDecoder,
    combat::data_storage::DataStorage,
    i18n::lookup::{NpcLookup, SkillLookup},
};

/// The summon's entity id: below 128, so its varint is one byte.
const PET: u8 = 120;
/// The player whose name the spawn carries.
const MASTER: i32 = 4_200;

/// `<len> 41 36 <PET> <mask> 01 <name len> <name> <24 zero bytes>`.
fn summon_frame(mask: &[u8], name: &str) -> Vec<u8> {
    let gate_and_name = [&[0x01, name.len() as u8][..], name.as_bytes()].concat();
    let payload = [&[0x41, 0x36, PET][..], mask, &gate_and_name, &[0; 24]].concat();
    // The length counts the payload and 4 more, in one varint byte here.
    let length = payload.len() + 4;
    assert!(length < 0x80, "{length}");
    std::iter::once(length as u8).chain(payload).collect()
}

/// The summon links after one spawn, with MASTER named before it.
fn links_after(mask: &[u8], name: &str) -> HashMap<i32, i32> {
    let (skills, npcs) = (Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
    let storage = Arc::new(DataStorage::new());
    let mut parser = PacketDecoder::new(Arc::clone(&storage), skills, npcs);
    // Linked by name: the owner is known before the spawn, as in the game.
    storage.name_entity(MASTER, "Abcd");
    parser.pin_clock(Some(1_000));
    parser.decode_buffer(&summon_frame(mask, name));
    parser.pin_clock(None);
    storage.pet_owners()
}

#[test]
fn a_four_byte_mask_still_links_the_summon_to_its_named_owner() {
    assert_eq!(links_after(&[0x5F, 0, 0, 0], "Abcd").get(&i32::from(PET)), Some(&MASTER));
}

#[test]
fn a_two_byte_mask_links_the_summon_to_its_named_owner() {
    assert_eq!(links_after(&[0x5F, 0], "Abcd").get(&i32::from(PET)), Some(&MASTER));
}

#[test]
fn a_name_nobody_has_links_nothing() {
    assert_eq!(links_after(&[0x5F, 0, 0, 0], "Qrstuv").get(&i32::from(PET)), None);
}
