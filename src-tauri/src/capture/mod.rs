// Parser core — compiled for wasm32 as well as the desktop app.
pub mod abnormal;
pub mod captured_payload;
pub mod evidence_slice;
pub mod framing;
pub(crate) mod names;
pub(crate) mod opcodes;
pub mod packet_accumulator;
pub mod ping_tracker;
pub mod stream_assembler;
pub mod stream_processor;
pub mod varint;

// Your own character's records, read only by the replay's character report.
#[cfg(test)]
pub(crate) mod character_report;

// Live capture. pcap needs libloading, the port detector reads the wall clock,
// and the file replay drives them both — none of which exist on wasm32.
#[cfg(feature = "backend")]
pub mod combat_port_detector;
// Owns the capture threads and the tokio channel they feed.
#[cfg(feature = "backend")]
pub mod dispatcher;
#[cfg(feature = "backend")]
pub mod file_replay;
#[cfg(feature = "backend")]
mod helper_process;
#[cfg(feature = "backend")]
pub mod live;
#[cfg(feature = "backend")]
pub mod pcap_capturer;
