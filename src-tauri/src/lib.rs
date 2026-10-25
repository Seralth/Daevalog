//! The Daevalog meter's backend: it reads Aion 2's game traffic and turns it
//! into fights, damage and players.
//!
//! The modules outside the `backend` feature are the parser core. They touch
//! no OS service, so they also build for `wasm32-unknown-unknown`. Everything
//! that needs the OS, the network or files sits behind `backend`: capture,
//! HTTP, the keyring, settings and the fight history.

pub mod capture;
pub mod character;
pub mod clock;
pub mod combat;
pub mod entity;
pub mod game_record;
pub mod i18n;
pub mod slice_rebuild;
pub mod version;

#[cfg(feature = "backend")]
pub mod account;
#[cfg(feature = "backend")]
pub mod config;
#[cfg(feature = "backend")]
pub mod history;
#[cfg(feature = "backend")]
pub mod live_feed;
#[cfg(feature = "backend")]
pub mod logging;
#[cfg(feature = "backend")]
pub mod migrate;
#[cfg(feature = "backend")]
pub mod platform;
#[cfg(feature = "backend")]
pub mod share;

#[cfg(test)]
mod built;
#[cfg(test)]
mod checks;
#[cfg(test)]
mod trace_tap;
