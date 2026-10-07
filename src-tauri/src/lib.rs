//! Daevalog DPS Meter.
//!
//! The crate is split in three by features, and the lines are load bearing
//! rather than tidy-mindedness. Everything outside the features — the packet
//! parser, the combat aggregation, the Evidence Slice builder — must compile to
//! `wasm32-unknown-unknown`, because the log service re-derives an uploaded
//! fight by running *this* code rather than trusting the numbers a client sent.
//! Anything that reaches for pcap, HTTP or Windows lives behind `backend`, and
//! anything that reaches for Tauri behind `desktop`, which includes `backend`.
//!
//! CI builds all three. If a `use` of the backend creeps into the parser core,
//! the wasm build fails rather than the divergence being discovered later in a
//! log whose numbers nobody can reproduce. If Tauri creeps into the backend,
//! the backend build fails.

// ── parser core: must stay wasm-clean ──────────────────────────────────────
pub mod capture;
pub mod clock;
pub mod combat;
pub mod entity;
pub mod game_record;
pub mod i18n;
pub mod rederive;
pub mod version;

// ── check tools: run through cargo test ────────────────────────────────────
#[cfg(test)]
mod checks;

// ── backend: the meter without a window ────────────────────────────────────
#[cfg(feature = "backend")]
pub mod account;
#[cfg(feature = "backend")]
pub mod config;
#[cfg(feature = "backend")]
pub mod history;
#[cfg(feature = "backend")]
pub mod logging;
#[cfg(feature = "backend")]
pub mod migrate;
#[cfg(feature = "backend")]
pub mod platform;
#[cfg(feature = "backend")]
pub mod share;

// ── desktop: the Tauri app ─────────────────────────────────────────────────
#[cfg(feature = "desktop")]
mod tray;

#[cfg(feature = "desktop")]
mod app;

#[cfg(feature = "desktop")]
mod blocking;

#[cfg(feature = "desktop")]
pub use app::run;
