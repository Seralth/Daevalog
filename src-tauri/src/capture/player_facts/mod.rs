//! What the packets state about the players met, for tools that count
//! players (the census, `tools/census_replay.rs`): player records, the lists
//! that state Item Level and Combat Power, and the party finder's room lists.
//! Read straight from packet bytes; nothing here feeds the meter's storage.
//! Parser core: no files, no clock of its own.

mod lists;
mod records;
mod rooms;

pub use lists::{read_list, ListKind, ListMember, ListRead};
pub use records::{player_records, PlayerRecord};
pub use rooms::{Room, RoomList, RoomMember, RoomSlot};
