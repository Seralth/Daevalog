//! The Character window's two inputs, and the contract between the parts
//! that make them and the window that reads them.
//!
//! - `file`: your character as the meter last read it from the game: gear,
//!   the stat sheet, skill levels, Specialty picks, Daevanion nodes and
//!   titles. Ids and numbers only; every name comes from the pack. `store`
//!   keeps one file per character in `characters/` in the data folder.
//! - `pack`: the game data that names and lays out those ids, in
//!   `src/data/character/` and `src/data/i18n/character/<language>.json`,
//!   beside the skill and stat names the meter already ships.
//!
//! Each file of either kind carries a version. A field that changes meaning
//! or goes away raises it; a new field does not, and readers ignore fields
//! they do not know. The contract tests (`tests/character_contract.rs`) read
//! a sample file and a sample pack (`tests/fixtures/character/`) and resolve
//! every id the file names.

pub mod file;
#[cfg(feature = "backend")]
pub mod pack;
#[cfg(feature = "backend")]
pub mod store;
