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
/// The self record: the character you are playing.
pub const SELF_IDENTITY: [u8; 2] = [0x33, 0x36];
/// Party roster.
pub const PARTY_ROSTER: [u8; 2] = [0x02, 0x97];
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
