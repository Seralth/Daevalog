// Parser core — compiled for wasm32 as well as the desktop app.
pub mod captured_payload;
pub mod evidence_slice;
pub mod framing;
mod names;
pub mod packet_accumulator;
pub mod stream_assembler;
pub mod stream_processor;
pub mod varint;

#[cfg(test)]
mod replay_report;
#[cfg(all(test, feature = "desktop"))]
mod record_check;

// Live capture. pcap needs libloading, the port detector reads the wall clock,
// and the file replay drives them both — none of which exist on wasm32.
#[cfg(feature = "desktop")]
pub mod combat_port_detector;
#[cfg(feature = "desktop")]
pub mod file_replay;
#[cfg(feature = "desktop")]
pub mod pcap_capturer;
