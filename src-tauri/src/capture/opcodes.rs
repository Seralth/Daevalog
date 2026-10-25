//! The opcodes the parser reads: the two bytes after a packet's length varint.

/// A damage record.
pub const DAMAGE: [u8; 2] = [0x04, 0x38];
/// A damage-over-time or heal-over-time tick.
pub const DOT: [u8; 2] = [0x05, 0x38];
/// A record about you and your party only.
pub const PARTY_SCOPE: [u8; 2] = [0x06, 0x38];
/// HP/MP update.
pub const HP_MP: [u8; 2] = [0x1B, 0x92];
/// Summon ownership, and the owner of a mob's loot.
pub const SUMMON_OWNERSHIP: [u8; 2] = [0x04, 0x8D];
/// Self/world teleport.
pub const ZONE_CHANGE: [u8; 2] = [0x23, 0x36];
/// Map load, sent on every zone load.
pub const MAP_LOAD: [u8; 2] = [0x21, 0x36];
/// An entity going into or out of combat; a boss with a timed enrage gets
/// the enrage deadline with it.
pub const COMBAT_STATE: [u8; 2] = [0x21, 0x8D];
/// The self record: the character you are playing. Your server and class
/// follow the name.
pub const SELF_IDENTITY: [u8; 2] = [0x33, 0x36];
/// The pet collection: every registered pet's level and souls.
pub const PET_COLLECTION: [u8; 2] = [0x00, 0x90];
/// Souls added to pets.
pub const PET_SOULS: [u8; 2] = [0x0D, 0x90];
/// A pet registered.
pub const PET_REGISTERED: [u8; 2] = [0x09, 0x90];
/// A pet reached a new level.
pub const PET_LEVEL: [u8; 2] = [0x14, 0x90];
/// The world map's lists: field boss times, rifts (only while the map is open).
pub const WORLD_MAP_LISTS: [u8; 2] = [0x01, 0x91];
/// An item in your bag changed: its id and the count now.
pub const BAG_ITEM: [u8; 2] = [0x1B, 0x56];
/// Party roster.
pub const PARTY_ROSTER: [u8; 2] = [0x02, 0x97];
/// Out of a party: the key of the party you were in.
pub const PARTY_LEAVE: [u8; 2] = [0x1B, 0x97];
/// One member of your party, in the roster's member layout (two kinds).
pub const PARTY_MEMBER_0B: [u8; 2] = [0x0B, 0x97];
pub const PARTY_MEMBER_1F: [u8; 2] = [0x1F, 0x97];
/// The party finder's room list (the Expedition window).
pub const PARTY_FINDER: [u8; 2] = [0x01, 0x97];
/// Your legion's members.
pub const LEGION_MEMBERS: [u8; 2] = [0x05, 0x8A];
/// Records about the local player only, led by their entity id, with how many
/// bytes follow it (`None`: it varies). What they carry is not decoded.
pub const OWN_RECORDS: [([u8; 2], Option<usize>); 4] =
    [([0x4A, 0x36], None), ([0x03, 0x8D], Some(4)), ([0x41, 0x37], Some(2)), ([0x42, 0x37], Some(0))];

// The June 2026 update shifted the 0x36 spawn/death family by +1. The parser
// accepts both, so `41 36` is a spawn now and was a death before.

/// Mob/summon spawn.
pub const SPAWN: [u8; 2] = [0x41, 0x36];
/// Mob/summon spawn before the June 2026 shift.
pub const SPAWN_OLD: [u8; 2] = [0x40, 0x36];
/// Player spawn.
pub const PLAYER_SPAWN: [u8; 2] = [0x45, 0x36];
/// Player spawn before the June 2026 shift.
pub const PLAYER_SPAWN_OLD: [u8; 2] = [0x44, 0x36];
/// Death.
pub const DEATH: [u8; 2] = [0x42, 0x36];
/// Death before the June 2026 shift.
pub const DEATH_OLD: [u8; 2] = [0x41, 0x36];
